use egui::{Color32, CornerRadius, Stroke, Visuals};

use super::tokens::*;

pub fn dark_visuals() -> Visuals {
    let mut v = Visuals::dark();

    v.panel_fill = DARK_PANEL;
    v.window_fill = DARK_PANEL;
    v.extreme_bg_color = DARK_CANVAS;
    v.faint_bg_color = Color32::from_rgb(0x18, 0x18, 0x18);

    v.override_text_color = Some(DARK_TEXT_PRIMARY);
    v.selection.bg_fill = DARK_ACCENT.gamma_multiply(0.4);
    v.selection.stroke = Stroke::new(1.0_f32, DARK_ACCENT);

    v.widgets.noninteractive.bg_fill = CARD_BG;
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, DARK_TEXT_SECONDARY);
    v.widgets.noninteractive.corner_radius = CornerRadius::same(WIDGET_ROUNDING);
    v.widgets.noninteractive.bg_stroke = Stroke::new(0.5_f32, CARD_BORDER);

    v.widgets.inactive.bg_fill = DARK_WIDGET_BG;
    v.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, DARK_TEXT_PRIMARY);
    v.widgets.inactive.corner_radius = CornerRadius::same(WIDGET_ROUNDING);
    v.widgets.inactive.bg_stroke = Stroke::new(0.5_f32, CARD_BORDER);

    v.widgets.hovered.bg_fill = DARK_WIDGET_BG_HOVER;
    v.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, DARK_TEXT_PRIMARY);
    v.widgets.hovered.corner_radius = CornerRadius::same(WIDGET_ROUNDING);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, DARK_ACCENT);

    v.widgets.active.bg_fill = DARK_WIDGET_BG_ACTIVE;
    v.widgets.active.fg_stroke = Stroke::new(1.0_f32, DARK_TEXT_PRIMARY);
    v.widgets.active.corner_radius = CornerRadius::same(WIDGET_ROUNDING);
    v.widgets.active.bg_stroke = Stroke::new(1.0_f32, DARK_ACCENT);

    v.widgets.open.bg_fill = DARK_WIDGET_BG_ACTIVE;
    v.widgets.open.fg_stroke = Stroke::new(1.0_f32, DARK_TEXT_PRIMARY);
    v.widgets.open.corner_radius = CornerRadius::same(WIDGET_ROUNDING);
    v.widgets.open.bg_stroke = Stroke::new(1.0_f32, DARK_ACCENT);

    v.window_corner_radius = CornerRadius::same(PANEL_ROUNDING);
    v.window_stroke = Stroke::new(1.0_f32, CARD_BORDER);

    v
}
