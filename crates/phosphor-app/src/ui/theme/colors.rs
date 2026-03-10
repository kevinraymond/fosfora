use egui::Color32;

/// Runtime theme color set used by all UI panels.
/// Stored in egui temp data, read via `theme_colors(ctx)`.
#[derive(Debug, Clone, Copy)]
pub struct ThemeColors {
    #[allow(dead_code)]
    pub canvas: Color32,
    pub panel: Color32,
    pub text_primary: Color32,
    pub text_secondary: Color32,
    pub accent: Color32,
    pub error: Color32,
    pub warning: Color32,
    pub success: Color32,
    pub widget_bg: Color32,
    pub card_bg: Color32,
    pub card_border: Color32,
    pub beat_color: Color32,
    pub meter_bg: Color32,
    #[allow(dead_code)]
    pub separator: Color32,
    pub text_dim: Color32,
    pub hover_fill: Color32,
    pub hover_border: Color32,
    pub backdrop: Color32,
}

impl ThemeColors {
    pub fn dark() -> Self {
        Self {
            canvas: Color32::from_rgb(0x12, 0x12, 0x12),
            panel: Color32::from_rgba_premultiplied(0x22, 0x22, 0x22, 0xE6),
            text_primary: Color32::from_rgb(0xE8, 0xE8, 0xE8),
            text_secondary: Color32::from_rgb(0xA0, 0xA0, 0xA0),
            accent: Color32::from_rgb(0x44, 0x88, 0xFF),
            error: Color32::from_rgb(0xE0, 0x60, 0x60),
            warning: Color32::from_rgb(0xD4, 0xA0, 0x40),
            success: Color32::from_rgb(0x50, 0xC0, 0x70),
            widget_bg: Color32::from_rgb(0x2A, 0x2A, 0x2A),
            card_bg: Color32::from_rgb(0x24, 0x24, 0x24),
            card_border: Color32::from_rgb(0x33, 0x33, 0x33),
            beat_color: Color32::from_rgb(0xFF, 0x55, 0x77),
            meter_bg: Color32::from_rgb(0x1A, 0x1A, 0x1A),
            separator: Color32::from_rgb(0x3A, 0x3A, 0x3A),
            text_dim: Color32::from_rgb(0x55, 0x55, 0x55),
            hover_fill: Color32::from_white_alpha(10),
            hover_border: Color32::from_white_alpha(12),
            backdrop: Color32::from_black_alpha(180),
        }
    }

    pub fn light() -> Self {
        Self {
            canvas: Color32::from_rgb(0xF5, 0xF5, 0xF5),
            panel: Color32::from_rgb(0xFF, 0xFF, 0xFF),
            text_primary: Color32::from_rgb(0x1A, 0x1A, 0x1A),
            text_secondary: Color32::from_rgb(0x5A, 0x5A, 0x5A),
            accent: Color32::from_rgb(0x09, 0x69, 0xA8),
            error: Color32::from_rgb(0xC0, 0x30, 0x30),
            warning: Color32::from_rgb(0xA0, 0x70, 0x10),
            success: Color32::from_rgb(0x20, 0x80, 0x40),
            widget_bg: Color32::from_rgb(0xE8, 0xE8, 0xE8),
            card_bg: Color32::from_rgb(0xF0, 0xF0, 0xF0),
            card_border: Color32::from_rgb(0xD5, 0xD5, 0xD5),
            beat_color: Color32::from_rgb(0xD0, 0x30, 0x50),
            meter_bg: Color32::from_rgb(0xE0, 0xE0, 0xE0),
            separator: Color32::from_rgb(0xD5, 0xD5, 0xD5),
            text_dim: Color32::from_rgb(0x99, 0x99, 0x99),
            hover_fill: Color32::from_black_alpha(10),
            hover_border: Color32::from_black_alpha(15),
            backdrop: Color32::from_black_alpha(120),
        }
    }

