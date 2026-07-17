//! PulseAudio capture backend for Linux.
//! Runtime-loaded via dlopen — no compile-time libpulse dependency.
//! Bypasses ALSA entirely by connecting directly to PipeWire's pipewire-pulse
//! service (or native PulseAudio).

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use libloading::{Library, Symbol};

use super::capture::RingBuffer;

// --- PulseAudio C ABI types ---

const PA_SAMPLE_FLOAT32LE: u32 = 5;
const PA_STREAM_RECORD: u32 = 2;

#[repr(C)]
struct pa_sample_spec {
    format: u32,
    rate: u32,
    channels: u8,
}

#[repr(C)]
struct pa_buffer_attr {
    maxlength: u32,
    tlength: u32,
    prebuf: u32,
    minreq: u32,
    fragsize: u32,
}

/// Opaque handle returned by pa_simple_new.
#[allow(non_camel_case_types)]
enum pa_simple {}

// --- Runtime-loaded function table ---

struct PulseLib {
    _lib_simple: Library,
    _lib_pulse: Library,
    pa_simple_new: unsafe extern "C" fn(
        server: *const c_char,
        name: *const c_char,
        dir: u32,
        dev: *const c_char,
        stream_name: *const c_char,
        ss: *const pa_sample_spec,
        map: *const c_void,
        attr: *const pa_buffer_attr,
        error: *mut c_int,
    ) -> *mut pa_simple,
    pa_simple_read: unsafe extern "C" fn(
        s: *mut pa_simple,
        data: *mut c_void,
        bytes: usize,
        error: *mut c_int,
    ) -> c_int,
    pa_simple_get_latency: unsafe extern "C" fn(s: *mut pa_simple, error: *mut c_int) -> u64,
    pa_simple_free: unsafe extern "C" fn(s: *mut pa_simple),
    pa_strerror: unsafe extern "C" fn(error: c_int) -> *const c_char,
}

// SAFETY: PulseLib stores function pointers and library handles. The function pointers
// are valid for the lifetime of the loaded libraries (held by _lib_simple/_lib_pulse).
// pa_simple is only accessed from the capture thread after creation.
unsafe impl Send for PulseLib {}
// SAFETY: See above — function pointers are inherently Sync (read-only after load).
unsafe impl Sync for PulseLib {}

impl PulseLib {
    fn load() -> Result<Self> {
        // SAFETY: Loading shared libraries and resolving symbols via dlopen/dlsym.
        // The returned function pointers are valid as long as _lib_simple and _lib_pulse
        // are kept alive (stored in Self). Symbol names are null-terminated and match
        // the PulseAudio Simple API.
        unsafe {
            // Load libpulse first (libpulse-simple depends on it)
            let lib_pulse = Library::new("libpulse.so.0")
                .map_err(|e| anyhow::anyhow!("Cannot load libpulse.so.0: {e}"))?;
            let lib_simple = Library::new("libpulse-simple.so.0")
                .map_err(|e| anyhow::anyhow!("Cannot load libpulse-simple.so.0: {e}"))?;

            let pa_simple_new: Symbol<
                unsafe extern "C" fn(
                    *const c_char,
                    *const c_char,
                    u32,
                    *const c_char,
                    *const c_char,
                    *const pa_sample_spec,
                    *const c_void,
                    *const pa_buffer_attr,
                    *mut c_int,
                ) -> *mut pa_simple,
            > = lib_simple.get(b"pa_simple_new\0")?;

            let pa_simple_read: Symbol<
                unsafe extern "C" fn(*mut pa_simple, *mut c_void, usize, *mut c_int) -> c_int,
            > = lib_simple.get(b"pa_simple_read\0")?;

            let pa_simple_get_latency: Symbol<
                unsafe extern "C" fn(*mut pa_simple, *mut c_int) -> u64,
            > = lib_simple.get(b"pa_simple_get_latency\0")?;

            let pa_simple_free: Symbol<unsafe extern "C" fn(*mut pa_simple)> =
                lib_simple.get(b"pa_simple_free\0")?;

            let pa_strerror: Symbol<unsafe extern "C" fn(c_int) -> *const c_char> =
                lib_pulse.get(b"pa_strerror\0")?;

            Ok(Self {
                pa_simple_new: *pa_simple_new,
                pa_simple_read: *pa_simple_read,
                pa_simple_get_latency: *pa_simple_get_latency,
                pa_simple_free: *pa_simple_free,
                pa_strerror: *pa_strerror,
                _lib_simple: lib_simple,
                _lib_pulse: lib_pulse,
            })
        }
    }

