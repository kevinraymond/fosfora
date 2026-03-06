use std::f32::consts::{PI, TAU};

use egui::{Color32, Mesh, Pos2, Rect, RichText, Shape, Stroke, Ui, Vec2, pos2};

use crate::audio::AudioSystem;
use crate::gpu::ShaderUniforms;
use crate::ui::theme::colors::theme_colors;
use crate::ui::theme::tokens::*;

// ── Band spectrum ──────────────────────────────────────────────────────

const BAND_LABELS: [&str; 7] = ["SB", "BS", "LM", "MD", "UM", "PR", "BR"];

const BAND_TOOLTIPS: [&str; 7] = [
    "Sub Bass \u{00b7} 20\u{2013}60 Hz\nKick drums, deep rumble",
    "Bass \u{00b7} 60\u{2013}250 Hz\nBasslines, low-end warmth",
    "Low Mid \u{00b7} 250\u{2013}500 Hz\nBody, fullness",
    "Mid \u{00b7} 500 Hz\u{2013}2 kHz\nVocals, instruments",
    "Upper Mid \u{00b7} 2\u{2013}4 kHz\nPresence, clarity",
    "Presence \u{00b7} 4\u{2013}6 kHz\nDefinition, edge",
    "Brilliance \u{00b7} 6\u{2013}20 kHz\nAir, sparkle, cymbals",
];

const BAND_COLORS: [Color32; 7] = [
    Color32::from_rgb(0xFF, 0x44, 0x66), // sub bass - red-pink
    Color32::from_rgb(0xFF, 0x88, 0x44), // bass - orange
    Color32::from_rgb(0xFF, 0xCC, 0x44), // low mid - yellow
    Color32::from_rgb(0x44, 0xFF, 0x88), // mid - green
    Color32::from_rgb(0x44, 0xCC, 0xFF), // upper mid - cyan
    Color32::from_rgb(0x44, 0x88, 0xFF), // presence - blue
    Color32::from_rgb(0xAA, 0x66, 0xFF), // brilliance - purple
];

// ── Dynamics ───────────────────────────────────────────────────────────

const DYNAMICS_LABELS: [&str; 7] = ["RMS", "Kick", "Onset", "Flux", "Cent", "Flat", "Roll"];

const DYNAMICS_TOOLTIPS: [&str; 7] = [
    "Root Mean Square\nOverall signal loudness",
    "Kick Detect\nLow-frequency transient pulse",
    "Onset Strength\nSudden energy increase across bands",
    "Spectral Flux\nRate of timbral change per frame",
    "Spectral Centroid\nPerceived brightness center",
    "Spectral Flatness\n0% = pure tone, 100% = white noise",
    "Spectral Rolloff\nFrequency containing 85% of energy",
];

const DYNAMICS_COLORS: [Color32; 7] = [
    Color32::from_rgb(0x66, 0xBB, 0xFF), // RMS - light blue
    Color32::WHITE,                      // Kick - white
    Color32::from_rgb(0xFF, 0xAA, 0x44), // Onset - orange
    Color32::from_rgb(0x44, 0x88, 0xFF), // Flux - blue
    Color32::from_rgb(0xBB, 0x66, 0xFF), // Centroid - purple
    Color32::from_rgb(0x44, 0xCC, 0xBB), // Flatness - teal
    Color32::from_rgb(0xFF, 0xCC, 0x88), // Rolloff - warm
];

// ── MFCC ───────────────────────────────────────────────────────────────

const MFCC_LABELS: [&str; 13] = [
    "DC", "Slope", "Shape", "Fmnt", "", "", "", "", "", "", "", "", "Det6",
];

const MFCC_TOOLTIPS: [&str; 13] = [
    "MFCC 0 \u{2014} Energy\nOverall spectral energy level",
    "MFCC 1 \u{2014} Slope\nSpectral tilt: bright vs dark",
    "MFCC 2 \u{2014} Shape\nBroad timbral contour",
    "MFCC 3 \u{2014} Formant\nVocal / resonance character",
    "MFCC 4\nFine timbral detail",
    "MFCC 5\nFine timbral detail",
    "MFCC 6\nFine timbral detail",
    "MFCC 7\nFine timbral detail",
    "MFCC 8\nFine timbral detail",
    "MFCC 9\nFine timbral detail",
    "MFCC 10\nFine timbral detail",
    "MFCC 11\nFine timbral detail",
    "MFCC 12 \u{2014} Detail\nHigh-order timbral texture",
];