    pub fn high_contrast() -> Self {
        Self {
            canvas: Color32::from_rgb(0x00, 0x00, 0x00),
            panel: Color32::from_rgb(0x0A, 0x0A, 0x0A),
            text_primary: Color32::WHITE,
            text_secondary: Color32::from_rgb(0xCC, 0xCC, 0xCC),
            accent: Color32::from_rgb(0x55, 0xAA, 0xFF),
            error: Color32::from_rgb(0xFF, 0x44, 0x44),
            warning: Color32::from_rgb(0xFF, 0xCC, 0x00),
            success: Color32::from_rgb(0x44, 0xFF, 0x44),
            widget_bg: Color32::from_rgb(0x1A, 0x1A, 0x1A),
            card_bg: Color32::from_rgb(0x11, 0x11, 0x11),
            card_border: Color32::from_rgb(0x66, 0x66, 0x66),
            beat_color: Color32::from_rgb(0xFF, 0x44, 0x88),
            meter_bg: Color32::from_rgb(0x08, 0x08, 0x08),
            separator: Color32::from_rgb(0x55, 0x55, 0x55),
            text_dim: Color32::from_rgb(0x88, 0x88, 0x88),
            hover_fill: Color32::from_white_alpha(12),
            hover_border: Color32::from_white_alpha(18),
            backdrop: Color32::from_black_alpha(200),
        }
    }

    /// Midnight: deep navy with steel blue and ice blue accent
    pub fn midnight() -> Self {
        Self {
            canvas: Color32::from_rgb(0x08, 0x0C, 0x14),
            panel: Color32::from_rgba_premultiplied(0x0E, 0x12, 0x1C, 0xE6),
            text_primary: Color32::from_rgb(0xD0, 0xE0, 0xF0),
            text_secondary: Color32::from_rgb(0x80, 0x98, 0xB0),
            accent: Color32::from_rgb(0x50, 0x90, 0xD0),
            error: Color32::from_rgb(0xD0, 0x60, 0x70),
            warning: Color32::from_rgb(0xC0, 0xA0, 0x50),
            success: Color32::from_rgb(0x50, 0xB0, 0x80),
            widget_bg: Color32::from_rgb(0x1A, 0x24, 0x36),
            card_bg: Color32::from_rgb(0x14, 0x1A, 0x28),
            card_border: Color32::from_rgb(0x2A, 0x38, 0x50),
            beat_color: Color32::from_rgb(0x60, 0xB0, 0xFF),
            meter_bg: Color32::from_rgb(0x0A, 0x10, 0x18),
            separator: Color32::from_rgb(0x24, 0x34, 0x4A),
            text_dim: Color32::from_rgb(0x50, 0x68, 0x80),
            hover_fill: Color32::from_white_alpha(8),
            hover_border: Color32::from_white_alpha(12),
            backdrop: Color32::from_black_alpha(180),
        }
    }

    /// Ember: warm charcoal with amber-tinted widgets and orange accent
    pub fn ember() -> Self {
        Self {
            canvas: Color32::from_rgb(0x10, 0x0C, 0x08),
            panel: Color32::from_rgba_premultiplied(0x1A, 0x14, 0x10, 0xE6),
            text_primary: Color32::from_rgb(0xE8, 0xD8, 0xC8),
            text_secondary: Color32::from_rgb(0xA8, 0x90, 0x78),
            accent: Color32::from_rgb(0xE0, 0x90, 0x30),
            error: Color32::from_rgb(0xE0, 0x50, 0x40),
            warning: Color32::from_rgb(0xD4, 0xA0, 0x40),
            success: Color32::from_rgb(0x80, 0xC0, 0x50),
            widget_bg: Color32::from_rgb(0x2C, 0x22, 0x1A),
            card_bg: Color32::from_rgb(0x24, 0x1C, 0x16),
            card_border: Color32::from_rgb(0x40, 0x30, 0x20),
            beat_color: Color32::from_rgb(0xFF, 0x80, 0x30),
            meter_bg: Color32::from_rgb(0x14, 0x10, 0x0C),
            separator: Color32::from_rgb(0x3A, 0x2C, 0x20),
            text_dim: Color32::from_rgb(0x70, 0x5C, 0x48),
            hover_fill: Color32::from_white_alpha(8),
            hover_border: Color32::from_white_alpha(12),
            backdrop: Color32::from_black_alpha(180),
        }
    }