    fn strerror(&self, error: c_int) -> String {
        // SAFETY: pa_strerror returns a static string pointer valid for the library's lifetime.
        // We check for null before dereferencing.
        unsafe {
            let ptr = (self.pa_strerror)(error);
            if ptr.is_null() {
                format!("PA error {error}")
            } else {
                CStr::from_ptr(ptr).to_string_lossy().into_owned()
            }
        }
    }
}

/// Check if PulseAudio libraries are available at runtime. Cached.
#[allow(dead_code)]
pub fn pulse_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    // SAFETY: Library::new calls dlopen, which is safe to call to probe for library availability.
    *AVAILABLE.get_or_init(|| unsafe {
        Library::new("libpulse-simple.so.0").is_ok() && Library::new("libpulse.so.0").is_ok()
    })
}

// --- Fragment sizing ---

/// Fragment size: 2048 interleaved f32 samples = 1024 stereo frames = 8192 bytes ≈ 23ms at 44.1kHz.
/// A13 (#1464): the stream is stereo, so a fragment holds `FRAG_SAMPLES/2` L/R frames.
const FRAG_SAMPLES: usize = 2048;
const FRAG_BYTES: u32 = (FRAG_SAMPLES * std::mem::size_of::<f32>()) as u32;

/// How often to log health stats (seconds).
const HEALTH_LOG_INTERVAL: f64 = 30.0;

/// A9 (#1460): consecutive `pa_simple_read` failures before the thread declares the stream
/// dead and exits. `pa_simple` has no recovery — once the server kills the stream every read
/// fails — so retrying past this is just log spam. ~1s at the 100ms retry cadence.
const MAX_CONSECUTIVE_ERRORS: u32 = 10;

// --- PulseCapture ---

pub struct PulseCapture {
    pub ring: Arc<RingBuffer>,
    pub sample_rate: u32,
    pub device_name: String,
    pub callback_count: Arc<AtomicU64>,
    /// A9 (#1460): set by the read thread when it gives up on a stream the server has killed.
    /// A positive death signal — unlike a frozen `callback_count`, which on a monitor source
    /// only means the sink is idle.
    pub capture_failed: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
    thread_handle: Option<thread::JoinHandle<()>>,
}

/// Query PulseAudio for the default sink's monitor source name.
///
/// `None` on any of: `pactl` missing, non-zero exit, or empty output — callers fall back to
/// the PA default source. Forks a subprocess, so it must never run on the render thread.
///
/// Visible to the parent module for A9b (#1617), which polls it to notice a default-sink
/// change; that poll is the reason the hit is logged at `debug` rather than `info`, since it
/// now runs on a 5s cadence rather than once per device open.
pub(super) fn find_monitor_source() -> Option<String> {
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
    log::debug!("PulseAudio default sink monitor: {monitor}");
    Some(monitor)
}

