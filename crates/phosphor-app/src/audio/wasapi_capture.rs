//! WASAPI loopback capture backend for Windows.
//! Uses COM APIs to capture the default audio render device's output mix
//! (desktop audio / what's playing through speakers).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::Duration;

use super::capture::RingBuffer;
use anyhow::Result;
use windows::Win32::Media::Audio::{
    AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK, IAudioCaptureClient, IAudioClient,
    IMMDeviceEnumerator, MMDeviceEnumerator, WAVEFORMATEXTENSIBLE, eConsole, eRender,
};
use windows::Win32::Media::Multimedia::KSDATAFORMAT_SUBTYPE_IEEE_FLOAT;
use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
    CoUninitialize, STGM_READ,
};
use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;

/// Check if WASAPI loopback is available at runtime. Cached.
#[allow(dead_code)]
pub fn wasapi_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    // SAFETY: COM APIs are FFI calls. CoInitializeEx/CoCreateInstance are safe to call
    // from any thread; we balance CoInitializeEx with CoUninitialize when we initialized it.
    *AVAILABLE.get_or_init(|| unsafe {
        let hr = CoInitializeEx(None, COINIT_MULTITHREADED);
        // Accept S_OK, S_FALSE, and RPC_E_CHANGED_MODE (0x80010106)
        let we_initialized = hr.is_ok();
        if hr.is_err() && hr != windows::core::HRESULT(0x80010106u32 as i32) {
            return false;
        }
        let result: std::result::Result<IMMDeviceEnumerator, _> =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL);
        let available = if let Ok(enumerator) = result {
            enumerator
                .GetDefaultAudioEndpoint(eRender, eConsole)
                .is_ok()
        } else {
            false
        };
        if we_initialized {
            CoUninitialize();
        }
        available
    })
}

/// Buffer duration in 100-nanosecond units (1 second for shared mode buffer request).
const BUFFER_DURATION_HNS: i64 = 10_000_000;

pub struct WasapiCapture {
    pub ring: Arc<RingBuffer>,
    pub sample_rate: u32,
    pub device_name: String,
    pub callback_count: Arc<AtomicU64>,
    /// A9 (#1460): set by the capture thread when the COM loop bails out. A positive death
    /// signal — the watchdog cannot infer death from a frozen `callback_count` here, because
    /// loopback delivers no packets at all while nothing is playing.
    pub capture_failed: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
    thread_handle: Option<thread::JoinHandle<()>>,
}

/// Get the friendly name of an audio endpoint device.
fn get_device_name(device: &windows::Win32::Media::Audio::IMMDevice) -> String {
    use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;

    fn inner(
        device: &windows::Win32::Media::Audio::IMMDevice,
    ) -> std::result::Result<String, windows::core::Error> {
        // SAFETY: Windows COM property store APIs are FFI calls. The device handle
        // is valid (passed by reference from the caller). PropVariantToStringAlloc
        // returns a valid PWSTR on success.
        unsafe {
            let store: IPropertyStore = device.OpenPropertyStore(STGM_READ)?;
            let prop = store.GetValue(&PKEY_Device_FriendlyName)?;
            let pwstr = PropVariantToStringAlloc(&prop)?;
            Ok(pwstr
                .to_string()
                .unwrap_or_else(|_| "Unknown Device".to_string()))
        }
    }
    inner(device).unwrap_or_else(|_| "Unknown Device".to_string())
}

/// Query device info (name, sample rate, channels, etc.) with COM initialized.
/// Caller must have called CoInitializeEx before this.
fn query_device_info() -> Result<(String, u32, u16, u16, u16)> {
    // SAFETY: Windows COM/WASAPI FFI calls. COM must be initialized before calling
    // (ensured by caller via com_init()). We free the mix_format_ptr with CoTaskMemFree
    // after reading its fields.
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
        let name = get_device_name(&device);

        let audio_client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
        let mix_format_ptr = audio_client.GetMixFormat()?;
        let fmt = &*mix_format_ptr;

        let sr = fmt.nSamplesPerSec;
        let ch = fmt.nChannels;
        let bps = fmt.wBitsPerSample;
        let ba = fmt.nBlockAlign;

        log::info!("WASAPI loopback: {name} ({sr}Hz, {ch}ch, {bps}bit, block_align={ba})");

        CoTaskMemFree(Some(
            mix_format_ptr.cast::<core::ffi::c_void>().cast_const(),
        ));
        Ok((name, sr, ch, bps, ba))
    }
}

