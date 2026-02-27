use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::Result;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::Stream;

/// Suppress noisy ALSA/JACK stderr messages during device enumeration.
/// ALSA/JACK C libraries print errors for missing JACK server, OSS devices, etc.
#[cfg(target_os = "linux")]
pub fn suppress_alsa_errors() {
    // Install a no-op ALSA error handler (the actual C type is variadic, but our
    // handler never reads va_args so a non-variadic extern "C" fn works fine).
    type AlsaErrorHandler = unsafe extern "C" fn(
        *const std::ffi::c_char,
        std::ffi::c_int,
        *const std::ffi::c_char,
        std::ffi::c_int,
        *const std::ffi::c_char,
    );
    unsafe extern "C" {
        fn snd_lib_error_set_handler(handler: Option<AlsaErrorHandler>) -> std::ffi::c_int;
    }
    unsafe extern "C" fn null_handler(
        _file: *const std::ffi::c_char,
        _line: std::ffi::c_int,
        _function: *const std::ffi::c_char,
        _err: std::ffi::c_int,
        _fmt: *const std::ffi::c_char,
    ) {
    }
    unsafe {
        snd_lib_error_set_handler(Some(null_handler));
    }
    // Suppress JACK "cannot connect to server" messages
    // Safety: called once at startup before any threads are spawned.
    unsafe { std::env::set_var("JACK_NO_START_SERVER", "1") };
}

#[cfg(not(target_os = "linux"))]
pub fn suppress_alsa_errors() {}

/// Ring buffer size (power of 2 for fast modular arithmetic).
const RING_SIZE: usize = 65536;
const RING_MASK: u32 = (RING_SIZE - 1) as u32;

/// Lock-free single-producer single-consumer ring buffer for audio samples.
pub struct RingBuffer {
    data: Box<[f32; RING_SIZE]>,
    write_pos: AtomicU32,
    read_pos: AtomicU32,
}

impl RingBuffer {
    pub fn new() -> Self {
        Self {
            data: Box::new([0.0; RING_SIZE]),
            write_pos: AtomicU32::new(0),
            read_pos: AtomicU32::new(0),
        }
    }

    /// Push samples (called from cpal callback thread).
    /// Safety: Only one thread should call push at a time (the cpal callback).
    pub fn push(&self, samples: &[f32]) {
        let mut wp = self.write_pos.load(Ordering::Relaxed);
        for &sample in samples {
            // Safety: we're the only writer, and RING_SIZE is a power of 2
            let idx = (wp & RING_MASK) as usize;
            // This is safe because we're the only writer and readers
            // can tolerate stale data gracefully.
            unsafe {
                let ptr = self.data.as_ptr() as *mut f32;
                *ptr.add(idx) = sample;
            }
            wp = wp.wrapping_add(1);
        }
        self.write_pos.store(wp, Ordering::Release);
    }

    /// Read available samples into dst. Returns number of samples read.
    pub fn read(&self, dst: &mut [f32]) -> usize {
        let wp = self.write_pos.load(Ordering::Acquire);
        let rp = self.read_pos.load(Ordering::Relaxed);
        let available = wp.wrapping_sub(rp) as usize;
        let to_read = available.min(dst.len());

        for i in 0..to_read {
            let idx = (rp.wrapping_add(i as u32) & RING_MASK) as usize;
            dst[i] = self.data[idx];
        }

        self.read_pos
            .store(rp.wrapping_add(to_read as u32), Ordering::Release);
        to_read
    }

    /// Number of samples available to read.
    pub fn available(&self) -> usize {
        let wp = self.write_pos.load(Ordering::Acquire);
        let rp = self.read_pos.load(Ordering::Relaxed);
        wp.wrapping_sub(rp) as usize
    }
}

// Safety: RingBuffer uses atomics for synchronization
unsafe impl Send for RingBuffer {}
unsafe impl Sync for RingBuffer {}

pub struct AudioCapture {
    _stream: Stream,
    pub ring: Arc<RingBuffer>,
    pub sample_rate: u32,
    pub device_name: String,
    pub callback_count: Arc<AtomicU64>,
}

