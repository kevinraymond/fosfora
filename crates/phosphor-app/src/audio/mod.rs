pub mod analyzer;
pub mod beat;
pub mod capture;
pub mod chroma;
pub mod downbeat;
pub mod features;
pub mod hpss;
pub mod interp;
pub mod key;
pub mod loudness;
pub mod normalizer;
pub mod pitch;
#[cfg(target_os = "linux")]
pub mod pulse_capture;
pub mod ranging;
pub mod reconnect;
pub mod schema;
pub mod smoother;
pub mod stereo;
pub mod structure;
#[cfg(target_os = "windows")]
pub mod wasapi_capture;

pub use features::AudioFeatures;

/// A single analyzed audio frame handed from the audio thread to the render thread.
/// Carries the scalar [`AudioFeatures`] plus the two array streams the A17 audio textures
/// need (#1468): a log-frequency magnitude spectrum and one mel-spectrogram column.
/// Bundling them in one message keeps all three consistent for the same frame and reuses
/// the existing bounded crossbeam channel — no extra handoff.
pub struct AudioFrame {
    pub features: AudioFeatures,
    /// Log-frequency magnitude spectrum, [`analyzer::SPECTRUM_BINS`] bins in 0..1
    /// (fills the `audio_spectrum` texture).
    pub spectrum: Box<[f32]>,
    /// One mel-spectrogram column, [`analyzer::SPECTROGRAM_MELS`] bands in 0..1
    /// (scrolls into the `audio_spectrogram` texture).
    pub mel: Box<[f32]>,
    /// A5 sample-clock time this hop was analyzed at (`samples_consumed / sample_rate`),
    /// seconds. Frames are exactly [`ANALYSIS_HOP`] apart; the A8 render playhead (#1459)
    /// interpolates between frames on this clock.
    pub timestamp: f64,
    /// The audio side froze `beat_phase` at 0 for this frame — the A10 perceptual silence
    /// flag (`loudness_m < −55 LUFS`), see [`beat`]`::BeatDetector::process`. Carried
    /// explicitly because the render thread only sees the *smoothed* rms, which lags the
    /// freeze by ~1s — long enough for A8's local phase to free-run over the start of a
    /// silence. (It cannot be re-derived from `rms` at all: the normalizer floors that at 0
    /// in the trough between every hit on perfectly loud audio — finding #1551.)
    pub phase_frozen: bool,
    /// Seconds per bar, the denominator the audio thread's `bar_phase` is running on, or 0.0
    /// when there is no bar clock yet. Carried so the A8b render playhead (#1554) can advance
    /// its own `bar_phase` at exactly this rate; see [`downbeat`]`::DownbeatTracker::bar_clock`.
    ///
    /// Refreshed *only on a downbeat*, in the same branch that zeroes `bar_phase` — which is
    /// the property the render side leans on. It is deliberately not `meter`: that changes on
    /// any beat, mid-bar, and re-deriving the rate from it would fight the phase it chases.
    pub bar_duration: f64,
}

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};

use self::analyzer::FftAnalyzer;
use self::beat::BeatDetector;
pub use self::beat::{TempoCommand, TempoConfig, TempoControl, TempoPreset};
use self::capture::{AudioCapture, RingBuffer};
use self::downbeat::DownbeatTracker;
use self::hpss::HpssAnalyzer;
use self::interp::FeatureInterpolator;
use self::key::KeyDetector;
use self::loudness::LoudnessMeter;
use self::normalizer::FeatureNormalizer;
use self::pitch::PitchAnalyzer;
use self::smoother::FeatureSmoother;
use self::stereo::StereoAnalyzer;
pub use self::structure::StructureConfig;
use self::structure::StructureTracker;
use crate::settings::BandScale;

/// Holds the capture backend, keeping it alive while the audio processing thread runs.
/// On Linux, this may be either PulseAudio (preferred) or cpal/ALSA (fallback).
/// On Windows, this may be WASAPI loopback (preferred) or cpal (fallback).
#[allow(dead_code)]
enum CaptureBackend {
    Cpal(AudioCapture),
    #[cfg(target_os = "linux")]
    Pulse(pulse_capture::PulseCapture),
    #[cfg(target_os = "windows")]
    Wasapi(wasapi_capture::WasapiCapture),
}

/// Result of successfully opening an audio backend.
struct OpenedBackend {
    ring: Arc<RingBuffer>,
    sample_rate: f32,
    device_name: String,
    callback_count: Arc<AtomicU64>,
    backend: CaptureBackend,
    using_native_backend: bool,
    /// A9 (#1460): set by the capture thread when it dies of its own accord. The watchdog's
    /// only unambiguous death signal — see [`reconnect::Health`].
    capture_failed: Arc<AtomicBool>,
    /// A9 (#1460): whether this backend keeps delivering data (zeros) while nothing is
    /// playing. Only then does a frozen `callback_count` mean the device is gone rather than
    /// merely idle. This is where the per-backend policy lives, so nothing downstream of
    /// `open_backend` has to know which platform it is on.
    silence_delivers_data: bool,
}

/// How long `callback_count` may stay frozen before the watchdog calls it a stall.
const STALL_TIMEOUT: Duration = Duration::from_secs(10);

/// A9b (#1617): how often to ask `pactl` which sink is default. Each poll forks a subprocess,
/// so this is a cadence rather than a per-frame check; it matches `list_devices`' rescan gap.
#[cfg(target_os = "linux")]
const SINK_POLL_INTERVAL: Duration = Duration::from_secs(5);

/// A9 (#1460): what the status bar's AUD dot shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioIndicator {
    Live,
    /// Stalled, but not being acted on — an idle loopback backend, or auto-reconnect off.
    Quiet,
    Reconnecting {
        attempt: u32,
    },
    /// Attempts exhausted, or no backend open at all.
    Failed,
}

/// A9 (#1460): how [`AudioSystem::adopt`] disposes of the outgoing capture backend.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Teardown {
    /// Join on the calling thread. Releases the device *before* the reopen, which an
    /// exclusive ALSA `hw:` device needs in order to open again. Correct for a user-initiated
    /// switch: the backend is healthy, so its capture thread exits within one read period.
    Blocking,
    /// Hand the backend to a detached reaper and return at once. A stalled backend's capture
    /// thread may be blocked in a timeout-less `pa_simple_read`; joining it here would freeze
    /// the render thread for as long as the stall lasts.
    Reap,
}

/// Try native loopback first (PulseAudio on Linux, WASAPI on Windows), then cpal.
/// When a specific device is requested, skip native loopback and go straight to cpal.
fn open_backend(device_name: Option<&str>) -> Result<OpenedBackend, String> {
    if device_name.is_none() {
        #[cfg(target_os = "linux")]
        {
            match pulse_capture::PulseCapture::new() {
                Ok(pulse) => {
                    return Ok(OpenedBackend {
                        ring: pulse.ring.clone(),
                        sample_rate: pulse.sample_rate as f32,
                        device_name: pulse.device_name.clone(),
                        callback_count: pulse.callback_count.clone(),
                        capture_failed: pulse.capture_failed.clone(),
                        backend: CaptureBackend::Pulse(pulse),
                        using_native_backend: true,
                        // A monitor source of a suspended sink stops delivering, which is
                        // indistinguishable from death by callback count alone. Read errors
                        // are this backend's real death signal.
                        silence_delivers_data: false,
                    });
                }
                Err(e) => {
                    log::info!("PulseAudio unavailable ({e}), falling back to ALSA");
                }
            }
        }

        #[cfg(target_os = "windows")]
        {
            match wasapi_capture::WasapiCapture::new() {
                Ok(wasapi) => {
                    return Ok(OpenedBackend {
                        ring: wasapi.ring.clone(),
                        sample_rate: wasapi.sample_rate as f32,
                        device_name: wasapi.device_name.clone(),
                        callback_count: wasapi.callback_count.clone(),
                        capture_failed: wasapi.capture_failed.clone(),
                        backend: CaptureBackend::Wasapi(wasapi),
                        using_native_backend: true,
                        // Loopback delivers no packets at all while nothing is playing, so a
                        // frozen callback count is just as likely to be a quiet passage as a
                        // dead endpoint. COM errors are this backend's real death signal.
                        silence_delivers_data: false,
                    });
                }
                Err(e) => {
                    log::info!("WASAPI loopback unavailable ({e}), falling back to cpal");
                }
            }
        }
    }

    match AudioCapture::new_with_device(device_name) {
        Ok(capture) => Ok(OpenedBackend {
            ring: capture.ring.clone(),
            sample_rate: capture.sample_rate as f32,
            device_name: capture.device_name.clone(),
            callback_count: capture.callback_count.clone(),
            capture_failed: capture.capture_failed.clone(),
            backend: CaptureBackend::Cpal(capture),
            using_native_backend: false,
            // An input stream's callbacks run on the device clock and deliver zeros through
            // silence, so here — and only here — a frozen callback count does mean death.
            silence_delivers_data: true,
        }),
        Err(e) => Err(format!("{e}")),
    }
}

