use egui::{Color32, Pos2, RichText, Ui};

use crate::bindings::types::*;
use crate::ui::theme::colors::theme_colors;

// JSX-aligned source colors
pub const AUDIO_COLOR: Color32 = Color32::from_rgb(0x50, 0xC0, 0x70); // green
pub const MIDI_COLOR: Color32 = Color32::from_rgb(0xA0, 0x60, 0xD0); // purple
pub const OSC_COLOR: Color32 = Color32::from_rgb(0x50, 0x90, 0xE0); // blue
pub const WS_COLOR: Color32 = Color32::from_rgb(0xE0, 0x90, 0x40); // orange

/// Per-layer parameter info for binding targets.
pub struct LayerParamInfo {
    /// Layer index.
    pub index: usize,
    /// Effect name on this layer (e.g. "Phosphor"), empty if no effect.
    pub effect_name: String,
    /// Param names available on this layer (Float and Bool only).
    pub param_names: Vec<String>,
}

/// Context passed to the bindings panel/matrix for building target/source pickers.
pub struct BindingPanelInfo {
    /// Per-layer parameter info for all layers in the preset.
    pub layers: Vec<LayerParamInfo>,
    /// Active layer index (for templates).
    pub active_layer: usize,
    /// Number of layers.
    pub layer_count: usize,
    /// Current preset name (for preset-scoped bindings).
    #[allow(dead_code)]
    pub preset_name: String,
}

impl BindingPanelInfo {
    /// Get the active layer's effect name (for templates).
    pub fn active_effect_name(&self) -> &str {
        self.layers
            .iter()
            .find(|l| l.index == self.active_layer)
            .map(|l| l.effect_name.as_str())
            .unwrap_or("")
    }

    /// Get the active layer's param names (for templates).
    pub fn active_param_names(&self) -> &[String] {
        self.layers
            .iter()
            .find(|l| l.index == self.active_layer)
            .map(|l| l.param_names.as_slice())
            .unwrap_or(&[])
    }
}

// ---------------------------------------------------------------------------
// Target options
// ---------------------------------------------------------------------------

pub struct TargetOption {
    pub id: String,
    pub label: String,
    pub group: std::borrow::Cow<'static, str>,
}

