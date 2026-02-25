use egui::{Color32, RichText, Ui, Vec2};

use crate::gpu::ShaderUniforms;
use crate::ui::theme::tokens::*;

const DIM: Color32 = Color32::from_rgb(0x55, 0x55, 0x55);
const LABEL_COLOR: Color32 = Color32::from_rgb(0x70, 0x70, 0x70);

fn dot(ui: &mut Ui, active: bool, active_color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(8.0, 8.0), egui::Sense::hover());
    let color = if active { active_color } else { DIM };
    ui.painter().circle_filled(rect.center(), 3.0, color);
}

fn label(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(MONO_SIZE).color(LABEL_COLOR));
}

/// Fixed-width value using monospace-style right-aligned layout.
fn fixed_value(ui: &mut Ui, text: &str, width: f32, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, ui.spacing().interact_size.y), egui::Sense::hover());
    let galley = ui.painter().layout_no_wrap(
        text.to_string(),
        egui::FontId::proportional(SMALL_SIZE),
        color,
    );
    // Right-align the text within the fixed rect
    let text_pos = egui::pos2(
        rect.right() - galley.size().x,
        rect.center().y - galley.size().y * 0.5,
    );
    ui.painter().galley(text_pos, galley, color);
}

pub fn draw_status_bar(
    ui: &mut Ui,
    shader_error: &Option<String>,
    uniforms: &ShaderUniforms,
    particle_count: Option<u32>,
    _midi_port: &str,
    midi_active: bool,
    midi_recently_active: bool,
    osc_enabled: bool,
    osc_recently_active: bool,
) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;

        // Shader errors
        if let Some(err) = shader_error {
            ui.add_space(4.0);
            ui.colored_label(DARK_ERROR, RichText::new(format!("ERR: {err}")).size(SMALL_SIZE));
        }

        // Push right-side items
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;

            // FPS (rightmost) — EMA-smoothed to avoid jitter
            let fps_raw = if uniforms.delta_time > 0.0 {
                1.0 / uniforms.delta_time
            } else {
                0.0
            };
            let fps_id = ui.id().with("smoothed_fps");
            let prev: f32 = ui.ctx().data_mut(|d| d.get_temp(fps_id).unwrap_or(fps_raw));
            let alpha = 0.02_f32;
            let smoothed = prev + alpha * (fps_raw - prev);
            ui.ctx().data_mut(|d| d.insert_temp(fps_id, smoothed));
            let fps = smoothed as u32;
            fixed_value(ui, &format!("{fps}"), 24.0, DARK_TEXT_SECONDARY);
            label(ui, "FPS");

            ui.add_space(6.0);

            // OSC
            if osc_enabled {
                dot(ui, osc_recently_active, DARK_SUCCESS);
                label(ui, "OSC");
                ui.add_space(6.0);
            }

            // MIDI
            if midi_active {
                dot(ui, midi_recently_active, DARK_SUCCESS);
                label(ui, "MIDI");
                ui.add_space(6.0);
            }

            // Particles
            if let Some(count) = particle_count {
                ui.label(
                    RichText::new(format!("{count}"))
                        .size(SMALL_SIZE)
                        .color(Color32::from_rgb(0x80, 0xB0, 0xE0)),
                );
                label(ui, "PTL");
                ui.add_space(6.0);
            }

            // BPM + beat dot
            let bpm = uniforms.bpm * 300.0;
            if bpm > 1.0 {
                let beat_on = uniforms.beat > 0.5;
                let bpm_color = if beat_on { BEAT_COLOR } else { DARK_TEXT_PRIMARY };
                // Fixed 3-char width for BPM value (prevents jitter on 2→3 digit changes)
                fixed_value(ui, &format!("{:.0}", bpm), 24.0, bpm_color);
                dot(ui, beat_on, BEAT_COLOR);
                label(ui, "BPM");
            }
        });
    });
}