/// Manages the audio pipeline: capture -> FFT -> normalize -> beat detect -> smooth -> send to main thread.
pub struct AudioSystem {
    receiver: Receiver<AudioFrame>,
    latest: Option<AudioFeatures>,
    /// A8 (#1459): blends the audio thread's 86.1 Hz frames up to render rate and locally
    /// advances `beat_phase`, so neither stair-steps on a 120-144 Hz display.
    interp: FeatureInterpolator,
    /// Newest log-frequency spectrum column received (A17 `audio_spectrum`, #1468).
    /// Held between polls so the render thread always has a value to upload.
    latest_spectrum: Vec<f32>,
    /// Mel-spectrogram columns received since the last `latest_features` poll, oldest
    /// first (A17 `audio_spectrogram`, #1468). Drained by the render thread each frame.
    pending_mel: Vec<Box<[f32]>>,
    /// Newest mel-spectrogram column received (A1b, #1512). Held between polls (like
    /// `latest_spectrum`) so the binding bus can expose `audio.mel.N` sources without
    /// stealing columns from the draining spectrogram-texture path (`pending_mel`).
    latest_mel: Vec<f32>,
    pub device_name: String,
    pub active: bool,
    pub last_error: Option<String>,
    shutdown: Arc<AtomicBool>,
    thread_handle: Option<thread::JoinHandle<()>>,
    callback_count: Arc<AtomicU64>,
    started_at: Instant,
    _capture: Option<CaptureBackend>,
    /// True when using a native backend (PulseAudio/WASAPI) for the current capture.
    using_native_backend: bool,
    /// Cached device list, refreshed in background to avoid blocking the UI thread.
    cached_devices: Arc<Mutex<Vec<String>>>,
    /// Whether a background scan is already in flight.
    scan_in_flight: Arc<AtomicBool>,
    /// When the last device scan completed.
    last_scan: Instant,
    /// Ring buffer mirroring audio samples for recording (written by audio thread).
    pub recording_ring: Arc<RingBuffer>,
    /// Audio sample rate in Hz.
    pub sample_rate: u32,
    /// How the analyzer scales the 7 bands (A1 #1452). Held so a device switch preserves it
    /// and `set_band_scale` can rebuild the pipeline with a new value.
    band_scale: BandScale,
    /// Live-tunable A18 structure-detection thresholds (#1510). Shared with the audio thread,
    /// which snapshots it once per hop; the audio panel writes it directly (no pipeline
    /// rebuild). Threaded through `switch_device` so user tuning survives a device change.
    tuning: Arc<Mutex<StructureConfig>>,
    /// Live tempo prior + one-shot octave/tap commands (A7 #1458). Same shape as `tuning`:
    /// shared with the audio thread, snapshotted once per hop, threaded through
    /// `switch_device`. The mailbox half carries UI/MIDI/OSC overrides to the detector.
    tempo: Arc<Mutex<TempoControl>>,
    /// Beat taps for tap tempo (A7 #1458). Held as `Instant`s rather than offsets from
    /// `started_at`, which `switch_device` resets — a reset clock mid-sequence would turn
    /// the stored taps into garbage intervals.
    tap_times: Vec<Instant>,
    /// Total beats detected, incremented by the audio thread. The consumer compares
    /// this against `beats_seen` so a 1-frame beat pulse survives channel overflow
    /// and the drain-to-newest loop in `latest_features()`.
    beat_counter: Arc<AtomicU32>,
    /// Value of `beat_counter` at the end of the previous `latest_features()` poll.
    beats_seen: u32,
    /// Total downbeats detected (A12 #1463), counted by the audio thread. Same pattern as
    /// `beat_counter`: the consumer compares against `downbeats_seen` so a 1-frame downbeat
    /// pulse survives channel overflow and the drain-to-newest loop in `latest_features()`.
    downbeat_counter: Arc<AtomicU32>,
    /// Value of `downbeat_counter` at the end of the previous `latest_features()` poll.
    downbeats_seen: u32,
    /// Total drops detected (A18 #1469), counted by the audio thread. Same counter-latch as
    /// `beat`/`downbeat` so the 1-frame drop pulse survives channel overflow and the
    /// drain-to-newest loop in `latest_features()`.
    drop_counter: Arc<AtomicU32>,
    /// Value of `drop_counter` at the end of the previous `latest_features()` poll.
    drops_seen: u32,
    /// When the last frame arrived over the channel (for stale-feature decay).
    last_frame_at: Instant,
    /// When `latest_features()` was last polled (for frame-rate-independent decay).
    last_poll_at: Instant,
    /// `callback_count` value at the last `poll_health()` call.
    last_cb_count: u64,
    /// When `callback_count` last advanced.
    cb_changed_at: Instant,
    /// Whether the current stall episode has already been reported.
    stall_reported: bool,
    /// A9 (#1460): set by the capture thread when it dies of its own accord — the watchdog's
    /// unambiguous death signal. Belongs to the backend, so `adopt` swaps it.
    capture_failed: Arc<AtomicBool>,
    /// A9 (#1460): whether a frozen `callback_count` means death on this backend. See
    /// [`OpenedBackend::silence_delivers_data`]. Belongs to the backend, so `adopt` swaps it.
    silence_delivers_data: bool,
    /// A9 (#1460): backoff and attempt bookkeeping for the current stall episode. Deliberately
    /// *not* swapped by `adopt` — like `band_scale`, it belongs to the system rather than to
    /// any one backend, and an episode outlives the backends it cycles through.
    reconnect: reconnect::ReconnectState,
    /// A9 (#1460): an `open_backend` call in flight on a worker thread. `None` when no reopen
    /// is pending. Sole owner of the receiver, so dropping it orphans the worker — which is
    /// how a manual switch cancels a reconnect.
    pending_open: Option<Receiver<Result<OpenedBackend, String>>>,
    /// A9 (#1460): what `pending_open` is trying to reopen, so the landing `adopt` can name it
    /// (a cpal open silently falls back to the default device, so the result cannot say).
    reopen_target: Option<String>,
    /// A9b (#1617): newest `<sink>.monitor` the background sink poll has seen, `None` until
    /// `pactl` first answers. Linux/Pulse only — `find_monitor_source` is Pulse-specific, and
    /// WASAPI's equivalent is a follow-up (finding #1616).
    #[cfg(target_os = "linux")]
    default_sink: Arc<Mutex<Option<String>>>,
    /// A9b (#1617): whether a background sink poll is already in flight.
    #[cfg(target_os = "linux")]
    sink_poll_in_flight: Arc<AtomicBool>,
    /// A9b (#1617): when the last sink poll was spawned.
    #[cfg(target_os = "linux")]
    last_sink_poll: Instant,
}

impl AudioSystem {
    #[allow(dead_code)]
    pub fn new() -> Self {
        Self::new_with_device(
            None,
            BandScale::default(),
            Arc::new(Mutex::new(StructureConfig::default())),
            Arc::new(Mutex::new(TempoControl::default())),
        )
    }

