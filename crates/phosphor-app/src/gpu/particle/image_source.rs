use std::path::Path;

use super::types::{ImageSampleDef, ParticleAux};

/// Maximum image dimension after resize.
/// 2048 supports up to ~1M particles with grid sampling (step=2 at 2048²).
const MAX_DIM: u32 = 2048;

/// Jitter magnitude as fraction of grid cell step.
/// ±0.4 × step breaks up row/column alignment in smooth gradient areas.
const GRID_JITTER: f32 = 0.4;

/// Sampled point with float pixel position, RGBA color, and gradient magnitude.
struct SamplePoint {
    /// Fractional pixel x (may be jittered off-grid).
    px: f32,
    /// Fractional pixel y (may be jittered off-grid).
    py: f32,
    r: u8,
    g: u8,
    b: u8,
    a: u8,
    /// Luminance gradient magnitude at the sample location.
    gradient: f32,
}

/// Deterministic per-index jitter in [-1, 1] using LCG hash.
fn grid_jitter(index: u32) -> (f32, f32) {
    // Two independent LCG steps seeded from index
    let s0 = (index as u64)
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    let s1 = s0
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    // Map upper bits to [-1, 1]
    let jx = ((s0 >> 33) as f32 / (u32::MAX >> 1) as f32) * 2.0 - 1.0;
    let jy = ((s1 >> 33) as f32 / (u32::MAX >> 1) as f32) * 2.0 - 1.0;
    (jx, jy)
}

/// Luminance of a pixel at integer coords (clamped to image bounds).
fn luminance_at(img: &image::RgbaImage, x: i32, y: i32, w: u32, h: u32) -> f32 {
    let x = x.clamp(0, w as i32 - 1) as u32;
    let y = y.clamp(0, h as i32 - 1) as u32;
    let p = img.get_pixel(x, y);
    p[0] as f32 * 0.299 + p[1] as f32 * 0.587 + p[2] as f32 * 0.114
}

/// Luminance gradient magnitude at integer pixel coords via central difference.
fn gradient_at(img: &image::RgbaImage, x: u32, y: u32, w: u32, h: u32) -> f32 {
    let ix = x as i32;
    let iy = y as i32;
    let dx = luminance_at(img, ix + 1, iy, w, h) - luminance_at(img, ix - 1, iy, w, h);
    let dy = luminance_at(img, ix, iy + 1, w, h) - luminance_at(img, ix, iy - 1, w, h);
    (dx * dx + dy * dy).sqrt()
}

/// Bilinear sample RGBA from fractional pixel coordinates.
fn bilinear_sample(img: &image::RgbaImage, fx: f32, fy: f32, w: u32, h: u32) -> (u8, u8, u8, u8) {
    let x0 = (fx.floor() as i32).clamp(0, w as i32 - 1) as u32;
    let y0 = (fy.floor() as i32).clamp(0, h as i32 - 1) as u32;
    let x1 = (x0 + 1).min(w - 1);
    let y1 = (y0 + 1).min(h - 1);
    let tx = fx - fx.floor();
    let ty = fy - fy.floor();

    let p00 = img.get_pixel(x0, y0);
    let p10 = img.get_pixel(x1, y0);
    let p01 = img.get_pixel(x0, y1);
    let p11 = img.get_pixel(x1, y1);

    let lerp = |c: usize| -> u8 {
        let v = p00[c] as f32 * (1.0 - tx) * (1.0 - ty)
            + p10[c] as f32 * tx * (1.0 - ty)
            + p01[c] as f32 * (1.0 - tx) * ty
            + p11[c] as f32 * tx * ty;
        v.round().clamp(0.0, 255.0) as u8
    };

    (lerp(0), lerp(1), lerp(2), lerp(3))
}