// ── Chroma ─────────────────────────────────────────────────────────────

const CHROMA_LABELS: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

// ── Layout constants ───────────────────────────────────────────────────

const BPM_RING_RADIUS: f32 = 20.0;
const CHROMA_OUTER_RADIUS: f32 = 74.0;
const CHROMA_INNER_RATIO: f32 = 0.48;
const DYNAMICS_ROW_HEIGHT: f32 = 18.0;
const DYNAMICS_BAR_HEIGHT: f32 = 4.0;
const MFCC_CELL_HEIGHT: f32 = 16.0;

/// Default label shown in the device dropdown.
const DEFAULT_DEVICE_LABEL: &str = "Default";

// ── Helpers ────────────────────────────────────────────────────────────

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (f32, f32, f32) {
    let i = (h * 6.0).floor();
    let f = h * 6.0 - i;
    let p = v * (1.0 - s);
    let q = v * (1.0 - f * s);
    let t = v * (1.0 - (1.0 - f) * s);
    match (i as i32) % 6 {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    }
}

fn chroma_color(pitch_class: usize, energy: f32) -> Color32 {
    let hue = pitch_class as f32 / 12.0;
    let (r, g, b) = hsv_to_rgb(hue, 0.8, 0.4 + 0.6 * energy.clamp(0.0, 1.0));
    Color32::from_rgb((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
}

fn truncate_device_name(name: &str, max: usize) -> String {
    if name.len() <= max {
        name.to_string()
    } else {
        format!("{}...", &name[..max - 3])
    }
}

/// Interpolate MFCC value (0..1) to dark blue → cyan → white.
fn mfcc_heat_color(t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 {
        // dark blue (0x10,0x20,0x40) → cyan (0x00,0xCC,0xFF)
        let f = t * 2.0;
        Color32::from_rgb(
            (0x10 as f32 * (1.0 - f)) as u8,
            (0x20 as f32 + (0xCC - 0x20) as f32 * f) as u8,
            (0x40 as f32 + (0xFF - 0x40) as f32 * f) as u8,
        )
    } else {
        // cyan → white
        let f = (t - 0.5) * 2.0;
        Color32::from_rgb(
            (0xCC as f32 * f) as u8,
            (0xCC as f32 + (0xFF - 0xCC) as f32 * f) as u8,
            0xFF,
        )
    }
}

// ── Device selector (unchanged) ────────────────────────────────────────

fn draw_device_selector(ui: &mut Ui, audio: &AudioSystem) {
    let tc = theme_colors(ui.ctx());

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

    let current = &audio.device_name;
    let selected_text = truncate_device_name(current, 24);

    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Input")
                .size(SMALL_SIZE)
                .color(tc.text_secondary),
        );

        egui::ComboBox::from_id_salt("audio_device_combo")
            .selected_text(RichText::new(&selected_text).size(SMALL_SIZE))
            .width(ui.available_width() - 4.0)
            .show_ui(ui, |ui| {
                let is_default = !devices.iter().any(|d| d == current);
                if ui
                    .selectable_label(
                        is_default,
                        RichText::new(DEFAULT_DEVICE_LABEL).size(SMALL_SIZE),
                    )
                    .clicked()
                {
                    ui.ctx().data_mut(|d| {
                        d.insert_temp(egui::Id::new("switch_audio_device"), String::new());
                    });
                }
                for dev in &devices {
                    let selected = dev == current;
                    let label = truncate_device_name(dev, 40);
                    if ui
                        .selectable_label(selected, RichText::new(&label).size(SMALL_SIZE))
                        .clicked()
                        && !selected
                    {
                        ui.ctx().data_mut(|d| {
                            d.insert_temp(egui::Id::new("switch_audio_device"), dev.clone());
                        });
                    }
                }
            });
    });

    ui.add_space(4.0);
}