    pub fn new_with_device(
        device_name: Option<&str>,
        band_scale: BandScale,
        tuning: Arc<Mutex<StructureConfig>>,
        tempo: Arc<Mutex<TempoControl>>,
    ) -> Self {
        Self::from_opened(
            open_backend(device_name),
            device_name,
            band_scale,
            tuning,
            tempo,
            Arc::new(RingBuffer::new()),
        )
    }

    /// Build the analysis pipeline and all render-facing state around an already-opened
    /// backend.
    ///
    /// Split out of `new_with_device` at the `open_backend` seam (A9 #1460) so the reconnect
    /// path can run that call — which forks a `pactl` subprocess and blocks on the PulseAudio
    /// server — on a worker thread and hand the result in here. See `start_reopen`.
    ///
    /// `requested` is what the caller asked for, needed only to name the device on the error
    /// path (where nothing opened, so `opened` cannot say). `recording_ring` is threaded in
    /// rather than created here so a reopen can keep the *same* ring: an in-progress recording
    /// holds a clone of it, and handing it a fresh one would silently strand its writer.
    fn from_opened(
        opened: Result<OpenedBackend, String>,
        requested: Option<&str>,
        band_scale: BandScale,
        tuning: Arc<Mutex<StructureConfig>>,
        tempo: Arc<Mutex<TempoControl>>,
        recording_ring: Arc<RingBuffer>,
    ) -> Self {
        let (tx, rx): (Sender<AudioFrame>, Receiver<AudioFrame>) = crossbeam_channel::bounded(4);

        let shutdown = Arc::new(AtomicBool::new(false));
        let beat_counter = Arc::new(AtomicU32::new(0));
        let downbeat_counter = Arc::new(AtomicU32::new(0));
        let drop_counter = Arc::new(AtomicU32::new(0));

        match opened {
            Ok(opened) => {
                let shutdown_flag = shutdown.clone();
                let ring = opened.ring.clone();
                let sample_rate = opened.sample_rate;
                let rec_ring = recording_ring.clone();
                let beats = beat_counter.clone();
                let downbeats = downbeat_counter.clone();
                let drops = drop_counter.clone();
                let tuning_thread = tuning.clone();
                let tempo_thread = tempo.clone();

                let thread_handle = thread::Builder::new()
                    .name("phosphor-audio".into())
                    .spawn(move || {
                        audio_thread(
                            ring,
                            sample_rate,
                            tx,
                            shutdown_flag,
                            rec_ring,
                            beats,
                            downbeats,
                            drops,
                            band_scale,
                            tuning_thread,
                            tempo_thread,
                        );
                    })
                    .expect("Failed to spawn audio thread");

                Self {
                    receiver: rx,
                    latest: None,
                    interp: FeatureInterpolator::new(sample_rate as u32),
                    latest_spectrum: vec![0.0; analyzer::SPECTRUM_BINS],
                    pending_mel: Vec::new(),
                    latest_mel: Vec::new(),
                    device_name: opened.device_name,
                    active: true,
                    last_error: None,
                    shutdown,
                    thread_handle: Some(thread_handle),
                    callback_count: opened.callback_count,
                    started_at: Instant::now(),
                    capture_failed: opened.capture_failed,
                    silence_delivers_data: opened.silence_delivers_data,
                    _capture: Some(opened.backend),
                    using_native_backend: opened.using_native_backend,
                    cached_devices: Arc::new(Mutex::new(Vec::new())),
                    scan_in_flight: Arc::new(AtomicBool::new(false)),
                    last_scan: Instant::now()
                        .checked_sub(Duration::from_secs(60))
                        .expect("60s subtraction from now cannot underflow"),
                    recording_ring,
                    sample_rate: sample_rate as u32,
                    band_scale,
                    tuning,
                    tempo,
                    tap_times: Vec::new(),
                    beat_counter,
                    beats_seen: 0,
                    downbeat_counter,
                    downbeats_seen: 0,
                    drop_counter,
                    drops_seen: 0,
                    last_frame_at: Instant::now(),
                    last_poll_at: Instant::now(),
                    last_cb_count: 0,
                    cb_changed_at: Instant::now(),
                    stall_reported: false,
                    reconnect: reconnect::ReconnectState::new(true),
                    pending_open: None,
                    reopen_target: None,
                    #[cfg(target_os = "linux")]
                    default_sink: Arc::new(Mutex::new(None)),
                    #[cfg(target_os = "linux")]
                    sink_poll_in_flight: Arc::new(AtomicBool::new(false)),
                    // Backdated like `last_scan`, so the first `poll_health` seeds the cache
                    // rather than waiting out an interval first.
                    #[cfg(target_os = "linux")]
                    last_sink_poll: Instant::now()
                        .checked_sub(Duration::from_secs(60))
                        .expect("60s subtraction from now cannot underflow"),
                }
            }
            Err(e) => {
                log::warn!("Audio capture unavailable: {e}");
                Self {
                    receiver: rx,
                    latest: None,
                    // No device: no frames will arrive, so this only ever serves the
                    // fallback path. Matches the `sample_rate` default below.
                    interp: FeatureInterpolator::new(44100),
                    latest_spectrum: vec![0.0; analyzer::SPECTRUM_BINS],
                    pending_mel: Vec::new(),
                    latest_mel: Vec::new(),
                    device_name: requested.unwrap_or("Default").to_string(),
                    active: false,
                    last_error: Some(e),
                    shutdown,
                    thread_handle: None,
                    callback_count: Arc::new(AtomicU64::new(0)),
                    started_at: Instant::now(),
                    capture_failed: Arc::new(AtomicBool::new(false)),
                    // Nothing is open, so nothing can freeze; `poll_health` gates the whole
                    // watchdog on `active` anyway.
                    silence_delivers_data: false,
                    _capture: None,
                    using_native_backend: false,
                    cached_devices: Arc::new(Mutex::new(Vec::new())),
                    scan_in_flight: Arc::new(AtomicBool::new(false)),
                    last_scan: Instant::now()
                        .checked_sub(Duration::from_secs(60))
                        .expect("60s subtraction from now cannot underflow"),
                    recording_ring,
                    sample_rate: 44100,
                    band_scale,
                    tuning,
                    tempo,
                    tap_times: Vec::new(),
                    beat_counter,
                    beats_seen: 0,
                    downbeat_counter,
                    downbeats_seen: 0,
                    drop_counter,
                    drops_seen: 0,
                    last_frame_at: Instant::now(),
                    last_poll_at: Instant::now(),
                    last_cb_count: 0,
                    cb_changed_at: Instant::now(),
                    stall_reported: false,
                    reconnect: reconnect::ReconnectState::new(true),
                    pending_open: None,
                    reopen_target: None,
                    #[cfg(target_os = "linux")]
                    default_sink: Arc::new(Mutex::new(None)),
                    #[cfg(target_os = "linux")]
                    sink_poll_in_flight: Arc::new(AtomicBool::new(false)),
                    #[cfg(target_os = "linux")]
                    last_sink_poll: Instant::now()
                        .checked_sub(Duration::from_secs(60))
                        .expect("60s subtraction from now cannot underflow"),
                }
            }
        }
    }

    /// Switch to a different audio device at runtime (user-initiated).
    pub fn switch_device(&mut self, device_name: Option<&str>) {
        // A9 (#1460): a manual pick supersedes any reconnect episode — drop the receiver to
        // orphan an in-flight worker (its send then fails harmlessly, and it drops the
        // backend it opened on its own thread), and forget the backoff.
        self.pending_open = None;
        self.reopen_target = None;
        self.adopt(device_name, open_backend(device_name), Teardown::Blocking);
        self.reconnect.reset();
    }

