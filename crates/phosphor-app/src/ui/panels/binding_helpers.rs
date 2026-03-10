use egui::{Color32, Pos2, RichText, Ui};

use crate::bindings::types::*;
use crate::ui::theme::colors::theme_colors;

// JSX-aligned source colors
pub const AUDIO_COLOR: Color32 = Color32::from_rgb(0x50, 0xC0, 0x70); // green
pub const MIDI_COLOR: Color32 = Color32::from_rgb(0xA0, 0x60, 0xD0); // purple
pub const OSC_COLOR: Color32 = Color32::from_rgb(0x50, 0x90, 0xE0); // blue
pub const WS_COLOR: Color32 = Color32::from_rgb(0xE0, 0x90, 0x40); // orange

/// Context passed to the bindings panel/matrix for building target/source pickers.
pub struct BindingPanelInfo {
    /// Effect name on the active layer (e.g. "Phosphor").
    pub effect_name: String,
    /// Param names available on the active layer (Float and Bool only).
    pub param_names: Vec<String>,
    /// Number of layers.
    pub layer_count: usize,
    /// Current preset name (for preset-scoped bindings).
    #[allow(dead_code)]
    pub preset_name: String,
}

// ---------------------------------------------------------------------------
// Target options
// ---------------------------------------------------------------------------

pub struct TargetOption {
    pub id: String,
    pub label: String,
    pub group: &'static str,
}

pub fn build_target_options(info: &BindingPanelInfo) -> Vec<TargetOption> {
    let mut targets = Vec::new();

    // Params (active layer)
    for name in &info.param_names {
        targets.push(TargetOption {
            id: format!("param.{}.{}", info.effect_name, name),
            label: name.clone(),
            group: "Params",
        });
    }

    // Layer targets
    for i in 0..info.layer_count {
        for (suffix, label_suffix) in
            [("opacity", "opacity"), ("blend", "blend"), ("enabled", "enabled")]
        {
            targets.push(TargetOption {
                id: format!("layer.{i}.{suffix}"),
                label: format!("Layer {i} {label_suffix}"),
                group: "Layers",
            });
        }
    }

    // PostFX targets
    for (id, label) in [
        ("postfx.bloom_threshold", "Bloom threshold"),
        ("postfx.bloom_intensity", "Bloom intensity"),
        ("postfx.vignette", "Vignette"),
        ("postfx.ca_intensity", "Chromatic aberration"),
        ("postfx.grain_intensity", "Film grain"),
    ] {
        targets.push(TargetOption {
            id: id.into(),
            label: label.into(),
            group: "PostFX",
        });
    }

    // Uniform targets (direct shader uniform override)
    for (field, label) in UNIFORM_TARGETS {
        targets.push(TargetOption {
            id: format!("uniform.{field}"),
            label: label.to_string(),
            group: "Uniforms",
        });
    }

    // Scene transport
    for (id, label) in [
        ("scene.transport.go", "Next cue"),
        ("scene.transport.prev", "Previous cue"),
        ("scene.transport.stop", "Stop scene"),
    ] {
        targets.push(TargetOption {
            id: id.into(),
            label: label.into(),
            group: "Scene",
        });
    }

    // Global
    targets.push(TargetOption {
        id: "global.master_opacity".into(),
        label: "Master opacity".into(),
        group: "Global",
    });

    targets
}

/// Bindable shader uniform fields: (field_name, display_label).
pub const UNIFORM_TARGETS: &[(&str, &str)] = &[
    ("sub_bass", "u.sub_bass"),
    ("bass", "u.bass"),
    ("low_mid", "u.low_mid"),
    ("mid", "u.mid"),
    ("upper_mid", "u.upper_mid"),
    ("presence", "u.presence"),
    ("brilliance", "u.brilliance"),
    ("rms", "u.rms"),
    ("kick", "u.kick"),
    ("centroid", "u.centroid"),
    ("flux", "u.flux"),
    ("flatness", "u.flatness"),
    ("rolloff", "u.rolloff"),
    ("bandwidth", "u.bandwidth"),
    ("zcr", "u.zcr"),
    ("onset", "u.onset"),
    ("beat", "u.beat"),
    ("beat_phase", "u.beat_phase"),
    ("bpm", "u.bpm"),
    ("beat_strength", "u.beat_strength"),
    ("dominant_chroma", "u.dominant_chroma"),
    ("feedback_decay", "u.feedback_decay"),
    ("time", "u.time"),
];

