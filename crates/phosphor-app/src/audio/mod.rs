pub mod analyzer;
pub mod beat;
pub mod capture;
pub mod features;
pub mod normalizer;
#[cfg(target_os = "linux")]
pub mod pulse_capture;
pub mod smoother;
#[cfg(target_os = "windows")]
pub mod wasapi_capture;

pub use features::AudioFeatures;

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};

use self::analyzer::FftAnalyzer;
use self::beat::BeatDetector;
use self::capture::{AudioCapture, RingBuffer};
use self::normalizer::AdaptiveNormalizer;
use self::smoother::FeatureSmoother;

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
    receiver: Receiver<AudioFeatures>,
    latest: Option<AudioFeatures>,
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
    /// Total beats detected, incremented by the audio thread. The consumer compares
    /// this against `beats_seen` so a 1-frame beat pulse survives channel overflow
    /// and the drain-to-newest loop in `latest_features()`.
    beat_counter: Arc<AtomicU32>,
    /// Value of `beat_counter` at the end of the previous `latest_features()` poll.
    beats_seen: u32,
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
        Self::new_with_device(None)
    }

    pub fn new_with_device(device_name: Option<&str>) -> Self {
        let (tx, rx): (Sender<AudioFeatures>, Receiver<AudioFeatures>) =
            crossbeam_channel::bounded(4);

        let shutdown = Arc::new(AtomicBool::new(false));
        let recording_ring = Arc::new(RingBuffer::new());
        let beat_counter = Arc::new(AtomicU32::new(0));

        match open_backend(device_name) {
            Ok(opened) => {
                let shutdown_flag = shutdown.clone();
                let ring = opened.ring.clone();
                let sample_rate = opened.sample_rate;
                let rec_ring = recording_ring.clone();
                let beats = beat_counter.clone();

                let thread_handle = thread::Builder::new()
                    .name("phosphor-audio".into())
                    .spawn(move || {
                        audio_thread(ring, sample_rate, tx, shutdown_flag, rec_ring, beats);
                    })
                    .expect("Failed to spawn audio thread");

                Self {
                    receiver: rx,
                    latest: None,
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
                    beat_counter,
                    beats_seen: 0,
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
                    beat_counter,
                    beats_seen: 0,
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

        // Create new system and swap all fields (mem::replace avoids move-out-of-Drop)
        let mut new = Self::new_with_device(device_name);
        self.receiver = std::mem::replace(&mut new.receiver, crossbeam_channel::bounded(1).1);
        self.latest = None;
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
        // Reset stale-feature decay and watchdog state for the fresh backend.
        self.last_frame_at = Instant::now();
        self.last_poll_at = Instant::now();
        self.last_cb_count = 0;
        self.cb_changed_at = Instant::now();
        self.stall_reported = false;
        // Keep existing cached_devices/scan_in_flight/last_scan — no need to re-scan on switch
        // `new` is dropped here — its Drop is a no-op since thread_handle is None and shutdown is true
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
        while let Ok(features) = self.receiver.try_recv() {
            self.latest = Some(features);
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
        let result = self.latest.map(|mut features| {
            features.beat = if beats_now == self.beats_seen {
                0.0
            } else {
                1.0
            };
            features
        });
        self.beats_seen = beats_now;

        result
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

/// Exponentially decay all features by `k`, except `bpm` (index 18 — a tempo
/// estimate, not an energy level, so it should hold its last value). `beat` is
/// forced to 0 so a stalled device can never hold a beat pulse high.
fn decay_features(features: &mut AudioFeatures, k: f32) {
    /// Index of `bpm` in `AudioFeatures::as_slice()` order.
    const BPM_INDEX: usize = 18;
    for (i, v) in features.as_slice_mut().iter_mut().enumerate() {
        if i != BPM_INDEX {
            *v *= k;
        }
    }
    features.beat = 0.0;
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

fn audio_thread(
    ring: Arc<RingBuffer>,
    sample_rate: f32,
    tx: Sender<AudioFeatures>,
    shutdown: Arc<AtomicBool>,
    recording_ring: Arc<RingBuffer>,
    beat_counter: Arc<AtomicU32>,
) {
    let mut analyzer = FftAnalyzer::new(sample_rate);
    let mut normalizer = AdaptiveNormalizer::new();
    let mut beat_detector = BeatDetector::new(sample_rate);
    let mut smoother = FeatureSmoother::new();
    let mut read_buf = vec![0.0f32; 8192]; // larger for 4096-pt FFT
    let mut last_time = Instant::now();
    let start_time = Instant::now();

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

        // Mirror samples to recording ring (lock-free, no overhead if nobody reads)
        if read > 0 {
            recording_ring.push(&read_buf[..read]);
        }
        if read == 0 {
            continue;
        }

        let now = Instant::now();
        let dt = now.duration_since(last_time).as_secs_f32();
        let timestamp = now.duration_since(start_time).as_secs_f64();
        last_time = now;

        // Multi-resolution FFT + feature extraction
        let mut raw = analyzer.analyze(&read_buf[..read]);

        // Adaptive normalization (replaces fixed gains)
        raw = normalizer.normalize(&raw);

        // Beat detection (on raw magnitude spectra)
        let beat_result = beat_detector.process(
            analyzer.bass_magnitude(),
            analyzer.mid_magnitude(),
            analyzer.high_magnitude(),
            raw.rms,
            timestamp,
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

        // Smoothing (per-feature asymmetric EMA; beat/beat_phase pass through)
        let smoothed = smoother.smooth(&raw, dt);

        // Non-blocking send; drop if main thread is behind
        let _ = tx.try_send(smoothed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() < eps
    }

    #[test]
    fn decay_scales_everything_except_bpm() {
        let mut f = AudioFeatures::default();
        for v in f.as_slice_mut().iter_mut() {
            *v = 1.0;
        }
        decay_features(&mut f, 0.5);
        for (i, &v) in f.as_slice().iter().enumerate() {
            match i {
                16 => assert!(approx_eq(v, 0.0, 1e-6), "beat (16) must be forced to 0"),
                18 => assert!(approx_eq(v, 1.0, 1e-6), "bpm (18) must not decay"),
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
