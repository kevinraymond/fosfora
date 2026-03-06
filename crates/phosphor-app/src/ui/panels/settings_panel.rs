use egui::{RichText, Ui};

use crate::settings::ParticleQuality;
use crate::ui::theme::ThemeMode;
use crate::ui::theme::colors::theme_colors;
use crate::ui::theme::tokens::*;

pub fn draw_settings_panel(
    ui: &mut Ui,
    current_theme: ThemeMode,
    current_quality: ParticleQuality,
) {
    let tc = theme_colors(ui.ctx());

    let label_width = 52.0;

    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(label_width, ui.spacing().interact_size.y),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.label(
                    RichText::new("Theme")
                        .size(SMALL_SIZE)
                        .color(tc.text_secondary),
                );
            },
        );
        egui::ComboBox::from_id_salt("theme_selector")
            .selected_text(RichText::new(current_theme.display_name()).size(SMALL_SIZE))
            .width(ui.available_width() - 4.0)
            .show_ui(ui, |ui| {
                for &mode in ThemeMode::ALL {
                    let r = ui.selectable_label(
                        mode == current_theme,
                        RichText::new(mode.display_name()).size(SMALL_SIZE),
                    );
                    if r.clicked() && mode != current_theme {
                        ui.ctx().data_mut(|d| {
                            d.insert_temp(egui::Id::new("set_theme"), mode);
                        });
                    }
                }
            });
    });

    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(label_width, ui.spacing().interact_size.y),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.label(
                    RichText::new("Particle\nQuality")
                        .size(SMALL_SIZE)
                        .color(tc.text_secondary),
                );
            },
        );
        egui::ComboBox::from_id_salt("particle_quality_selector")
            .selected_text(RichText::new(current_quality.display_name()).size(SMALL_SIZE))
            .width(ui.available_width() - 4.0)
            .show_ui(ui, |ui| {
                for &q in ParticleQuality::ALL {
                    let r = ui.selectable_label(
                        q == current_quality,
                        RichText::new(q.display_name()).size(SMALL_SIZE),
                    );
                    if r.clicked() && q != current_quality {
                        ui.ctx().data_mut(|d| {
                            d.insert_temp(egui::Id::new("set_particle_quality"), q);
                        });
                    }
                }
            });
    });
}
