//! WASAPI loopback capture backend for Windows.
//! Uses COM APIs to capture the default audio render device's output mix
//! (desktop audio / what's playing through speakers).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::Duration;

use anyhow::Result;
use windows::core::Interface;
use windows::Win32::Media::Audio::{
    eConsole, eRender, IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator,
    MMDeviceEnumerator, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK,
    WAVEFORMATEXTENSIBLE,
};
use windows::Win32::Media::Multimedia::KSDATAFORMAT_SUBTYPE_IEEE_FLOAT;
use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
    COINIT_MULTITHREADED, STGM_READ,
};
use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;

use super::capture::RingBuffer;

/// Check if WASAPI loopback is available at runtime. Cached.
pub fn wasapi_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
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
    shutdown: Arc<AtomicBool>,
    thread_handle: Option<thread::JoinHandle<()>>,
}

/// Get the friendly name of an audio endpoint device.
fn get_device_name(device: &windows::Win32::Media::Audio::IMMDevice) -> String {
    use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;

    fn inner(
        device: &windows::Win32::Media::Audio::IMMDevice,
    ) -> std::result::Result<String, windows::core::Error> {
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

        CoTaskMemFree(Some(mix_format_ptr as *const _ as *const _));
        Ok((name, sr, ch, bps, ba))
    }
}

/// Initialize COM, tolerating RPC_E_CHANGED_MODE (already initialized in another mode).
/// Returns true if we initialized COM (and should call CoUninitialize), false if it was already init'd.
fn com_init() -> Result<bool> {
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
        hr.ok().map_err(|e| anyhow::anyhow!("COM init failed: {e}"))?;
        Ok(true)
    }
}

impl WasapiCapture {
    pub fn new() -> Result<Self> {
        let we_initialized_com = com_init()?;

        let result = query_device_info();
        if we_initialized_com {
            unsafe { CoUninitialize() };
        }
        let (device_name, sample_rate, channels, bits_per_sample, block_align) = result?;

        let ring = Arc::new(RingBuffer::new());
        let ring_clone = ring.clone();
        let callback_count = Arc::new(AtomicU64::new(0));
        let callback_count_clone = callback_count.clone();
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = shutdown.clone();

        let thread_handle = thread::Builder::new()
            .name("phosphor-wasapi".into())
            .spawn(move || {
                wasapi_capture_thread(
                    ring_clone,
                    callback_count_clone,
                    shutdown_clone,
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
    channels: usize,
    bits_per_sample: u16,
    block_align: usize,
) {
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
        }

        CoUninitialize();
    }
}

/// Inner capture loop separated for cleaner error handling.
/// Safety: must be called on a thread with COM initialized.
fn wasapi_capture_loop(
    ring: &Arc<RingBuffer>,
    callback_count: &Arc<AtomicU64>,
    shutdown: &Arc<AtomicBool>,
    channels: usize,
    bits_per_sample: u16,
    block_align: usize,
) -> Result<()> {
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

                        let mono = convert_to_mono_f32(
                            raw_data,
                            channels,
                            is_float,
                            bits_per_sample,
                            block_align,
                        );
                        ring.push(&mono);
                    }
                }

                capture_client.ReleaseBuffer(num_frames)?;
            }
        }

        audio_client.Stop()?;
        Ok(())
    }
}

/// Convert raw audio bytes to mono f32 samples.
fn convert_to_mono_f32(
    data: &[u8],
    channels: usize,
    is_float: bool,
    bits_per_sample: u16,
    frame_bytes: usize,
) -> Vec<f32> {
    let num_frames = data.len() / frame_bytes;
    let mut mono = Vec::with_capacity(num_frames);

    for i in 0..num_frames {
        let frame_start = i * frame_bytes;
        let mut sum = 0.0f32;

        for ch in 0..channels {
            let sample = if is_float && bits_per_sample == 32 {
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
                    let raw = i16::from_le_bytes([data[offset], data[offset + 1]]);
                    raw as f32 / 32768.0
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
            };
            sum += sample;
        }

        mono.push(sum / channels as f32);
    }

    mono
}
