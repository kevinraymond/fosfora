pub mod analyzer;
pub mod beat;
pub mod capture;
pub mod features;
pub mod normalizer;
#[cfg(target_os = "linux")]
pub mod pulse_capture;
pub mod smoother;

pub use features::AudioFeatures;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
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
enum CaptureBackend {
    Cpal(AudioCapture),
    #[cfg(target_os = "linux")]
    Pulse(pulse_capture::PulseCapture),
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
}

impl AudioSystem {
    pub fn new() -> Self {
        Self::new_with_device(None)
    }

    pub fn new_with_device(device_name: Option<&str>) -> Self {
        let (tx, rx): (Sender<AudioFeatures>, Receiver<AudioFeatures>) =
            crossbeam_channel::bounded(4);

        let shutdown = Arc::new(AtomicBool::new(false));
        let callback_count = Arc::new(AtomicU64::new(0));
        let mut resolved_name = device_name.unwrap_or("Default").to_string();
        let mut active = false;
        let mut last_error = None;
        let mut thread_handle = None;
        let mut capture_backend: Option<CaptureBackend> = None;

        // On Linux, try PulseAudio first (bypasses ALSA, works reliably with PipeWire).
        // PulseAudio uses the system default input device (configurable via pavucontrol/pipewire).
        #[cfg(target_os = "linux")]
        {
            match pulse_capture::PulseCapture::new() {
                Ok(pulse) => {
                    resolved_name = pulse.device_name.clone();
                    active = true;
                    let sample_rate = pulse.sample_rate as f32;
                    let ring = pulse.ring.clone();
                    let cb_count = pulse.callback_count.clone();
                    let shutdown_flag = shutdown.clone();

                    thread_handle = Some(
                        thread::Builder::new()
                            .name("phosphor-audio".into())
                            .spawn(move || {
                                audio_thread(ring, sample_rate, tx, shutdown_flag);
                            })
                            .expect("Failed to spawn audio thread"),
                    );

                    capture_backend = Some(CaptureBackend::Pulse(pulse));

                    return Self {
                        receiver: rx,
                        latest: None,
                        device_name: resolved_name,
                        active,
                        last_error,
                        shutdown,
                        thread_handle,
                        callback_count: cb_count,
                        started_at: Instant::now(),
                        _capture: capture_backend,
                    };
                }
                Err(e) => {
                    log::info!("PulseAudio unavailable ({e}), falling back to ALSA");
                }
            }
        }

        // cpal/ALSA backend (primary on macOS/Windows, fallback on Linux)
        match AudioCapture::new_with_device(device_name) {
            Ok(capture) => {
                resolved_name = capture.device_name.clone();
                active = true;
                let sample_rate = capture.sample_rate as f32;
                let ring = capture.ring.clone();
                let cb_count = capture.callback_count.clone();
                let shutdown_flag = shutdown.clone();

                thread_handle = Some(
                    thread::Builder::new()
                        .name("phosphor-audio".into())
                        .spawn(move || {
                            audio_thread(ring, sample_rate, tx, shutdown_flag);
                        })
                        .expect("Failed to spawn audio thread"),
                );

                capture_backend = Some(CaptureBackend::Cpal(capture));

                return Self {
                    receiver: rx,
                    latest: None,
                    device_name: resolved_name,
                    active,
                    last_error,
                    shutdown,
                    thread_handle,
                    callback_count: cb_count,
                    started_at: Instant::now(),
                    _capture: capture_backend,
                };
            }
            Err(e) => {
                log::warn!("Audio capture unavailable: {e}");
                last_error = Some(format!("{e}"));
            }
        }

        Self {
            receiver: rx,
            latest: None,
            device_name: resolved_name,
            active,
            last_error,
            shutdown,
            thread_handle,
            callback_count,
            started_at: Instant::now(),
            _capture: None,
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

        // Create new system
        let new = Self::new_with_device(device_name);
        self.receiver = new.receiver;
        self.latest = None;
        self.device_name = new.device_name;
        self.active = new.active;
        self.last_error = new.last_error;
        self.shutdown = new.shutdown;
        self.thread_handle = new.thread_handle;
        self.callback_count = new.callback_count;
        self.started_at = new.started_at;
        self._capture = new._capture;
    }

    /// List available input devices.
    pub fn list_devices() -> Vec<String> {
        AudioCapture::list_devices()
    }

    /// Drain the channel and return the most recent features.
    pub fn latest_features(&mut self) -> Option<AudioFeatures> {
        while let Ok(features) = self.receiver.try_recv() {
            self.latest = Some(features);
        }

        // Health check: detect stalled audio callbacks
        if self.active
            && self.last_error.is_none()
            && self.callback_count.load(Ordering::Relaxed) == 0
            && self.started_at.elapsed() > Duration::from_secs(3)
        {
            let msg = "Device opened but no audio data received — check audio routing";
            log::warn!("{msg}");
            self.last_error = Some(msg.to_string());
        }

        self.latest
    }
}

fn audio_thread(ring: Arc<RingBuffer>, sample_rate: f32, tx: Sender<AudioFeatures>, shutdown: Arc<AtomicBool>) {
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

        // Smoothing (per-feature asymmetric EMA; beat/beat_phase pass through)
        let smoothed = smoother.smooth(&raw, dt);

        // Non-blocking send; drop if main thread is behind
        let _ = tx.try_send(smoothed);
    }
}
