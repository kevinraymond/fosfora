use egui::{Color32, Rect, RichText, Ui, Vec2};

use crate::gpu::ShaderUniforms;
use crate::ui::theme::tokens::*;

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
        // Left: RMS mini meter bar
        let rms = uniforms.rms.clamp(0.0, 1.0);
        let meter_width = 30.0;
        let meter_height = 10.0;
        let (rect, _) = ui.allocate_exact_size(Vec2::new(meter_width, meter_height), egui::Sense::hover());
        ui.painter().rect_filled(rect, 2.0, METER_BG);
        let fill_rect = Rect::from_min_size(
            rect.min,
            Vec2::new(rect.width() * rms, rect.height()),
        );
        let rms_color = if rms > 0.8 {
            DARK_ERROR
        } else if rms > 0.5 {
            DARK_WARNING
        } else {
            DARK_SUCCESS
        };
        ui.painter().rect_filled(fill_rect, 2.0, rms_color);

        // Center: shader errors only
        if let Some(err) = shader_error {
            ui.separator();
            ui.colored_label(DARK_ERROR, RichText::new(format!("ERR: {err}")).size(SMALL_SIZE));
        }

        // Spacer to push right-side items
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // Right items (right-to-left order, so first item is rightmost)

            // FPS
            let fps = if uniforms.delta_time > 0.0 {
                (1.0 / uniforms.delta_time) as u32
            } else {
                0
            };
            ui.label(RichText::new(format!("{fps}")).size(SMALL_SIZE).color(DARK_TEXT_SECONDARY));

            // OSC dot
            if osc_enabled {
                let color = if osc_recently_active {
                    DARK_SUCCESS
                } else {
                    Color32::from_rgb(0x55, 0x55, 0x55)
                };
                let (dot_rect, _) = ui.allocate_exact_size(Vec2::new(8.0, 8.0), egui::Sense::hover());
                ui.painter().circle_filled(dot_rect.center(), 3.0, color);
            }

            // MIDI dot
            if midi_active {
                let color = if midi_recently_active {
                    DARK_SUCCESS
                } else {
                    Color32::from_rgb(0x55, 0x55, 0x55)
                };
                let (dot_rect, _) = ui.allocate_exact_size(Vec2::new(8.0, 8.0), egui::Sense::hover());
                ui.painter().circle_filled(dot_rect.center(), 3.0, color);
            }

            // Particle count
            if let Some(count) = particle_count {
                ui.label(
                    RichText::new(format!("{count}p"))
                        .size(SMALL_SIZE)
                        .color(Color32::from_rgb(0x80, 0xB0, 0xE0)),
                );
            }

            // BPM + beat dot
            let bpm = uniforms.bpm * 300.0;
            if bpm > 1.0 {
                let beat_on = uniforms.beat > 0.5;
                let bpm_color = if beat_on { BEAT_COLOR } else { DARK_TEXT_PRIMARY };
                ui.label(RichText::new(format!("{:.0}", bpm)).size(SMALL_SIZE).color(bpm_color).strong());

                // Beat dot
                let dot_color = if beat_on { BEAT_COLOR } else { Color32::from_rgb(0x44, 0x44, 0x44) };
                let (dot_rect, _) = ui.allocate_exact_size(Vec2::new(8.0, 8.0), egui::Sense::hover());
                ui.painter().circle_filled(dot_rect.center(), 3.0, dot_color);
            }
        });
    });
}
