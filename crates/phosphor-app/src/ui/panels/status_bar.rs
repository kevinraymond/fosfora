use egui::{Color32, RichText, Ui, Vec2};

use crate::gpu::ShaderUniforms;
use crate::ui::theme::colors::theme_colors;
use crate::ui::theme::tokens::*;

const OFF: Color32 = Color32::from_rgb(0x33, 0x33, 0x33);
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
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(width, ui.spacing().interact_size.y),
        egui::Sense::hover(),
    );
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

/// Duration to show status errors before auto-clearing.
const ERROR_DISPLAY_SECS: f64 = 6.0;

pub fn draw_status_bar(
    ui: &mut Ui,
    shader_error: &Option<String>,
    uniforms: &ShaderUniforms,
    particle_count: Option<u32>,
    midi_enabled: bool,
    midi_recently_active: bool,
    osc_enabled: bool,
    osc_recently_active: bool,
    web_enabled: bool,
    web_client_count: usize,
    ndi_running: bool,
    scene_active: bool,
    scene_cue: Option<(usize, usize)>,
    status_error: &Option<(String, std::time::Instant)>,
    preset_loading: Option<&str>,
) {
    let tc = theme_colors(ui.ctx());

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;

        // Transient status error (higher priority, auto-clears)
        if let Some((msg, when)) = status_error {
            let elapsed = when.elapsed().as_secs_f64();
            if elapsed < ERROR_DISPLAY_SECS {
                ui.add_space(4.0);
                ui.colored_label(tc.error, RichText::new(msg).size(SMALL_SIZE));
            }
        }
        // Shader errors (dismissable)
        else if let Some(err) = shader_error {
            ui.add_space(4.0);
            ui.colored_label(
                tc.error,
                RichText::new(format!("ERR: {err}")).size(SMALL_SIZE),
            );
            if ui.small_button("×").clicked() {
                ui.ctx()
                    .data_mut(|d| d.insert_temp(egui::Id::new("dismiss_shader_error"), true));
            }
        }
        // Preset loading indicator
        else if let Some(name) = preset_loading {
            ui.add_space(4.0);
            // Pulsing dots animation
            let t = ui.input(|i| i.time);
            let dots = match ((t * 3.0) as usize) % 4 {
                0 => "",
                1 => ".",
                2 => "..",
                _ => "...",
            };
            ui.colored_label(
                tc.accent,
                RichText::new(format!("Loading {name}{dots}")).size(SMALL_SIZE),
            );
            ui.ctx().request_repaint();
        }
        // Keyboard hints when idle
        else {
            ui.add_space(4.0);
            ui.label(
                RichText::new("D toggle overlay · F fullscreen")
                    .size(SMALL_SIZE)
                    .color(tc.text_secondary),
            );
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
            fixed_value(ui, &format!("{fps}"), 24.0, tc.text_secondary);
            label(ui, "FPS");

            ui.add_space(6.0);

            // NDI — always show (no separate enabled flag; running=on)
            dot(ui, ndi_running, Color32::from_rgb(0x40, 0xC0, 0x40));
            label(ui, "NDI");
            ui.add_space(6.0);

            // Web — always show
            {
                let color = if web_client_count > 0 {
                    Color32::from_rgb(0x50, 0x90, 0xE0)
                } else if web_enabled {
                    DIM
                } else {
                    OFF
                };
                let (rect, _) =
                    ui.allocate_exact_size(Vec2::new(8.0, 8.0), egui::Sense::hover());
                ui.painter().circle_filled(rect.center(), 3.0, color);
            }
            label(ui, "WEB");
            ui.add_space(6.0);

            // OSC — always show
            {
                let color = if osc_recently_active {
                    tc.success
                } else if osc_enabled {
                    DIM
                } else {
                    OFF
                };
                let (rect, _) =
                    ui.allocate_exact_size(Vec2::new(8.0, 8.0), egui::Sense::hover());
                ui.painter().circle_filled(rect.center(), 3.0, color);
            }
            label(ui, "OSC");
            ui.add_space(6.0);

            // MIDI — always show
            {
                let color = if midi_recently_active {
                    tc.success
                } else if midi_enabled {
                    DIM
                } else {
                    OFF
                };
                let (rect, _) =
                    ui.allocate_exact_size(Vec2::new(8.0, 8.0), egui::Sense::hover());
                ui.painter().circle_filled(rect.center(), 3.0, color);
            }
            label(ui, "MIDI");
            ui.add_space(6.0);

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

            // Scene
            if scene_active {
                if let Some((current, total)) = scene_cue {
                    fixed_value(
                        ui,
                        &format!("{}/{}", current + 1, total),
                        28.0,
                        Color32::from_rgb(0xFF, 0xA0, 0x40),
                    );
                }
                dot(ui, true, Color32::from_rgb(0xFF, 0xA0, 0x40));
                label(ui, "SCN");
                ui.add_space(6.0);
            }

            // BPM + beat dot
            let bpm = uniforms.bpm * 300.0;
            if bpm > 1.0 {
                let beat_on = uniforms.beat > 0.5;
                let bpm_color = if beat_on {
                    tc.beat_color
                } else {
                    tc.text_primary
                };
                // Fixed 3-char width for BPM value (prevents jitter on 2→3 digit changes)
                fixed_value(ui, &format!("{:.0}", bpm), 24.0, bpm_color);
                dot(ui, beat_on, tc.beat_color);
                label(ui, "BPM");
            }
        });
    });
}