/// Initialize COM, tolerating RPC_E_CHANGED_MODE (already initialized in another mode).
/// Returns true if we initialized COM (and should call CoUninitialize), false if it was already init'd.
fn com_init() -> Result<bool> {
    // SAFETY: CoInitializeEx is safe to call from any thread. We handle all HRESULT
    // return values including RPC_E_CHANGED_MODE (already initialized in another mode).
    unsafe {
        let hr = CoInitializeEx(None, COINIT_MULTITHREADED);
        if hr.is_ok() {
            return Ok(true); // We initialized it
        }
        // RPC_E_CHANGED_MODE (0x80010106): COM already initialized with different mode — that's fine
        if hr == windows::core::HRESULT(0x80010106u32 as i32) {
            log::info!("COM already initialized (apartment-threaded), reusing");
            return Ok(false); // Already initialized, don't uninitialize
        }
        hr.ok()
            .map_err(|e| anyhow::anyhow!("COM init failed: {e}"))?;
        Ok(true)
    }
}

impl WasapiCapture {
    pub fn new() -> Result<Self> {
        let we_initialized_com = com_init()?;

        let result = query_device_info();
        if we_initialized_com {
            // SAFETY: We initialized COM above via com_init(), so we must uninitialize it.
            unsafe { CoUninitialize() };
        }
        let (device_name, sample_rate, channels, bits_per_sample, block_align) = result?;

        let ring = Arc::new(RingBuffer::new());
        let ring_clone = ring.clone();
        let callback_count = Arc::new(AtomicU64::new(0));
        let callback_count_clone = callback_count.clone();
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = shutdown.clone();
        let capture_failed = Arc::new(AtomicBool::new(false));
        let capture_failed_clone = capture_failed.clone();

        let thread_handle = thread::Builder::new()
            .name("phosphor-wasapi".into())
            .spawn(move || {
                wasapi_capture_thread(
                    ring_clone,
                    callback_count_clone,
                    shutdown_clone,
                    capture_failed_clone,
                    channels as usize,
                    bits_per_sample,
                    block_align as usize,
                );
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
}

impl Drop for WasapiCapture {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }
    }
}

fn wasapi_capture_thread(
    ring: Arc<RingBuffer>,
    callback_count: Arc<AtomicU64>,
    shutdown: Arc<AtomicBool>,
    capture_failed: Arc<AtomicBool>,
    channels: usize,
    bits_per_sample: u16,
    block_align: usize,
) {
    // SAFETY: COM must be initialized per-thread for WASAPI calls. We initialize at thread
    // start and uninitialize at thread end. The capture loop uses only COM/WASAPI FFI calls
    // with handles obtained within this thread's COM context.
    unsafe {
        if CoInitializeEx(None, COINIT_MULTITHREADED).is_err() {
            log::error!("WASAPI thread: COM init failed");
            return;
        }

        let result = wasapi_capture_loop(
            &ring,
            &callback_count,
            &shutdown,
            channels,
            bits_per_sample,
            block_align,
        );
        if let Err(e) = result {
            log::error!("WASAPI capture error: {e}");
            // A9 (#1460): the loop only returns Err on a COM failure — a clean shutdown
            // returns Ok — so this is a positive "the endpoint is gone" signal. The watchdog
            // needs it because a frozen callback_count cannot mean death here: loopback
            // delivers no packets at all while nothing is playing.
            //
            // Guarded on !shutdown so a failing `audio_client.Stop()` during a normal
            // teardown does not report a death that never happened.
            if !shutdown.load(Ordering::Acquire) {
                capture_failed.store(true, Ordering::Release);
            }
        }

        CoUninitialize();
    }
}

/// Inner capture loop separated for cleaner error handling.
/// Must be called on a thread with COM initialized.
fn wasapi_capture_loop(
    ring: &Arc<RingBuffer>,
    callback_count: &Arc<AtomicU64>,
    shutdown: &Arc<AtomicBool>,
    channels: usize,
    bits_per_sample: u16,
    block_align: usize,
) -> Result<()> {
    // SAFETY: All WASAPI/COM FFI calls within. COM is initialized by the caller
    // (wasapi_capture_thread). Buffer pointer from GetBuffer is valid for num_frames *
    // block_align bytes until ReleaseBuffer is called. We call ReleaseBuffer each iteration.
    // mix_format_ptr is valid until we drop the audio_client.
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
        let audio_client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;

        let mix_format_ptr = audio_client.GetMixFormat()?;

        // Initialize in shared mode with loopback flag
        audio_client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK,
            BUFFER_DURATION_HNS,
            0, // periodicity (must be 0 for shared mode)
            mix_format_ptr,
            None,
        )?;

