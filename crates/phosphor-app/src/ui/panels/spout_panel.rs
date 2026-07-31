use egui::{RichText, Ui};

use crate::spout::types::OutputResolution;
use crate::ui::theme::colors::theme_colors;
use crate::ui::theme::tokens::*;

/// Snapshot of Spout state for UI (avoids passing &mut SpoutSystem through draw_panels).
#[derive(Clone, Default)]
pub struct SpoutInfo {
    pub enabled: bool,
    pub running: bool,
    pub sender_name: String,
    pub resolution: OutputResolution,
    pub frames_sent: u64,
    pub frames_dropped: u64,
    pub output_width: u32,
    pub output_height: u32,
    pub error: Option<String>,
}

pub fn draw_spout_panel(ui: &mut Ui, info: &SpoutInfo) {
    let tc = theme_colors(ui.ctx());

    // Enable checkbox. No availability probe: the Spout SDK is statically
    // linked, so unlike NDI there is no runtime to be missing.
    let mut enabled = info.enabled;
    if ui
        .checkbox(
            &mut enabled,
            RichText::new("Enable Spout output").size(SMALL_SIZE),
        )
        .changed()
    {
        ui.ctx().data_mut(|d| {
            d.insert_temp(egui::Id::new("spout_set_enabled"), enabled);
        });
    }

    // Sender name
    ui.horizontal(|ui| {
        ui.label(RichText::new("Sender").size(SMALL_SIZE));
        let mut name = info.sender_name.clone();
        let resp = ui.add(
            egui::TextEdit::singleline(&mut name)
                .desired_width(140.0)
                .font(egui::FontId::proportional(SMALL_SIZE)),
        );
        if resp.lost_focus() && name != info.sender_name {
            ui.ctx().data_mut(|d| {
                d.insert_temp(egui::Id::new("spout_sender_name"), name);
            });
        }
    });

    // Resolution dropdown
    ui.horizontal(|ui| {
        ui.label(RichText::new("Resolution").size(SMALL_SIZE));
        let current = info.resolution;
        egui::ComboBox::from_id_salt("spout_resolution")
            .selected_text(current.display_name())
            .width(120.0)
            .show_ui(ui, |ui| {
                for (i, &res) in OutputResolution::ALL.iter().enumerate() {
                    if ui
                        .selectable_label(current == res, res.display_name())
                        .clicked()
                    {
                        ui.ctx().data_mut(|d| {
                            d.insert_temp(egui::Id::new("spout_resolution_change"), i as u8);
                        });
                    }
                }
            });
    });

    // Show failure if any (D3D11 device creation, sender creation, send)
    if let Some(ref err) = info.error {
        ui.label(RichText::new(err).size(SMALL_SIZE).color(tc.error));
    }

    // Status when running
    if info.running && info.output_width > 0 {
        ui.label(
            RichText::new(format!(
                "{}x{} · sent {} · dropped {}",
                info.output_width, info.output_height, info.frames_sent, info.frames_dropped
            ))
            .size(SMALL_SIZE)
            .color(tc.text_secondary),
        );
    }
}
