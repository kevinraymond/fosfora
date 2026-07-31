//! Runtime loader + ObjC bridge for the Syphon framework.
//!
//! Syphon.framework is deliberately NOT linked at build time: a build-time
//! link makes the binary fail at launch (dyld) when the framework is absent,
//! killing graceful degradation and non-mac CI builds. Instead the framework
//! binary is dlopen'd on first use — which registers its ObjC classes with
//! the runtime — and `SyphonMetalServer` is resolved dynamically by name.
//! The typed Metal side (`objc2-metal`) links Metal.framework normally; that
//! one is a system framework and always present.

use std::path::PathBuf;
use std::sync::OnceLock;

use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, ProtocolObject};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use objc2_metal::{MTLCommandBuffer, MTLDevice, MTLTexture};

/// Cached framework-load result with search diagnostics.
struct SyphonAvailability {
    /// Keeps the framework image mapped for the life of the process. An image
    /// that has registered ObjC classes can never be safely unloaded, so this
    /// handle is intentionally never dropped (unlike the NDI probe, which
    /// re-opens per sender).
    library: Option<libloading::Library>,
    diagnostics: Vec<String>,
}

static SYPHON_AVAILABILITY: OnceLock<SyphonAvailability> = OnceLock::new();

fn availability() -> &'static SyphonAvailability {
    SYPHON_AVAILABILITY.get_or_init(|| {
        let mut diagnostics = Vec::new();
        match load_syphon_framework(&mut diagnostics) {
            Ok(lib) => {
                log::info!("Syphon framework loaded");
                SyphonAvailability {
                    library: Some(lib),
                    diagnostics,
                }
            }
            Err(e) => {
                log::info!("Syphon framework not available: {e}");
                SyphonAvailability {
                    library: None,
                    diagnostics,
                }
            }
        }
    })
}

/// Check whether the Syphon framework is available (cached).
pub fn syphon_available() -> bool {
    availability().library.is_some()
}

/// The paths searched during framework discovery (for UI diagnostics).
pub fn syphon_search_diagnostics() -> &'static [String] {
    availability().diagnostics.as_slice()
}

/// Directories that may contain `Syphon.framework`, in search order.
fn framework_search_dirs(diagnostics: &mut Vec<String>) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    // 1. Explicit override (dir containing Syphon.framework).
    if let Ok(dir) = std::env::var("SYPHON_FRAMEWORK_PATH") {
        diagnostics.push(format!("SYPHON_FRAMEWORK_PATH={dir}"));
        dirs.push(PathBuf::from(dir));
    }
    // 2. The app bundle: Contents/MacOS/../Frameworks (where the release DMG
    //    ships it).
    if let Ok(exe) = std::env::current_exe() {
        if let Some(macos_dir) = exe.parent() {
            dirs.push(macos_dir.join("../Frameworks"));
        }
    }
    // 3. Conventional user/system framework locations.
    if let Ok(home) = std::env::var("HOME") {
        dirs.push(PathBuf::from(home).join("Library/Frameworks"));
    }
    dirs.push(PathBuf::from("/Library/Frameworks"));
    dirs
}

fn load_syphon_framework(diagnostics: &mut Vec<String>) -> Result<libloading::Library, String> {
    for dir in framework_search_dirs(diagnostics) {
        let full = dir.join("Syphon.framework").join("Syphon");
        // SAFETY: dlopen of the Syphon framework binary. Its ObjC classes are
        // registered with the runtime on load; global constructors run.
        match unsafe { libloading::Library::new(&full) } {
            Ok(lib) => {
                log::info!("Syphon framework loaded from {}", full.display());
                return Ok(lib);
            }
            Err(e) => {
                log::debug!("Syphon: {} failed: {e}", full.display());
                diagnostics.push(format!("{}  ✗ {e}", full.display()));
            }
        }
    }
    Err(format!(
        "Syphon.framework not found. Searched: {}",
        diagnostics.join(", ")
    ))
}

/// A running `SyphonMetalServer`, resolved dynamically from the dlopen'd
/// framework. Documented thread-safe by Syphon; ours lives on the sender
/// thread. Not `Send` (holds an ObjC object), which `FrameSink` permits.
pub struct SyphonServer {
    instance: Retained<AnyObject>,
}

impl SyphonServer {
    /// Create and start a server. `device` is the `MTLDevice` the published
    /// textures live on.
    pub fn new(name: &str, device: &ProtocolObject<dyn MTLDevice>) -> Result<Self, String> {
        if !syphon_available() {
            return Err(
                "Syphon framework not found (see panel for searched locations)".to_string(),
            );
        }
        let class = AnyClass::get(c"SyphonMetalServer").ok_or_else(|| {
            "Syphon framework loaded but has no SyphonMetalServer class — \
             it predates Metal support (needs SDK ≥ the 2020 Metal rewrite)"
                .to_string()
        })?;
        let ns_name = NSString::from_str(name);

        // SAFETY: `alloc` on a resolved ObjC class returns a +1 uninitialized
        // instance (or nil on allocation failure).
        let allocated: *mut AnyObject = unsafe { msg_send![class, alloc] };
        if allocated.is_null() {
            return Err("SyphonMetalServer alloc returned nil".to_string());
        }
        // SAFETY: initWithName:device:options: is SyphonMetalServer's
        // designated initializer; name and options are documented nullable,
        // device is a live MTLDevice. It consumes alloc's +1 and returns a +1
        // instance, or nil on failure (documented).
        let instance: *mut AnyObject = unsafe {
            msg_send![
                allocated,
                initWithName: &*ns_name,
                device: device,
                options: std::ptr::null::<AnyObject>(),
            ]
        };
        // SAFETY: `instance` is the +1 reference returned by init;
        // Retained::from_raw takes ownership of exactly that reference.
        let instance = unsafe { Retained::from_raw(instance) }
            .ok_or_else(|| format!("SyphonMetalServer init failed for server name '{name}'"))?;
        Ok(Self { instance })
    }

    /// Publish one frame: Syphon copies `texture` into its own IOSurface-backed
    /// texture on `command_buffer` (the caller commits it afterwards).
    pub fn publish_frame(
        &self,
        texture: &ProtocolObject<dyn MTLTexture>,
        command_buffer: &ProtocolObject<dyn MTLCommandBuffer>,
        width: u32,
        height: u32,
    ) {
        let region = NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(f64::from(width), f64::from(height)),
        );
        // flipped: true — Syphon's shared-surface orientation is GL-style
        // bottom-up; our readback rows are top-down, so the server must
        // flip-render (verbatim-blitting them with flipped:false shows
        // upside-down in every client). Caught by eye in Simple Client: a
        // raw-memory probe cannot see this convention.
        //
        // SAFETY: publishFrameTexture:onCommandBuffer:imageRegion:flipped:
        // with a texture and command buffer from the server's own MTLDevice;
        // the region is within the texture bounds. Documented thread-safe.
        unsafe {
            let _: () = msg_send![
                &*self.instance,
                publishFrameTexture: texture,
                onCommandBuffer: command_buffer,
                imageRegion: region,
                flipped: true,
            ];
        }
    }
}

impl Drop for SyphonServer {
    fn drop(&mut self) {
        // SAFETY: `stop` (on SyphonServerBase) unregisters the server; safe to
        // call once before release, from any thread.
        unsafe {
            let _: () = msg_send![&*self.instance, stop];
        }
        log::info!("Syphon server stopped");
    }
}