    /// Neon: dark purple-black with vivid magenta accent and cyan highlights
    pub fn neon() -> Self {
        Self {
            canvas: Color32::from_rgb(0x06, 0x04, 0x0C),
            panel: Color32::from_rgba_premultiplied(0x0C, 0x0A, 0x14, 0xE6),
            text_primary: Color32::from_rgb(0xE0, 0xD0, 0xF0),
            text_secondary: Color32::from_rgb(0x90, 0x80, 0xB0),
            accent: Color32::from_rgb(0xFF, 0x50, 0xC0),
            error: Color32::from_rgb(0xFF, 0x40, 0x60),
            warning: Color32::from_rgb(0xE0, 0xC0, 0x40),
            success: Color32::from_rgb(0x40, 0xE0, 0xC0),
            widget_bg: Color32::from_rgb(0x1A, 0x14, 0x2A),
            card_bg: Color32::from_rgb(0x14, 0x10, 0x20),
            card_border: Color32::from_rgb(0x30, 0x20, 0x44),
            beat_color: Color32::from_rgb(0xFF, 0x50, 0xC0),
            meter_bg: Color32::from_rgb(0x08, 0x06, 0x10),
            separator: Color32::from_rgb(0x28, 0x1C, 0x3C),
            text_dim: Color32::from_rgb(0x60, 0x50, 0x80),
            hover_fill: Color32::from_white_alpha(8),
            hover_border: Color32::from_white_alpha(12),
            backdrop: Color32::from_black_alpha(180),
        }
    }
}

const THEME_COLORS_ID: &str = "phosphor_theme_colors";

/// Store theme colors in egui temp data.
pub fn set_theme_colors(ctx: &egui::Context, colors: ThemeColors) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::new(THEME_COLORS_ID), colors));
}

/// Read theme colors from egui temp data (fallback: dark).
pub fn theme_colors(ctx: &egui::Context) -> ThemeColors {
    ctx.data(|d| d.get_temp(egui::Id::new(THEME_COLORS_ID)))
        .unwrap_or_else(ThemeColors::dark)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_theme_canvas() {
        let tc = ThemeColors::dark();
        assert_eq!(tc.canvas, Color32::from_rgb(0x12, 0x12, 0x12));
    }

    #[test]
    fn light_theme_canvas() {
        let tc = ThemeColors::light();
        assert_eq!(tc.canvas, Color32::from_rgb(0xF5, 0xF5, 0xF5));
    }

    #[test]
    fn high_contrast_canvas_is_black() {
        let tc = ThemeColors::high_contrast();
        assert_eq!(tc.canvas, Color32::from_rgb(0x00, 0x00, 0x00));
    }

    #[test]
    fn midnight_has_navy_canvas() {
        let tc = ThemeColors::midnight();
        assert_eq!(tc.canvas, Color32::from_rgb(0x08, 0x0C, 0x14));
        assert_eq!(tc.accent, Color32::from_rgb(0x50, 0x90, 0xD0));
    }

    #[test]
    fn ember_has_warm_canvas() {
        let tc = ThemeColors::ember();
        assert_eq!(tc.canvas, Color32::from_rgb(0x10, 0x0C, 0x08));
        assert_eq!(tc.accent, Color32::from_rgb(0xE0, 0x90, 0x30));
    }

    #[test]
    fn neon_has_purple_canvas() {
        let tc = ThemeColors::neon();
        assert_eq!(tc.canvas, Color32::from_rgb(0x06, 0x04, 0x0C));
        assert_eq!(tc.accent, Color32::from_rgb(0xFF, 0x50, 0xC0));
    }
}
