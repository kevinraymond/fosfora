use egui::{RichText, Ui};

use crate::ui::theme::colors::theme_colors;
use crate::ui::theme::tokens::*;

/// Snapshot of webcam layer state for UI (avoids borrow conflicts).
#[derive(Debug, Clone)]
pub struct WebcamInfo {
    pub device_name: String,
    pub width: u32,
    pub height: u32,
    pub mirror: bool,
}

pub fn draw_webcam_panel(ui: &mut Ui, info: &WebcamInfo) {
    let tc = theme_colors(ui.ctx());

    ui.label(RichText::new(&info.device_name).size(BODY_SIZE).strong());
    ui.label(
        RichText::new(format!("{}x{}", info.width, info.height))
            .size(SMALL_SIZE)
            .color(tc.text_secondary),
    );

    ui.add_space(4.0);

    // Mirror toggle
    let mut mirror = info.mirror;
    if ui
        .checkbox(&mut mirror, RichText::new("Mirror").size(SMALL_SIZE))
        .changed()
    {
        ui.ctx().data_mut(|d| {
            d.insert_temp(egui::Id::new("webcam_mirror"), mirror);
        });
    }

    ui.add_space(4.0);

    // Stop/disconnect button
    if ui
        .button(
            RichText::new("Disconnect")
                .size(SMALL_SIZE)
                .color(tc.text_secondary),
        )
        .clicked()
    {
        ui.ctx().data_mut(|d| {
            d.insert_temp(egui::Id::new("webcam_disconnect"), true);
        });
    }
}
