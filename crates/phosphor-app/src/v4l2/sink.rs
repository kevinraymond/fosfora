//! Frame writer + BGRA→YUYV conversion for the v4l2 sender thread.

use std::io::Write;

use crate::output::sink::{FrameLayout, FrameSink, OutputFrame};

use super::types::V4l2PixelFormat;

/// Writes frames to an open, format-negotiated loopback device.
pub struct V4l2Sink {
    device: v4l::Device,
    format: V4l2PixelFormat,
    /// Negotiated (held) geometry — the device never sees a size change.
    width: u32,
    height: u32,
    /// Reused conversion scratch buffer.
    packed: Vec<u8>,
    warned_dims: bool,
}

impl V4l2Sink {
    pub fn new(device: v4l::Device, format: V4l2PixelFormat, width: u32, height: u32) -> Self {
        Self {
            device,
            format,
            width,
            height,
            packed: Vec::new(),
            warned_dims: false,
        }
    }
}

impl FrameSink for V4l2Sink {
    fn write_frame(&mut self, frame: &OutputFrame) -> Result<(), String> {
        // Held-geometry policy means this should never fire; guard anyway so a
        // logic error upstream skips frames instead of shearing the stream.
        if frame.width != self.width || frame.height != self.height {
            if !self.warned_dims {
                log::warn!(
                    "v4l2 sink: frame {}x{} != negotiated {}x{}, skipping",
                    frame.width,
                    frame.height,
                    self.width,
                    self.height
                );
                self.warned_dims = true;
            }
            return Ok(());
        }

        let bytes: &[u8] = match self.format {
            V4l2PixelFormat::Yuyv => {
                bgra_to_yuyv(
                    &frame.data,
                    frame.width,
                    frame.height,
                    frame.layout,
                    &mut self.packed,
                );
                &self.packed
            }
            V4l2PixelFormat::Bgrx => match frame.layout {
                FrameLayout::Bgra8 => &frame.data,
                FrameLayout::Rgba8 => {
                    // Swizzle R<->B into scratch so memory order matches BGR4.
                    self.packed.clear();
                    self.packed.reserve(frame.data.len());
                    for px in frame.data.chunks_exact(4) {
                        self.packed.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
                    }
                    &self.packed
                }
            },
        };

        self.device
            .write_all(bytes)
            .map_err(|e| format!("v4l2 write failed: {e} — was the v4l2loopback module unloaded?"))
    }
}

/// Per-layout byte offsets of R, G, B within a 4-byte pixel.
fn rgb_offsets(layout: FrameLayout) -> (usize, usize, usize) {
    match layout {
        FrameLayout::Bgra8 => (2, 1, 0),
        FrameLayout::Rgba8 => (0, 1, 2),
    }
}