// ── Section header ─────────────────────────────────────────────────────

fn draw_section_header(ui: &mut Ui, label: &str, right: &str) {
    let tc = theme_colors(ui.ctx());
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(label)
                .size(SMALL_SIZE)
                .color(tc.text_secondary)
                .strong(),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new(right).size(8.0).color(tc.text_secondary));
        });
    });
    ui.add_space(2.0);
}

// ── BPM ring ───────────────────────────────────────────────────────────

fn draw_bpm_ring(ui: &mut Ui, uniforms: &ShaderUniforms) -> egui::Response {
    let tc = theme_colors(ui.ctx());
    let r = BPM_RING_RADIUS;
    let size = Vec2::splat(r * 2.0 + 4.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
    let painter = ui.painter();
    let center = rect.center();

    let bpm = uniforms.bpm * 300.0;
    let phase = uniforms.beat_phase;

    // Dim background ring
    painter.circle_stroke(center, r, Stroke::new(2.0, tc.widget_bg));

    if bpm > 1.0 {
        // Arc: generate points from -PI/2 clockwise by phase * TAU
        let segments = 32;
        let arc_end = phase * TAU;
        if arc_end > 0.01 {
            let points: Vec<Pos2> = (0..=segments)
                .map(|i| {
                    let t = i as f32 / segments as f32;
                    let angle = -PI / 2.0 + t * arc_end;
                    pos2(center.x + r * angle.cos(), center.y + r * angle.sin())
                })
                .collect();
            painter.add(Shape::line(points, Stroke::new(2.5, tc.accent)));
        }

        // Orbiting dot at current phase
        let dot_angle = -PI / 2.0 + phase * TAU;
        let dot_pos = pos2(
            center.x + r * dot_angle.cos(),
            center.y + r * dot_angle.sin(),
        );
        let dot_color = if uniforms.beat > 0.5 {
            Color32::WHITE
        } else {
            tc.accent
        };
        let dot_r = if uniforms.beat > 0.5 { 3.5 } else { 2.5 };
        painter.circle_filled(dot_pos, dot_r, dot_color);
    }

    // BPM number centered
    if bpm > 1.0 {
        painter.text(
            center,
            egui::Align2::CENTER_CENTER,
            format!("{:.0}", bpm),
            egui::FontId::proportional(9.0),
            tc.text_primary,
        );
    }

    response.on_hover_text("Beat Phase \u{00b7} BPM\nArc tracks position within current beat\nDot flashes white on downbeat")
}

// ── Header row (title + BPM ring) ──────────────────────────────────────

fn draw_header_row(ui: &mut Ui, uniforms: &ShaderUniforms) {
    let tc = theme_colors(ui.ctx());
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("AUDIO")
                .size(HEADING_SIZE)
                .strong()
                .color(tc.text_primary),
        );
        ui.label(
            RichText::new("45 features")
                .size(8.0)
                .color(tc.text_secondary),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            draw_bpm_ring(ui, uniforms);
        });
    });
}

// ── Spectrum bars (gradient + peak hold) ───────────────────────────────

