use super::image_source;
use super::types::{ImageSampleDef, ParticleAux};

const FONT_DATA: &[u8] = include_bytes!("../../../../../assets/fonts/Inter-Bold.ttf");

/// Render a text string to particle positions by rasterizing glyphs onto a bitmap
/// and sampling through the standard image pipeline.
pub fn render_text_to_particles(
    text: &str,
    max_particles: u32,
    particle_size: f32,
) -> Vec<ParticleAux> {
    if text.is_empty() {
        return Vec::new();
    }

    let font = match fontdue::Font::from_bytes(FONT_DATA, fontdue::FontSettings::default()) {
        Ok(f) => f,
        Err(e) => {
            log::error!("Failed to parse font: {}", e);
            return Vec::new();
        }
    };

    let char_count = text.chars().count().max(1) as f32;
    let px_size = (1024.0 / char_count * 0.6).clamp(32.0, 256.0);

    // Rasterize each glyph and compute total width
    let mut glyphs: Vec<(fontdue::Metrics, Vec<u8>)> = Vec::new();
    let mut total_width: i32 = 0;
    let mut max_ascent: i32 = 0;
    let mut max_descent: i32 = 0;
    let mut prev_char: Option<char> = None;

    for ch in text.chars() {
        if let Some(prev) = prev_char {
            if let Some(kern) = font.horizontal_kern(prev, ch, px_size) {
                total_width += kern as i32;
            }
        }
        let (metrics, bitmap) = font.rasterize(ch, px_size);
        let glyph_top = metrics.ymin + metrics.height as i32;
        max_ascent = max_ascent.max(glyph_top);
        max_descent = max_descent.min(metrics.ymin);
        total_width += metrics.advance_width as i32;
        glyphs.push((metrics, bitmap));
        prev_char = Some(ch);
    }

    let bitmap_width = (total_width + 4).max(1) as u32;
    let bitmap_height = ((max_ascent - max_descent) + 4).max(1) as u32;
    let baseline_y = max_ascent + 2;

    // Composite glyphs into RGBA bitmap (white text, alpha = coverage)
    let mut rgba = vec![0u8; (bitmap_width * bitmap_height * 4) as usize];
    let mut cursor_x: i32 = 2;
    prev_char = None;

    for (i, ch) in text.chars().enumerate() {
        if let Some(prev) = prev_char {
            if let Some(kern) = font.horizontal_kern(prev, ch, px_size) {
                cursor_x += kern as i32;
            }
        }

        let (ref metrics, ref bitmap) = glyphs[i];
        let gx = cursor_x + metrics.xmin;
        let gy = baseline_y - (metrics.ymin + metrics.height as i32);

        for row in 0..metrics.height {
            for col in 0..metrics.width {
                let px = gx + col as i32;
                let py = gy + row as i32;
                if px >= 0 && px < bitmap_width as i32 && py >= 0 && py < bitmap_height as i32 {
                    let coverage = bitmap[row * metrics.width + col];
                    if coverage > 0 {
                        let idx = ((py as u32 * bitmap_width + px as u32) * 4) as usize;
                        // Max blend for overlapping glyphs
                        rgba[idx] = 255;
                        rgba[idx + 1] = 255;
                        rgba[idx + 2] = 255;
                        rgba[idx + 3] = rgba[idx + 3].max(coverage);
                    }
                }
            }
        }

        cursor_x += metrics.advance_width as i32;
        prev_char = Some(ch);
    }

    let sample_def = ImageSampleDef {
        mode: "grid".to_string(),
        threshold: 0.1,
        scale: 1.0,
    };

    let _ = particle_size; // density handled by image sampler
    image_source::sample_rgba_buffer(&rgba, bitmap_width, bitmap_height, &sample_def, max_particles)
}
