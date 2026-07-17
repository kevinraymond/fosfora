pub mod analyzer;
pub mod beat;
pub mod capture;
pub mod chroma;
pub mod downbeat;
pub mod features;
pub mod key;
pub mod loudness;
pub mod normalizer;
#[cfg(target_os = "linux")]
pub mod pulse_capture;
pub mod ranging;
pub mod schema;
pub mod smoother;
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
}

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};

use self::analyzer::FftAnalyzer;
use self::beat::BeatDetector;
use self::capture::{AudioCapture, RingBuffer};
use self::downbeat::DownbeatTracker;
use self::key::KeyDetector;
use self::loudness::LoudnessMeter;
use self::normalizer::FeatureNormalizer;
use self::smoother::FeatureSmoother;
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
                        backend: CaptureBackend::Pulse(pulse),
                        using_native_backend: true,
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
                        backend: CaptureBackend::Wasapi(wasapi),
                        using_native_backend: true,
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
            backend: CaptureBackend::Cpal(capture),
            using_native_backend: false,
        }),
        Err(e) => Err(format!("{e}")),
    }
}

/// Manages the audio pipeline: capture -> FFT -> normalize -> beat detect -> smooth -> send to main thread.
pub struct AudioSystem {
    receiver: Receiver<AudioFrame>,
    latest: Option<AudioFeatures>,
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
}

impl AudioSystem {
    #[allow(dead_code)]
    pub fn new() -> Self {
        Self::new_with_device(None, BandScale::default())
    }

    pub fn new_with_device(device_name: Option<&str>, band_scale: BandScale) -> Self {
        let (tx, rx): (Sender<AudioFrame>, Receiver<AudioFrame>) = crossbeam_channel::bounded(4);

        let shutdown = Arc::new(AtomicBool::new(false));
        let recording_ring = Arc::new(RingBuffer::new());
        let beat_counter = Arc::new(AtomicU32::new(0));
        let downbeat_counter = Arc::new(AtomicU32::new(0));
        let drop_counter = Arc::new(AtomicU32::new(0));

        match open_backend(device_name) {
            Ok(opened) => {
                let shutdown_flag = shutdown.clone();
                let ring = opened.ring.clone();
                let sample_rate = opened.sample_rate;
                let rec_ring = recording_ring.clone();
                let beats = beat_counter.clone();
                let downbeats = downbeat_counter.clone();
                let drops = drop_counter.clone();

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
                        );
                    })
                    .expect("Failed to spawn audio thread");