/// Sample pixels from an image into ParticleAux data.
/// Returns aux data with home positions (clip space) and packed RGBA colors.
pub fn sample_image(
    path: &Path,
    sample_def: &ImageSampleDef,
    max_particles: u32,
) -> Result<Vec<ParticleAux>, String> {
    let img = image::open(path).map_err(|e| format!("Failed to load image: {e}"))?;

    // Resize to cap dimensions while preserving aspect ratio
    let img = if img.width() > MAX_DIM || img.height() > MAX_DIM {
        img.resize(MAX_DIM, MAX_DIM, image::imageops::FilterType::Lanczos3)
    } else {
        img
    };

    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();

    let samples = match sample_def.mode.as_str() {
        "threshold" => sample_threshold(&rgba, w, h, sample_def.threshold, max_particles),
        "random" => sample_random(&rgba, w, h, max_particles),
        _ => sample_grid(&rgba, w, h, max_particles), // "grid" is default
    };

    // Map pixel coords to clip space with aspect ratio correction
    let aspect = w as f32 / h as f32;
    let scale = sample_def.scale;
    let aux: Vec<ParticleAux> = samples
        .into_iter()
        .map(|sp| {
            let cx = ((sp.px / w as f32) * 2.0 - 1.0) * scale;
            let cy = -((sp.py / h as f32) * 2.0 - 1.0) * scale; // flip Y
            // Correct for non-square aspect ratio
            let cx = if aspect > 1.0 { cx } else { cx * aspect };
            let cy = if aspect > 1.0 { cy / aspect } else { cy };
            // Pack RGBA into f32 via bitcast
            let packed = pack_rgba(sp.r, sp.g, sp.b, sp.a);
            ParticleAux {
                home: [cx, cy, packed, sp.gradient],
            }
        })
        .collect();

    log::info!(
        "Sampled {} particles from {}x{} image (mode: {}, scale: {:.1})",
        aux.len(),
        w,
        h,
        sample_def.mode,
        scale
    );
    Ok(aux)
}

/// Grid sampling with jittered positions, bilinear color, and gradient.
/// Every Nth pixel to fit within max_particles, with ±GRID_JITTER×step random offset.
fn sample_grid(img: &image::RgbaImage, w: u32, h: u32, max_particles: u32) -> Vec<SamplePoint> {
    let total_pixels = w * h;
    let step = ((total_pixels as f32 / max_particles as f32).sqrt().ceil() as u32).max(1);
    let step_f = step as f32;
    let mut samples = Vec::new();
    let mut index: u32 = 0;

    for y in (0..h).step_by(step as usize) {
        for x in (0..w).step_by(step as usize) {
            if samples.len() >= max_particles as usize {
                break;
            }
            // Fast reject: check transparency at original grid position
            let pixel = img.get_pixel(x, y);
            if pixel[3] < 10 {
                index += 1;
                continue;
            }

            // Compute gradient at original integer position (stable across video frames)
            let grad = gradient_at(img, x, y, w, h);

            // Apply deterministic jitter
            let (jx, jy) = grid_jitter(index);
            let fx = (x as f32 + jx * GRID_JITTER * step_f).clamp(0.0, (w - 1) as f32);
            let fy = (y as f32 + jy * GRID_JITTER * step_f).clamp(0.0, (h - 1) as f32);

            // Bilinear sample at jittered position
            let (r, g, b, a) = bilinear_sample(img, fx, fy, w, h);

            // Second alpha check after bilinear interpolation
            if a < 10 {
                index += 1;
                continue;
            }

            samples.push(SamplePoint {
                px: fx,
                py: fy,
                r,
                g,
                b,
                a,
                gradient: grad,
            });
            index += 1;
        }
    }
    samples
}

/// Threshold sampling: pixels above brightness threshold.
fn sample_threshold(
    img: &image::RgbaImage,
    w: u32,
    h: u32,
    threshold: f32,
    max_particles: u32,
) -> Vec<SamplePoint> {
    let mut candidates = Vec::new();

    for y in 0..h {
        for x in 0..w {
            let pixel = img.get_pixel(x, y);
            if pixel[3] < 10 {
                continue;
            }
            let brightness =
                (pixel[0] as f32 * 0.299 + pixel[1] as f32 * 0.587 + pixel[2] as f32 * 0.114)
                    / 255.0;
            if brightness > threshold {
                let grad = gradient_at(img, x, y, w, h);
                candidates.push(SamplePoint {
                    px: x as f32,
                    py: y as f32,
                    r: pixel[0],
                    g: pixel[1],
                    b: pixel[2],
                    a: pixel[3],
                    gradient: grad,
                });
            }
        }
    }

    // Subsample if too many
    if candidates.len() > max_particles as usize {
        let step = candidates.len() / max_particles as usize;
        candidates
            .into_iter()
            .step_by(step.max(1))
            .take(max_particles as usize)
            .collect()
    } else {
        candidates
    }
}

/// Random sampling: uniform random subset.
fn sample_random(img: &image::RgbaImage, w: u32, h: u32, max_particles: u32) -> Vec<SamplePoint> {
    // Simple deterministic pseudo-random using hash
    let total = w * h;
    let count = max_particles.min(total);
    let mut samples = Vec::with_capacity(count as usize);
    let mut seen = std::collections::HashSet::new();

    // LCG pseudo-random
    let mut rng: u64 = 42;
    let a: u64 = 6364136223846793005;
    let c: u64 = 1442695040888963407;

    while samples.len() < count as usize {
        rng = rng.wrapping_mul(a).wrapping_add(c);
        let idx = (rng >> 33) as u32 % total;
        if seen.contains(&idx) {
            continue;
        }
        seen.insert(idx);

        let x = idx % w;
        let y = idx / w;
        let pixel = img.get_pixel(x, y);
        if pixel[3] < 10 {
            continue;
        }
        let grad = gradient_at(img, x, y, w, h);
        samples.push(SamplePoint {
            px: x as f32,
            py: y as f32,
            r: pixel[0],
            g: pixel[1],
            b: pixel[2],
            a: pixel[3],
            gradient: grad,
        });
    }
    samples
}

