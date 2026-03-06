use egui::{Color32, Margin, RichText, Shape, Stroke, Ui, Vec2, pos2, vec2};

use crate::effect::format::AudioMapping;
use crate::ui::theme::colors::theme_colors;
use crate::ui::theme::tokens::*;

/// Color for a given audio feature name.
fn feature_color(feature: &str) -> Color32 {
    match feature {
        "sub_bass" => Color32::from_rgb(0xFF, 0x44, 0x66),
        "bass" => Color32::from_rgb(0xFF, 0x88, 0x44),
        "low_mid" => Color32::from_rgb(0xFF, 0xCC, 0x44),
        "mid" => Color32::from_rgb(0x44, 0xFF, 0x88),
        "upper_mid" => Color32::from_rgb(0x44, 0xCC, 0xFF),
        "presence" => Color32::from_rgb(0x44, 0x88, 0xFF),
        "brilliance" => Color32::from_rgb(0xAA, 0x66, 0xFF),
        _ => Color32::from_rgb(0x88, 0xCC, 0xFF), // accent for rms, kick, onset, beat, etc.
    }
}

/// Human-readable display name for an audio feature.
fn feature_display_name(feature: &str) -> &str {
    match feature {
        "sub_bass" => "Sub Bass",
        "bass" => "Bass",
        "low_mid" => "Low Mid",
        "mid" => "Mid",
        "upper_mid" => "Upper Mid",
        "presence" => "Presence",
        "brilliance" => "Brilliance",
        "rms" => "RMS",
        "kick" => "Kick",
        "onset" => "Onset",
        "flux" => "Flux",
        "centroid" => "Centroid",
        "flatness" => "Flatness",
        "beat" => "Beat",
        "beat_phase" => "Beat Phase",
        "bpm" => "BPM",
        "beat_strength" => "Beat Str",
        other => other,
    }
}

/// Draw a small right-pointing triangle arrow (custom-drawn, no font dependency).
fn draw_arrow_right(ui: &mut Ui, color: Color32) {
    let size = SMALL_SIZE;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(size, size), egui::Sense::hover());
    let c = rect.center();
    let half = size * 0.3;
    let points = vec![
        pos2(c.x - half * 0.5, c.y - half),
        pos2(c.x + half * 0.5, c.y),
        pos2(c.x - half * 0.5, c.y + half),
    ];
    ui.painter()
        .add(Shape::convex_polygon(points, color, Stroke::NONE));
}

pub fn draw_audio_mappings(ui: &mut Ui, mappings: &[AudioMapping]) {
    let tc = theme_colors(ui.ctx());

    if mappings.is_empty() {
        ui.label(
            RichText::new("No audio mappings")
                .size(SMALL_SIZE)
                .color(tc.text_secondary),
        );
        return;
    }

    for mapping in mappings {
        let id = ui.id().with(&mapping.feature).with(&mapping.target);
        let hovered: bool = ui.ctx().data(|d| d.get_temp(id).unwrap_or(false));

        let fill = if hovered {
            Color32::from_rgba_unmultiplied(255, 255, 255, 8)
        } else {
            Color32::TRANSPARENT
        };

        let resp = egui::Frame::NONE
            .fill(fill)
            .corner_radius(4.0)
            .inner_margin(Margin::symmetric(4, 4))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.allocate_ui_with_layout(
                        vec2(52.0, 16.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.label(
                                RichText::new(feature_display_name(&mapping.feature))
                                    .size(SMALL_SIZE)
                                    .color(feature_color(&mapping.feature))
                                    .strong(),
                            );
                        },
                    );
                    draw_arrow_right(ui, tc.text_secondary);
                    ui.add(egui::Label::new(
                        RichText::new(&mapping.target)
                            .size(SMALL_SIZE)
                            .color(tc.text_primary),
                    ).truncate());
                });
            });

        let is_hovered = resp.response.hovered();
        ui.ctx().data_mut(|d| d.insert_temp(id, is_hovered));
    }
}