pub fn build_target_options(info: &BindingPanelInfo) -> Vec<TargetOption> {
    let mut targets = Vec::new();

    // Params — per layer
    for lp in &info.layers {
        if lp.param_names.is_empty() || lp.effect_name.is_empty() {
            continue;
        }
        // Cow, not Box::leak — this runs every frame while the matrix is open,
        // and a leaked label per layer-group per frame is an unbounded leak.
        let group_label: std::borrow::Cow<'static, str> = if info.layer_count == 1 {
            "Params".into()
        } else {
            format!("Layer {} \u{2022} {}", lp.index, lp.effect_name).into()
        };
        for name in &lp.param_names {
            targets.push(TargetOption {
                id: format!("param.{}.{}.{}", lp.index, lp.effect_name, name),
                label: name.clone(),
                group: group_label.clone(),
            });
        }
    }

    // Layer targets
    for i in 0..info.layer_count {
        for (suffix, label_suffix) in [
            ("opacity", "opacity"),
            ("blend", "blend"),
            ("enabled", "enabled"),
        ] {
            targets.push(TargetOption {
                id: format!("layer.{i}.{suffix}"),
                label: format!("Layer {i} {label_suffix}"),
                group: "Layers".into(),
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
            group: "PostFX".into(),
        });
    }

    // Particle targets
    for (id, label) in [
        ("particle.emit_rate", "Emit rate"),
        ("particle.burst_on_beat", "Burst on beat"),
        ("particle.lifetime", "Lifetime"),
        ("particle.speed", "Speed"),
        ("particle.size", "Size"),
        ("particle.drag", "Drag"),
        ("particle.turbulence", "Turbulence"),
        ("particle.gravity_x", "Gravity X"),
        ("particle.gravity_y", "Gravity Y"),
        ("particle.vortex_strength", "Vortex strength"),
        ("particle.obstacle_enabled", "Obstacle enabled"),
        ("particle.obstacle_mode", "Obstacle mode"),
        ("particle.obstacle_threshold", "Obstacle threshold"),
        ("particle.obstacle_elasticity", "Obstacle elasticity"),
    ] {
        targets.push(TargetOption {
            id: id.into(),
            label: label.into(),
            group: "Particles".into(),
        });
    }

    // Uniform targets (direct shader uniform override)
    for (field, label) in UNIFORM_TARGETS {
        targets.push(TargetOption {
            id: format!("uniform.{field}"),
            label: (*label).to_string(),
            group: "Uniforms".into(),
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
            group: "Scene".into(),
        });
    }

    // Global
    targets.push(TargetOption {
        id: "global.master_opacity".into(),
        label: "Master opacity".into(),
        group: "Global".into(),
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

/// Display name and group for the v2 (#1505) / v3 (#1629) audio features.
///
/// These 28 were collected by `bindings::sources` every frame from the day their
/// detectors landed, but appeared in neither picker, so the only way to bind one was to
/// hand-edit `global-bindings.json` — while the README promised all 74 features were
/// bindable. Their uniform names match the key suffix exactly, so `audio_source_info`'s
/// generic `u.{short}` fallback is already correct for every one of them; only the
/// friendly name and sub-group were missing.
const EXTENDED_SOURCES: &[(&str, &str, &str)] = &[
    // (key suffix, friendly name, sub-group)
    ("loudness_m", "Momentary Loudness", "Loudness"),
    ("loudness_s", "Short-term Loudness", "Loudness"),
    ("loudness_trend", "Loudness Trend", "Loudness"),
    ("contrast_0", "Contrast 200 Hz", "Timbre"),
    ("contrast_1", "Contrast 400 Hz", "Timbre"),
    ("contrast_2", "Contrast 800 Hz", "Timbre"),
    ("contrast_3", "Contrast 1.6 kHz", "Timbre"),
    ("contrast_4", "Contrast 3.2 kHz", "Timbre"),
    ("contrast_5", "Contrast 6.4 kHz", "Timbre"),
    ("contrast_mean", "Contrast Mean", "Timbre"),
    ("timbre_flux", "Timbre Flux", "Timbre"),
    ("downbeat", "Downbeat", "Beat"),
    ("bar_phase", "Bar Phase", "Beat"),
    ("beat_in_bar", "Beat in Bar", "Beat"),
    ("section_novelty", "Section Novelty", "Structure"),
    ("buildup", "Build-up", "Structure"),
    ("drop", "Drop", "Structure"),
    ("percussive_energy", "Percussive Energy", "Harmonic"),
    ("harmonic_energy", "Harmonic Energy", "Harmonic"),
    ("harmonic_ratio", "Harmonic Ratio", "Harmonic"),
    ("pan", "Pan", "Stereo"),
    ("stereo_width", "Stereo Width", "Stereo"),
    ("stereo_corr", "L/R Correlation", "Stereo"),
    ("band_pan_sub_bass", "Sub Bass Pan", "Stereo"),
    ("band_pan_bass", "Bass Pan", "Stereo"),
    ("band_pan_low_mid", "Low Mid Pan", "Stereo"),
    ("band_pan_mid", "Mid Pan", "Stereo"),
    ("band_pan_upper_mid", "Upper Mid Pan", "Stereo"),
    ("band_pan_presence", "Presence Pan", "Stereo"),
    ("band_pan_brilliance", "Brilliance Pan", "Stereo"),
    ("pitch", "Pitch", "Pitch"),
    ("pitch_confidence", "Pitch Confidence", "Pitch"),
    ("key_class", "Key Root", "Key"),
    ("key_is_minor", "Minor Key", "Key"),
    ("key_confidence", "Key Confidence", "Key"),
];

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
        // Key — circle-of-fifths hue derived CPU-side (Chromatica #1477). Like
        // mel/dmfcc it has no GPU uniform; it drives parameter bindings only.
        "audio.key_hue" => AudioSourceInfo {
            friendly: "Key Hue".into(),
            uniform: "(binding only)".into(),
            sub_group: "Key",
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
            if let Some(n) = key.strip_prefix("audio.mel.") {
                // A1b (#1512): mel bands come from the A17 spectrogram column, not a GPU
                // uniform — they drive parameter bindings only, so there's no `u.*` to show.
                return AudioSourceInfo {
                    friendly: format!("Mel {n}"),
                    uniform: "(binding only)".into(),
                    sub_group: "Mel",
                };
            }
            if let Some(n) = key.strip_prefix("audio.dmfcc.") {
                // A16 (#1467): delta-MFCC slopes are bindings-only for the same reason
                // as mel — no uniform budget for another 13 floats.
                return AudioSourceInfo {
                    friendly: format!("ΔMFCC {n}"),
                    uniform: "(binding only)".into(),
                    sub_group: "DMFCC",
                };
            }
            let short = key.strip_prefix("audio.").unwrap_or(key);
            if let Some((_, friendly, sub_group)) =
                EXTENDED_SOURCES.iter().find(|(k, _, _)| *k == short)
            {
                return AudioSourceInfo {
                    friendly: (*friendly).into(),
                    uniform: format!("u.{short}"),
                    sub_group,
                };
            }
            // Fallback
            AudioSourceInfo {
                friendly: short.to_string(),
                uniform: format!("u.{short}"),
                sub_group: "Other",
            }
        }
    }
}

/// Canonical audio source ordering for the picker (by sub-group).
///
/// Each sub-group MUST be one contiguous run: `draw_matrix_source_picker` emits a header
/// every time `sub_group` changes, so a split run renders the same header twice.
/// `audio.mfcc.*`, `audio.chroma.*`, `audio.mel.*` and `audio.dmfcc.*` are enumerated
/// dynamically from the live snapshot instead of being listed here.
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
    // Loudness
    "audio.loudness_m",
    "audio.loudness_s",
    "audio.loudness_trend",
    // Features
    "audio.kick",
    "audio.centroid",
    "audio.flux",
    "audio.flatness",
    "audio.rolloff",
    "audio.bandwidth",
    "audio.zcr",
    // Timbre
    "audio.contrast_0",
    "audio.contrast_1",
    "audio.contrast_2",
    "audio.contrast_3",
    "audio.contrast_4",
    "audio.contrast_5",
    "audio.contrast_mean",
    "audio.timbre_flux",
    // Beat
    "audio.onset",
    "audio.beat",
    "audio.beat_phase",
    "audio.bpm",
    "audio.beat_strength",
    "audio.downbeat",
    "audio.bar_phase",
    "audio.beat_in_bar",
    // Structure
    "audio.section_novelty",
    "audio.buildup",
    "audio.drop",
    // Harmonic
    "audio.percussive_energy",
    "audio.harmonic_energy",
    "audio.harmonic_ratio",
    // Stereo
    "audio.pan",
    "audio.stereo_width",
    "audio.stereo_corr",
    "audio.band_pan_sub_bass",
    "audio.band_pan_bass",
    "audio.band_pan_low_mid",
    "audio.band_pan_mid",
    "audio.band_pan_upper_mid",
    "audio.band_pan_presence",
    "audio.band_pan_brilliance",
    // Pitch
    "audio.pitch",
    "audio.pitch_confidence",
    // Key
    "audio.key_class",
    "audio.key_is_minor",
    "audio.key_confidence",
    "audio.key_hue",
    // Chroma — abuts the dynamically-enumerated chroma bins.
    "audio.dominant_chroma",
];

