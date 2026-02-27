//! Raw FFI definitions and runtime loader for the NDI SDK shared library.
//! Loads libndi dynamically at runtime so no build-time SDK dependency is needed.

use std::ffi::CString;
use std::os::raw::{c_char, c_float, c_int};
use std::sync::OnceLock;

/// Opaque NDI sender instance handle.
pub type NdiSendInstance = *mut std::ffi::c_void;

/// FourCC for BGRA pixel format.
pub const FOURCC_BGRA: u32 = fourcc(b'B', b'G', b'R', b'A');

/// Timecode value that tells NDI to synthesize timing.
pub const TIMECODE_SYNTHESIZE: i64 = i64::MAX;

/// Progressive frame format.
pub const FRAME_FORMAT_PROGRESSIVE: u32 = 1;

const fn fourcc(a: u8, b: u8, c: u8, d: u8) -> u32 {
    (a as u32) | ((b as u32) << 8) | ((c as u32) << 16) | ((d as u32) << 24)
}

/// NDI sender creation descriptor.
#[repr(C)]
pub struct NdiSendCreate {
    pub p_ndi_name: *const c_char,
    pub p_groups: *const c_char,
    pub clock_video: bool,
    pub clock_audio: bool,
}

/// NDI video frame descriptor (v2).
#[repr(C)]
pub struct NdiVideoFrame {
    pub xres: c_int,
    pub yres: c_int,
    pub four_cc: u32,
    pub frame_rate_n: c_int,
    pub frame_rate_d: c_int,
    pub picture_aspect_ratio: c_float,
    pub frame_format_type: u32,
    pub timecode: i64,
    pub p_data: *const u8,
    pub line_stride_in_bytes: c_int,
    pub p_metadata: *const c_char,
    pub timestamp: i64,
}

/// Type aliases for NDI function signatures.
type FnInitialize = unsafe extern "C" fn() -> bool;
type FnDestroy = unsafe extern "C" fn();
type FnSendCreate = unsafe extern "C" fn(*const NdiSendCreate) -> NdiSendInstance;
type FnSendDestroy = unsafe extern "C" fn(NdiSendInstance);
type FnSendVideo = unsafe extern "C" fn(NdiSendInstance, *const NdiVideoFrame);

/// Loaded NDI library with resolved function pointers.
pub struct NdiLib {
    _lib: libloading::Library,
    pub fn_initialize: FnInitialize,
    pub fn_destroy: FnDestroy,
    pub fn_send_create: FnSendCreate,
    pub fn_send_destroy: FnSendDestroy,
    pub fn_send_video: FnSendVideo,
}

// Safety: NdiLib only stores function pointers and a library handle, all thread-safe.
unsafe impl Send for NdiLib {}
unsafe impl Sync for NdiLib {}

impl NdiLib {
    /// Try to load the NDI runtime library.
    pub fn load() -> Result<Self, String> {
        let lib = load_ndi_library()?;

        unsafe {
            let fn_initialize: FnInitialize = *lib
                .get::<FnInitialize>(b"NDIlib_initialize\0")
                .map_err(|e| format!("NDIlib_initialize not found: {e}"))?;
            let fn_destroy: FnDestroy = *lib
                .get::<FnDestroy>(b"NDIlib_destroy\0")
                .map_err(|e| format!("NDIlib_destroy not found: {e}"))?;
            let fn_send_create: FnSendCreate = *lib
                .get::<FnSendCreate>(b"NDIlib_send_create_v2\0")
                .or_else(|_| lib.get::<FnSendCreate>(b"NDIlib_send_create\0"))
                .map_err(|e| format!("NDIlib_send_create not found: {e}"))?;
            let fn_send_destroy: FnSendDestroy = *lib
                .get::<FnSendDestroy>(b"NDIlib_send_destroy\0")
                .map_err(|e| format!("NDIlib_send_destroy not found: {e}"))?;
            let fn_send_video: FnSendVideo = *lib
                .get::<FnSendVideo>(b"NDIlib_send_send_video_v2\0")
                .map_err(|e| format!("NDIlib_send_send_video_v2 not found: {e}"))?;

            Ok(Self {
                _lib: lib,
                fn_initialize,
                fn_destroy,
                fn_send_create,
                fn_send_destroy,
                fn_send_video,
            })
        }
    }
}

/// Higher-level NDI sender that owns the library and send instance.
pub struct NdiSender {
    lib: NdiLib,
    instance: NdiSendInstance,
    _source_name: CString,
}

// Safety: NDI sender is thread-safe per NDI SDK documentation.
unsafe impl Send for NdiSender {}

impl NdiSender {
    /// Create a new NDI sender with the given source name.
    pub fn new(source_name: &str) -> Result<Self, String> {
        let lib = NdiLib::load()?;

        let ok = unsafe { (lib.fn_initialize)() };
        if !ok {
            return Err("NDIlib_initialize failed".into());
        }
        log::info!("NDI SDK initialized");

        let c_name = CString::new(source_name).map_err(|e| format!("Invalid source name: {e}"))?;
        let create = NdiSendCreate {
            p_ndi_name: c_name.as_ptr(),
            p_groups: std::ptr::null(),
            clock_video: true,
            clock_audio: false,
        };

        let instance = unsafe { (lib.fn_send_create)(&create) };
        if instance.is_null() {
            unsafe { (lib.fn_destroy)() };
            return Err("NDIlib_send_create failed".into());
        }

        log::info!("NDI sender created: source='{source_name}'");

        Ok(Self {
            lib,
            instance,
            _source_name: c_name,
        })
    }