/// Sample pixels from raw RGBA buffer data into ParticleAux data.
/// Like `sample_image()`, but skips file I/O and image crate decode — used for
/// per-frame updates from video or webcam sources where data is already decoded RGBA.
pub fn sample_rgba_buffer(
    data: &[u8],
    width: u32,
    height: u32,
    sample_def: &ImageSampleDef,
    max_particles: u32,
) -> Vec<ParticleAux> {
    if data.is_empty() || width == 0 || height == 0 {
        return Vec::new();
    }

    // Resize if needed (reuse image crate for downscale)
    let (rgba_data, w, h) = if width > MAX_DIM || height > MAX_DIM {
        let img = image::RgbaImage::from_raw(width, height, data.to_vec());
        match img {
            Some(img) => {
                let resized = image::imageops::resize(
                    &img,
                    MAX_DIM.min(width),
                    MAX_DIM.min(height),
                    image::imageops::FilterType::Nearest,
                );
                let (rw, rh) = resized.dimensions();
                (resized.into_raw(), rw, rh)
            }
            None => return Vec::new(),
        }
    } else {
        (data.to_vec(), width, height)
    };

    // Build a temporary RgbaImage for sampling functions
    let img = match image::RgbaImage::from_raw(w, h, rgba_data) {
        Some(img) => img,
        None => return Vec::new(),
    };

    let samples = match sample_def.mode.as_str() {
        "threshold" => sample_threshold(&img, w, h, sample_def.threshold, max_particles),
        "random" => sample_random(&img, w, h, max_particles),
        _ => sample_grid(&img, w, h, max_particles),
    };

    // Map pixel coords to clip space with aspect ratio correction
    let aspect = w as f32 / h as f32;
    let scale = sample_def.scale;
    samples
        .into_iter()
        .map(|sp| {
            let cx = ((sp.px / w as f32) * 2.0 - 1.0) * scale;
            let cy = -((sp.py / h as f32) * 2.0 - 1.0) * scale;
            let cx = if aspect > 1.0 { cx } else { cx * aspect };
            let cy = if aspect > 1.0 { cy / aspect } else { cy };
            let packed = pack_rgba(sp.r, sp.g, sp.b, sp.a);
            ParticleAux {
                home: [cx, cy, packed, sp.gradient],
            }
        })
        .collect()
}

