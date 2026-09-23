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

const THEME_COLORS_ID: &str = "fosfora_theme_colors";

/// Store theme colors in egui temp data.
pub fn set_theme_colors(ctx: &egui::Context, colors: ThemeColors) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::new(THEME_COLORS_ID), colors));
}

/// Read theme colors from egui temp data (fallback: Gray).
pub fn theme_colors(ctx: &egui::Context) -> ThemeColors {
    ctx.data(|d| d.get_temp(egui::Id::new(THEME_COLORS_ID)))
        .unwrap_or_else(|| super::palette::Palette::GRAY.colors())
}
