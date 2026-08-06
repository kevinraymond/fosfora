use egui::{RichText, Ui};

use crate::syphon::ffi::syphon_search_diagnostics;
use crate::syphon::types::OutputResolution;
use crate::ui::theme::colors::theme_colors;
use crate::ui::theme::tokens::*;

/// Snapshot of Syphon state for UI (avoids passing &mut SyphonSystem through draw_panels).
#[derive(Clone, Default)]
pub struct SyphonInfo {
    pub available: bool,
    pub enabled: bool,
    pub running: bool,
    pub server_name: String,
    pub resolution: OutputResolution,
    pub frames_sent: u64,
    pub frames_dropped: u64,
    pub output_width: u32,
    pub output_height: u32,
    pub error: Option<String>,
}

pub fn draw_syphon_panel(ui: &mut Ui, info: &SyphonInfo) {
    let tc = theme_colors(ui.ctx());

    // Unlike Spout (statically linked SDK) the Syphon framework is dlopen'd
    // at runtime — it ships inside the release app bundle, but a dev build or
    // stripped install can be missing it.
    if !info.available {
        ui.label(
            RichText::new("Syphon framework not found.")
                .size(SMALL_SIZE)
                .color(tc.text_secondary),
        );
        ui.label(
            RichText::new(
                "The release app bundles it. For dev builds, install\n\
                 Syphon.framework to ~/Library/Frameworks or set\n\
                 SYPHON_FRAMEWORK_PATH to its parent directory.",
            )
            .size(SMALL_SIZE - 1.0)
            .color(tc.text_secondary),
        );
        ui.add_space(4.0);

        let diagnostics = syphon_search_diagnostics();
        if !diagnostics.is_empty() {
            ui.collapsing(RichText::new("Searched locations").size(SMALL_SIZE), |ui| {
                for path in diagnostics {
                    ui.label(
                        RichText::new(path)
                            .size(SMALL_SIZE - 1.0)
                            .color(tc.text_secondary),
                    );
                }
            });
        }
        return;
    }

    // Enable checkbox
    let mut enabled = info.enabled;
    if ui
        .checkbox(
            &mut enabled,
            RichText::new("Enable Syphon output").size(SMALL_SIZE),
        )
        .changed()
    {
        ui.ctx().data_mut(|d| {
            d.insert_temp(egui::Id::new("syphon_set_enabled"), enabled);
        });
    }

    // Server name
    ui.horizontal(|ui| {
        ui.label(RichText::new("Server").size(SMALL_SIZE));
        let mut name = info.server_name.clone();
        let resp = ui.add(
            egui::TextEdit::singleline(&mut name)
                .desired_width(140.0)
                .font(egui::FontId::proportional(SMALL_SIZE)),
        );
        if resp.lost_focus() && name != info.server_name {
            ui.ctx().data_mut(|d| {
                d.insert_temp(egui::Id::new("syphon_server_name"), name);
            });
        }
    });

    // Resolution dropdown
    ui.horizontal(|ui| {
        ui.label(RichText::new("Resolution").size(SMALL_SIZE));
        let current = info.resolution;
        egui::ComboBox::from_id_salt("syphon_resolution")
            .selected_text(current.display_name())
            .width(120.0)
            .show_ui(ui, |ui| {
                for (i, &res) in OutputResolution::ALL.iter().enumerate() {
                    if ui
                        .selectable_label(current == res, res.display_name())
                        .clicked()
                    {
                        ui.ctx().data_mut(|d| {
                            d.insert_temp(egui::Id::new("syphon_resolution_change"), i as u8);
                        });
                    }
                }
            });
    });

    // Show failure if any (Metal device, server creation, publish)
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
