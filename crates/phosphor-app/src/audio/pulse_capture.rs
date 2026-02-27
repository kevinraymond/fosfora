//! PulseAudio capture backend for Linux.
//! Bypasses ALSA entirely by using libpulse-simple, which connects directly
//! to PipeWire's pipewire-pulse service (or native PulseAudio).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;

use anyhow::Result;
use libpulse_binding::sample::{Format, Spec};
use libpulse_binding::stream::Direction;
use libpulse_simple_binding::Simple;

use super::capture::RingBuffer;

pub struct PulseCapture {
    pub ring: Arc<RingBuffer>,
    pub sample_rate: u32,
    pub device_name: String,
    pub callback_count: Arc<AtomicU64>,
    shutdown: Arc<AtomicBool>,
    thread_handle: Option<thread::JoinHandle<()>>,
}

impl PulseCapture {
    pub fn new() -> Result<Self> {
        let sample_rate = 44100u32;
        let channels = 1u8; // request mono directly from PulseAudio

        let spec = Spec {
            format: Format::FLOAT32NE,
            channels,
            rate: sample_rate,
        };
        assert!(spec.is_valid(), "PulseAudio sample spec is invalid");

        // Find the monitor source for the default output sink.
        // This captures system audio (what the user is listening to) rather than
        // the default recording source (which might be a suspended microphone).
        let monitor_device = Self::find_monitor_source();
        let device_str = monitor_device.as_deref();

        let simple = Simple::new(
            None,                // default server
            "phosphor",          // app name
            Direction::Record,
            device_str,          // monitor of default sink, or None for PA default
            "audio capture",     // stream description
            &spec,
            None,                // default channel map
            None,                // default buffer attributes
        )
        .map_err(|e| anyhow::anyhow!("PulseAudio: {}", e))?;

        let source_desc = device_str.unwrap_or("default");
        log::info!("PulseAudio capture opened: {source_desc} ({}Hz, mono)", sample_rate);

        let ring = Arc::new(RingBuffer::new());
        let ring_clone = ring.clone();
        let callback_count = Arc::new(AtomicU64::new(0));
        let callback_count_clone = callback_count.clone();
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = shutdown.clone();

        // PulseAudio simple API is blocking — read in a dedicated thread
        let thread_handle = thread::Builder::new()
            .name("phosphor-pulse".into())
            .spawn(move || {
                // 1024 f32 samples = 4096 bytes ≈ 23ms at 44.1kHz
                let mut buf = vec![0u8; 1024 * std::mem::size_of::<f32>()];

                loop {
                    if shutdown_clone.load(Ordering::Acquire) {
                        log::info!("PulseAudio read thread shutting down");
                        break;
                    }

                    match simple.read(&mut buf) {
                        Ok(()) => {
                            // Interpret raw bytes as f32 samples
                            let samples: &[f32] = bytemuck::cast_slice(&buf);
                            let count = callback_count_clone.fetch_add(1, Ordering::Relaxed);
                            if count == 0 {
                                log::info!(
                                    "PulseAudio audio data received (first: {} samples)",
                                    samples.len()
                                );
                            }
                            ring_clone.push(samples);
                        }
                        Err(e) => {
                            log::error!("PulseAudio read error: {e}");
                            // Brief pause before retrying to avoid busy-looping on persistent errors
                            thread::sleep(std::time::Duration::from_millis(100));
                        }
                    }
                }

                // simple is dropped here, closing the PulseAudio connection
            })?;

        let device_name = monitor_device
            .as_deref()
            .unwrap_or("PulseAudio Default")
            .to_string();

        Ok(Self {
            ring,
            sample_rate,
            device_name,
            callback_count,
            shutdown,
            thread_handle: Some(thread_handle),
        })
    }

    /// Query PulseAudio for the default sink's monitor source name.
    /// Returns e.g. "alsa_output.usb-Foo.analog-stereo.monitor".
    fn find_monitor_source() -> Option<String> {
        let output = std::process::Command::new("pactl")
            .arg("get-default-sink")
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let sink = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if sink.is_empty() {
            return None;
        }
        let monitor = format!("{sink}.monitor");
        log::info!("PulseAudio default sink monitor: {monitor}");
        Some(monitor)
    }
}

impl Drop for PulseCapture {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }
    }
}