        let capture_client: IAudioCaptureClient = audio_client.GetService()?;
        audio_client.Start()?;
        log::info!("WASAPI loopback capture started");

        // Detect float format
        let is_float = bits_per_sample == 32 && {
            let fmt = &*mix_format_ptr;
            if fmt.cbSize >= 22 {
                let ext = &*(mix_format_ptr as *const WAVEFORMATEXTENSIBLE);
                // Copy SubFormat to avoid unaligned access on packed struct
                let sub_format = std::ptr::addr_of!(ext.SubFormat).read_unaligned();
                sub_format == KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
            } else {
                fmt.wFormatTag == 3 // WAVE_FORMAT_IEEE_FLOAT
            }
        };

        loop {
            if shutdown.load(Ordering::Acquire) {
                log::info!("WASAPI capture thread shutting down");
                break;
            }

            // Sleep briefly to let packets accumulate (~10ms)
            thread::sleep(Duration::from_millis(10));

            // Drain all available packets
            loop {
                let packet_length = capture_client.GetNextPacketSize()?;
                if packet_length == 0 {
                    break;
                }

                let mut buffer_ptr: *mut u8 = std::ptr::null_mut();
                let mut num_frames: u32 = 0;
                let mut flags: u32 = 0;
                capture_client.GetBuffer(
                    &mut buffer_ptr,
                    &mut num_frames,
                    &mut flags,
                    None,
                    None,
                )?;

                if num_frames > 0 {
                    let count = callback_count.fetch_add(1, Ordering::Relaxed);
                    if count == 0 {
                        log::info!(
                            "WASAPI first data: {num_frames} frames, {channels}ch, {}bit, float={is_float}",
                            bits_per_sample
                        );
                    }

                    // AUDCLNT_BUFFERFLAGS_SILENT = 0x2
                    let is_silent = (flags & 0x2) != 0;

                    if !is_silent && !buffer_ptr.is_null() {
                        let total_bytes = num_frames as usize * block_align;
                        let raw_data = std::slice::from_raw_parts(buffer_ptr, total_bytes);

                        let stereo = convert_to_stereo_f32(
                            raw_data,
                            channels,
                            is_float,
                            bits_per_sample,
                            block_align,
                        );
                        ring.push(&stereo);
                    }
                }

                capture_client.ReleaseBuffer(num_frames)?;
            }
        }

        audio_client.Stop()?;
        Ok(())
    }
}

/// Decode one channel of one interleaved frame to f32. Out-of-range offsets read as 0.
fn decode_sample(
    data: &[u8],
    frame_start: usize,
    ch: usize,
    is_float: bool,
    bits_per_sample: u16,
) -> f32 {
    if is_float && bits_per_sample == 32 {
        let offset = frame_start + ch * 4;
        if offset + 4 <= data.len() {
            f32::from_le_bytes([
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
            ])
        } else {
            0.0
        }
    } else if bits_per_sample == 16 {
        let offset = frame_start + ch * 2;
        if offset + 2 <= data.len() {
            i16::from_le_bytes([data[offset], data[offset + 1]]) as f32 / 32768.0
        } else {
            0.0
        }
    } else if bits_per_sample == 24 {
        let offset = frame_start + ch * 3;
        if offset + 3 <= data.len() {
            let raw = (data[offset] as i32)
                | ((data[offset + 1] as i32) << 8)
                | ((data[offset + 2] as i32) << 16);
            // Sign extend from 24 bits
            let raw = if raw & 0x800000 != 0 {
                raw | !0xFFFFFF
            } else {
                raw
            };
            raw as f32 / 8388608.0
        } else {
            0.0
        }
    } else {
        0.0
    }
}

/// Convert raw interleaved audio bytes to interleaved `L,R` stereo f32.
///
/// A13 (#1464): keep the front L/R pair (channels 0/1) rather than downmixing — the analysis thread
/// derives the mono mix. A mono source is duplicated to both. Output is always even-length, upholding
/// the capture ring's L/R parity invariant.
fn convert_to_stereo_f32(
    data: &[u8],
    channels: usize,
    is_float: bool,
    bits_per_sample: u16,
    frame_bytes: usize,
) -> Vec<f32> {
    let num_frames = data.len() / frame_bytes;
    let mut stereo = Vec::with_capacity(num_frames * 2);

    for i in 0..num_frames {
        let frame_start = i * frame_bytes;
        let l = decode_sample(data, frame_start, 0, is_float, bits_per_sample);
        let r = if channels >= 2 {
            decode_sample(data, frame_start, 1, is_float, bits_per_sample)
        } else {
            l
        };
        stereo.push(l);
        stereo.push(r);
    }

    stereo
}