/// Convert 4-byte-per-pixel RGB data to packed YUYV 4:2:2.
///
/// BT.601 limited range, integer fixed-point, applied to gamma-encoded bytes
/// (correct: YCbCr matrices are defined on R'G'B', which is exactly what the
/// sRGB-encoded readback bytes are). Per pixel pair: Y0 U Y1 V, chroma from
/// the averaged pair. An odd trailing pixel emits Y U only, keeping each row
/// at exactly `width * 2` bytes.
pub fn bgra_to_yuyv(src: &[u8], width: u32, height: u32, layout: FrameLayout, out: &mut Vec<u8>) {
    let (ro, go, bo) = rgb_offsets(layout);
    let w = width as usize;
    let h = height as usize;

    out.clear();
    out.reserve(w * h * 2);

    let luma = |r: i32, g: i32, b: i32| -> u8 {
        (16 + ((66 * r + 129 * g + 25 * b + 128) >> 8)).clamp(0, 255) as u8
    };

    for row in 0..h {
        let row_base = row * w * 4;
        let mut x = 0;
        while x + 1 < w {
            let p0 = row_base + x * 4;
            let p1 = p0 + 4;
            let (r0, g0, b0) = (
                src[p0 + ro] as i32,
                src[p0 + go] as i32,
                src[p0 + bo] as i32,
            );
            let (r1, g1, b1) = (
                src[p1 + ro] as i32,
                src[p1 + go] as i32,
                src[p1 + bo] as i32,
            );

            // Chroma from the averaged pair (4:2:2 subsampling).
            let (ra, ga, ba) = (
                i32::midpoint(r0, r1),
                i32::midpoint(g0, g1),
                i32::midpoint(b0, b1),
            );
            let u = (128 + ((-38 * ra - 74 * ga + 112 * ba + 128) >> 8)).clamp(0, 255) as u8;
            let v = (128 + ((112 * ra - 94 * ga - 18 * ba + 128) >> 8)).clamp(0, 255) as u8;

            out.extend_from_slice(&[luma(r0, g0, b0), u, luma(r1, g1, b1), v]);
            x += 2;
        }
        if x < w {
            // Odd trailing pixel: Y + its own U, keeping the row at width*2 bytes.
            let p0 = row_base + x * 4;
            let (r, g, b) = (
                src[p0 + ro] as i32,
                src[p0 + go] as i32,
                src[p0 + bo] as i32,
            );
            let u = (128 + ((-38 * r - 74 * g + 112 * b + 128) >> 8)).clamp(0, 255) as u8;
            out.extend_from_slice(&[luma(r, g, b), u]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a 2x1 BGRA image of two identical pixels and convert it.
    fn convert_pair(b: u8, g: u8, r: u8) -> (u8, u8, u8) {
        let src = [b, g, r, 255, b, g, r, 255];
        let mut out = Vec::new();
        bgra_to_yuyv(&src, 2, 1, FrameLayout::Bgra8, &mut out);
        assert_eq!(out.len(), 4);
        assert_eq!(out[0], out[2], "identical pixels must share Y");
        (out[0], out[1], out[3]) // (Y, U, V)
    }

    fn assert_near(actual: (u8, u8, u8), expected: (u8, u8, u8)) {
        let d = |a: u8, b: u8| (a as i32 - b as i32).abs();
        assert!(
            d(actual.0, expected.0) <= 1
                && d(actual.1, expected.1) <= 1
                && d(actual.2, expected.2) <= 1,
            "got {actual:?}, expected {expected:?} ±1"
        );
    }

    #[test]
    fn yuyv_black_white_limited_range() {
        // BT.601 limited range: black Y=16, white Y=235, both achromatic.
        assert_near(convert_pair(0, 0, 0), (16, 128, 128));
        assert_near(convert_pair(255, 255, 255), (235, 128, 128));
    }

    #[test]
    fn yuyv_primaries_bt601() {
        // Reference BT.601 triples for full-intensity primaries.
        assert_near(convert_pair(0, 0, 255), (82, 90, 240)); // red
        assert_near(convert_pair(0, 255, 0), (145, 54, 34)); // green
        assert_near(convert_pair(255, 0, 0), (41, 240, 110)); // blue
    }

    #[test]
    fn yuyv_layout_parity() {
        // The same color through Bgra8 and Rgba8 layouts must convert identically.
        let bgra = [0u8, 128, 255, 255, 0, 128, 255, 255]; // orange-ish, B=0 G=128 R=255
        let rgba = [255u8, 128, 0, 255, 255, 128, 0, 255];
        let (mut out_b, mut out_r) = (Vec::new(), Vec::new());
        bgra_to_yuyv(&bgra, 2, 1, FrameLayout::Bgra8, &mut out_b);
        bgra_to_yuyv(&rgba, 2, 1, FrameLayout::Rgba8, &mut out_r);
        assert_eq!(out_b, out_r);
    }

    #[test]
    fn yuyv_output_length_even_and_odd_width() {
        let src = vec![100u8; 4 * 4 * 3]; // 4x3
        let mut out = Vec::new();
        bgra_to_yuyv(&src, 4, 3, FrameLayout::Bgra8, &mut out);
        assert_eq!(out.len(), 4 * 3 * 2);

        let src = vec![100u8; 5 * 4 * 3]; // 5x3, odd width
        bgra_to_yuyv(&src, 5, 3, FrameLayout::Bgra8, &mut out);
        assert_eq!(out.len(), 5 * 3 * 2);
    }

    #[test]
    fn yuyv_chroma_averages_the_pair() {
        // Red next to blue: U/V must come from the average, not either pixel.
        let src = [0u8, 0, 255, 255, 255, 0, 0, 255]; // BGRA: red, blue
        let mut out = Vec::new();
        bgra_to_yuyv(&src, 2, 1, FrameLayout::Bgra8, &mut out);
        // Averaged pair is (127, 0, 127) — magenta-ish; both chroma above center.
        assert!(
            out[1] > 128 && out[3] > 128,
            "got U={} V={}",
            out[1],
            out[3]
        );
        // Lumas stay per-pixel and differ (red brighter than blue in Y).
        assert!(out[0] > out[2]);
    }
}