    /// Replace the live capture pipeline with `opened`, disposing of the old one per
    /// `teardown` (A9 #1460). Everything below the teardown block is the pre-A9
    /// `switch_device` body.
    fn adopt(
        &mut self,
        requested: Option<&str>,
        opened: Result<OpenedBackend, String>,
        teardown: Teardown,
    ) {
        // Signal the old audio thread to stop
        self.shutdown.store(true, Ordering::Release);

        // The analysis thread only ever polls this flag and sleeps 10ms — it never blocks on
        // the device — so joining it is bounded even mid-stall, and only `_capture` can hang.
        // Joining it *here* is load-bearing for `recording_ring` below: `RingBuffer::push` is
        // single-producer, so the old thread must be gone before the new one can share it.
        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }

        // Drop old capture backend before creating new one
        let old_capture = self._capture.take();
        match teardown {
            Teardown::Blocking => drop(old_capture),
            Teardown::Reap => reconnect::reap("phosphor-audio-reaper", old_capture),
        }

        // Create new system and swap all fields (mem::replace avoids move-out-of-Drop).
        // Preserve the current band scale (A1 #1452), the A18 tuning Arc (#1510) and the A7
        // tempo control (#1458) across the switch — passing the same Arcs keeps user tuning
        // live (the fresh audio thread receives a clone of each), so `self.tuning` and
        // `self.tempo` are deliberately left unswapped below. Same for `recording_ring`
        // (A9 #1460): an in-progress recording holds a clone, so handing the fresh thread a
        // new ring would leave that recording's writer draining one nobody writes to.
        let mut new = Self::from_opened(
            opened,
            requested,
            self.band_scale,
            self.tuning.clone(),
            self.tempo.clone(),
            self.recording_ring.clone(),
        );
        self.receiver = std::mem::replace(&mut new.receiver, crossbeam_channel::bounded(1).1);
        self.latest = None;
        // A8 (#1459): the fresh audio thread restarts `samples_consumed` at 0, so the next
        // frame's timestamp jumps *backwards* by the whole session length. Drop the
        // interpolation state so the playhead re-seeds from the new clock instead of
        // slewing across that gap. (`push` also guards this, but the device may also have
        // changed sample rate — so rebuild rather than just reset.)
        self.interp = FeatureInterpolator::new(new.sample_rate);
        self.latest_spectrum = std::mem::take(&mut new.latest_spectrum);
        self.pending_mel.clear();
        self.latest_mel.clear();
        self.device_name = std::mem::take(&mut new.device_name);
        self.active = new.active;
        self.last_error = new.last_error.take();
        self.shutdown = std::mem::replace(&mut new.shutdown, Arc::new(AtomicBool::new(true)));
        self.thread_handle = new.thread_handle.take();
        self.callback_count =
            std::mem::replace(&mut new.callback_count, Arc::new(AtomicU64::new(0)));
        self.started_at = new.started_at;
        self._capture = new._capture.take();
        self.using_native_backend = new.using_native_backend;
        self.capture_failed =
            std::mem::replace(&mut new.capture_failed, Arc::new(AtomicBool::new(false)));
        self.silence_delivers_data = new.silence_delivers_data;
        // `recording_ring` is deliberately NOT swapped (A9 #1460) — `from_opened` was handed
        // ours, so the new audio thread already writes to the same ring an in-progress
        // recording is draining.
        self.sample_rate = new.sample_rate;
        self.beat_counter = std::mem::replace(&mut new.beat_counter, Arc::new(AtomicU32::new(0)));
        self.beats_seen = self.beat_counter.load(Ordering::Relaxed);
        self.downbeat_counter =
            std::mem::replace(&mut new.downbeat_counter, Arc::new(AtomicU32::new(0)));
        self.downbeats_seen = self.downbeat_counter.load(Ordering::Relaxed);
        self.drop_counter = std::mem::replace(&mut new.drop_counter, Arc::new(AtomicU32::new(0)));
        self.drops_seen = self.drop_counter.load(Ordering::Relaxed);
        // Reset stale-feature decay and watchdog state for the fresh backend.
        self.last_frame_at = Instant::now();
        self.last_poll_at = Instant::now();
        self.last_cb_count = 0;
        self.cb_changed_at = Instant::now();
        self.stall_reported = false;
        // Keep existing cached_devices/scan_in_flight/last_scan — no need to re-scan on switch
        // A9b's default_sink/sink_poll_in_flight/last_sink_poll (#1617) are unswapped for the
        // same reason, and one more: the cached sink is what *triggered* this adopt, so keeping
        // it is what stops the fresh `device_name` re-firing against a stale `None`.
        // `self.reconnect` is likewise deliberately unswapped (A9 #1460): an episode spans the
        // backends it cycles through, so `new`'s fresh state must not clobber the live one.
        // `new` is dropped here — its Drop is a no-op since thread_handle is None and shutdown is true
    }

    /// Shared, live-tunable A18 structure-detection thresholds (#1510). The audio panel locks
    /// and writes this directly; the audio thread snapshots it once per hop. No pipeline
    /// rebuild, and it survives a device switch (the same Arc is threaded through).
    pub fn tuning(&self) -> &Arc<Mutex<StructureConfig>> {
        &self.tuning
    }

    /// Shared tempo prior + command mailbox (A7 #1458). The audio panel locks and writes the
    /// config directly; the audio thread snapshots it once per hop. Survives a device switch.
    pub fn tempo(&self) -> &Arc<Mutex<TempoControl>> {
        &self.tempo
    }

    /// Queue a one-shot tempo override (A7 #1458) for the audio thread — the half/double and
    /// tap-tempo controls, reachable from the UI and from MIDI/OSC triggers.
    pub fn send_tempo_command(&self, cmd: TempoCommand) {
        self.tempo
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(cmd);
    }

    /// Register one beat tap (A7 #1458), sending the averaged tempo to the detector once
    /// enough taps have landed. Owned here so the panel's Tap button and the MIDI/OSC tap
    /// trigger feed a single sequence — tapping across both still averages correctly.
    /// Averaging on this side keeps wall-clock tap times out of the audio thread's sample
    /// clock; only the resulting BPM crosses over.
    pub fn tap_tempo(&mut self) {
        let now = Instant::now();
        if self
            .tap_times
            .last()
            .is_some_and(|&t| now.duration_since(t).as_secs_f64() > TAP_RESET_SECS)
        {
            self.tap_times.clear();
        }
        self.tap_times.push(now);
        if self.tap_times.len() > TAP_WINDOW {
            let excess = self.tap_times.len() - TAP_WINDOW;
            self.tap_times.drain(..excess);
        }
        if self.tap_times.len() >= TAP_MIN_TAPS {
            let span = now.duration_since(self.tap_times[0]).as_secs_f64();
            let mean_interval = span / (self.tap_times.len() - 1) as f64;
            if mean_interval > 0.0 {
                self.send_tempo_command(TempoCommand::Tap(60.0 / mean_interval));
            }
        }
    }

    /// Change the band scaling (A1 #1452) at runtime. Rebuilds the capture pipeline (a brief
    /// re-open, like a device switch) so the audio-thread analyzer picks up the new scale.
    pub fn set_band_scale(&mut self, band_scale: BandScale) {
        if self.band_scale == band_scale {
            return;
        }
        self.band_scale = band_scale;
        // `switch_device` carries `self.band_scale` into the new pipeline.
        let device = self.current_target();
        self.switch_device(device.as_deref());
    }

    /// How to reopen whatever we are listening to now: `None` for the native loopback backend
    /// (which `open_backend` only tries when no device is named), else the cpal device name.
    fn current_target(&self) -> Option<String> {
        if self.using_native_backend {
            None
        } else {
            Some(self.device_name.clone())
        }
    }

    /// Return cached input device list. Triggers a background rescan every 5 seconds
    /// so device enumeration never blocks the UI thread.
    pub fn list_devices(&mut self) -> Vec<String> {
        if self.last_scan.elapsed() > Duration::from_secs(5)
            && !self.scan_in_flight.swap(true, Ordering::AcqRel)
        {
            let cache = self.cached_devices.clone();
            let flag = self.scan_in_flight.clone();
            thread::Builder::new()
                .name("phosphor-devscan".into())
                .spawn(move || {
                    // Pre-load libjack and install null error handlers before cpal touches ALSA
                    capture::suppress_jack_errors();
                    let devs = AudioCapture::list_devices();
                    // Recover from poisoned mutex — device list is non-critical UI data.
                    *cache.lock().unwrap_or_else(|e| e.into_inner()) = devs;
                    flag.store(false, Ordering::Release);
                })
                .ok();
            self.last_scan = Instant::now();
        }
        self.cached_devices
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Drain the channel and return the features at the render playhead.
    ///
    /// A8 (#1459): analysis runs at a fixed 86.1 Hz hop while the render loop may poll at
    /// 144 Hz, so rather than repeat the newest frame this interpolates the two frames
    /// bracketing a playhead held [`TARGET_DELAY_HOPS`] behind the audio clock, per each
    /// slot's [`schema::InterpPolicy`], and advances `beat_phase` — and `bar_phase` (A8b,
    /// #1554) — locally. `dt` is the render frame time.
    pub fn latest_features(&mut self, dt: f32) -> Option<AudioFeatures> {
        let now = Instant::now();
        let poll_dt = now.duration_since(self.last_poll_at).as_secs_f32();
        self.last_poll_at = now;

        let mut got_frame = false;
        while let Ok(frame) = self.receiver.try_recv() {
            self.latest = Some(frame.features);
            self.interp.push(
                frame.timestamp,
                &frame.features,
                frame.phase_frozen,
                frame.bar_duration,
            );
            // Keep the newest spectrum; accumulate every mel column so the spectrogram
            // scrolls smoothly even when several audio frames arrive between polls.
            self.latest_spectrum.clear();
            self.latest_spectrum.extend_from_slice(&frame.spectrum);
            // Keep the newest mel column for the binding bus (A1b, #1512) before pushing
            // it onto the drain-once texture queue.
            self.latest_mel.clear();
            self.latest_mel.extend_from_slice(&frame.mel);
            self.pending_mel.push(frame.mel);
            got_frame = true;
        }

        if got_frame {
            self.last_frame_at = now;
        } else if self.last_frame_at.elapsed() > Duration::from_millis(250) {
            // No frames for a while (device stalled or removed): decay the held
            // features toward silence instead of freezing on the last loud frame.
            // Compounds per poll, so visuals settle to zero in roughly a second.
            if let Some(ref mut latest) = self.latest {
                let k = (-poll_dt / 0.3).exp();
                decay_features(latest, k);
            }
            // A8: the decay above mutates `self.latest`, which the interpolator does not
            // read — leaving its ring live here would keep serving the last *undecayed*
            // frame and silently defeat the decay, freezing visuals loud on a dead device.
            // Resetting drops us onto the `self.latest` fallback path; the ring refills
            // ~23 ms after the device recovers.
            self.interp.reset();
        }

        // Health check: detect stalled audio callbacks
        if self.active
            && self.last_error.is_none()
            && self.callback_count.load(Ordering::Relaxed) == 0
            && self.started_at.elapsed() > Duration::from_secs(5)
        {
            let msg = "Device opened but no audio data received — check audio routing";
            log::warn!("{msg}");
            self.last_error = Some(msg.to_string());
        }

        // A8 (#1459): blend the two frames bracketing the render playhead and advance
        // `beat_phase` (and `bar_phase`, A8b #1554) locally, falling back to the newest held
        // frame when the ring can't bracket it (startup, stall, device switch) — the pre-A8
        // behaviour.
        let interpolated = self.interp.sample(dt, self.latest.as_ref());

        // Beat pulses are 1-frame events that the drain-to-newest loop above (and
        // the channel's drop-on-full policy) can lose. The audio thread counts
        // beats in an atomic, so derive `beat` from the counter instead of the
        // channel's field: any advance since the last poll means a beat fired.
        // Runs last so it always wins over the interpolated slots.
        let beats_now = self.beat_counter.load(Ordering::Relaxed);
        let downbeats_now = self.downbeat_counter.load(Ordering::Relaxed);
        let drops_now = self.drop_counter.load(Ordering::Relaxed);
        let result = interpolated.map(|mut features| {
            features.beat = if beats_now == self.beats_seen {
                0.0
            } else {
                1.0
            };
            // Same latch for the A12 downbeat trigger (#1463).
            features.downbeat = if downbeats_now == self.downbeats_seen {
                0.0
            } else {
                1.0
            };
            // Same latch for the A18 drop trigger (#1469).
            features.drop = if drops_now == self.drops_seen {
                0.0
            } else {
                1.0
            };
            features
        });
        self.beats_seen = beats_now;
        self.downbeats_seen = downbeats_now;
        self.drops_seen = drops_now;

        result
    }

    /// Newest log-frequency magnitude spectrum (A17 `audio_spectrum`, #1468), 0..1 per
    /// bin. Call after `latest_features` each frame; returns zeros until the first frame
    /// arrives. Length is [`analyzer::SPECTRUM_BINS`].
    pub fn latest_spectrum(&self) -> &[f32] {
        &self.latest_spectrum
    }

    /// Newest mel-spectrogram column (A1b `audio.mel.N` binding sources, #1512), 0..1 per
    /// band. Call after `latest_features` each frame; empty until the first frame arrives.
    /// Length is [`analyzer::SPECTROGRAM_MELS`]. Unlike `take_mel_columns`, this does not
    /// drain — the render thread's spectrogram-texture path is unaffected.
    pub fn latest_mel(&self) -> &[f32] {
        &self.latest_mel
    }

    /// Take the mel-spectrogram columns accumulated since the last call (oldest first),
    /// leaving the pending buffer empty (A17 `audio_spectrogram`, #1468). The render
    /// thread scrolls these into the spectrogram texture.
    pub fn take_mel_columns(&mut self) -> Vec<Box<[f32]>> {
        std::mem::take(&mut self.pending_mel)
    }

    /// Turn auto-reconnect on or off (A9 #1460). Off abandons any episode in flight.
    pub fn set_auto_reconnect(&mut self, enabled: bool) {
        self.reconnect.set_enabled(enabled);
    }

    /// What the status bar's AUD dot should show (A9 #1460).
    pub fn indicator(&self) -> AudioIndicator {
        if self.reconnect.is_reconnecting() {
            AudioIndicator::Reconnecting {
                attempt: self.reconnect.attempt(),
            }
        } else if !self.active || self.reconnect.is_exhausted() {
            AudioIndicator::Failed
        } else if self.stall_reported {
            AudioIndicator::Quiet
        } else {
            AudioIndicator::Live
        }
    }

    /// Reap the current backend and start a reopen of the same source on a worker thread
    /// (A9 #1460).
    ///
    /// `open_backend` must not run on the render thread: on Linux it forks a `pactl`
    /// subprocess and makes two blocking PulseAudio server round-trips, and a dead server —
    /// the likeliest cause of the stall we are answering — bounds neither. Running it inline
    /// would put an unbounded wait back on the render thread and undo the reaper.
    fn start_reopen(&mut self) {
        let target = self.current_target();
        self.reopen_target = target.clone();

        // Reap first: it releases the device before the reopen (an exclusive ALSA device will
        // not open twice) and stops the stalled thread fighting for the source. Adopting the
        // `Err` shell parks us in the existing "no device" state, whose stale-feature decay in
        // `latest_features` already carries the visuals gracefully across the gap.
        self.adopt(
            target.as_deref(),
            Err("Reconnecting…".to_string()),
            Teardown::Reap,
        );

        let (tx, rx) = crossbeam_channel::bounded(1);
        self.pending_open = Some(rx);
        thread::Builder::new()
            .name("phosphor-audio-reopen".into())
            .spawn(move || {
                let _ = tx.send(open_backend(target.as_deref()));
            })
            // Spawn failure drops `tx`, so `rx` disconnects and `poll_health` fails the
            // attempt on its next tick — no special case needed.
            .ok();
    }

    /// A9b (#1617): notice that the PulseAudio/PipeWire default sink changed, and follow it.
    ///
    /// This is not a stall and not a death, so the watchdog in `poll_health` structurally
    /// cannot see it: PipeWire silently *migrates* a `pa_simple` stream to the new default's
    /// monitor, so reads keep flowing at 43/s and `callback_count` keeps advancing while we
    /// capture the wrong device (finding #1614, runtime-verified). The sink name is the only
    /// signal there is, so poll it.
    ///
    /// Reopening needs no new machinery: `current_target` returns `None` on this backend, so
    /// `start_reopen` re-runs `find_monitor_source` and lands on whatever is default *now*.
    ///
    /// Gated on `using_native_backend` for two independent reasons: `device_name` only holds a
    /// PA monitor source on that path (it is a cpal device name otherwise, so the comparison
    /// would be nonsense), and a user who explicitly picked a device did not ask to follow the
    /// system default. Reuses the `auto_reconnect` setting rather than adding a knob — picking
    /// the native backend already means "capture whatever my system is playing", and following
    /// the default sink is that choice's literal meaning.
    ///
    /// The poll runs on a worker because `pactl` is a subprocess fork; this only ever reads the
    /// cached answer. Polling is the cheap half of #1617 — the event-driven version wants
    /// `pa_context_subscribe` on `PA_SUBSCRIPTION_MASK_SERVER`, which needs ~12 more dlopen'd
    /// symbols from the async API that this simple-API backend does not bind.
    #[cfg(target_os = "linux")]
    fn poll_default_sink(&mut self) -> Option<String> {
        if !self.using_native_backend
            || !self.reconnect.is_enabled()
            || self.reconnect.is_reconnecting()
            || self.pending_open.is_some()
        {
            return None;
        }

        if self.last_sink_poll.elapsed() > SINK_POLL_INTERVAL
            && !self.sink_poll_in_flight.swap(true, Ordering::AcqRel)
        {
            let cell = self.default_sink.clone();
            let flag = self.sink_poll_in_flight.clone();
            thread::Builder::new()
                .name("phosphor-sinkpoll".into())
                .spawn(move || {
                    let found = pulse_capture::find_monitor_source();
                    // Recover from a poisoned mutex like `list_devices` does — a sink name is
                    // non-critical, and a panic here must not take the watchdog down with it.
                    *cell.lock().unwrap_or_else(|e| e.into_inner()) = found;
                    flag.store(false, Ordering::Release);
                })
                .ok();
            self.last_sink_poll = Instant::now();
        }

        let current = self
            .default_sink
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()?;
        if current == self.device_name {
            return None;
        }

        log::info!(
            "Default sink changed: {} -> {current} — following",
            self.device_name
        );
        self.reconnect.note_reopen_started();
        self.start_reopen();
        Some(format!("Audio output changed — following to {current}"))
    }

    /// Non-Linux: no `pactl`, and WASAPI's equivalent is a follow-up (finding #1616).
    #[cfg(not(target_os = "linux"))]
    fn poll_default_sink(&mut self) -> Option<String> {
        None
    }

    /// Land a completed reopen, if one is in flight. Returns a message for the status toast.
    fn poll_pending_open(&mut self) -> Option<String> {
        let rx = self.pending_open.as_ref()?;
        match rx.try_recv() {
            Ok(result) => {
                self.pending_open = None;
                let opened = result.is_ok();
                let target = self.reopen_target.take();
                // Nothing to tear down — `start_reopen` already reaped it and adopted the
                // `Err` shell, so this join is over a `None` handle.
                self.adopt(target.as_deref(), result, Teardown::Blocking);
                self.reconnect.note_attempt(Instant::now(), opened);
                opened.then(|| format!("Audio reconnected: {}", self.device_name))
            }
            Err(crossbeam_channel::TryRecvError::Empty) => None,
            Err(crossbeam_channel::TryRecvError::Disconnected) => {
                // The worker never ran, or died before sending.
                self.pending_open = None;
                self.reopen_target = None;
                self.reconnect.note_attempt(Instant::now(), false);
                None
            }
        }
    }

    /// Watchdog: detect a capture backend that died or stalled mid-session and, when
    /// auto-reconnect is on (A9 #1460), reap it off-thread and reopen. Returns a one-shot
    /// message for the status toast.
    ///
    /// The trigger is a *positive* death signal (`capture_failed`) rather than a frozen
    /// callback count, because a freeze is ambiguous on both loopback backends: WASAPI
    /// delivers no packets while nothing plays, and a PulseAudio monitor of a suspended sink
    /// stops delivering. Only cpal, whose callbacks run on the device clock and deliver zeros
    /// through silence, may reconnect on a freeze alone — see `silence_delivers_data`.
    ///
    /// The 5-second startup check in `latest_features()` covers devices that never delivered.
    pub fn poll_health(&mut self) -> Option<String> {
        // Land any completed reopen first, so this frame's state machine sees the result.
        if let Some(msg) = self.poll_pending_open() {
            log::info!("{msg}");
            return Some(msg);
        }

        // A9b (#1617) must run *before* the healthy-path return below, not after: a migrated
        // stream keeps delivering (finding #1614), so a default-sink change only ever reaches
        // this function with `callback_count` advancing — i.e. down the one path that returns
        // early. Placing it after would make it dead code in exactly the case it exists for.
        if let Some(msg) = self.poll_default_sink() {
            self.last_error = Some(msg.clone());
            return Some(msg);
        }

        let cb = self.callback_count.load(Ordering::Relaxed);
        if cb != self.last_cb_count {
            self.last_cb_count = cb;
            self.cb_changed_at = Instant::now();
            self.stall_reported = false;
            self.reconnect.note_healthy();
            return None;
        }

        // Only trip once callbacks have flowed at least once and then stalled.
        let frozen = self.active && cb > 0 && self.cb_changed_at.elapsed() > STALL_TIMEOUT;
        let health = reconnect::Health {
            died: self.active && self.capture_failed.load(Ordering::Relaxed),
            frozen,
            any_callbacks: cb > 0,
            silence_delivers_data: self.silence_delivers_data,
        };

        let action = self.reconnect.poll(Instant::now(), health);
        let msg = match action {
            reconnect::ReconnectAction::Reopen { attempt } => {
                self.start_reopen();
                Some(format!(
                    "Audio device lost — reconnecting ({attempt}/{})…",
                    reconnect::MAX_ATTEMPTS
                ))
            }
            reconnect::ReconnectAction::GiveUp => Some(format!(
                "Audio reconnect failed after {} attempts (Settings → Audio)",
                reconnect::MAX_ATTEMPTS
            )),
            reconnect::ReconnectAction::Idle => None,
        };
        if let Some(msg) = msg {
            log::warn!("{msg}");
            self.last_error = Some(msg.clone());
            return Some(msg);
        }

        // Detection-only fallback: the stalls we deliberately do not act on — a quiet loopback
        // backend, or auto-reconnect turned off. Neutral wording because on those backends
        // this really may just be silence.
        if frozen && !self.stall_reported {
            self.stall_reported = true;
            let msg =
                "No audio data for 10s — device may be idle or disconnected (Settings → Audio)";
            self.last_error = Some(msg.to_string());
            log::warn!("{msg}");
            return Some(msg.to_string());
        }
        None
    }
}

