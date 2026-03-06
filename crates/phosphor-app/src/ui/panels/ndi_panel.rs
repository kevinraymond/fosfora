use egui::{RichText, Ui};

use crate::ndi::ffi::ndi_search_diagnostics;
use crate::ndi::types::OutputResolution;
use crate::ui::theme::colors::theme_colors;
use crate::ui::theme::tokens::*;

/// Snapshot of NDI state for UI (avoids passing &mut NdiSystem through draw_panels).
#[derive(Clone, Default)]
pub struct NdiInfo {
    pub enabled: bool,
    pub running: bool,
    pub ndi_available: bool,
    pub source_name: String,
    pub resolution: OutputResolution,
    pub frames_sent: u64,
    pub output_width: u32,
    pub output_height: u32,
    pub alpha_from_luma: bool,
}

pub fn draw_ndi_panel(ui: &mut Ui, info: &NdiInfo) {
    let tc = theme_colors(ui.ctx());

    if !info.ndi_available {
        ui.label(
            RichText::new("NDI® runtime not found. Install NDI® Runtime:")
                .size(SMALL_SIZE)
                .color(tc.text_secondary),
        );
        ui.hyperlink_to(
            RichText::new("ndi.video →").size(SMALL_SIZE),
            "https://ndi.video",
        );
        ui.add_space(4.0);

        let diagnostics = ndi_search_diagnostics();
        if !diagnostics.is_empty() {
            ui.collapsing(
                RichText::new("Searched locations").size(SMALL_SIZE),
                |ui| {
                    for path in diagnostics {
                        ui.label(
                            RichText::new(path)
                                .size(SMALL_SIZE - 1.0)
                                .color(tc.text_secondary),
                        );
                    }
                    #[cfg(target_os = "macos")]
                    {
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new("Tip: if you see a \"different Team IDs\" error,\nupdate to the latest release (includes library\nvalidation entitlement fix).\n\nAlso check quarantine with:\n  xattr -d com.apple.quarantine /usr/local/lib/libndi.dylib")
                                .size(SMALL_SIZE - 1.0)
                                .color(tc.text_secondary),
                        );
                    }
                },
            );
        }
        return;
    }

    // Enable checkbox
    let mut enabled = info.enabled;
    if ui
        .checkbox(&mut enabled, RichText::new("Enable NDI®").size(SMALL_SIZE))
        .changed()
    {
        ui.ctx().data_mut(|d| {
            d.insert_temp(egui::Id::new("ndi_set_enabled"), enabled);
        });
    }

    // Source name
    ui.horizontal(|ui| {
        ui.label(RichText::new("Source").size(SMALL_SIZE));
        let mut name = info.source_name.clone();
        let resp = ui.add(
            egui::TextEdit::singleline(&mut name)
                .desired_width(140.0)
                .font(egui::FontId::proportional(SMALL_SIZE)),
        );
        if resp.lost_focus() && name != info.source_name {
            ui.ctx().data_mut(|d| {
                d.insert_temp(egui::Id::new("ndi_source_name"), name);
            });
        }
    });

    // Resolution dropdown
    ui.horizontal(|ui| {
        ui.label(RichText::new("Resolution").size(SMALL_SIZE));
        let current = info.resolution;
        egui::ComboBox::from_id_salt("ndi_resolution")
            .selected_text(current.display_name())
            .width(120.0)
            .show_ui(ui, |ui| {
                for (i, &res) in OutputResolution::ALL.iter().enumerate() {
                    if ui
                        .selectable_label(current == res, res.display_name())
                        .clicked()
                    {
                        ui.ctx().data_mut(|d| {
                            d.insert_temp(egui::Id::new("ndi_resolution_change"), i as u8);
                        });
                    }
                }
            });
    });

    // Alpha from luma toggle
    ui.horizontal(|ui| {
        let mut alpha_luma = info.alpha_from_luma;
        if ui
            .checkbox(
                &mut alpha_luma,
                RichText::new("Alpha from brightness").size(SMALL_SIZE),
            )
            .changed()
        {
            ui.ctx().data_mut(|d| {
                d.insert_temp(egui::Id::new("ndi_alpha_from_luma"), alpha_luma);
            });
        }
    });

    // Show output dimensions when running
    if info.running && info.output_width > 0 {
        ui.label(
            RichText::new(format!(
                "Output: {}x{}",
                info.output_width, info.output_height
            ))
            .size(SMALL_SIZE)
            .color(tc.text_secondary),
        );
    }

    ui.add_space(4.0);
    ui.label(
        RichText::new("NDI® is a registered trademark of Vizrt NDI AB.")
            .size(SMALL_SIZE - 1.0)
            .color(tc.text_secondary),
    );
}
