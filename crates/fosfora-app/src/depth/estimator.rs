use std::path::Path;

use anyhow::Result;

/// MiDaS v2.1 small depth estimator via ONNX Runtime.
///
/// Input: 1×3×256×256 float32 (RGB normalized 0-1, CHW layout)
/// Output: 1×256×256 float32 inverse depth (near=high value)
pub struct DepthEstimator {
    session: ort::session::Session,
}

/// Target resolution for MiDaS v2.1 small model.
pub const DEPTH_SIZE: u32 = 256;

impl DepthEstimator {
    /// Load ONNX model from file path. Uses CPU execution provider.
    pub fn new(model_path: &Path) -> Result<Self> {
        let session = ort::session::Session::builder()?
            .with_optimization_level(ort::session::builder::GraphOptimizationLevel::Level3)?
            .commit_from_file(model_path)?;
        Ok(Self { session })
    }

    /// Run depth estimation on an RGBA image.
    /// Returns an aspect-correct grayscale depth map (near=bright, far=dark)
    /// plus its dimensions (#1790): the source is letterboxed into the square
    /// model input (edge-replicated padding, no squash), and the output is
    /// cropped back to the valid region — e.g. 256×144 for a 16:9 frame —
    /// normalized over the valid region only.
    pub fn estimate(&mut self, rgba: &[u8], w: u32, h: u32) -> Result<(Vec<u8>, u32, u32)> {
        let size = DEPTH_SIZE as usize;
        let (x0, y0, vw, vh) = letterbox_region(w, h, size);

        // 1. Aspect-preserving downscale into the centered valid region,
        //    edge-replicated to fill the square model input, RGB f32 [0,1].
        let rgb_f32 = letterbox_rgba_to_rgb_f32(rgba, w, h, size);

        // 2. Convert HWC → CHW layout (MiDaS expects 1×3×256×256)
        let mut chw = vec![0.0f32; 3 * size * size];
        for y in 0..size {
            for x in 0..size {
                let src_idx = (y * size + x) * 3;
                let px = y * size + x;
                chw[px] = rgb_f32[src_idx]; // R (channel 0)
                chw[size * size + px] = rgb_f32[src_idx + 1]; // G (channel 1)
                chw[2 * size * size + px] = rgb_f32[src_idx + 2]; // B (channel 2)
            }
        }

        // 3. Create input tensor (1×3×256×256) from (shape, data) tuple
        let input =
            ort::value::Tensor::<f32>::from_array(([1i64, 3, size as i64, size as i64], chw))?;

        // 4. Run inference
        let outputs = self.session.run(ort::inputs![input])?;

        // 5. Extract output (inverse depth float)
        let (_shape, depth_data) = outputs[0].try_extract_tensor::<f32>()?;

        // 6. Crop back to the valid region and normalize to 0-255 over it
        //    (MiDaS output is relative; the replicated padding bands must not
        //    skew the per-frame range).
        let grayscale = crop_normalize(depth_data, size, x0, y0, vw, vh);

        Ok((grayscale, vw as u32, vh as u32))
    }
}

/// Centered aspect-preserving valid region of the square model input.
/// Returns (x0, y0, valid_w, valid_h) with the valid extent rounded to the
/// nearest texel and clamped to [1, target].
fn letterbox_region(w: u32, h: u32, target: usize) -> (usize, usize, usize, usize) {
    let t = target as u64;
    let (vw, vh) = if w >= h {
        (
            t,
            ((h as u64 * t + w as u64 / 2) / w.max(1) as u64).clamp(1, t),
        )
    } else {
        (
            ((w as u64 * t + h as u64 / 2) / h.max(1) as u64).clamp(1, t),
            t,
        )
    };
    let (vw, vh) = (vw as usize, vh as usize);
    ((target - vw) / 2, (target - vh) / 2, vw, vh)
}

/// Crop the square model output to the valid region and normalize that region
/// to 0-255 grayscale. Padding texels are excluded from the min/max.
fn crop_normalize(
    depth: &[f32],
    size: usize,
    x0: usize,
    y0: usize,
    vw: usize,
    vh: usize,
) -> Vec<u8> {
    let mut min_val = f32::MAX;
    let mut max_val = f32::MIN;
    for y in y0..y0 + vh {
        for x in x0..x0 + vw {
            let v = depth[y * size + x];
            if v < min_val {
                min_val = v;
            }
            if v > max_val {
                max_val = v;
            }
        }
    }

    let range = (max_val - min_val).max(1e-6);
    let mut grayscale = Vec::with_capacity(vw * vh);
    for y in y0..y0 + vh {
        for x in x0..x0 + vw {
            let v = depth[y * size + x];
            grayscale.push(((v - min_val) / range * 255.0) as u8);
        }
    }
    grayscale
}

