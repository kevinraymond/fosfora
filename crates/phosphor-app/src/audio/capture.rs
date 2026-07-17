use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use anyhow::Result;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Sample, SampleFormat, Stream};

/// Suppress noisy ALSA/JACK stderr messages during device enumeration.
/// ALSA/JACK C libraries print errors for missing JACK server, OSS devices, etc.
#[cfg(target_os = "linux")]
pub fn suppress_audio_library_noise() {
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
    // SAFETY: snd_lib_error_set_handler is a well-defined ALSA API. Our null_handler
    // matches the expected signature (minus va_args which we don't read). Called at
    // startup to suppress noisy ALSA error output.
    unsafe {
        snd_lib_error_set_handler(Some(null_handler));
    }
    // SAFETY: Called once at startup before any threads are spawned, so no data race
    // on the environment. Suppresses JACK "cannot connect to server" messages.
    unsafe { std::env::set_var("JACK_NO_START_SERVER", "1") };

    // Also try to silence JACK callbacks now (may be no-op if libjack not yet loaded)
    suppress_jack_errors();
}

/// Silence JACK error/info callbacks by loading libjack and installing no-op handlers.
/// Must be called *before* cpal device enumeration so ALSA's JACK plugin inherits the handlers.
/// Safe to call multiple times; no-op if libjack is not installed.
#[cfg(target_os = "linux")]
pub fn suppress_jack_errors() {
    type JackMsgCallback = unsafe extern "C" fn(*const std::ffi::c_char);
    type JackSetFn = unsafe extern "C" fn(Option<JackMsgCallback>);
    unsafe extern "C" fn jack_null_handler(_msg: *const std::ffi::c_char) {}
    unsafe extern "C" {
        fn dlopen(
            filename: *const std::ffi::c_char,
            flags: std::ffi::c_int,
        ) -> *mut std::ffi::c_void;
        fn dlsym(
            handle: *mut std::ffi::c_void,
            symbol: *const std::ffi::c_char,
        ) -> *mut std::ffi::c_void;
        fn dlclose(handle: *mut std::ffi::c_void) -> std::ffi::c_int;
    }
    const RTLD_NOW: std::ffi::c_int = 2;
    // SAFETY: dlopen/dlsym/dlclose are standard POSIX APIs. We check for null returns
    // before using handles/symbols. transmute converts the dlsym void pointer to the
    // correct JACK callback setter signature. jack_null_handler matches the expected
    // JackMsgCallback type. The library handle is closed after installing handlers.
    unsafe {
        // Proactively load libjack so we can install handlers before ALSA's JACK plugin runs
        let handle = dlopen(c"libjack.so.0".as_ptr(), RTLD_NOW);
        if !handle.is_null() {
            for name in [c"jack_set_error_function", c"jack_set_info_function"] {
                let sym = dlsym(handle, name.as_ptr());
                if !sym.is_null() {
                    let set_fn: JackSetFn = std::mem::transmute(sym);
                    set_fn(Some(jack_null_handler));
                }
            }
            dlclose(handle);
        }
    }
}

#[cfg(not(target_os = "linux"))]
pub fn suppress_jack_errors() {}

#[cfg(not(target_os = "linux"))]
pub fn suppress_audio_library_noise() {}

/// Ring buffer size (power of 2 for fast modular arithmetic).
const RING_SIZE: usize = 65536;
const RING_MASK: u32 = (RING_SIZE - 1) as u32;

/// Lock-free single-producer single-consumer ring buffer for audio samples.
pub struct RingBuffer {
    data: Box<[f32]>,
    write_pos: AtomicU32,
    read_pos: AtomicU32,
}

