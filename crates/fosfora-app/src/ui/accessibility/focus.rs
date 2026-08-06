use egui::{Response, Stroke, StrokeKind, Ui};

use crate::ui::theme::tokens::{DARK_ACCENT, FOCUS_RING_WIDTH};

/// Draw a 2px focus ring around a widget when it has keyboard focus (WCAG 2.4.11).
pub fn draw_focus_ring(ui: &Ui, response: &Response) {
    if response.has_focus() {
        let rect = response.rect.expand(FOCUS_RING_WIDTH);
        ui.painter().rect_stroke(
            rect,
            response.rect.height() * 0.15, // slight rounding
            Stroke::new(FOCUS_RING_WIDTH, DARK_ACCENT),
            StrokeKind::Outside,
        );
    }
}