// ---------------------------------------------------------------------------
// Source display helpers
// ---------------------------------------------------------------------------

/// Metadata for an audio source entry in the picker.
pub struct AudioSourceInfo {
    /// Display name shown in the picker (e.g., "Sub Bass", "kick", "MFCC 5").
    pub friendly: String,
    /// WGSL uniform reference (e.g., "u.sub_bass", "u.mfcc[5]").
    pub uniform: String,
    /// Sub-group within Audio (Bands, Features, Beat, MFCC, Chroma).
    pub sub_group: &'static str,
}

/// Get display metadata for an audio source key.
pub fn audio_source_info(key: &str) -> AudioSourceInfo {
    match key {
        // Bands
        "audio.band.0" => AudioSourceInfo {
            friendly: "Sub Bass".into(),
            uniform: "u.sub_bass".into(),
            sub_group: "Bands",
        },
        "audio.band.1" => AudioSourceInfo {
            friendly: "Bass".into(),
            uniform: "u.bass".into(),
            sub_group: "Bands",
        },
        "audio.band.2" => AudioSourceInfo {
            friendly: "Low Mid".into(),
            uniform: "u.low_mid".into(),
            sub_group: "Bands",
        },
        "audio.band.3" => AudioSourceInfo {
            friendly: "Mid".into(),
            uniform: "u.mid".into(),
            sub_group: "Bands",
        },
        "audio.band.4" => AudioSourceInfo {
            friendly: "Upper Mid".into(),
            uniform: "u.upper_mid".into(),
            sub_group: "Bands",
        },
        "audio.band.5" => AudioSourceInfo {
            friendly: "Presence".into(),
            uniform: "u.presence".into(),
            sub_group: "Bands",
        },
        "audio.band.6" => AudioSourceInfo {
            friendly: "Brilliance".into(),
            uniform: "u.brilliance".into(),
            sub_group: "Bands",
        },
        "audio.rms" => AudioSourceInfo {
            friendly: "RMS".into(),
            uniform: "u.rms".into(),
            sub_group: "Bands",
        },
        // Features
        "audio.kick" => AudioSourceInfo {
            friendly: "Kick".into(),
            uniform: "u.kick".into(),
            sub_group: "Features",
        },
        "audio.centroid" => AudioSourceInfo {
            friendly: "Centroid".into(),
            uniform: "u.centroid".into(),
            sub_group: "Features",
        },
        "audio.flux" => AudioSourceInfo {
            friendly: "Flux".into(),
            uniform: "u.flux".into(),
            sub_group: "Features",
        },
        "audio.flatness" => AudioSourceInfo {
            friendly: "Flatness".into(),
            uniform: "u.flatness".into(),
            sub_group: "Features",
        },
        "audio.rolloff" => AudioSourceInfo {
            friendly: "Rolloff".into(),
            uniform: "u.rolloff".into(),
            sub_group: "Features",
        },
        "audio.bandwidth" => AudioSourceInfo {
            friendly: "Bandwidth".into(),
            uniform: "u.bandwidth".into(),
            sub_group: "Features",
        },
        "audio.zcr" => AudioSourceInfo {
            friendly: "ZCR".into(),
            uniform: "u.zcr".into(),
            sub_group: "Features",
        },
        // Beat
        "audio.onset" => AudioSourceInfo {
            friendly: "Onset".into(),
            uniform: "u.onset".into(),
            sub_group: "Beat",
        },
        "audio.beat" => AudioSourceInfo {
            friendly: "Beat".into(),
            uniform: "u.beat".into(),
            sub_group: "Beat",
        },
        "audio.beat_phase" => AudioSourceInfo {
            friendly: "Beat Phase".into(),
            uniform: "u.beat_phase".into(),
            sub_group: "Beat",
        },
        "audio.bpm" => AudioSourceInfo {
            friendly: "BPM".into(),
            uniform: "u.bpm".into(),
            sub_group: "Beat",
        },
        "audio.beat_strength" => AudioSourceInfo {
            friendly: "Beat Strength".into(),
            uniform: "u.beat_strength".into(),
            sub_group: "Beat",
        },
        // Chroma
        "audio.dominant_chroma" => AudioSourceInfo {
            friendly: "Dominant Chroma".into(),
            uniform: "u.dominant_chroma".into(),
            sub_group: "Chroma",
        },
        _ => {
            // Dynamic: mfcc.N, chroma.N
            if let Some(n) = key.strip_prefix("audio.mfcc.") {
                return AudioSourceInfo {
                    friendly: format!("MFCC {n}"),
                    uniform: format!("u.mfcc[{n}]"),
                    sub_group: "MFCC",
                };
            }
            if let Some(n) = key.strip_prefix("audio.chroma.") {
                let note = match n {
                    "0" => "C",
                    "1" => "C#",
                    "2" => "D",
                    "3" => "D#",
                    "4" => "E",
                    "5" => "F",
                    "6" => "F#",
                    "7" => "G",
                    "8" => "G#",
                    "9" => "A",
                    "10" => "A#",
                    "11" => "B",
                    _ => n,
                };
                return AudioSourceInfo {
                    friendly: format!("Chroma {note}"),
                    uniform: format!("u.chroma[{n}]"),
                    sub_group: "Chroma",
                };
            }
            // Fallback
            let short = key.strip_prefix("audio.").unwrap_or(key);
            AudioSourceInfo {
                friendly: short.to_string(),
                uniform: format!("u.{short}"),
                sub_group: "Other",
            }
        }
    }
}

