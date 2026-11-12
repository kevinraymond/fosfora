//! Opt-in hardware video decoding for the ffmpeg pipes.
//!
//! `FOSFORA_HWACCEL=<method>` (`cuda`, `vaapi`, `qsv`, `vulkan`, `auto`, ...)
//! adds `-hwaccel <method>` before ffmpeg's input, moving H.264/HEVC decode
//! off the CPU. Frames are still downloaded to system memory for the RGBA
//! conversion and the pipe, so only decode moves. Unset, empty or `none`
//! leaves the command exactly as before, and ffmpeg itself falls back to
//! software for a codec the device cannot decode.
//!
//! ffmpeg does not fall back when the device itself cannot start: it exits
//! without decoding a frame. So a named method is checked once per process
//! by opening the device, and a method that fails is dropped with a warning
//! and the clip decodes in software. `auto` picks a working device or
//! software by itself and is not checked.

use std::process::{Command, Stdio};
use std::sync::OnceLock;

/// The `-hwaccel` method to use: the one `FOSFORA_HWACCEL` asks for, if its
/// device starts on this machine. Read and checked once per process.
pub fn method() -> Option<String> {
    static METHOD: OnceLock<Option<String>> = OnceLock::new();
    METHOD
        .get_or_init(|| {
            let requested = parse(std::env::var("FOSFORA_HWACCEL").ok().as_deref());
            usable(requested, device_starts)
        })
        .clone()
}

/// The requested method if its device starts, else none (with a warning).
fn usable(requested: Option<String>, starts: impl Fn(&str) -> bool) -> Option<String> {
    let m = requested?;
    if m == "auto" || starts(&m) {
        return Some(m);
    }
    log::warn!(
        "FOSFORA_HWACCEL={m}: ffmpeg cannot start this device here, decoding video in software"
    );
    None
}

/// Whether ffmpeg can open the hardware device for `method`: one frame of a
/// generated 16x16 source, no file involved.
fn device_starts(method: &str) -> bool {
    Command::new("ffmpeg")
        .args(["-v", "error", "-nostdin", "-init_hw_device", method])
        .args(["-f", "lavfi", "-i", "nullsrc=s=16x16"])
        .args(["-frames:v", "1", "-f", "null", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn parse(raw: Option<&str>) -> Option<String> {
    let m = raw?.trim();
    let valid = !m.is_empty()
        && !m.eq_ignore_ascii_case("none")
        && m.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    valid.then(|| m.to_ascii_lowercase())
}

/// ffmpeg input options for the requested method: `["-hwaccel", m]` or none.
pub fn input_args() -> Vec<String> {
    method().map_or_else(Vec::new, |m| vec!["-hwaccel".to_owned(), m])
}

/// ffmpeg output options for the fast path. With hardware decode the
/// remaining CPU cost is the YUV to RGBA conversion, and the clip is never
/// resized, so `fast_bilinear` only touches chroma upsampling. Measured on a
/// 16 s 1080p30 H.264 clip: 17.0 s CPU stock, 13.2 s with `-hwaccel cuda`,
/// 6.5 s with both.
pub fn output_args() -> Vec<String> {
    method().map_or_else(Vec::new, |_| {
        vec!["-sws_flags".to_owned(), "fast_bilinear".to_owned()]
    })
}

#[cfg(test)]
mod tests {
    use super::{parse, usable};
    use std::cell::Cell;

    #[test]
    fn a_method_whose_device_does_not_start_decodes_in_software() {
        let some = |m: &str| Some(m.to_owned());
        assert_eq!(usable(some("cuda"), |_| true).as_deref(), Some("cuda"));
        assert_eq!(usable(some("vaapi"), |_| false), None);
        assert_eq!(usable(None, |_| true), None);
    }

    #[test]
    fn auto_is_not_checked() {
        let checked = Cell::new(false);
        let m = usable(Some("auto".to_owned()), |_| {
            checked.set(true);
            false
        });
        assert_eq!(m.as_deref(), Some("auto"));
        assert!(!checked.get());
    }

    #[test]
    fn only_a_plain_method_name_is_passed_to_ffmpeg() {
        assert_eq!(parse(Some("cuda")).as_deref(), Some("cuda"));
        assert_eq!(parse(Some(" VAAPI ")).as_deref(), Some("vaapi"));
        assert_eq!(parse(Some("none")), None);
        assert_eq!(parse(Some("")), None);
        assert_eq!(parse(None), None);
        assert_eq!(parse(Some("cuda -y")), None, "no smuggled arguments");
    }
}