impl RingBuffer {
    pub fn new() -> Self {
        Self {
            data: vec![0.0; RING_SIZE].into_boxed_slice(),
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
            // SAFETY: Single-producer guarantee: only one thread calls push() (the audio
            // callback). The write position is updated atomically with Release ordering
            // after all writes. Readers tolerate momentarily stale data gracefully.
            unsafe {
                let ptr = self.data.as_ptr().cast_mut();
                *ptr.add(idx) = sample;
            }
            wp = wp.wrapping_add(1);
        }
        self.write_pos.store(wp, Ordering::Release);
    }

    /// Read available samples into dst. Returns number of samples read.
    pub fn read(&self, dst: &mut [f32]) -> usize {
        let wp = self.write_pos.load(Ordering::Acquire);
        let mut rp = self.read_pos.load(Ordering::Relaxed);
        let raw = wp.wrapping_sub(rp) as usize;
        // Consumer overrun: the producer lapped us, so anything older than the
        // newest RING_SIZE samples has been overwritten. Skip ahead to the newest
        // full window (read_pos is consumer-owned, so this is race-free).
        if raw > RING_SIZE {
            rp = wp.wrapping_sub(RING_SIZE as u32);
        }
        let available = raw.min(RING_SIZE);
        let to_read = available.min(dst.len());

        for i in 0..to_read {
            let idx = (rp.wrapping_add(i as u32) & RING_MASK) as usize;
            dst[i] = self.data[idx];
        }

        self.read_pos
            .store(rp.wrapping_add(to_read as u32), Ordering::Release);
        to_read
    }

    /// Copy the newest `dst.len()` samples without advancing the read cursor.
    ///
    /// Non-consuming: unlike `read`, this leaves `read_pos` untouched, so the recording
    /// consumer's stream is unaffected. Used render-side by the A17 waveform texture
    /// (#1468), which wants the freshest window every frame regardless of what the
    /// recorder has drained. Returns the number of samples copied (`dst.len()`, capped at
    /// the ring capacity). Before the ring has produced a full window the leading slots
    /// read as zero-initialized silence, which is harmless for a scope trace.
    pub fn peek_latest(&self, dst: &mut [f32]) -> usize {
        let wp = self.write_pos.load(Ordering::Acquire);
        // Newest window is [wp - n, wp), read via wrapping_sub so it stays correct across
        // the u32 write_pos wraparound (the ring is zero-filled, so pre-roll slots are 0).
        let n = dst.len().min(RING_SIZE);
        let start = wp.wrapping_sub(n as u32);
        for (i, slot) in dst[..n].iter_mut().enumerate() {
            let idx = (start.wrapping_add(i as u32) & RING_MASK) as usize;
            *slot = self.data[idx];
        }
        n
    }

    /// Number of samples available to read, capped at the ring capacity
    /// (anything older has already been overwritten by the producer).
    pub fn available(&self) -> usize {
        let wp = self.write_pos.load(Ordering::Acquire);
        let rp = self.read_pos.load(Ordering::Relaxed);
        (wp.wrapping_sub(rp) as usize).min(RING_SIZE)
    }

    /// Skip the read position to the current write position, discarding history.
    /// Consumer-side call; safe while the producer is pushing (SPSC).
    pub fn skip_to_write_pos(&self) {
        let wp = self.write_pos.load(Ordering::Acquire);
        self.read_pos.store(wp, Ordering::Release);
    }
}

// SAFETY: RingBuffer uses atomic u32 positions for synchronization (Acquire/Release).
// The single-producer constraint is upheld by design: only one thread calls push().
// Readers use atomic loads and only access indices behind the write position.
unsafe impl Send for RingBuffer {}
// SAFETY: See above — atomics provide the cross-thread synchronization guarantees.
unsafe impl Sync for RingBuffer {}

pub struct AudioCapture {
    _stream: Stream,
    pub ring: Arc<RingBuffer>,
    pub sample_rate: u32,
    pub device_name: String,
    #[allow(dead_code)]
    pub callback_count: Arc<AtomicU64>,
    /// A9 (#1460): set by cpal's error callback when the device goes away. See
    /// [`err_callback`].
    pub capture_failed: Arc<AtomicBool>,
}