fn draw_spectrum_bars(ui: &mut Ui, bands: &[f32; 7]) {
    let tc = theme_colors(ui.ctx());
    let available_width = ui.available_width();
    let total_gaps = (bands.len() - 1) as f32 * METER_GAP;
    let bar_width = ((available_width - total_gaps) / bands.len() as f32).max(8.0);
    let height = METER_HEIGHT;

    // Peak hold state
    let peak_id = egui::Id::new("spectrum_peaks");
    let mut peaks: [f32; 7] = ui.ctx().data(|d| d.get_temp(peak_id)).unwrap_or([0.0; 7]);
    for i in 0..7 {
        if bands[i] > peaks[i] {
            peaks[i] = bands[i];
        } else {
            peaks[i] *= 0.95;
        }
    }
    ui.ctx().data_mut(|d| d.insert_temp(peak_id, peaks));

    let (rect, bars_resp) =
        ui.allocate_exact_size(Vec2::new(available_width, height), egui::Sense::hover());
    let (label_rect, _) =
        ui.allocate_exact_size(Vec2::new(available_width, 12.0), egui::Sense::hover());

    let painter = ui.painter();

    // Background
    painter.rect_filled(rect, 2.0, tc.meter_bg);

    // Grid lines at 25%, 50%, 75%
    let grid_color = Color32::from_rgb(0x2A, 0x2A, 0x2A);
    for frac in [0.25, 0.5, 0.75] {
        let y = rect.bottom() - height * frac;
        painter.line_segment(
            [pos2(rect.left(), y), pos2(rect.right(), y)],
            Stroke::new(0.5, grid_color),
        );
    }

    // Gradient bars via mesh
    for (i, &value) in bands.iter().enumerate() {
        let v = value.clamp(0.0, 1.0);
        if v < 0.001 {
            continue;
        }
        let x = rect.left() + i as f32 * (bar_width + METER_GAP);
        let fill_height = height * v;
        let top = rect.bottom() - fill_height;
        let bottom = rect.bottom();

        let color = BAND_COLORS[i];
        let faded = Color32::from_rgba_premultiplied(
            color.r() / 3,
            color.g() / 3,
            color.b() / 3,
            color.a(),
        );

        let mut mesh = Mesh::default();
        // top-left, top-right (full color), bottom-left, bottom-right (faded)
        mesh.colored_vertex(pos2(x, top), color);
        mesh.colored_vertex(pos2(x + bar_width, top), color);
        mesh.colored_vertex(pos2(x + bar_width, bottom), faded);
        mesh.colored_vertex(pos2(x, bottom), faded);
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(0, 2, 3);
        painter.add(Shape::mesh(mesh));

        // Peak hold marker
        let peak_y = rect.bottom() - height * peaks[i].clamp(0.0, 1.0);
        if peaks[i] > 0.01 {
            painter.line_segment(
                [pos2(x, peak_y), pos2(x + bar_width, peak_y)],
                Stroke::new(1.0, color),
            );
        }
    }

    // Labels below
    for (i, label) in BAND_LABELS.iter().enumerate() {
        let x = label_rect.left() + i as f32 * (bar_width + METER_GAP) + bar_width * 0.5;
        painter.text(
            pos2(x, label_rect.center().y),
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(SMALL_SIZE),
            tc.text_secondary,
        );
    }

    // Per-bar tooltip
    let step = bar_width + METER_GAP;
    let rect_left = rect.left();
    bars_resp.on_hover_ui_at_pointer(|ui| {
        if let Some(pos) = ui.input(|i| i.pointer.hover_pos()) {
            let idx = ((pos.x - rect_left) / step).floor() as usize;
            if idx < 7 {
                ui.label(BAND_TOOLTIPS[idx]);
            }
        }
    });
}

// ── Dynamics rows ──────────────────────────────────────────────────────