/// Canonical audio source ordering for the picker (by sub-group).
pub const AUDIO_SOURCE_ORDER: &[&str] = &[
    // Bands
    "audio.band.0",
    "audio.band.1",
    "audio.band.2",
    "audio.band.3",
    "audio.band.4",
    "audio.band.5",
    "audio.band.6",
    "audio.rms",
    // Features
    "audio.kick",
    "audio.centroid",
    "audio.flux",
    "audio.flatness",
    "audio.rolloff",
    "audio.bandwidth",
    "audio.zcr",
    // Beat
    "audio.onset",
    "audio.beat",
    "audio.beat_phase",
    "audio.bpm",
    "audio.beat_strength",
    // Chroma
    "audio.dominant_chroma",
];

// ---------------------------------------------------------------------------
// Color / badge helpers
// ---------------------------------------------------------------------------

pub fn source_color(source: &str) -> Color32 {
    if source.starts_with("audio.") {
        AUDIO_COLOR
    } else if source.starts_with("midi.") {
        MIDI_COLOR
    } else if source.starts_with("osc.") {
        OSC_COLOR
    } else if source.starts_with("ws.") {
        WS_COLOR
    } else {
        Color32::GRAY
    }
}

#[allow(dead_code)]
pub fn source_badge_info(source: &str) -> (&'static str, Color32) {
    if source.starts_with("audio.") {
        ("AUD", AUDIO_COLOR)
    } else if source.starts_with("midi.") {
        ("MID", MIDI_COLOR)
    } else if source.starts_with("osc.") {
        ("OSC", OSC_COLOR)
    } else if source.starts_with("ws.") {
        ("WS", WS_COLOR)
    } else {
        ("---", Color32::GRAY)
    }
}

#[allow(dead_code)]
pub fn draw_source_badge(ui: &mut Ui, source: &str) {
    let (abbrev, color) = source_badge_info(source);
    ui.add(
        egui::Button::new(
            RichText::new(abbrev)
                .size(7.0)
                .color(Color32::WHITE)
                .strong(),
        )
        .fill(color.linear_multiply(0.7))
        .corner_radius(3.0)
        .min_size(egui::vec2(0.0, 12.0))
        .sense(egui::Sense::hover()),
    );
}

// ---------------------------------------------------------------------------
// Inline bar helper
// ---------------------------------------------------------------------------