/// Open a PulseAudio Simple connection with explicit buffer attributes.
/// Returns the raw pa_simple pointer and device name.
fn open_connection(
    lib: &PulseLib,
    app_name: &str,
    stream_desc: &str,
    sample_rate: u32,
) -> Result<(*mut pa_simple, String)> {
    let spec = pa_sample_spec {
        format: PA_SAMPLE_FLOAT32LE,
        rate: sample_rate,
        // A13 (#1464): capture native stereo. The server delivers interleaved L,R, which is exactly
        // the layout the capture ring expects; the analysis thread derives the mono mix.
        channels: 2,
    };

    let attr = pa_buffer_attr {
        maxlength: FRAG_BYTES * 4,
        tlength: u32::MAX,
        prebuf: u32::MAX,
        minreq: u32::MAX,
        fragsize: FRAG_BYTES,
    };

    let monitor_device = find_monitor_source();

    let c_app = CString::new(app_name).expect("app name must not contain null bytes");
    let c_stream = CString::new(stream_desc).expect("stream desc must not contain null bytes");
    let c_dev = monitor_device.as_ref().map(|s| {
        CString::new(s.as_str()).expect("PulseAudio device name must not contain null bytes")
    });
    let dev_ptr = c_dev.as_ref().map_or(std::ptr::null(), |c| c.as_ptr());

    let mut error: c_int = 0;
    // SAFETY: Calling pa_simple_new via FFI. All CString pointers are valid for the
    // duration of this call. The spec and attr structs are repr(C) with correct layout.
    // We check for null return (error) before using the handle.
    let handle = unsafe {
        (lib.pa_simple_new)(
            std::ptr::null(), // default server
            c_app.as_ptr(),
            PA_STREAM_RECORD,
            dev_ptr,
            c_stream.as_ptr(),
            &spec,
            std::ptr::null(), // default channel map
            &attr,
            &mut error,
        )
    };

    if handle.is_null() {
        return Err(anyhow::anyhow!("PulseAudio: {}", lib.strerror(error)));
    }

    let source_desc = monitor_device.as_deref().unwrap_or("default");
    log::info!(
        "PulseAudio capture opened: {source_desc} ({}Hz stereo, fragsize={}B/{}samples)",
        sample_rate,
        FRAG_BYTES,
        FRAG_SAMPLES,
    );

    // Log actual latency if available
    let mut lat_err: c_int = 0;
    // SAFETY: handle is non-null (checked above). pa_simple_get_latency is a simple query.
    let latency_us = unsafe { (lib.pa_simple_get_latency)(handle, &mut lat_err) };
    if lat_err == 0 {
        log::info!(
            "PulseAudio reported latency: {:.1}ms",
            latency_us as f64 / 1000.0
        );
    }

    let name = monitor_device.unwrap_or_else(|| "PulseAudio Default".to_string());
    Ok((handle, name))
}

/// Tracks per-read timing for health logging.
struct ReadStats {
    count: u64,
    total_duration: Duration,
    min_duration: Duration,
    max_duration: Duration,
    last_log: Instant,
    total_samples: u64,
}

impl ReadStats {
    fn new() -> Self {
        Self {
            count: 0,
            total_duration: Duration::ZERO,
            min_duration: Duration::from_secs(999),
            max_duration: Duration::ZERO,
            last_log: Instant::now(),
            total_samples: 0,
        }
    }

    fn record(&mut self, duration: Duration, samples: usize) {
        self.count += 1;
        self.total_duration += duration;
        self.total_samples += samples as u64;
        if duration < self.min_duration {
            self.min_duration = duration;
        }
        if duration > self.max_duration {
            self.max_duration = duration;
        }
    }

    fn log_and_reset(&mut self) {
        let elapsed = self.last_log.elapsed().as_secs_f64();
        if elapsed < HEALTH_LOG_INTERVAL {
            return;
        }

        if self.count > 0 {
            let avg = self.total_duration.as_secs_f64() / self.count as f64;
            let reads_per_sec = self.count as f64 / elapsed;
            let throughput = self.total_samples as f64 / elapsed;
            log::info!(
                "PA health: {:.0} reads/s, latency avg={:.1}ms min={:.1}ms max={:.1}ms, {:.0} samples/s",
                reads_per_sec,
                avg * 1000.0,
                self.min_duration.as_secs_f64() * 1000.0,
                self.max_duration.as_secs_f64() * 1000.0,
                throughput,
            );
        } else {
            log::warn!("PA health: 0 reads in {:.0}s — audio stalled", elapsed);
        }

        self.count = 0;
        self.total_duration = Duration::ZERO;
        self.min_duration = Duration::from_secs(999);
        self.max_duration = Duration::ZERO;
        self.last_log = Instant::now();
        self.total_samples = 0;
    }
}