/// Box-filter (area-average) the full source image into a `vw`×`vh` sub-rect
/// at (`x0`,`y0`) of an RGB f32 [0,1] buffer with row stride `stride` pixels.
#[allow(clippy::too_many_arguments)]
fn downscale_rgba_into(
    rgba: &[u8],
    w: u32,
    h: u32,
    rgb: &mut [f32],
    stride: usize,
    x0: usize,
    y0: usize,
    vw: usize,
    vh: usize,
) {
    let scale_x = w as f64 / vw as f64;
    let scale_y = h as f64 / vh as f64;

    for ty in 0..vh {
        for tx in 0..vw {
            let sx0 = (tx as f64 * scale_x) as u32;
            let sy0 = (ty as f64 * scale_y) as u32;
            let sx1 = (((tx + 1) as f64 * scale_x) as u32).min(w);
            let sy1 = (((ty + 1) as f64 * scale_y) as u32).min(h);

            let mut r_sum = 0.0f64;
            let mut g_sum = 0.0f64;
            let mut b_sum = 0.0f64;
            let mut count = 0u32;

            for sy in sy0..sy1 {
                for sx in sx0..sx1 {
                    let idx = (sy * w + sx) as usize * 4;
                    if idx + 2 < rgba.len() {
                        r_sum += rgba[idx] as f64;
                        g_sum += rgba[idx + 1] as f64;
                        b_sum += rgba[idx + 2] as f64;
                        count += 1;
                    }
                }
            }

            let dst_idx = ((y0 + ty) * stride + (x0 + tx)) * 3;
            if count > 0 {
                let inv = 1.0 / (count as f64 * 255.0);
                rgb[dst_idx] = (r_sum * inv) as f32;
                rgb[dst_idx + 1] = (g_sum * inv) as f32;
                rgb[dst_idx + 2] = (b_sum * inv) as f32;
            }
        }
    }
}

/// Downscale RGBA image to target_size × target_size RGB f32 [0,1].
/// Non-square sources are squashed; prefer `letterbox_rgba_to_rgb_f32`.
#[cfg(test)]
fn downscale_rgba_to_rgb_f32(rgba: &[u8], w: u32, h: u32, target_size: usize) -> Vec<f32> {
    let mut rgb = vec![0.0f32; target_size * target_size * 3];
    downscale_rgba_into(
        rgba,
        w,
        h,
        &mut rgb,
        target_size,
        0,
        0,
        target_size,
        target_size,
    );
    rgb
}