/// Exponentially decay all features by `k`, per each feature's
/// [`DecayPolicy`](schema::DecayPolicy): energy levels scale toward silence,
/// `bpm` holds its last value (a tempo estimate, not a level), and `beat` is
/// forced to 0 so a stalled device can never hold a beat pulse high.
fn decay_features(features: &mut AudioFeatures, k: f32) {
    use schema::DecayPolicy;
    for (i, v) in features.as_slice_mut().iter_mut().enumerate() {
        match schema::FEATURES[i].decay {
            DecayPolicy::Scale => *v *= k,
            DecayPolicy::Hold => {}
            DecayPolicy::ForceZero => *v = 0.0,
        }
    }
}

impl Drop for AudioSystem {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }
        // A9 (#1460): reap rather than drop the backend here. Dropping it joins the capture
        // thread, which during a stall may be blocked in a timeout-less read — that used to
        // hang the whole process on quit. The reaper is detached, so the process exits and the
        // OS reclaims the thread.
        if let Some(capture) = self._capture.take() {
            reconnect::reap("phosphor-audio-reaper", capture);
        }
    }
}

/// A5 (#1456): fixed analysis hop. The audio thread accumulates capture reads into a FIFO
/// and runs one analyze/beat/… frame per exactly this many samples, so spectral-flux and
/// onset amplitudes, the tempo frame rate (`sr / ANALYSIS_HOP`), and A18's decimation no
/// longer drift with scheduler jitter or read-burst size. 512 @ 44.1 kHz ≈ 11.6 ms/frame
/// (87.5% overlap of the 4096-sample analysis window).
pub const ANALYSIS_HOP: usize = 512;

