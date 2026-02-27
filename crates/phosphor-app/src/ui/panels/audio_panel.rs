use egui::{Color32, Rect, RichText, Ui, Vec2};

use crate::audio::AudioSystem;
use crate::gpu::ShaderUniforms;
use crate::ui::theme::colors::theme_colors;
use crate::ui::theme::tokens::*;

const BAND_LABELS: [&str; 7] = ["SB", "BS", "LM", "MD", "UM", "PR", "BR"];

const BAND_COLORS: [Color32; 7] = [
    Color32::from_rgb(0xFF, 0x44, 0x66), // sub bass - red-pink
    Color32::from_rgb(0xFF, 0x88, 0x44), // bass - orange
    Color32::from_rgb(0xFF, 0xCC, 0x44), // low mid - yellow
    Color32::from_rgb(0x44, 0xFF, 0x88), // mid - green
    Color32::from_rgb(0x44, 0xCC, 0xFF), // upper mid - cyan
    Color32::from_rgb(0x44, 0x88, 0xFF), // presence - blue
    Color32::from_rgb(0xAA, 0x66, 0xFF), // brilliance - purple
];

/// Default label shown in the device dropdown.
const DEFAULT_DEVICE_LABEL: &str = "Default";

fn draw_device_selector(ui: &mut Ui, audio: &AudioSystem) {
    let tc = theme_colors(ui.ctx());

    // Cache device list in egui temp data, refresh every 2s
    let list_id = egui::Id::new("audio_device_list");
    let list_time_id = egui::Id::new("audio_device_list_time");

    let now = ui.input(|i| i.time);
    let last_scan: f64 = ui.ctx().data(|d| d.get_temp(list_time_id)).unwrap_or(0.0);

    let devices: Vec<String> = if now - last_scan > 2.0 {
        let devs = audio.list_devices();
        ui.ctx().data_mut(|d| {
            d.insert_temp(list_id, devs.clone());
            d.insert_temp(list_time_id, now);
        });
        devs
    } else {
        ui.ctx().data(|d| d.get_temp(list_id)).unwrap_or_default()
    };

    // Current selection: device name or "Default"
    let current = &audio.device_name;

    let selected_text = truncate_device_name(current, 24);

    ui.horizontal(|ui| {
        ui.label(RichText::new("Input").size(SMALL_SIZE).color(tc.text_secondary));

        egui::ComboBox::from_id_salt("audio_device_combo")
            .selected_text(RichText::new(&selected_text).size(SMALL_SIZE))
            .width(ui.available_width() - 4.0)
            .show_ui(ui, |ui| {
                // "Default" option — empty string signals default device
                let is_default = !devices.iter().any(|d| d == current);
                if ui.selectable_label(is_default, RichText::new(DEFAULT_DEVICE_LABEL).size(SMALL_SIZE)).clicked() {
                    ui.ctx().data_mut(|d| {
                        d.insert_temp(egui::Id::new("switch_audio_device"), String::new());
                    });
                }
                // Each available device
                for dev in &devices {
                    let selected = dev == current;
                    let label = truncate_device_name(dev, 40);
                    if ui.selectable_label(selected, RichText::new(&label).size(SMALL_SIZE)).clicked() && !selected {
                        ui.ctx().data_mut(|d| {
                            d.insert_temp(egui::Id::new("switch_audio_device"), dev.clone());
                        });
                    }
                }
            });
    });

    ui.add_space(4.0);
}

fn truncate_device_name(name: &str, max: usize) -> String {
    if name.len() <= max {
        name.to_string()
    } else {
        format!("{}...", &name[..max - 3])
    }
}