/// Wrapper that ensures pa_simple_free is called.
/// Stores pointer as usize to avoid Send issues with raw pointers.
struct SimpleHandle {
    ptr: usize,
    free_fn: unsafe extern "C" fn(*mut pa_simple),
}

// SAFETY: SimpleHandle stores a pa_simple pointer as usize. The pointer is only used
// from a single thread (the capture thread) after construction. The free_fn is called
// once on drop.
unsafe impl Send for SimpleHandle {}

impl SimpleHandle {
    fn new(ptr: *mut pa_simple, free_fn: unsafe extern "C" fn(*mut pa_simple)) -> Self {
        Self {
            ptr: ptr as usize,
            free_fn,
        }
    }
    fn as_ptr(&self) -> *mut pa_simple {
        self.ptr as *mut pa_simple
    }
}

impl Drop for SimpleHandle {
    fn drop(&mut self) {
        // SAFETY: self.ptr was obtained from pa_simple_new (non-null, valid).
        // free_fn is pa_simple_free, which is safe to call once on a valid handle.
        unsafe { (self.free_fn)(self.as_ptr()) };
    }
}

impl PulseCapture {
    pub fn new() -> Result<Self> {
        let lib = PulseLib::load()?;
        let sample_rate = 44100u32;
        let (handle, device_name) =
            open_connection(&lib, "phosphor", "audio capture", sample_rate)?;

        let ring = Arc::new(RingBuffer::new());
        let ring_clone = ring.clone();
        let callback_count = Arc::new(AtomicU64::new(0));
        let callback_count_clone = callback_count.clone();
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = shutdown.clone();
        let capture_failed = Arc::new(AtomicBool::new(false));
        let capture_failed_clone = capture_failed.clone();

        let verbose = std::env::var("PHOSPHOR_AUDIO_DEBUG").map_or(false, |v| v == "1");

        // Move the handle + needed function pointers into the thread
        let simple = SimpleHandle::new(handle, lib.pa_simple_free);
        let read_fn = lib.pa_simple_read;
        let strerror_fn = lib.pa_strerror;
        // Keep lib alive by moving it into the thread
        let _lib = lib;

        let thread_handle =
            thread::Builder::new()
                .name("phosphor-pulse".into())
                .spawn(move || {
                    let _lib = _lib; // ensure library stays loaded
                    let mut buf = vec![0u8; FRAG_BYTES as usize];
                    let mut stats = ReadStats::new();
                    let start = Instant::now();
                    let mut consecutive_errors: u32 = 0;

                    loop {
                        if shutdown_clone.load(Ordering::Acquire) {
                            log::info!("PulseAudio read thread shutting down");
                            break;
                        }

                        let t0 = Instant::now();
                        let mut error: c_int = 0;
                        // SAFETY: pa_simple_read FFI call. simple.as_ptr() is a valid
                        // pa_simple handle. buf is a valid, correctly-sized buffer.
                        let ret = unsafe {
                            (read_fn)(
                                simple.as_ptr(),
                                buf.as_mut_ptr().cast::<c_void>(),
                                buf.len(),
                                &mut error,
                            )
                        };

                        if ret < 0 {
                            // SAFETY: pa_strerror returns a static string pointer.
                            // We check for null before dereferencing.
                            let msg = unsafe {
                                let ptr = (strerror_fn)(error);
                                if ptr.is_null() {
                                    format!("PA error {error}")
                                } else {
                                    CStr::from_ptr(ptr).to_string_lossy().into_owned()
                                }
                            };
                            log::error!("PulseAudio read error: {msg}");
                            consecutive_errors += 1;
                            if consecutive_errors >= MAX_CONSECUTIVE_ERRORS {
                                // A9 (#1460): pa_simple cannot recover a stream the server has
                                // killed, so every subsequent read fails too — this loop used
                                // to spin here at 10Hz forever. Publish the death and exit:
                                // the watchdog reconnects, and the reaper's join returns at
                                // once instead of parking a thread.
                                log::error!(
                                    "PulseAudio: {consecutive_errors} consecutive read errors, \
                                     giving up on this stream"
                                );
                                capture_failed_clone.store(true, Ordering::Release);
                                break;
                            }
                            thread::sleep(Duration::from_millis(100));
                            continue;
                        }
                        consecutive_errors = 0;

                        let read_dur = t0.elapsed();
                        let samples: &[f32] = bytemuck::cast_slice(&buf);
                        let count = callback_count_clone.fetch_add(1, Ordering::Relaxed);

                        if count == 0 {
                            let since_start = start.elapsed();
                            log::info!(
                                "PulseAudio first data received: {} samples, {:.0}ms after open",
                                samples.len(),
                                since_start.as_secs_f64() * 1000.0,
                            );
                        }

                        if verbose {
                            log::debug!(
                                "PA read: {}samples in {:.1}ms",
                                samples.len(),
                                read_dur.as_secs_f64() * 1000.0,
                            );
                        }

                        stats.record(read_dur, samples.len());
                        stats.log_and_reset();

                        ring_clone.push(samples);
                    }

                    // simple (SimpleHandle) dropped here — calls pa_simple_free
                    // _lib dropped here — unloads library
                })?;

        Ok(Self {
            ring,
            sample_rate,
            device_name,
            callback_count,
            capture_failed,
            shutdown,
            thread_handle: Some(thread_handle),
        })
    }

