/// Detects and tracks prefers-reduced-motion state.
pub struct ReducedMotion {
    pub enabled: bool,
    pub user_override: Option<bool>,
}

impl ReducedMotion {
    pub fn detect() -> Self {
        let system = detect_system_preference();
        Self {
            enabled: system,
            user_override: None,
        }
    }

    pub fn should_reduce(&self) -> bool {
        self.user_override.unwrap_or(self.enabled)
    }

    pub fn set_override(&mut self, value: Option<bool>) {
        self.user_override = value;
    }
}

#[cfg(target_os = "linux")]
fn detect_system_preference() -> bool {
    // Try reading from D-Bus org.gnome.desktop.interface enable-animations
    // For now, default to false (don't reduce)
    std::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "enable-animations"])
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|s| s.trim() == "false")
        .unwrap_or(false)
}

#[cfg(target_os = "macos")]
fn detect_system_preference() -> bool {
    use objc2_app_kit::NSWorkspace;
    // `sharedWorkspace` returns the process-wide NSWorkspace singleton;
    // `accessibilityDisplayShouldReduceMotion` reads a Bool property (both are
    // safe in objc2-app-kit — no ownership transfer or threading requirements).
    NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion()
}

#[cfg(target_os = "windows")]
fn detect_system_preference() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{
        SPI_GETCLIENTAREAANIMATION, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
    };
    // SPI_GETCLIENTAREAANIMATION writes a BOOL (as i32): nonzero when client-area
    // animations are enabled. Reduce motion when they are disabled.
    let mut animations_enabled: i32 = 1;
    let pv = std::ptr::from_mut(&mut animations_enabled).cast::<core::ffi::c_void>();
    // SAFETY: FFI call into user32. `pvParam` points to a single i32 we own, which
    // matches the BOOL this action writes. `fWinIni` is unused for read actions.
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some(pv),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    // On query failure, default to not reducing motion.
    ok.is_ok() && animations_enabled == 0
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn detect_system_preference() -> bool {
    false
}
