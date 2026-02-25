use egui::{Color32, Ui, Vec2};

use crate::audio::AudioSystem;
use crate::gpu::ShaderUniforms;
use crate::ui::accessibility::focus::draw_focus_ring;

const BAND_NAMES: [&str; 7] = [
    "Sub Bass", "Bass", "Low Mid", "Mid", "Upper Mid", "Presence", "Brilliance",
];

const FEATURE_NAMES: [&str; 13] = [
    "RMS", "Kick", "Centroid", "Flux", "Flatness", "Rolloff", "Bandwidth", "ZCR",
    "Onset", "Beat", "Phase", "BPM", "Strength",
];

/// HSV-cycled colors for spectrum bars.
fn bar_color(index: usize, total: usize) -> Color32 {
    let hue = (index as f32 / total as f32) * 360.0;
    let (r, g, b) = hsv_to_rgb(hue, 0.7, 0.85);
    Color32::from_rgb(
        (r * 255.0) as u8,
        (g * 255.0) as u8,
        (b * 255.0) as u8,
    )
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (f32, f32, f32) {
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match (h as u32 / 60) % 6 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    (r + m, g + m, b + m)
}

fn draw_bar(ui: &mut Ui, name: &str, value: f32, color: Color32, available_width: f32) {
    ui.horizontal(|ui| {
        ui.label(format!("{name:>10}"));
        let bar_height = 14.0;
        let (rect, response) = ui.allocate_exact_size(
            Vec2::new((available_width - 130.0).max(1.0), bar_height),
            egui::Sense::hover(),
        );

        ui.painter().rect_filled(
            rect,
            2.0,
            Color32::from_rgb(0x2A, 0x2A, 0x2A),
        );

        let fill_width = rect.width() * value.clamp(0.0, 1.0);
        let fill_rect = egui::Rect::from_min_size(
            rect.min,
            Vec2::new(fill_width, rect.height()),
        );
        ui.painter().rect_filled(fill_rect, 2.0, color);

        draw_focus_ring(ui, &response);

        ui.label(format!("{value:.2}"));
    });
}

pub fn draw_audio_panel(ui: &mut Ui, audio: &AudioSystem, uniforms: &ShaderUniforms) {
    ui.label(format!("Device: {}", audio.device_name));

    if !audio.active {
        ui.colored_label(Color32::from_rgb(0xE0, 0x60, 0x60), "No audio input");
        return;
    }

    ui.add_space(4.0);
    let available_width = ui.available_width();

    // 7-band frequency display
    ui.label("Frequency Bands");
    let bands: [f32; 7] = [
        uniforms.sub_bass, uniforms.bass, uniforms.low_mid, uniforms.mid,
        uniforms.upper_mid, uniforms.presence, uniforms.brilliance,
    ];
    for (i, (name, &value)) in BAND_NAMES.iter().zip(bands.iter()).enumerate() {
        draw_bar(ui, name, value, bar_color(i, 7), available_width);
    }

    ui.add_space(4.0);
    ui.label("Features");

    let features: [f32; 13] = [
        uniforms.rms, uniforms.kick, uniforms.centroid, uniforms.flux,
        uniforms.flatness, uniforms.rolloff, uniforms.bandwidth, uniforms.zcr,
        uniforms.onset, uniforms.beat, uniforms.beat_phase, uniforms.bpm,
        uniforms.beat_strength,
    ];
    for (i, (name, &value)) in FEATURE_NAMES.iter().zip(features.iter()).enumerate() {
        let color = bar_color(i + 7, 20);
        draw_bar(ui, name, value, color, available_width);
    }

    // BPM display
    let bpm_display = uniforms.bpm * 300.0;
    if bpm_display > 1.0 {
        ui.add_space(4.0);
        ui.colored_label(
            Color32::from_rgb(0xE0, 0xA0, 0x40),
            format!("BPM: {:.0}", bpm_display),
        );
    }
}
