use egui::{RichText, Ui};

use crate::settings::{BandScale, ParticleQuality};
use crate::ui::theme::ThemeMode;
use crate::ui::theme::colors::theme_colors;
use crate::ui::theme::tokens::*;

pub fn draw_settings_panel(
    ui: &mut Ui,
    current_theme: ThemeMode,
    current_quality: ParticleQuality,
    current_band_scale: BandScale,
    use_ffmpeg_webcam: bool,
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

    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(label_width, ui.spacing().interact_size.y),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.label(
                    RichText::new("Band\nScale")
                        .size(SMALL_SIZE)
                        .color(tc.text_secondary),
                );
            },
        );
        egui::ComboBox::from_id_salt("band_scale_selector")
            .selected_text(RichText::new(current_band_scale.display_name()).size(SMALL_SIZE))
            .width(ui.available_width() - 4.0)
            .show_ui(ui, |ui| {
                for &bs in BandScale::ALL {
                    let r = ui.selectable_label(
                        bs == current_band_scale,
                        RichText::new(bs.display_name()).size(SMALL_SIZE),
                    );
                    if r.clicked() && bs != current_band_scale {
                        ui.ctx().data_mut(|d| {
                            d.insert_temp(egui::Id::new("set_band_scale"), bs);
                        });
                    }
                }
            });
    })
    .response
    .on_hover_text(
        "How the 7 frequency bands are scaled. Unified dB makes all bands comparable \
         (recommended); Legacy is the old linear/dB split, for presets tuned to it.",
    );

    // FFmpeg webcam backend (webcam feature only)
    #[cfg(feature = "webcam")]
    {
        let mut ffmpeg = use_ffmpeg_webcam;
        let resp = ui
            .checkbox(&mut ffmpeg, RichText::new("FFmpeg webcam").size(SMALL_SIZE))
            .on_hover_text(
                "Use FFmpeg for webcam capture. Enable if your camera isn't detected \
                 (e.g. virtual cameras like Irium or DroidCam). Requires FFmpeg installed.",
            );
        if resp.changed() {
            ui.ctx().data_mut(|d| {
                d.insert_temp(egui::Id::new("set_ffmpeg_webcam"), ffmpeg);
            });
        }
    }
    #[cfg(not(feature = "webcam"))]
    let _ = use_ffmpeg_webcam;
}