    /// Send a BGRA video frame.
    pub fn send_video(&self, data: &[u8], width: u32, height: u32) {
        let stride = (width * 4) as c_int;
        let frame = NdiVideoFrame {
            xres: width as c_int,
            yres: height as c_int,
            four_cc: FOURCC_BGRA,
            frame_rate_n: 60,
            frame_rate_d: 1,
            picture_aspect_ratio: 0.0, // square pixels
            frame_format_type: FRAME_FORMAT_PROGRESSIVE,
            timecode: TIMECODE_SYNTHESIZE,
            p_data: data.as_ptr(),
            line_stride_in_bytes: stride,
            p_metadata: std::ptr::null(),
            timestamp: 0,
        };
        unsafe { (self.lib.fn_send_video)(self.instance, &frame) };
    }
}

impl Drop for NdiSender {
    fn drop(&mut self) {
        if !self.instance.is_null() {
            unsafe { (self.lib.fn_send_destroy)(self.instance) };
        }
        unsafe { (self.lib.fn_destroy)() };
        log::info!("NDI sender destroyed");
    }
}

/// Check whether the NDI runtime is available (cached).
pub fn ndi_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        match load_ndi_library() {
            Ok(_) => {
                log::info!("NDI runtime library found");
                true
            }
            Err(e) => {
                log::info!("NDI runtime not available: {e}");
                false
            }
        }
    })
}

/// Try to load the NDI library from a specific directory.
fn try_load_from_dir(dir: &std::path::Path) -> Option<libloading::Library> {
    for name in platform_lib_names() {
        let full = dir.join(name);
        if full.exists() {
            match unsafe { libloading::Library::new(&full) } {
                Ok(lib) => return Some(lib),
                Err(e) => log::debug!("NDI lib exists at {} but failed to load: {e}", full.display()),
            }
        }
    }
    None
}

/// Try to find and load the NDI shared library.
fn load_ndi_library() -> Result<libloading::Library, String> {
    let mut checked: Vec<String> = Vec::new();

    // 1. NDILIB_REDIST_FOLDER env var (official NDI SDK recommendation).
    if let Ok(folder) = std::env::var("NDILIB_REDIST_FOLDER") {
        checked.push(format!("NDILIB_REDIST_FOLDER={folder}"));
        if let Some(lib) = try_load_from_dir(std::path::Path::new(&folder)) {
            return Ok(lib);
        }
    }

    // 2. Platform-specific env vars and well-known paths.
    #[cfg(target_os = "windows")]
    {
        // NDI 6/5 Runtime installer sets these env vars.
        for var in &["NDI_RUNTIME_DIR_V6", "NDI_RUNTIME_DIR_V5"] {
            if let Ok(folder) = std::env::var(var) {
                checked.push(format!("{var}={folder}"));
                if let Some(lib) = try_load_from_dir(std::path::Path::new(&folder)) {
                    return Ok(lib);
                }
            }
        }
        // Hardcoded fallback paths for standard installs.
        for path in &[
            r"C:\Program Files\NDI\NDI 6 Runtime\v6",
            r"C:\Program Files\NDI\NDI 5 Runtime\v5",
        ] {
            checked.push(path.to_string());
            if let Some(lib) = try_load_from_dir(std::path::Path::new(path)) {
                return Ok(lib);
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        let mut mac_paths = vec![
            "/usr/local/lib".to_string(),
            "/Library/NDI SDK for Apple/lib/macOS".to_string(),
        ];
        if let Ok(home) = std::env::var("HOME") {
            mac_paths.push(format!("{home}/NDI SDK for Apple/lib/macOS"));
        }
        for path in &mac_paths {
            checked.push(path.clone());
            if let Some(lib) = try_load_from_dir(std::path::Path::new(path)) {
                return Ok(lib);
            }
        }
    }

    // 3. Bare library names via system linker (LD_LIBRARY_PATH, /usr/lib, etc.).
    checked.push("system linker search".to_string());
    for name in platform_lib_names() {
        match unsafe { libloading::Library::new(name) } {
            Ok(lib) => return Ok(lib),
            Err(_) => continue,
        }
    }

    Err(format!(
        "NDI library not found. Install the NDI Runtime from https://ndi.video/tools/\n\
         Searched: {}",
        checked.join(", ")
    ))
}

fn platform_lib_names() -> &'static [&'static str] {
    #[cfg(target_os = "linux")]
    {
        &["libndi.so.6", "libndi.so.5", "libndi.so"]
    }
    #[cfg(target_os = "macos")]
    {
        &["libndi.6.dylib", "libndi.5.dylib", "libndi.dylib"]
    }
    #[cfg(target_os = "windows")]
    {
        &[
            "Processing.NDI.Lib.x64.dll",
            "Processing.NDI.Lib.x86.dll",
        ]
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        &[]
    }
}