pub fn draw_inline_bar(ui: &mut Ui, value: f32, width: f32, height: f32, fill_color: Color32, bg_color: Color32) {
    let (bar_rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    ui.painter().rect_filled(
        bar_rect,
        1.0,
        bg_color,
    );
    let filled = egui::Rect::from_min_size(
        bar_rect.min,
        egui::vec2(
            bar_rect.width() * value.clamp(0.0, 1.0),
            bar_rect.height(),
        ),
    );
    ui.painter().rect_filled(filled, 1.0, fill_color);
}

// ---------------------------------------------------------------------------
// Display label helpers
// ---------------------------------------------------------------------------

#[allow(dead_code)]
pub fn transform_short_label(t: &TransformDef) -> String {
    match t {
        TransformDef::Smooth { factor } => format!("smooth({factor:.1})"),
        TransformDef::Remap {
            in_lo,
            in_hi,
            out_lo,
            out_hi,
        } => format!("remap({in_lo:.1}\u{2013}{in_hi:.1}\u{2192}{out_lo:.1}\u{2013}{out_hi:.1})"),
        TransformDef::Quantize { steps } => format!("quantize({steps})"),
        TransformDef::Gate { threshold } => format!("gate({threshold:.1})"),
        TransformDef::Scale { factor } => format!("scale({factor:.1})"),
        TransformDef::Offset { value } => format!("offset({value:.1})"),
        TransformDef::Clamp { lo, hi } => format!("clamp({lo:.1}\u{2013}{hi:.1})"),
        TransformDef::Deadzone { lo, hi } => format!("dz({lo:.1}\u{2013}{hi:.1})"),
        TransformDef::Curve { curve_type } => format!("curve({curve_type})"),
        TransformDef::Invert => "invert".into(),
    }
}

pub fn make_display_name(source: &str, target: &str) -> String {
    let src = if source.starts_with("audio.") {
        audio_source_info(source).friendly
    } else {
        friendly_source(source)
    };
    let tgt = friendly_target(target);
    if src.is_empty() && tgt.is_empty() {
        "(new binding)".into()
    } else if src.is_empty() {
        format!("? \u{2192} {tgt}")
    } else if tgt.is_empty() {
        format!("{src} \u{2192} ?")
    } else {
        format!("{src} \u{2192} {tgt}")
    }
}

pub fn friendly_source(source: &str) -> String {
    if source.is_empty() {
        return String::new();
    }
    if source.starts_with("midi.") {
        let parts: Vec<&str> = source.split('.').collect();
        if parts.len() >= 5 {
            let msg_type = parts[2];
            let cc = parts[4];
            return match msg_type {
                "cc" => format!("CC {cc}"),
                "note" => format!("Note {cc}"),
                _ => parts.last().unwrap_or(&"?").to_string(),
            };
        }
        return source
            .strip_prefix("midi.")
            .unwrap_or(source)
            .to_string();
    }
    if source.starts_with("audio.") {
        return source
            .strip_prefix("audio.")
            .unwrap_or(source)
            .to_string();
    }
    if source.starts_with("osc.") {
        let addr = source.strip_prefix("osc.").unwrap_or(source);
        return addr.rsplit('/').next().unwrap_or(addr).to_string();
    }
    if source.starts_with("ws.") {
        let rest = source.strip_prefix("ws.").unwrap_or(source);
        return rest.rsplit('.').next().unwrap_or(rest).to_string();
    }
    source.to_string()
}

/// Convert a WS source name (hyphenated slug) to a display label.
/// e.g. "smart-lfo" → "Smart LFO", "mediapipe-hands" → "MediaPipe Hands"
pub fn ws_source_display_name(source_name: &str) -> String {
    // Known display names for built-in bridges
    match source_name {
        "smart-lfo" => return "Smart LFO".to_string(),
        "mediapipe-hands" => return "MediaPipe Hands".to_string(),
        "mediapipe-pose" => return "MediaPipe Pose".to_string(),
        "mediapipe-face" => return "MediaPipe Face".to_string(),
        "yolo-detect" => return "YOLO Detect".to_string(),
        "realsense-depth" => return "RealSense Depth".to_string(),
        "iphone-arkit" => return "iPhone ARKit".to_string(),
        "leap-motion" => return "Leap Motion".to_string(),
        "kinect-body" => return "Kinect Body".to_string(),
        _ => {}
    }
    // Fallback: Title Case from hyphenated slug
    source_name
        .split('-')
        .map(|word| {
            let mut c = word.chars();
            match c.next() {
                None => String::new(),
                Some(f) => f.to_uppercase().to_string() + c.as_str(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn friendly_target(target: &str) -> String {
    if target.is_empty() {
        return String::new();
    }
    let parts: Vec<&str> = target.split('.').collect();
    match parts.first().copied() {
        Some("param") => parts.get(2).unwrap_or(&"?").to_string(),
        Some("layer") => {
            let idx = parts.get(1).unwrap_or(&"?");
            let field = parts.get(2).unwrap_or(&"?");
            format!("L{idx} {field}")
        }
        Some("global") => {
            let field = parts.get(1).unwrap_or(&"?");
            field.replace('_', " ")
        }
        Some("postfx") => {
            let field = parts.get(1).unwrap_or(&"?");
            field.replace('_', " ")
        }
        Some("uniform") => {
            let field = parts.get(1).unwrap_or(&"?");
            format!("u.{field}")
        }
        Some("scene") => {
            let action = parts.get(2).unwrap_or(&"?");
            format!("scene {action}")
        }
        _ => target.to_string(),
    }
}

pub fn target_display_label(target: &str, targets: &[TargetOption]) -> String {
    if target.is_empty() {
        return "(select target)".into();
    }
    targets
        .iter()
        .find(|t| t.id == target)
        .map(|t| t.label.clone())
        .unwrap_or_else(|| friendly_target(target))
}

/// Draw a source row in the picker popup.
/// Layout: [name ·····  bar 0.42  u.field]
pub fn draw_source_row(
    ui: &mut Ui,
    key: &str,
    friendly_name: &str,
    uniform_ref: &str,
    val: f32,
    color: Color32,
    selected: bool,
    source_out: &mut String,
) {
    let row_height = 18.0;
    let avail_width = ui.available_width().max(260.0);
    let desired = egui::vec2(avail_width, row_height);
    let (rect, resp) = ui.allocate_exact_size(desired, egui::Sense::click());

    if resp.clicked() {
        *source_out = key.to_string();
    }

    let painter = ui.painter();
    if selected {
        painter.rect_filled(rect, 2.0, ui.visuals().selection.bg_fill);
    } else if resp.hovered() {
        painter.rect_filled(rect, 2.0, ui.visuals().widgets.hovered.bg_fill);
    }

    let text_color = if selected {
        ui.visuals().selection.stroke.color
    } else {
        ui.visuals().text_color()
    };
    let tc = theme_colors(ui.ctx());
    let dim_color = tc.text_dim;

    let left = rect.left() + 6.0;
    let cy = rect.center().y;

    let uniform_right = rect.right() - 4.0;
    let val_right = rect.right() - 70.0;
    let bar_right = val_right - 6.0;
    let bar_width = 36.0;
    let bar_left = bar_right - bar_width;

    // Name
    painter.text(
        Pos2::new(left, cy),
        egui::Align2::LEFT_CENTER,
        friendly_name,
        egui::FontId::proportional(9.0),
        text_color,
    );

    // Mini bar
    let bar_rect = egui::Rect::from_min_size(
        Pos2::new(bar_left, cy - 2.0),
        egui::vec2(bar_width, 4.0),
    );
    painter.rect_filled(bar_rect, 1.0, tc.meter_bg);
    let fill_w = bar_width * val.clamp(0.0, 1.0);
    if fill_w > 0.5 {
        let fill_rect = egui::Rect::from_min_size(bar_rect.min, egui::vec2(fill_w, 4.0));
        painter.rect_filled(fill_rect, 1.0, color.linear_multiply(0.7));
    }

    // Value
    painter.text(
        Pos2::new(val_right, cy),
        egui::Align2::LEFT_CENTER,
        &format!("{val:.2}"),
        egui::FontId::proportional(8.0),
        dim_color,
    );

    // Uniform ref
    if !uniform_ref.is_empty() {
        painter.text(
            Pos2::new(uniform_right, cy),
            egui::Align2::RIGHT_CENTER,
            uniform_ref,
            egui::FontId::proportional(7.0),
            tc.text_dim,
        );
    }

    resp.on_hover_text(key);
}