/// Aspect-preserving downscale into a centered valid region of a square
/// `target`×`target` RGB f32 buffer, with the surrounding letterbox bands
/// edge-replicated from the nearest valid texel (#1790) — replication avoids
/// feeding MiDaS a synthetic hard edge at the frame border.
fn letterbox_rgba_to_rgb_f32(rgba: &[u8], w: u32, h: u32, target: usize) -> Vec<f32> {
    let (x0, y0, vw, vh) = letterbox_region(w, h, target);
    let mut rgb = vec![0.0f32; target * target * 3];
    downscale_rgba_into(rgba, w, h, &mut rgb, target, x0, y0, vw, vh);

    // Replicate edge columns outward (left/right bands).
    for y in y0..y0 + vh {
        let row = y * target;
        let first = (row + x0) * 3;
        let last = (row + x0 + vw - 1) * 3;
        let (first_px, last_px) = (
            [rgb[first], rgb[first + 1], rgb[first + 2]],
            [rgb[last], rgb[last + 1], rgb[last + 2]],
        );
        for x in 0..x0 {
            let d = (row + x) * 3;
            rgb[d..d + 3].copy_from_slice(&first_px);
        }
        for x in x0 + vw..target {
            let d = (row + x) * 3;
            rgb[d..d + 3].copy_from_slice(&last_px);
        }
    }
    // Replicate edge rows outward (top/bottom bands), now full-width.
    let row_len = target * 3;
    let top = y0 * row_len;
    let bottom = (y0 + vh - 1) * row_len;
    let top_row = rgb[top..top + row_len].to_vec();
    let bottom_row = rgb[bottom..bottom + row_len].to_vec();
    for y in 0..y0 {
        rgb[y * row_len..(y + 1) * row_len].copy_from_slice(&top_row);
    }
    for y in y0 + vh..target {
        rgb[y * row_len..(y + 1) * row_len].copy_from_slice(&bottom_row);
    }

    rgb
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downscale_rgba_basic() {
        // 2×2 image → 1×1 target: should average all pixels
        let rgba = vec![
            255, 0, 0, 255, // red
            0, 255, 0, 255, // green
            0, 0, 255, 255, // blue
            255, 255, 0, 255, // yellow
        ];
        let result = downscale_rgba_to_rgb_f32(&rgba, 2, 2, 1);
        assert_eq!(result.len(), 3);
        // R: (255+0+0+255)/4/255 ≈ 0.5
        assert!((result[0] - 0.5).abs() < 0.01);
        // G: (0+255+0+255)/4/255 ≈ 0.5
        assert!((result[1] - 0.5).abs() < 0.01);
        // B: (0+0+255+0)/4/255 ≈ 0.25
        assert!((result[2] - 0.25).abs() < 0.01);
    }

    #[test]
    fn downscale_rgba_same_size() {
        // 1×1 → 1×1
        let rgba = vec![128, 64, 32, 255];
        let result = downscale_rgba_to_rgb_f32(&rgba, 1, 1, 1);
        assert!((result[0] - 128.0 / 255.0).abs() < 0.01);
        assert!((result[1] - 64.0 / 255.0).abs() < 0.01);
        assert!((result[2] - 32.0 / 255.0).abs() < 0.01);
    }

    // --- Letterbox pipeline (#1790) ---

    #[test]
    fn letterbox_region_cases() {
        // 16:9 landscape: full width, 144 rows centered vertically.
        assert_eq!(letterbox_region(1920, 1080, 256), (0, 56, 256, 144));
        // 4:3 landscape.
        assert_eq!(letterbox_region(640, 480, 256), (0, 32, 256, 192));
        // 9:16 portrait: full height, 144 cols centered horizontally.
        assert_eq!(letterbox_region(1080, 1920, 256), (56, 0, 144, 256));
        // Square: no letterbox.
        assert_eq!(letterbox_region(500, 500, 256), (0, 0, 256, 256));
    }

    #[test]
    fn letterbox_pad_replicates_edges() {
        // 4×2 source (2:1) into a 4×4 target: valid rows 1..3, bands above/below.
        // Rows: top half red, bottom half blue.
        let mut rgba = Vec::new();
        for _ in 0..4 {
            rgba.extend_from_slice(&[255, 0, 0, 255]);
        }
        for _ in 0..4 {
            rgba.extend_from_slice(&[0, 0, 255, 255]);
        }
        let rgb = letterbox_rgba_to_rgb_f32(&rgba, 4, 2, 4);
        assert_eq!(letterbox_region(4, 2, 4), (0, 1, 4, 2));
        let px = |x: usize, y: usize| [rgb[(y * 4 + x) * 3], rgb[(y * 4 + x) * 3 + 2]];
        // Valid rows: row 1 = red, row 2 = blue.
        assert_eq!(px(0, 1), [1.0, 0.0]);
        assert_eq!(px(3, 2), [0.0, 1.0]);
        // Padding rows replicate the nearest valid row, not black.
        assert_eq!(px(0, 0), [1.0, 0.0]);
        assert_eq!(px(3, 3), [0.0, 1.0]);
    }

    #[test]
    fn crop_normalize_ignores_padding() {
        // 8×8 field: valid region rows 2..6 holds 0.0..=1.0, padding holds
        // extreme values that must not affect normalization.
        let size = 8;
        let (x0, y0, vw, vh) = (0, 2, 8, 4);
        let mut depth = vec![-100.0f32; size * size];
        for y in y0..y0 + vh {
            for x in x0..x0 + vw {
                depth[y * size + x] = (y - y0) as f32 / (vh - 1) as f32;
            }
        }
        depth[0] = 1e6; // extreme padding values
        let out = crop_normalize(&depth, size, x0, y0, vw, vh);
        assert_eq!(out.len(), vw * vh);
        // Full 0..255 range over the valid region alone.
        assert_eq!(out[0], 0);
        assert_eq!(*out.last().unwrap(), 255);
        // Middle rows sit strictly inside the range.
        assert!(out[vw] > 0 && out[vw] < 255);
    }
}