/// Tap tempo (A7 #1458). A gap longer than this means the user stopped and started over,
/// so the sequence resets; `TAP_WINDOW` taps are averaged and `TAP_MIN_TAPS` (2 intervals)
/// are needed before a tempo is inferred at all.
const TAP_RESET_SECS: f64 = 3.0;
const TAP_WINDOW: usize = 4;
const TAP_MIN_TAPS: usize = 3;

fn audio_thread(
    ring: Arc<RingBuffer>,
    sample_rate: f32,
    tx: Sender<AudioFrame>,
    shutdown: Arc<AtomicBool>,
    recording_ring: Arc<RingBuffer>,
    beat_counter: Arc<AtomicU32>,
    downbeat_counter: Arc<AtomicU32>,
    drop_counter: Arc<AtomicU32>,
    band_scale: BandScale,
    tuning: Arc<Mutex<StructureConfig>>,
    tempo: Arc<Mutex<TempoControl>>,
) {
    let mut analyzer = FftAnalyzer::new(sample_rate, band_scale);
    let mut normalizer = FeatureNormalizer::new();
    let mut beat_detector = BeatDetector::new(
        sample_rate,
        tempo.lock().unwrap_or_else(|e| e.into_inner()).config,
    );
    let mut key_detector = KeyDetector::new(sample_rate);
    let mut loudness_meter = LoudnessMeter::new(sample_rate);
    let mut downbeat_tracker = DownbeatTracker::new();
    let mut structure_tracker = StructureTracker::new(sample_rate / ANALYSIS_HOP as f32);
    let mut smoother = FeatureSmoother::new();
    let mut stereo_analyzer = StereoAnalyzer::new();
    let mut hpss_analyzer = HpssAnalyzer::new();
    let mut pitch_analyzer = PitchAnalyzer::new(sample_rate);
    // A13 (#1464): the capture ring yields interleaved L,R. `read_buf` reads it raw; `mono_scratch`
    // holds the mono mix derived from it (fed to the recording mirror + FFT, exactly as before).
    let mut read_buf = vec![0.0f32; 8192]; // 4096 stereo frames; larger for the 4096-pt FFT
    let mut mono_scratch: Vec<f32> = Vec::with_capacity(read_buf.len() / 2);

    // A5 (#1456): accumulate capture reads here and analyze exactly ANALYSIS_HOP samples at
    // a time. `samples_consumed` is a sample clock — each frame's timestamp is derived from
    // it, so frame timing is exact and independent of scheduler jitter or read-burst size.
    let mut fifo: Vec<f32> = Vec::with_capacity(read_buf.len() + ANALYSIS_HOP);
    // A13 (#1464): interleaved L/R queued in lockstep with `fifo` (2 samples per mono frame), so
    // each hop's stereo slice aligns exactly with its mono hop.
    let mut fifo_stereo: Vec<f32> = Vec::with_capacity((read_buf.len() + ANALYSIS_HOP) * 2);
    let mut samples_consumed: u64 = 0;
    // Fixed per-frame delta for time-constant smoothing (attack/release EMAs, onset decay).
    let dt = ANALYSIS_HOP as f32 / sample_rate;

    loop {
        if shutdown.load(Ordering::Acquire) {
            log::info!("Audio thread shutting down");
            break;
        }
        thread::sleep(Duration::from_millis(10));

        let available = ring.available();
        if available == 0 {
            continue;
        }

        let to_read = available.min(read_buf.len());
        let read = ring.read(&mut read_buf[..to_read]);
        if read == 0 {
            continue;
        }
        // Interleaved L,R off the capture ring — always even-length (ring L/R parity invariant).
        let stereo = &read_buf[..read];

        // A13 (#1464): derive the mono mix once from the stereo frames. Everything mono-facing — the
        // recording mirror and the FFT feed — consumes this, so their behavior is unchanged (for a
        // 2ch source it equals the old capture-time downmix).
        mono_scratch.clear();
        mono_scratch.extend(stereo.chunks_exact(2).map(|f| (f[0] + f[1]) * 0.5));

        // Mirror the mono mix to the recording ring (lock-free, no overhead if nobody reads).
        recording_ring.push(&mono_scratch);
        // Queue mono for hop-aligned analysis, and the interleaved stereo in lockstep.
        fifo.extend_from_slice(&mono_scratch);
        fifo_stereo.extend_from_slice(stereo);

        // Process every complete hop the read produced (>=2 when catching up after a stall).
        let mut offset = 0;
        while fifo.len() - offset >= ANALYSIS_HOP {
            let hop = &fifo[offset..offset + ANALYSIS_HOP];
            // A13 (#1464): the interleaved L/R for this same hop (2 samples per mono frame).
            let hop_stereo = &fifo_stereo[offset * 2..(offset + ANALYSIS_HOP) * 2];
            offset += ANALYSIS_HOP;
            samples_consumed += ANALYSIS_HOP as u64;
            let timestamp = samples_consumed as f64 / sample_rate as f64;

            // Multi-resolution FFT + feature extraction. The analyzer shifts this hop into
            // its 4096-sample window, so consecutive hops overlap 87.5%.
            let mut raw = analyzer.analyze(hop);

            // A10 (#1461): perceptual loudness on the fresh hop (each sample once). Fields
            // are Passthrough, so — like the beat block — they survive normalize/smooth
            // unrescaled.
            let loud = loudness_meter.process(hop);
            raw.loudness_m = loud.m;
            raw.loudness_s = loud.s;
            raw.loudness_trend = loud.trend;
            // A6 (#1457): the onset detector gates on this perceptual silence flag.
            let loud_silent = loudness_meter.is_silent();

            // A13 (#1464): stereo field over the rolling window. Gated inside the analyzer on total
            // stereo energy — NOT the mono `loud_silent` flag, which a fully anti-phase (maximally
            // wide) signal would trip by cancelling to mono silence. The fields are Passthrough, so
            // they survive normalize()/smooth() unrescaled, like the loudness/key blocks below.
            let stereo_field = stereo_analyzer.process(hop_stereo);
            raw.pan = stereo_field.pan;
            raw.stereo_width = stereo_field.stereo_width;
            raw.stereo_corr = stereo_field.stereo_corr;

            // A14 (#1465): harmonic/percussive split from the medium (1024-pt) magnitude. The two
            // energies are raw levels set before normalize() so the adaptive normalizer ranges and
            // silence-gates them like the bands (Adaptive); `harmonic_ratio` is a level-invariant
            // 0..1 balance (Passthrough), neutral-gated inside the analyzer on `loud_silent`.
            let hpss = hpss_analyzer.process(analyzer.mid_magnitude(), loud_silent);
            raw.percussive_energy = hpss.percussive_energy;
            raw.harmonic_energy = hpss.harmonic_energy;
            raw.harmonic_ratio = hpss.harmonic_ratio;

            // A15 (#1466): monophonic f0 via YIN on the analyzer's raw time-domain window. Producer-
            // normalized to a 0..1 log-frequency (Passthrough); confidence = YIN periodicity
            // (1 − aperiodicity). Held through unvoiced gaps with confidence 0, so a pitch-keyed
            // visual doesn't snap to the lowest note on rests. Set before normalize() like the block
            // above (a Passthrough field survives normalize/smooth unrescaled).
            let pitch = pitch_analyzer.process(analyzer.time_domain(), loud_silent);
            raw.pitch = pitch.pitch;
            raw.pitch_confidence = pitch.pitch_confidence;

            // A3 (#1454): fill `kick` now that the silence flag is known — a single
            // detector-owned P95 normalizer, gated so noise-floor log-flux can't fire. Set
            // before the pre-norm snapshot so structure/downbeat see the true kick, and it
            // survives normalize() unchanged (kick is Passthrough).
            raw.kick = analyzer.kick_envelope(loud_silent);

            // A11 (#1462): key detection on the fresh CQT chroma, before normalization
            // rescales it. Key fields are Passthrough, so they survive normalize/smooth.
            let key_result = key_detector.process(&raw.chroma, dt);
            raw.key_class = key_result.key_class;
            raw.key_is_minor = key_result.is_minor;
            raw.key_confidence = key_result.confidence;

            // A12 (#1463): capture pre-normalization chroma + per-band flux for the downbeat
            // tracker. The adaptive normalizer rescales chroma per-bin, which would distort
            // the inter-beat chord-change magnitude, so snapshot both before normalize().
            let pre_norm_chroma = raw.chroma;
            let band_flux = analyzer.band_flux_3();
            // A18 (#1469): snapshot the whole feature set before normalize() for the structure
            // tracker — it keys on the true loudness / sub-bass / centroid dynamics the adaptive
            // normalizer would flatten. (`AudioFeatures` is Copy; loudness + spectral shape are
            // already filled at this point; onset/bpm come from `beat_result` below.)
            let pre_norm = raw;

            // A2 (#1453): per-feature normalization (gated percentile / fixed-range /
            // z-score / passthrough), silence-gated on the A10 perceptual flag.
            raw = normalizer.normalize(&raw, loud_silent);

            // A7 (#1458): snapshot the shared tempo config and drain the command mailbox
            // once per hop, same as the A18 tuning above. In auto mode the estimator owns the
            // prior centre, so publish what it adapted to back into the shared config — that's
            // what the UI slider reads, and where it freezes when auto is switched off.
            let (tempo_cfg, tempo_cmds) = {
                let mut t = tempo.lock().unwrap_or_else(|e| e.into_inner());
                if t.config.auto_prior {
                    t.config.prior_center_bpm = beat_detector.prior_center_bpm();
                }
                (t.config, t.drain())
            };
            beat_detector.set_tempo_config(tempo_cfg);
            for cmd in tempo_cmds {
                beat_detector.apply_tempo_command(cmd);
            }

            // Beat detection (on raw magnitude spectra)
            let beat_result = beat_detector.process(
                analyzer.bass_magnitude(),
                analyzer.mid_magnitude(),
                analyzer.high_magnitude(),
                timestamp,
                loud_silent,
            );
            raw.onset = beat_result.onset_strength;
            raw.beat = beat_result.beat;
            raw.beat_phase = beat_result.beat_phase;
            raw.bpm = beat_result.bpm / 300.0; // normalize to 0-1
            raw.beat_strength = beat_result.beat_strength;

            // Count beats in an atomic so the consumer can't miss a 1-frame pulse
            // when the channel overflows or it drains multiple frames at once.
            if beat_result.beat > 0.5 {
                beat_counter.fetch_add(1, Ordering::Relaxed);
            }

            // A12 (#1463): bar/downbeat/meter tracking. Runs every frame (advances bar_phase
            // on the audio clock, integrates flux); heavy scoring gates on a fired beat.
            let db = downbeat_tracker.process(
                &beat_result,
                band_flux,
                raw.rms,
                &pre_norm_chroma,
                timestamp,
                loud_silent,
            );
            raw.downbeat = db.downbeat;
            raw.bar_phase = db.bar_phase;
            raw.beat_in_bar = db.beat_in_bar;
            // Counter-back the downbeat trigger, same as `beat`, so a 1-frame pulse survives.
            if db.downbeat > 0.5 {
                downbeat_counter.fetch_add(1, Ordering::Relaxed);
            }

            // A18 (#1469): section novelty / build-up / drop. Reads the pre-normalization
            // snapshot + the beat result; heavy work is decimated to ~10 Hz internally.
            // Snapshot the shared A18 tuning once per hop (#1510) so this frame's structure
            // detection sees a consistent set of thresholds; the UI may be writing it live.
            let struct_cfg = *tuning.lock().unwrap_or_else(|e| e.into_inner());
            let structure =
                structure_tracker.process(struct_cfg, &pre_norm, &beat_result, timestamp);
            raw.section_novelty = structure.section_novelty;
            raw.buildup = structure.buildup;
            raw.drop = structure.drop;
            // Counter-back the drop trigger, same as `beat`/`downbeat`.
            if structure.drop > 0.5 {
                drop_counter.fetch_add(1, Ordering::Relaxed);
            }

            // Smoothing (per-feature asymmetric EMA; beat/beat_phase pass through)
            let smoothed = smoother.smooth(&raw, dt);

            // A17 (#1468): sample the render-facing spectrum + mel column from the analyzer's
            // fresh magnitude, so all three ride the same frame across the channel.
            let frame = AudioFrame {
                features: smoothed,
                spectrum: Box::new(analyzer.log_spectrum_512()),
                mel: Box::new(analyzer.spectrogram_column()),
                timestamp,
                // Mirrors the silence gate in `BeatDetector::process` exactly: it pins
                // phase at 0 under perceptual silence, so A8's local oscillator must follow
                // rather than free-run. Same flag the detector gates on — `raw.rms` would
                // be wrong here, since it is post-normalization and hits 0 at the bottom of
                // the adaptive range on loud audio.
                phase_frozen: loud_silent,
                // A8b (#1554): the tracker's own bar-clock denominator, so the render side
                // advances `bar_phase` on the same rate that produced the phase above.
                bar_duration: db.bar_duration,
            };

            // Non-blocking send; drop if main thread is behind
            let _ = tx.try_send(frame);
        }

        // Drop the samples we consumed; keep the sub-hop remainder for next time.
        if offset > 0 {
            fifo.drain(..offset);
            fifo_stereo.drain(..offset * 2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() < eps
    }

    #[test]
    fn decay_scales_everything_except_holds() {
        let mut f = AudioFeatures::default();
        for v in f.as_slice_mut().iter_mut() {
            *v = 1.0;
        }
        decay_features(&mut f, 0.5);
        for (i, &v) in f.as_slice().iter().enumerate() {
            match i {
                // The beat (16), downbeat (52) and drop (60) triggers are forced to 0 on silence.
                16 | 52 | 60 => {
                    assert!(approx_eq(v, 0.0, 1e-6), "trigger {i} must be forced to 0");
                }
                // bpm (18), the categorical key fields key_class (49) / key_is_minor (50), and the
                // A15 pitch estimate (64) hold their last value rather than sweeping toward silence.
                18 | 49 | 50 | 64 => assert!(approx_eq(v, 1.0, 1e-6), "index {i} must hold"),
                _ => assert!(approx_eq(v, 0.5, 1e-6), "index {i} should decay to 0.5"),
            }
        }
    }

    #[test]
    fn decay_forces_beat_to_zero() {
        let mut f = AudioFeatures {
            beat: 1.0,
            bpm: 0.4,
            rms: 0.8,
            ..Default::default()
        };
        decay_features(&mut f, 0.9);
        assert!(approx_eq(f.beat, 0.0, 1e-6));
        assert!(approx_eq(f.bpm, 0.4, 1e-6));
        assert!(approx_eq(f.rms, 0.72, 1e-6));
    }

    #[test]
    fn decay_compounds_toward_silence() {
        let mut f = AudioFeatures {
            rms: 1.0,
            ..Default::default()
        };
        // ~60 polls at 16.7ms with tau=0.3s should settle well below 5%
        let k = (-0.0167f32 / 0.3).exp();
        for _ in 0..60 {
            decay_features(&mut f, k);
        }
        assert!(f.rms < 0.05, "rms should settle near zero, got {}", f.rms);
    }
}
