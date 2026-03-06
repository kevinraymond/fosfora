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
    /// Returns a 256×256 grayscale depth map (near=bright, far=dark).
    pub fn estimate(&mut self, rgba: &[u8], w: u32, h: u32) -> Result<Vec<u8>> {
        let size = DEPTH_SIZE as usize;

        // 1. Downscale to 256×256 and convert RGBA → RGB f32 normalized [0,1]
        let rgb_f32 = downscale_rgba_to_rgb_f32(rgba, w, h, size);

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

        // 6. Normalize to 0-255 (MiDaS output is relative, needs per-frame normalization)
        let mut min_val = f32::MAX;
        let mut max_val = f32::MIN;
        for &v in depth_data {
            if v < min_val {
                min_val = v;
            }
            if v > max_val {
                max_val = v;
            }
        }

        let range = (max_val - min_val).max(1e-6);
        let grayscale: Vec<u8> = depth_data
            .iter()
            .map(|&v| ((v - min_val) / range * 255.0) as u8)
            .collect();

        Ok(grayscale)
    }
}

/// Downscale RGBA image to target_size × target_size RGB f32 [0,1].
/// Uses simple box-filter (area averaging) for quality downscaling.
fn downscale_rgba_to_rgb_f32(rgba: &[u8], w: u32, h: u32, target_size: usize) -> Vec<f32> {
    let mut rgb = vec![0.0f32; target_size * target_size * 3];
    let src_w = w as f64;
    let src_h = h as f64;
    let scale_x = src_w / target_size as f64;
    let scale_y = src_h / target_size as f64;

    for ty in 0..target_size {
        for tx in 0..target_size {
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

            let dst_idx = (ty * target_size + tx) * 3;
            if count > 0 {
                let inv = 1.0 / (count as f64 * 255.0);
                rgb[dst_idx] = (r_sum * inv) as f32;
                rgb[dst_idx + 1] = (g_sum * inv) as f32;
                rgb[dst_idx + 2] = (b_sum * inv) as f32;
            }
        }
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
}