/// Pack RGBA bytes into a single f32 via bitcast from u32.
fn pack_rgba(r: u8, g: u8, b: u8, a: u8) -> f32 {
    let packed = (r as u32) | ((g as u32) << 8) | ((b as u32) << 16) | ((a as u32) << 24);
    f32::from_bits(packed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_rgba_buffer_empty() {
        let def = ImageSampleDef {
            mode: "grid".to_string(),
            threshold: 0.1,
            scale: 1.0,
        };
        assert!(sample_rgba_buffer(&[], 0, 0, &def, 100).is_empty());
    }

    #[test]
    fn sample_rgba_buffer_single_pixel() {
        // 1x1 white pixel
        let data = vec![255, 255, 255, 255];
        let def = ImageSampleDef {
            mode: "grid".to_string(),
            threshold: 0.1,
            scale: 1.0,
        };
        let result = sample_rgba_buffer(&data, 1, 1, &def, 100);
        assert_eq!(result.len(), 1);
        // Position should be at origin (single pixel maps to center)
        assert!(result[0].home[0].abs() < 1.1);
        assert!(result[0].home[1].abs() < 1.1);
    }

    #[test]
    fn sample_rgba_buffer_grid_mode() {
        // 4x4 opaque red image
        let mut data = Vec::with_capacity(4 * 4 * 4);
        for _ in 0..16 {
            data.extend_from_slice(&[255, 0, 0, 255]);
        }
        let def = ImageSampleDef {
            mode: "grid".to_string(),
            threshold: 0.1,
            scale: 1.0,
        };
        let result = sample_rgba_buffer(&data, 4, 4, &def, 100);
        assert!(!result.is_empty());
        assert!(result.len() <= 16);
    }

    #[test]
    fn sample_rgba_buffer_transparent_skipped() {
        // 2x2 fully transparent image
        let data = vec![0u8; 2 * 2 * 4];
        let def = ImageSampleDef {
            mode: "grid".to_string(),
            threshold: 0.1,
            scale: 1.0,
        };
        let result = sample_rgba_buffer(&data, 2, 2, &def, 100);
        assert!(result.is_empty());
    }

    #[test]
    fn sample_rgba_buffer_respects_max_particles() {
        // 100x100 opaque image = 10000 pixels, limit to 50
        let data = vec![255u8; 100 * 100 * 4];
        let def = ImageSampleDef {
            mode: "grid".to_string(),
            threshold: 0.1,
            scale: 1.0,
        };
        let result = sample_rgba_buffer(&data, 100, 100, &def, 50);
        assert!(result.len() <= 50);
    }

    // --- Tests for new helpers ---

    #[test]
    fn grid_jitter_deterministic() {
        // Same index always produces same jitter
        let (jx1, jy1) = grid_jitter(42);
        let (jx2, jy2) = grid_jitter(42);
        assert_eq!(jx1, jx2);
        assert_eq!(jy1, jy2);
    }

    #[test]
    fn grid_jitter_range() {
        // Jitter values should be in [-1, 1]
        for i in 0..1000 {
            let (jx, jy) = grid_jitter(i);
            assert!(jx >= -1.0 && jx <= 1.0, "jx={jx} out of range for i={i}");
            assert!(jy >= -1.0 && jy <= 1.0, "jy={jy} out of range for i={i}");
        }
    }

    #[test]
    fn grid_jitter_varies() {
        // Different indices should produce different values (very unlikely collision)
        let (jx0, jy0) = grid_jitter(0);
        let (jx1, jy1) = grid_jitter(1);
        assert!(jx0 != jx1 || jy0 != jy1);
    }

    #[test]
    fn bilinear_sample_at_pixel_center() {
        // 2x2 image: top-left red, top-right green, bottom-left blue, bottom-right white
        let data = vec![
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
        ];
        let img = image::RgbaImage::from_raw(2, 2, data).unwrap();
        // Sample at (0.0, 0.0) — top-left pixel
        let (r, g, b, a) = bilinear_sample(&img, 0.0, 0.0, 2, 2);
        assert_eq!((r, g, b, a), (255, 0, 0, 255));
    }

    #[test]
    fn bilinear_sample_interpolated() {
        // 2x1 image: black, white
        let data = vec![0, 0, 0, 255, 255, 255, 255, 255];
        let img = image::RgbaImage::from_raw(2, 1, data).unwrap();
        // Sample at midpoint (0.5, 0.0) — should be ~128
        let (r, g, b, _) = bilinear_sample(&img, 0.5, 0.0, 2, 1);
        assert!((r as i32 - 128).abs() <= 1);
        assert!((g as i32 - 128).abs() <= 1);
        assert!((b as i32 - 128).abs() <= 1);
    }

    #[test]
    fn gradient_at_uniform() {
        // 4x4 uniform gray image — gradient should be 0
        let mut data = Vec::with_capacity(4 * 4 * 4);
        for _ in 0..16 {
            data.extend_from_slice(&[128, 128, 128, 255]);
        }
        let img = image::RgbaImage::from_raw(4, 4, data).unwrap();
        let grad = gradient_at(&img, 2, 2, 4, 4);
        assert!(
            grad.abs() < 0.01,
            "uniform image gradient should be ~0, got {grad}"
        );
    }

    #[test]
    fn gradient_at_horizontal_edge() {
        // 4x1 image: black, black, white, white — gradient at x=1 should be nonzero
        let data = vec![
            0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255, 255, 255, 255, 255, 255,
        ];
        let img = image::RgbaImage::from_raw(4, 1, data).unwrap();
        let grad = gradient_at(&img, 1, 0, 4, 1);
        assert!(grad > 100.0, "edge gradient should be large, got {grad}");
    }

    #[test]
    fn grid_jitter_stored_in_home_w() {
        // Grid-sampled buffer should have gradient in home[3]
        // 8x8 image with horizontal gradient (left=black, right=white)
        let mut data = Vec::with_capacity(8 * 8 * 4);
        for _y in 0..8 {
            for x in 0..8 {
                let v = (x * 255 / 7) as u8;
                data.extend_from_slice(&[v, v, v, 255]);
            }
        }
        let def = ImageSampleDef {
            mode: "grid".to_string(),
            threshold: 0.1,
            scale: 1.0,
        };
        let result = sample_rgba_buffer(&data, 8, 8, &def, 100);
        assert!(!result.is_empty());
        // At least some particles should have nonzero gradient
        let has_gradient = result.iter().any(|p| p.home[3] > 0.0);
        assert!(
            has_gradient,
            "gradient image should produce nonzero home[3]"
        );
    }
}
