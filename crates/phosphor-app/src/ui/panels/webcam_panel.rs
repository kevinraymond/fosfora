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
    pub available_devices: Vec<(u32, String)>,
    pub device_index: u32,
    pub capture_running: bool,
}

pub fn draw_webcam_panel(ui: &mut Ui, info: &WebcamInfo) {
    let tc = theme_colors(ui.ctx());

    // Device selector (only show if multiple devices)
    if info.available_devices.len() > 1 {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Camera")
                    .size(SMALL_SIZE)
                    .color(tc.text_secondary),
            );

            let selected_name = info
                .available_devices
                .iter()
                .find(|(idx, _)| *idx == info.device_index)
                .map(|(_, name)| name.as_str())
                .unwrap_or(&info.device_name);

            egui::ComboBox::from_id_salt("webcam_device_combo")
                .selected_text(RichText::new(truncate_name(selected_name, 24)).size(SMALL_SIZE))
                .width(ui.available_width() - 4.0)
                .show_ui(ui, |ui| {
                    for (idx, name) in &info.available_devices {
                        let selected = *idx == info.device_index;
                        let label = truncate_name(name, 40);
                        if ui
                            .selectable_label(selected, RichText::new(&label).size(SMALL_SIZE))
                            .clicked()
                            && !selected
                        {
                            ui.ctx().data_mut(|d| {
                                d.insert_temp(egui::Id::new("switch_webcam_device"), *idx);
                            });
                        }
                    }
                });
        });
        ui.add_space(4.0);
    }

    ui.label(RichText::new(&info.device_name).size(BODY_SIZE).strong());
    ui.label(
        RichText::new(format!("{}x{}", info.width, info.height))
            .size(SMALL_SIZE)
            .color(tc.text_secondary),
    );

    if !info.capture_running {
        ui.add_space(4.0);
        ui.label(
            RichText::new("Camera not available")
                .size(SMALL_SIZE)
                .color(tc.error),
        );
    }

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

fn truncate_name(name: &str, max: usize) -> String {
    if name.len() <= max {
        name.to_string()
    } else {
        format!("{}…", &name[..max - 1])
    }
}
