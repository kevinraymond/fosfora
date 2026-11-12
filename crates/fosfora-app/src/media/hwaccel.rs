//! Opt-in hardware video decoding for the ffmpeg pipes.
//!
//! `FOSFORA_HWACCEL=<method>` (`cuda`, `vaapi`, `qsv`, `vulkan`, `auto`, ...)
//! adds `-hwaccel <method>` before ffmpeg's input, moving H.264/HEVC decode
//! off the CPU. Frames are still downloaded to system memory for the RGBA
//! conversion and the pipe, so only decode moves. Unset, empty or `none`
//! leaves the command exactly as before, and ffmpeg itself falls back to
//! software for a codec the device cannot decode.

/// The `-hwaccel` method requested by `FOSFORA_HWACCEL`, if any.
pub fn method() -> Option<String> {
    parse(std::env::var("FOSFORA_HWACCEL").ok().as_deref())
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
    use super::parse;

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