/// A9 (#1460): build cpal's stream error callback, publishing device loss to the watchdog.
///
/// `DeviceNotAvailable`/`StreamInvalidated` mean the device is gone, so the watchdog can
/// reconnect at once instead of waiting out the 10s stall window. `BufferUnderrun` is
/// deliberately excluded — it is a glitch on a live device, not a death.
///
/// A factory rather than a plain closure because each `sample_format` arm below builds its own
/// stream and so needs its own `Arc` clone.
fn err_callback(capture_failed: Arc<AtomicBool>) -> impl FnMut(cpal::StreamError) + Send + 'static {
    move |err: cpal::StreamError| {
        log::error!("Audio stream error: {err}");
        if matches!(
            err,
            cpal::StreamError::DeviceNotAvailable | cpal::StreamError::StreamInvalidated
        ) {
            capture_failed.store(true, Ordering::Release);
        }
    }
}

impl AudioCapture {
    #[allow(dead_code)]
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

        let device_name = device
            .description()
            .map(|d| d.name().to_string())
            .unwrap_or_else(|_| "Unknown".into());
        log::info!("Audio capture device: {device_name}");

        let config = device.default_input_config()?;
        let sample_rate = config.sample_rate();
        let channels = config.channels() as usize;
        log::info!(
            "Audio config: {sample_rate}Hz, {channels}ch, {:?}",
            config.sample_format()
        );

        let ring = Arc::new(RingBuffer::new());
        let callback_count = Arc::new(AtomicU64::new(0));
        let capture_failed = Arc::new(AtomicBool::new(false));

        let sample_format = config.sample_format();
        let stream_config: cpal::StreamConfig = config.into();

        let stream = match sample_format {
            SampleFormat::I16 => {
                let ring_clone = ring.clone();
                let cb_clone = callback_count.clone();
                device.build_input_stream(
                    &stream_config,
                    move |data: &[i16], _: &cpal::InputCallbackInfo| {
                        push_samples(&ring_clone, &cb_clone, data, channels);
                    },
                    err_callback(capture_failed.clone()),
                    None,
                )?
            }
            SampleFormat::I32 => {
                let ring_clone = ring.clone();
                let cb_clone = callback_count.clone();
                device.build_input_stream(
                    &stream_config,
                    move |data: &[i32], _: &cpal::InputCallbackInfo| {
                        push_samples(&ring_clone, &cb_clone, data, channels);
                    },
                    err_callback(capture_failed.clone()),
                    None,
                )?
            }
            _ => {
                let ring_clone = ring.clone();
                let cb_clone = callback_count.clone();
                device.build_input_stream(
                    &stream_config,
                    move |data: &[f32], _: &cpal::InputCallbackInfo| {
                        push_samples(&ring_clone, &cb_clone, data, channels);
                    },
                    err_callback(capture_failed.clone()),
                    None,
                )?
            }
        };

        stream.play()?;
        log::info!("Audio capture started");