    /// Run a standalone diagnostic — opens PA, reads for N seconds, prints stats.
    /// No GPU needed; works over SSH.
    pub fn run_diagnostic(duration_secs: u32) {
        let sample_rate = 44100u32;
        println!("=== Fosfora Audio Diagnostic ===");
        println!("Opening PulseAudio capture...");

        let lib = match PulseLib::load() {
            Ok(v) => v,
            Err(e) => {
                println!("FAIL: Could not load PulseAudio libraries: {e}");
                println!("  - Is PipeWire/PulseAudio installed?");
                println!("  - Try: apt install libpulse0   (or equivalent)");
                std::process::exit(1);
            }
        };

        let (handle, device_name) =
            match open_connection(&lib, "phosphor-diag", "audio test", sample_rate) {
                Ok(v) => v,
                Err(e) => {
                    println!("FAIL: Could not open PulseAudio: {e}");
                    println!("  - Is PipeWire/PulseAudio running?");
                    println!("  - Try: pactl info");
                    std::process::exit(1);
                }
            };

        let simple = SimpleHandle::new(handle, lib.pa_simple_free);

        println!("Source: {device_name}");
        println!("Config: {sample_rate}Hz mono, fragsize={FRAG_BYTES}B ({FRAG_SAMPLES} samples)");
        println!("Reading for {duration_secs}s...\n");

        let mut buf = vec![0u8; FRAG_BYTES as usize];
        let mut stats = DiagnosticStats::new();
        let deadline = Instant::now() + Duration::from_secs(duration_secs as u64);

        while Instant::now() < deadline {
            let t0 = Instant::now();
            let mut error: c_int = 0;
            // SAFETY: pa_simple_read FFI call. simple holds a valid pa_simple handle.
            // buf is correctly sized for FRAG_BYTES of f32 data.
            let ret = unsafe {
                (lib.pa_simple_read)(
                    simple.as_ptr(),
                    buf.as_mut_ptr().cast::<c_void>(),
                    buf.len(),
                    &mut error,
                )
            };

            if ret < 0 {
                println!("  Read error: {}", lib.strerror(error));
                stats.errors += 1;
                thread::sleep(Duration::from_millis(50));
                continue;
            }

            let dur = t0.elapsed();
            let samples: &[f32] = bytemuck::cast_slice(&buf);
            stats.record_read(dur, samples);
        }

        stats.print_report(sample_rate);
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

/// Accumulated stats for the diagnostic mode.
struct DiagnosticStats {
    reads: u64,
    errors: u64,
    total_duration: Duration,
    min_duration: Duration,
    max_duration: Duration,
    total_samples: u64,
    peak_abs: f32,
    sum_sq: f64,
    start: Instant,
}

impl DiagnosticStats {
    fn new() -> Self {
        Self {
            reads: 0,
            errors: 0,
            total_duration: Duration::ZERO,
            min_duration: Duration::from_secs(999),
            max_duration: Duration::ZERO,
            total_samples: 0,
            peak_abs: 0.0,
            sum_sq: 0.0,
            start: Instant::now(),
        }
    }

    fn record_read(&mut self, duration: Duration, samples: &[f32]) {
        self.reads += 1;
        self.total_duration += duration;
        if duration < self.min_duration {
            self.min_duration = duration;
        }
        if duration > self.max_duration {
            self.max_duration = duration;
        }
        self.total_samples += samples.len() as u64;
        for &s in samples {
            let a = s.abs();
            if a > self.peak_abs {
                self.peak_abs = a;
            }
            self.sum_sq += (s as f64) * (s as f64);
        }
    }

    fn print_report(&self, sample_rate: u32) {
        let wall = self.start.elapsed().as_secs_f64();
        let reads_per_sec = if wall > 0.0 {
            self.reads as f64 / wall
        } else {
            0.0
        };
        let throughput = if wall > 0.0 {
            self.total_samples as f64 / wall
        } else {
            0.0
        };

        println!("--- Results ---");
        println!("Reads:      {} ({:.1}/s)", self.reads, reads_per_sec);
        println!("Errors:     {}", self.errors);

        if self.reads > 0 {
            let avg_ms = self.total_duration.as_secs_f64() / self.reads as f64 * 1000.0;
            let min_ms = self.min_duration.as_secs_f64() * 1000.0;
            let max_ms = self.max_duration.as_secs_f64() * 1000.0;
            println!("Read time:  avg={avg_ms:.1}ms  min={min_ms:.1}ms  max={max_ms:.1}ms");
        }

        println!(
            "Throughput: {:.0} samples/s (expect ~{sample_rate})",
            throughput
        );
        println!("Samples:    {} total", self.total_samples);

        let rms = if self.total_samples > 0 {
            (self.sum_sq / self.total_samples as f64).sqrt()
        } else {
            0.0
        };
        let peak_dbfs = if self.peak_abs > 0.0 {
            20.0 * (self.peak_abs as f64).log10()
        } else {
            -120.0
        };
        let rms_dbfs = if rms > 0.0 {
            20.0 * rms.log10()
        } else {
            -120.0
        };
        println!("Peak:       {:.4} ({:.1} dBFS)", self.peak_abs, peak_dbfs);
        println!("RMS:        {:.4} ({:.1} dBFS)", rms, rms_dbfs);

        println!();
        let expected_reads = FRAG_SAMPLES as f64 / sample_rate as f64;
        let expected_rps = 1.0 / expected_reads;
        let ratio = reads_per_sec / expected_rps;

        if self.reads == 0 {
            println!("FAIL: No audio data received at all.");
            println!("  - Check: pactl list sources short");
            println!("  - Check: is audio playing?");
        } else if ratio < 0.5 {
            println!(
                "FAIL: Audio updates too slow ({:.1}/s, expected ~{:.0}/s).",
                reads_per_sec, expected_rps,
            );
            println!("  - fragsize may not be taking effect");
            println!("  - Try: PHOSPHOR_AUDIO_DEBUG=1 cargo run");
        } else if self.peak_abs < 0.0001 {
            println!(
                "WARN: Audio capture working ({:.1} reads/s) but signal is silent.",
                reads_per_sec,
            );
            println!("  - Is audio playing through the default output?");
            println!("  - Check monitor source: pactl get-default-sink");
        } else {
            println!(
                "OK: Audio capture working normally ({:.1} reads/s, peak {:.1} dBFS).",
                reads_per_sec, peak_dbfs,
            );
        }
    }
}