/// The static audio sources grouped for the SOURCES column, as
/// `(display label, collapse id, keys)`.
///
/// Derived from [`AUDIO_SOURCE_ORDER`] so the column and the expanded-card picker cannot
/// list different things — they did, and both were missing the same 28 features.
/// `Chroma` is excluded: `dominant_chroma` heads the dynamically-built chroma group,
/// alongside the twelve pitch-class bins.
pub fn audio_source_groups() -> Vec<(String, String, Vec<&'static str>)> {
    let mut out: Vec<(String, String, Vec<&'static str>)> = Vec::new();
    for &key in AUDIO_SOURCE_ORDER {
        let sub_group = audio_source_info(key).sub_group;
        if sub_group == "Chroma" {
            continue;
        }
        match out.last_mut() {
            Some((label, _, keys)) if label.ends_with(sub_group) => keys.push(key),
            _ => out.push((
                format!("Audio \u{00b7} {sub_group}"),
                format!("audio_{}", sub_group.to_lowercase()),
                vec![key],
            )),
        }
    }
    out
}

/// Collapse ids for every audio group, static and dynamic — what "Collapse all" writes
/// and what the default-collapsed set is chosen from.
pub fn audio_group_ids() -> Vec<String> {
    let mut ids: Vec<String> = audio_source_groups()
        .into_iter()
        .map(|(_, id, _)| id)
        .collect();
    ids.extend(
        ["audio_mfcc", "audio_chroma", "audio_mel", "audio_dmfcc"]
            .iter()
            .map(|s| s.to_string()),
    );
    ids
}

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

pub fn draw_inline_bar(
    ui: &mut Ui,
    value: f32,
    width: f32,
    height: f32,
    fill_color: Color32,
    bg_color: Color32,
) {
    let (bar_rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    ui.painter().rect_filled(bar_rect, 1.0, bg_color);
    let filled = egui::Rect::from_min_size(
        bar_rect.min,
        egui::vec2(bar_rect.width() * value.clamp(0.0, 1.0), bar_rect.height()),
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
                _ => (*parts.last().unwrap_or(&"?")).to_string(),
            };
        }
        return source.strip_prefix("midi.").unwrap_or(source).to_string();
    }
    if source.starts_with("audio.") {
        return source.strip_prefix("audio.").unwrap_or(source).to_string();
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
        Some("param") => {
            // New format: param.{layer}.{effect}.{name} (4 parts)
            // Old format: param.{effect}.{name} (3 parts)
            if parts.len() >= 4 {
                let idx = parts[1];
                let name = parts[3];
                format!("L{idx} {name}")
            } else {
                (*parts.get(2).unwrap_or(&"?")).to_string()
            }
        }
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
        Some("particle") => {
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
    let bar_rect =
        egui::Rect::from_min_size(Pos2::new(bar_left, cy - 2.0), egui::vec2(bar_width, 4.0));
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
        format!("{val:.2}"),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_collected_audio_source_is_reachable_from_the_picker() {
        // 28 of the 74 audio features were collected every frame and listed in neither
        // picker, so the only way to bind one was to hand-edit global-bindings.json —
        // while the README promised any of the 74 could drive any parameter.
        let snap = crate::bindings::sources::collect_audio(&Default::default());
        let listed: std::collections::HashSet<&str> = AUDIO_SOURCE_ORDER.iter().copied().collect();
        let missing: Vec<&String> = snap
            .keys()
            // mfcc/chroma bins are enumerated from the live snapshot, not this table
            .filter(|k| !k.starts_with("audio.mfcc.") && !k.starts_with("audio.chroma."))
            .filter(|k| !listed.contains(k.as_str()))
            .collect();
        assert!(
            missing.is_empty(),
            "collected but unreachable in the picker: {missing:?}"
        );
    }

    #[test]
    fn audio_source_order_keeps_sub_groups_contiguous() {
        // draw_matrix_source_picker emits a header on every sub_group change, and
        // audio_source_groups() starts a new group the same way, so a sub-group split
        // across two runs of the table renders its header twice.
        let mut seen = std::collections::HashSet::new();
        let mut prev = "";
        for &k in AUDIO_SOURCE_ORDER {
            let g = audio_source_info(k).sub_group;
            if g != prev {
                assert!(
                    seen.insert(g),
                    "sub-group '{g}' appears in two separate runs"
                );
                prev = g;
            }
        }
    }

    #[test]
    fn every_listed_audio_source_has_a_real_name() {
        // A key that falls through to the generic arm shows its raw id in the picker.
        for &k in AUDIO_SOURCE_ORDER {
            let info = audio_source_info(k);
            assert_ne!(
                info.sub_group, "Other",
                "'{k}' has no display metadata — it would render as a raw source id"
            );
        }
    }

    #[test]
    fn source_column_groups_cover_the_order_table() {
        // The column derives its groups from AUDIO_SOURCE_ORDER; only dominant_chroma
        // is meant to be absent (it heads the dynamically-built Chroma group).
        let grouped: Vec<&str> = audio_source_groups()
            .into_iter()
            .flat_map(|(_, _, keys)| keys)
            .collect();
        let expected: Vec<&str> = AUDIO_SOURCE_ORDER
            .iter()
            .copied()
            .filter(|k| *k != "audio.dominant_chroma")
            .collect();
        assert_eq!(grouped, expected);
    }
}