        Ok(Self {
            _stream: stream,
            ring,
            sample_rate,
            device_name,
            callback_count,
            capture_failed,
        })
    }

    #[allow(dead_code)]
    pub fn callback_count(&self) -> u64 {
        self.callback_count.load(Ordering::Relaxed)
    }

    pub fn list_devices() -> Vec<String> {
        let host = cpal::default_host();
        let mut seen = std::collections::HashSet::new();
        host.input_devices()
            .map(|devices| {
                devices
                    .filter(|d| d.default_input_config().is_ok())
                    .filter_map(|d| d.description().ok().map(|desc| desc.name().to_string()))
                    .filter(|name| seen.insert(name.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// Convert any cpal sample type to f32 and push interleaved `L,R` stereo to the ring buffer.
///
/// A13 (#1464): the ring carries native stereo (front L/R pair) so the analysis thread can measure
/// pan/width/correlation; it derives the mono mix downstream. For >2 channels we take channels 0/1
/// as L/R; a mono source is duplicated to both. The push is always even-length, which the ring's
/// L/R parity invariant relies on (see [`RingBuffer::read`]).
fn push_samples<T: Sample>(
    ring: &RingBuffer,
    callback_count: &AtomicU64,
    data: &[T],
    channels: usize,
) where
    f32: cpal::FromSample<T>,
{
    let count = callback_count.fetch_add(1, Ordering::Relaxed);
    if count == 0 {
        log::info!("Audio callback fired (first data: {} samples)", data.len());
    }
    let conv = |s: T| <f32 as cpal::FromSample<T>>::from_sample_(s);
    let mut stereo: Vec<f32> = Vec::with_capacity(data.len() / channels.max(1) * 2);
    if channels == 1 {
        for &s in data {
            let v = conv(s);
            stereo.push(v);
            stereo.push(v);
        }
    } else {
        for frame in data.chunks(channels) {
            stereo.push(conv(frame[0]));
            stereo.push(conv(frame[1]));
        }
    }
    ring.push(&stereo);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skip_to_write_pos_discards_history() {
        let ring = RingBuffer::new();
        // Fill with more than a full ring of history (wraps the buffer)
        let junk = vec![1.0f32; 70_000];
        ring.push(&junk);
        ring.skip_to_write_pos();
        assert_eq!(ring.available(), 0);

        // Only samples pushed after the skip should be readable
        let fresh: Vec<f32> = (0..10).map(|i| i as f32).collect();
        ring.push(&fresh);
        assert_eq!(ring.available(), 10);
        let mut dst = [0.0f32; 16];
        let n = ring.read(&mut dst);
        assert_eq!(n, 10);
        assert_eq!(&dst[..10], fresh.as_slice());
    }

    #[test]
    fn peek_latest_is_non_consuming_and_newest() {
        let ring = RingBuffer::new();
        let samples: Vec<f32> = (0..1000).map(|i| i as f32).collect();
        ring.push(&samples);

        // Newest 4 samples are 996..1000, and peeking must not advance read_pos.
        let before = ring.available();
        let mut dst = [0.0f32; 4];
        let n = ring.peek_latest(&mut dst);
        assert_eq!(n, 4);
        assert_eq!(dst, [996.0, 997.0, 998.0, 999.0]);
        assert_eq!(ring.available(), before, "peek must not consume");

        // A subsequent consuming read still sees the full history from the start.
        let mut rd = [0.0f32; 8];
        let m = ring.read(&mut rd);
        assert_eq!(m, 8);
        assert_eq!(&rd, &[0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0]);
    }

    #[test]
    fn peek_latest_warmup_zero_padded() {
        let ring = RingBuffer::new();
        // Only 3 samples pushed; a peek of 5 returns the newest window with the two
        // leading slots reading as zero-initialized silence.
        ring.push(&[7.0, 8.0, 9.0]);
        let mut dst = [-1.0f32; 5];
        let n = ring.peek_latest(&mut dst);
        assert_eq!(n, 5);
        assert_eq!(dst, [0.0, 0.0, 7.0, 8.0, 9.0]);
    }

    #[test]
    fn overrun_clamps_to_newest_window() {
        let ring = RingBuffer::new();
        // Push two full rings without reading: values 0..2*RING_SIZE
        let samples: Vec<f32> = (0..2 * RING_SIZE).map(|i| i as f32).collect();
        ring.push(&samples);
        assert_eq!(ring.available(), RING_SIZE);

        // read() must clamp to the newest full window: RING_SIZE..2*RING_SIZE
        let mut dst = vec![0.0f32; RING_SIZE];
        let n = ring.read(&mut dst);
        assert_eq!(n, RING_SIZE);
        assert_eq!(dst[0], RING_SIZE as f32);
        assert_eq!(dst[RING_SIZE - 1], (2 * RING_SIZE - 1) as f32);
        // Everything consumed
        assert_eq!(ring.available(), 0);
    }
}