impl AudioCapture {
    pub fn new() -> Result<Self> {
        Self::new_with_device(None)
    }

    pub fn new_with_device(device_name: Option<&str>) -> Result<Self> {
        let host = cpal::default_host();
        let device = if let Some(name) = device_name {
            // Try to find device by name
            let found = host.input_devices().ok().and_then(|mut devices| {
                devices.find(|d| {
                    d.description()
                        .ok()
                        .map(|desc| desc.name() == name)
                        .unwrap_or(false)
                })
            });
            match found {
                Some(d) => d,
                None => {
                    log::warn!("Audio device '{name}' not found, falling back to default");
                    host.default_input_device()
                        .ok_or_else(|| anyhow::anyhow!("No audio input device found"))?
                }
            }
        } else {
            host.default_input_device()
                .ok_or_else(|| anyhow::anyhow!("No audio input device found"))?
        };

        let device_name = device.description().map(|d| d.name().to_string()).unwrap_or_else(|_| "Unknown".into());
        log::info!("Audio capture device: {device_name}");

        let config = device.default_input_config()?;
        let sample_rate = config.sample_rate();
        let channels = config.channels() as usize;
        log::info!("Audio config: {sample_rate}Hz, {channels}ch, {:?}", config.sample_format());

        // Retry loop: PipeWire's ALSA plugin can race with stream setup,
        // causing callbacks to never fire. Rebuilding the stream gives PipeWire
        // time to finish routing setup.
        let max_attempts = 3;
        for attempt in 1..=max_attempts {
            let ring = Arc::new(RingBuffer::new());
            let ring_clone = ring.clone();
            let callback_count = Arc::new(AtomicU64::new(0));
            let callback_count_clone = callback_count.clone();

            let stream = device.build_input_stream(
                &config.clone().into(),
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    let count = callback_count_clone.fetch_add(1, Ordering::Relaxed);
                    if count == 0 {
                        log::info!("Audio callback fired (first data: {} samples)", data.len());
                    }
                    // Downmix to mono
                    if channels == 1 {
                        ring_clone.push(data);
                    } else {
                        let mono: Vec<f32> = data
                            .chunks(channels)
                            .map(|frame| frame.iter().sum::<f32>() / channels as f32)
                            .collect();
                        ring_clone.push(&mono);
                    }
                },
                |err| {
                    log::error!("Audio stream error: {err}");
                },
                None,
            )?;

            stream.play()?;

            // Wait briefly for callbacks to start arriving
            let wait_ms = 200 * attempt as u64;
            std::thread::sleep(std::time::Duration::from_millis(wait_ms));

            if callback_count.load(Ordering::Relaxed) > 0 {
                log::info!("Audio capture started (attempt {attempt})");
                return Ok(Self {
                    _stream: stream,
                    ring,
                    sample_rate,
                    device_name,
                    callback_count,
                });
            }

            if attempt < max_attempts {
                log::warn!(
                    "Audio callbacks not firing after {wait_ms}ms (attempt {attempt}/{max_attempts}), retrying..."
                );
                // Drop stream before retry — releases ALSA device
                drop(stream);
                std::thread::sleep(std::time::Duration::from_millis(100));
            } else {
                // Last attempt: return the stream anyway; health monitoring will warn later
                log::warn!(
                    "Audio callbacks not firing after {max_attempts} attempts — stream may be stalled"
                );
                return Ok(Self {
                    _stream: stream,
                    ring,
                    sample_rate,
                    device_name,
                    callback_count,
                });
            }
        }

        unreachable!()
    }

    pub fn callback_count(&self) -> u64 {
        self.callback_count.load(Ordering::Relaxed)
    }

    pub fn list_devices() -> Vec<String> {
        let host = cpal::default_host();
        host.input_devices()
            .map(|devices| {
                devices
                    .filter(|d| d.default_input_config().is_ok())
                    .filter_map(|d| d.description().ok().map(|desc| desc.name().to_string()))
                    .collect()
            })
            .unwrap_or_default()
    }
}