fn draw_dynamics_rows(ui: &mut Ui, uniforms: &ShaderUniforms) {
    let tc = theme_colors(ui.ctx());
    let values: [f32; 7] = [
        uniforms.rms,
        uniforms.kick,
        uniforms.onset,
        uniforms.flux,
        uniforms.centroid,
        uniforms.flatness,
        uniforms.rolloff,
    ];

    let available_width = ui.available_width();
    let label_width = 36.0;
    let value_width = 34.0;
    let bar_left = label_width + 8.0;
    let bar_right = available_width - value_width - 8.0;
    let bar_track_width = (bar_right - bar_left).max(20.0);

    for (i, (&label, &value)) in DYNAMICS_LABELS.iter().zip(values.iter()).enumerate() {
        let (row_rect, row_resp) = ui.allocate_exact_size(
            Vec2::new(available_width, DYNAMICS_ROW_HEIGHT),
            egui::Sense::hover(),
        );
        let painter = ui.painter();
        let cy = row_rect.center().y;
        let row_left = row_rect.left();

        // Label (right-aligned within label column)
        painter.text(
            pos2(row_left + label_width, cy),
            egui::Align2::RIGHT_CENTER,
            label,
            egui::FontId::proportional(SMALL_SIZE),
            tc.text_secondary,
        );

        let color = DYNAMICS_COLORS[i];
        let v = value.clamp(0.0, 1.0);

        let track_left = row_left + bar_left;

        if i == 1 {
            // Kick: boolean dot at start of track area
            let dot_color = if v > 0.5 {
                Color32::WHITE
            } else {
                Color32::from_rgb(0x33, 0x33, 0x33)
            };
            painter.circle_filled(pos2(track_left + 4.0, cy), 3.5, dot_color);
        } else {
            // Track background
            let track_rect = Rect::from_min_size(
                pos2(track_left, cy - DYNAMICS_BAR_HEIGHT * 0.5),
                Vec2::new(bar_track_width, DYNAMICS_BAR_HEIGHT),
            );
            painter.rect_filled(track_rect, 2.0, tc.meter_bg);

            // Fill bar
            if v > 0.001 {
                let fill_rect = Rect::from_min_size(
                    track_rect.min,
                    Vec2::new(bar_track_width * v, DYNAMICS_BAR_HEIGHT),
                );
                painter.rect_filled(fill_rect, 2.0, color);
            }
        }

        // Value as percentage (right-aligned)
        let pct = (value * 100.0).clamp(0.0, 999.0);
        painter.text(
            pos2(row_rect.right(), cy),
            egui::Align2::RIGHT_CENTER,
            format!("{:.0}%", pct),
            egui::FontId::monospace(SMALL_SIZE),
            tc.text_secondary,
        );

        row_resp.on_hover_text(DYNAMICS_TOOLTIPS[i]);
    }
}

// ── Chroma wheel ───────────────────────────────────────────────────────

fn draw_chroma_wheel(ui: &mut Ui, chroma: &[f32; 12]) {
    let tc = theme_colors(ui.ctx());
    let outer_r = CHROMA_OUTER_RADIUS;
    let size = Vec2::splat(outer_r * 2.0 + 24.0); // extra for labels
    let (rect, wheel_resp) = ui.allocate_exact_size(size, egui::Sense::hover());
    let painter = ui.painter();
    let center = rect.center();
    let inner_r = outer_r * CHROMA_INNER_RATIO;

    let segment_angle = TAU / 12.0;
    let gap_angle = 0.02; // small gap between segments

    // Find dominant pitch class
    let (dominant_idx, dominant_val) = chroma
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
        .unwrap_or((0, &0.0));

    for (i, &energy) in chroma.iter().enumerate() {
        let e = energy.clamp(0.0, 1.0);
        let start_angle = -PI / 2.0 + i as f32 * segment_angle + gap_angle;
        let end_angle = start_angle + segment_angle - gap_angle * 2.0;

        // Dynamic radius: inner_r to inner_r + (outer_r - inner_r) * energy
        let seg_outer = inner_r + (outer_r - inner_r) * (0.15 + 0.85 * e);

        // Build convex polygon: outer arc + inner arc (reversed)
        let arc_steps = 7;
        let mut points = Vec::with_capacity(arc_steps * 2);

        // Outer arc
        for s in 0..arc_steps {
            let t = s as f32 / (arc_steps - 1) as f32;
            let a = start_angle + t * (end_angle - start_angle);
            points.push(pos2(
                center.x + seg_outer * a.cos(),
                center.y + seg_outer * a.sin(),
            ));
        }
        // Inner arc (reversed)
        for s in (0..arc_steps).rev() {
            let t = s as f32 / (arc_steps - 1) as f32;
            let a = start_angle + t * (end_angle - start_angle);
            points.push(pos2(
                center.x + inner_r * a.cos(),
                center.y + inner_r * a.sin(),
            ));
        }

        let color = chroma_color(i, e);
        painter.add(Shape::convex_polygon(points, color, Stroke::NONE));

        // Label at outer_r + 9
        let mid_angle = (start_angle + end_angle) * 0.5;
        let label_r = outer_r + 9.0;
        let label_pos = pos2(
            center.x + label_r * mid_angle.cos(),
            center.y + label_r * mid_angle.sin(),
        );
        painter.text(
            label_pos,
            egui::Align2::CENTER_CENTER,
            CHROMA_LABELS[i],
            egui::FontId::proportional(7.0),
            tc.text_secondary,
        );
    }

    // Dominant note in center
    if *dominant_val > 0.05 {
        let dom_color = chroma_color(dominant_idx, *dominant_val);
        painter.text(
            center,
            egui::Align2::CENTER_CENTER,
            CHROMA_LABELS[dominant_idx],
            egui::FontId::proportional(14.0),
            dom_color,
        );
    }

    wheel_resp.on_hover_text(
        "Pitch Class Energy\nConstant-Q chromagram \u{2014} segment size\nshows energy of each note (C through B)",
    );
}