                Self {
                    receiver: rx,
                    latest: None,
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
                }
            }
            Err(e) => {
                log::warn!("Audio capture unavailable: {e}");
                Self {
                    receiver: rx,
                    latest: None,
                    latest_spectrum: vec![0.0; analyzer::SPECTRUM_BINS],
                    pending_mel: Vec::new(),
                    latest_mel: Vec::new(),
                    device_name: device_name.unwrap_or("Default").to_string(),
                    active: false,
                    last_error: Some(e),
                    shutdown,
                    thread_handle: None,
                    callback_count: Arc::new(AtomicU64::new(0)),
                    started_at: Instant::now(),
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
                }
            }
        }
    }

    /// Switch to a different audio device at runtime.
    pub fn switch_device(&mut self, device_name: Option<&str>) {
        // Signal the old audio thread to stop
        self.shutdown.store(true, Ordering::Release);

        // Wait for the old thread to finish so the device is fully released
        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }

        // Drop old capture backend before creating new one
        self._capture = None;

        // Create new system and swap all fields (mem::replace avoids move-out-of-Drop).
        // Preserve the current band scale (A1 #1452) across the switch.
        let mut new = Self::new_with_device(device_name, self.band_scale);
        self.receiver = std::mem::replace(&mut new.receiver, crossbeam_channel::bounded(1).1);
        self.latest = None;
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
        self.recording_ring =
            std::mem::replace(&mut new.recording_ring, Arc::new(RingBuffer::new()));
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
        // `new` is dropped here — its Drop is a no-op since thread_handle is None and shutdown is true
    }

    /// Change the band scaling (A1 #1452) at runtime. Rebuilds the capture pipeline (a brief
    /// re-open, like a device switch) so the audio-thread analyzer picks up the new scale.
    pub fn set_band_scale(&mut self, band_scale: BandScale) {
        if self.band_scale == band_scale {
            return;
        }
        self.band_scale = band_scale;
        // Reopen the same source: `None` for the native loopback backend, else the cpal
        // device name. `switch_device` carries `self.band_scale` into the new pipeline.
        let device = if self.using_native_backend {
            None
        } else {
            Some(self.device_name.clone())
        };
        self.switch_device(device.as_deref());
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

    /// Drain the channel and return the most recent features.
    pub fn latest_features(&mut self) -> Option<AudioFeatures> {
        let now = Instant::now();
        let poll_dt = now.duration_since(self.last_poll_at).as_secs_f32();
        self.last_poll_at = now;

        let mut got_frame = false;
        while let Ok(frame) = self.receiver.try_recv() {
            self.latest = Some(frame.features);
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

        // Beat pulses are 1-frame events that the drain-to-newest loop above (and
        // the channel's drop-on-full policy) can lose. The audio thread counts
        // beats in an atomic, so derive `beat` from the counter instead of the
        // channel's field: any advance since the last poll means a beat fired.
        let beats_now = self.beat_counter.load(Ordering::Relaxed);
        let downbeats_now = self.downbeat_counter.load(Ordering::Relaxed);
        let drops_now = self.drop_counter.load(Ordering::Relaxed);
        let result = self.latest.map(|mut features| {
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

    /// Watchdog: detect a device that stopped delivering data mid-session and
    /// report it once per stall episode. Detection only — automatically tearing
    /// down a stalled backend is unsafe: dropping it joins a capture thread that
    /// may be blocked in a timeout-less read (e.g. `pa_simple_read`), which would
    /// hang the render thread for as long as the stall lasts. The 5-second
    /// startup check in `latest_features()` covers devices that never delivered.
    pub fn poll_health(&mut self) -> Option<String> {
        let cb = self.callback_count.load(Ordering::Relaxed);
        if cb != self.last_cb_count {
            self.last_cb_count = cb;
            self.cb_changed_at = Instant::now();
            self.stall_reported = false;
            return None;
        }

        // Only trip once callbacks have flowed at least once and then stalled.
        // WASAPI loopback delivers no packets while nothing is playing, so a
        // long playback pause on Windows can look like a stall — hence the
        // neutral wording and once-per-episode reporting.
        if self.active
            && cb > 0
            && !self.stall_reported
            && self.cb_changed_at.elapsed() > Duration::from_secs(10)
        {
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
        // _capture is dropped automatically
    }
}

/// A5 (#1456): fixed analysis hop. The audio thread accumulates capture reads into a FIFO
/// and runs one analyze/beat/… frame per exactly this many samples, so spectral-flux and
/// onset amplitudes, the tempo frame rate (`sr / ANALYSIS_HOP`), and A18's decimation no
/// longer drift with scheduler jitter or read-burst size. 512 @ 44.1 kHz ≈ 11.6 ms/frame
/// (87.5% overlap of the 4096-sample analysis window).
pub const ANALYSIS_HOP: usize = 512;

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
) {
    let mut analyzer = FftAnalyzer::new(sample_rate, band_scale);
    let mut normalizer = FeatureNormalizer::new();
    let mut beat_detector = BeatDetector::new(sample_rate);
    let mut key_detector = KeyDetector::new(sample_rate);
    let mut loudness_meter = LoudnessMeter::new(sample_rate);
    let mut downbeat_tracker = DownbeatTracker::new();
    let mut structure_tracker = StructureTracker::new(sample_rate / ANALYSIS_HOP as f32);
    let mut smoother = FeatureSmoother::new();
    let mut read_buf = vec![0.0f32; 8192]; // larger for 4096-pt FFT

    // A5 (#1456): accumulate capture reads here and analyze exactly ANALYSIS_HOP samples at
    // a time. `samples_consumed` is a sample clock — each frame's timestamp is derived from
    // it, so frame timing is exact and independent of scheduler jitter or read-burst size.
    let mut fifo: Vec<f32> = Vec::with_capacity(read_buf.len() + ANALYSIS_HOP);
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

        // Mirror samples to recording ring (lock-free, no overhead if nobody reads).
        recording_ring.push(&read_buf[..read]);
        // Queue for hop-aligned analysis.
        fifo.extend_from_slice(&read_buf[..read]);

        // Process every complete hop the read produced (>=2 when catching up after a stall).
        let mut offset = 0;
        while fifo.len() - offset >= ANALYSIS_HOP {
            let hop = &fifo[offset..offset + ANALYSIS_HOP];
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

            // Beat detection (on raw magnitude spectra)
            let beat_result = beat_detector.process(
                analyzer.bass_magnitude(),
                analyzer.mid_magnitude(),
                analyzer.high_magnitude(),
                raw.rms,
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
            let structure = structure_tracker.process(&pre_norm, &beat_result, timestamp);
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
            };

            // Non-blocking send; drop if main thread is behind
            let _ = tx.try_send(frame);
        }

        // Drop the samples we consumed; keep the sub-hop remainder for next time.
        if offset > 0 {
            fifo.drain(..offset);
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
                // bpm (18) and the categorical key fields key_class (49) / key_is_minor
                // (50) hold their last value rather than sweeping toward silence.
                18 | 49 | 50 => assert!(approx_eq(v, 1.0, 1e-6), "index {i} must hold"),
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
