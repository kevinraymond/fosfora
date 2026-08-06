use egui::{Color32, CornerRadius, Stroke, Visuals};

use super::tokens::*;

pub fn light_visuals() -> Visuals {
    let mut v = Visuals::light();

    v.panel_fill = LIGHT_PANEL;
    v.window_fill = LIGHT_PANEL;
    v.extreme_bg_color = LIGHT_CANVAS;
    v.faint_bg_color = Color32::from_rgb(0xF0, 0xF0, 0xF0);

    v.override_text_color = Some(LIGHT_TEXT_PRIMARY);
    v.selection.bg_fill = LIGHT_ACCENT.gamma_multiply(0.2);
    v.selection.stroke = Stroke::new(1.0_f32, LIGHT_ACCENT);

    v.widgets.noninteractive.bg_fill = LIGHT_PANEL;
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, LIGHT_TEXT_SECONDARY);
    v.widgets.noninteractive.corner_radius = CornerRadius::same(WIDGET_ROUNDING);
    v.widgets.noninteractive.bg_stroke = Stroke::new(0.5_f32, LIGHT_SEPARATOR);

    v.widgets.inactive.bg_fill = LIGHT_WIDGET_BG;
    v.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, LIGHT_TEXT_PRIMARY);
    v.widgets.inactive.corner_radius = CornerRadius::same(WIDGET_ROUNDING);
    v.widgets.inactive.bg_stroke = Stroke::new(0.5_f32, LIGHT_SEPARATOR);

    v.widgets.hovered.bg_fill = LIGHT_WIDGET_BG_HOVER;
    v.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, LIGHT_TEXT_PRIMARY);
    v.widgets.hovered.corner_radius = CornerRadius::same(WIDGET_ROUNDING);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, LIGHT_ACCENT);

    v.widgets.active.bg_fill = LIGHT_WIDGET_BG_ACTIVE;
    v.widgets.active.fg_stroke = Stroke::new(1.0_f32, LIGHT_TEXT_PRIMARY);
    v.widgets.active.corner_radius = CornerRadius::same(WIDGET_ROUNDING);
    v.widgets.active.bg_stroke = Stroke::new(1.0_f32, LIGHT_ACCENT);

    v.widgets.open.bg_fill = LIGHT_WIDGET_BG_ACTIVE;
    v.widgets.open.fg_stroke = Stroke::new(1.0_f32, LIGHT_TEXT_PRIMARY);
    v.widgets.open.corner_radius = CornerRadius::same(WIDGET_ROUNDING);
    v.widgets.open.bg_stroke = Stroke::new(1.0_f32, LIGHT_ACCENT);

    v.window_corner_radius = CornerRadius::same(PANEL_ROUNDING);
    v.window_stroke = Stroke::new(1.0_f32, LIGHT_SEPARATOR);

    v
}