fn draw_vertical_meters(ui: &mut Ui, bands: &[f32; 7]) {
    let tc = theme_colors(ui.ctx());
    let available_width = ui.available_width();
    let total_gaps = (bands.len() - 1) as f32 * METER_GAP;
    let bar_width = ((available_width - total_gaps) / bands.len() as f32).max(8.0);
    let height = METER_HEIGHT;

    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(available_width, height),
        egui::Sense::hover(),
    );

    // Background
    ui.painter().rect_filled(rect, 2.0, tc.meter_bg);

    // Grid lines at 25%, 50%, 75%
    let grid_color = Color32::from_rgb(0x2A, 0x2A, 0x2A);
    for frac in [0.25, 0.5, 0.75] {
        let y = rect.bottom() - height * frac;
        ui.painter().line_segment(
            [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
            egui::Stroke::new(0.5, grid_color),
        );
    }

    // Bars
    for (i, &value) in bands.iter().enumerate() {
        let x = rect.left() + i as f32 * (bar_width + METER_GAP);
        let fill_height = height * value.clamp(0.0, 1.0);
        let bar_rect = Rect::from_min_size(
            egui::pos2(x, rect.bottom() - fill_height),
            Vec2::new(bar_width, fill_height),
        );
        ui.painter().rect_filled(bar_rect, 1.0, BAND_COLORS[i]);
    }

    // Labels below
    let (label_rect, _) = ui.allocate_exact_size(
        Vec2::new(available_width, 12.0),
        egui::Sense::hover(),
    );
    for (i, label) in BAND_LABELS.iter().enumerate() {
        let x = label_rect.left() + i as f32 * (bar_width + METER_GAP) + bar_width * 0.5;
        let y = label_rect.center().y;
        ui.painter().text(
            egui::pos2(x, y),
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(SMALL_SIZE),
            tc.text_secondary,
        );
    }
}

fn draw_feature_grid(ui: &mut Ui, uniforms: &ShaderUniforms) {
    let tc = theme_colors(ui.ctx());
    let features: [(&str, f32); 6] = [
        ("RMS", uniforms.rms),
        ("Kick", uniforms.kick),
        ("Onset", uniforms.onset),
        ("Flux", uniforms.flux),
        ("Cent", uniforms.centroid),
        ("Flat", uniforms.flatness),
    ];

    egui::Grid::new("audio_features")
        .num_columns(4)
        .spacing([8.0, 2.0])
        .show(ui, |ui| {
            for (i, (name, value)) in features.iter().enumerate() {
                ui.label(RichText::new(*name).size(SMALL_SIZE).color(tc.text_secondary));
                ui.label(RichText::new(format!("{:.2}", value)).size(SMALL_SIZE).monospace());
                if i % 2 == 1 {
                    ui.end_row();
                }
            }
        });
}

fn draw_bpm_display(ui: &mut Ui, uniforms: &ShaderUniforms) {
    let tc = theme_colors(ui.ctx());
    let bpm = uniforms.bpm * 300.0;
    if bpm <= 1.0 {
        return;
    }
    ui.horizontal(|ui| {
        // Beat dot
        let (dot_rect, _) = ui.allocate_exact_size(Vec2::new(10.0, 10.0), egui::Sense::hover());
        let color = if uniforms.beat > 0.5 {
            tc.beat_color
        } else {
            Color32::from_rgb(0x44, 0x44, 0x44)
        };
        ui.painter().circle_filled(dot_rect.center(), 4.0, color);

        ui.label(
            RichText::new(format!("{:.0} BPM", bpm))
                .size(BODY_SIZE)
                .strong()
                .color(if uniforms.beat > 0.5 { tc.beat_color } else { tc.text_primary }),
        );
    });
}

pub fn draw_audio_panel(ui: &mut Ui, audio: &AudioSystem, uniforms: &ShaderUniforms) {
    // Device selector always shown (helps diagnose issues)
    draw_device_selector(ui, audio);

    let tc = theme_colors(ui.ctx());
    if !audio.active {
        if let Some(err) = &audio.last_error {
            ui.colored_label(tc.error, format!("Audio error: {err}"));
        } else {
            ui.colored_label(tc.error, "No audio input");
        }
        return;
    }

    // Vertical frequency bars
    let bands: [f32; 7] = [
        uniforms.sub_bass, uniforms.bass, uniforms.low_mid, uniforms.mid,
        uniforms.upper_mid, uniforms.presence, uniforms.brilliance,
    ];
    draw_vertical_meters(ui, &bands);

    ui.add_space(4.0);

    // Compact feature grid
    draw_feature_grid(ui, uniforms);

    ui.add_space(2.0);

    // BPM + beat dot
    draw_bpm_display(ui, uniforms);
}