// ── MFCC heatmap ───────────────────────────────────────────────────────

fn draw_mfcc_heatmap(ui: &mut Ui, mfcc: &[f32; 16]) {
    let tc = theme_colors(ui.ctx());
    let available_width = ui.available_width();
    let gap = 1.0;
    let cell_width = ((available_width - 12.0 * gap) / 13.0).max(6.0);
    let total_w = cell_width * 13.0 + gap * 12.0;

    let (rect, heat_resp) =
        ui.allocate_exact_size(Vec2::new(total_w, MFCC_CELL_HEIGHT), egui::Sense::hover());
    let (label_rect, _) = ui.allocate_exact_size(Vec2::new(total_w, 10.0), egui::Sense::hover());
    let painter = ui.painter();

    for i in 0..13 {
        let v = mfcc[i].clamp(0.0, 1.0);
        let x = rect.left() + i as f32 * (cell_width + gap);
        let cell =
            Rect::from_min_size(pos2(x, rect.top()), Vec2::new(cell_width, MFCC_CELL_HEIGHT));
        painter.rect_filled(cell, 2.0, mfcc_heat_color(v));
    }

    // Selective labels below
    for (i, label) in MFCC_LABELS.iter().enumerate() {
        if label.is_empty() {
            continue;
        }
        let x = label_rect.left() + i as f32 * (cell_width + gap) + cell_width * 0.5;
        painter.text(
            pos2(x, label_rect.center().y),
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(7.0),
            tc.text_secondary,
        );
    }

    // Per-cell tooltip
    let step = cell_width + gap;
    let rect_left = rect.left();
    heat_resp.on_hover_ui_at_pointer(|ui| {
        if let Some(pos) = ui.input(|i| i.pointer.hover_pos()) {
            let idx = ((pos.x - rect_left) / step).floor() as usize;
            if idx < 13 {
                ui.label(MFCC_TOOLTIPS[idx]);
            }
        }
    });
}

// ── Footer ─────────────────────────────────────────────────────────────

fn draw_footer(ui: &mut Ui) {
    let tc = theme_colors(ui.ctx());
    ui.add_space(4.0);
    ui.separator();
    ui.label(
        RichText::new("7 bands · 7 dynamics · 12 chroma · 13 mfcc · 512 fft")
            .size(7.0)
            .color(tc.text_secondary),
    );
}

// ── Main entry ─────────────────────────────────────────────────────────

pub fn draw_audio_panel(ui: &mut Ui, audio: &AudioSystem, uniforms: &ShaderUniforms) {
    // Header: AUDIO + feature count + BPM ring
    draw_header_row(ui, uniforms);

    // Device selector
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

    // Spectrum
    let bands: [f32; 7] = [
        uniforms.sub_bass,
        uniforms.bass,
        uniforms.low_mid,
        uniforms.mid,
        uniforms.upper_mid,
        uniforms.presence,
        uniforms.brilliance,
    ];
    draw_section_header(ui, "SPECTRUM", "7 bands");
    draw_spectrum_bars(ui, &bands);

    // Dynamics
    draw_section_header(ui, "DYNAMICS", "7 features");
    draw_dynamics_rows(ui, uniforms);

    // Chroma
    draw_section_header(ui, "CHROMA", "12 pitch classes");
    ui.vertical_centered(|ui| {
        draw_chroma_wheel(ui, &uniforms.chroma);
    });

    // MFCC
    draw_section_header(ui, "TIMBRE · MFCC", "13 coefficients");
    draw_mfcc_heatmap(ui, &uniforms.mfcc);

    // Footer
    draw_footer(ui);
}
