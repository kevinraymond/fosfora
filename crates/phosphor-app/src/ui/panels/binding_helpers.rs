use egui::{Color32, Pos2, RichText, Ui};

use crate::bindings::sources::SourceSnapshot;
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
        //
        // Always named, even with one layer: a bare "Params" left the user to
        // work out which effect it belonged to, and every target under it is
        // pinned to that layer's index.
        let group_label: std::borrow::Cow<'static, str> =
            format!("Layer {} \u{2022} {}", lp.index, lp.effect_name).into();
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
            ("displace", "displace"),
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
        ("postfx.grain_rate", "Film grain rate"),
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

/// One group of bindable sources, as both the column and the card picker show it.
pub struct SourceGroup {
    pub label: String,
    /// Collapse id, also what "Collapse all" writes.
    pub id: String,
    pub color: Color32,
    pub keys: Vec<String>,
}

/// Every bindable source this frame, grouped and ordered.
///
/// ONE definition on purpose. The column and the expanded-card picker each used
/// to walk the snapshot with their own hardcoded list; they drifted (20 keys
/// against 21) and both missed the same 28 features, which were collected every
/// frame but reachable only by hand-editing `global-bindings.json` while the
/// README promised all 74 were bindable. Anything that reads the snapshot for
/// display goes through here now, so a new source cannot appear in one list and
/// not the other.
pub fn all_source_groups(snapshot: &SourceSnapshot) -> Vec<SourceGroup> {
    let mut out: Vec<SourceGroup> = Vec::new();

    let has_audio = snapshot.keys().any(|k| k.starts_with("audio."));
    if has_audio {
        for (label, id, keys) in audio_source_groups() {
            out.push(SourceGroup {
                label,
                id,
                color: AUDIO_COLOR,
                keys: keys.iter().map(|k| (*k).to_string()).collect(),
            });
        }
    }

    // Numerically-indexed audio families, present only once their detector runs.
    // Sorted by index rather than lexically, so band 2 does not follow band 11.
    fn indexed(
        snapshot: &SourceSnapshot,
        prefix: &str,
        label: &str,
        id: &str,
    ) -> Option<SourceGroup> {
        let mut keys: Vec<String> = snapshot
            .keys()
            .filter(|k| k.starts_with(prefix))
            .cloned()
            .collect();
        if keys.is_empty() {
            return None;
        }
        keys.sort_by_key(|k| {
            k.strip_prefix(prefix)
                .and_then(|n| n.parse::<u32>().ok())
                .unwrap_or(u32::MAX)
        });
        Some(SourceGroup {
            label: label.to_string(),
            id: id.to_string(),
            color: AUDIO_COLOR,
            keys,
        })
    }
    out.extend(indexed(
        snapshot,
        "audio.mfcc.",
        "Audio \u{00b7} MFCC",
        "audio_mfcc",
    ));

    // Chroma leads with the dominant pitch class, then the twelve bins.
    {
        let mut keys: Vec<String> = Vec::new();
        if snapshot.contains_key("audio.dominant_chroma") {
            keys.push("audio.dominant_chroma".to_string());
        }
        let mut bins: Vec<String> = snapshot
            .keys()
            .filter(|k| k.starts_with("audio.chroma."))
            .cloned()
            .collect();
        bins.sort_by_key(|k| {
            k.strip_prefix("audio.chroma.")
                .and_then(|n| n.parse::<u32>().ok())
                .unwrap_or(u32::MAX)
        });
        keys.extend(bins);
        if !keys.is_empty() {
            out.push(SourceGroup {
                label: "Audio \u{00b7} Chroma".to_string(),
                id: "audio_chroma".to_string(),
                color: AUDIO_COLOR,
                keys,
            });
        }
    }

    out.extend(indexed(
        snapshot,
        "audio.mel.",
        "Audio \u{00b7} Mel",
        "audio_mel",
    ));
    out.extend(indexed(
        snapshot,
        "audio.dmfcc.",
        "Audio \u{00b7} \u{0394}MFCC",
        "audio_dmfcc",
    ));

    for (prefix, label, id, color) in [
        ("midi.", "MIDI", "midi", MIDI_COLOR),
        ("osc.", "OSC", "osc", OSC_COLOR),
    ] {
        let mut keys: Vec<String> = snapshot
            .keys()
            .filter(|k| k.starts_with(prefix))
            .cloned()
            .collect();
        if keys.is_empty() {
            continue;
        }
        keys.sort();
        out.push(SourceGroup {
            label: label.to_string(),
            id: id.to_string(),
            color,
            keys,
        });
    }

    // WebSocket bridges get one group each, keyed `ws.{bridge}.{field}`.
    let mut ws_keys: Vec<String> = snapshot
        .keys()
        .filter(|k| k.starts_with("ws."))
        .cloned()
        .collect();
    ws_keys.sort();
    for key in ws_keys {
        let bridge = key
            .strip_prefix("ws.")
            .and_then(|rest| rest.split('.').next())
            .unwrap_or("ws")
            .to_string();
        let group_id = format!("ws.{bridge}");
        match out.last_mut() {
            Some(g) if g.id == group_id => g.keys.push(key),
            _ => out.push(SourceGroup {
                label: ws_source_display_name(&bridge),
                id: group_id,
                color: WS_COLOR,
                keys: vec![key],
            }),
        }
    }

    out
}

/// The keys of `group` matching `filter`, case-insensitively, over both the raw
/// key and the friendly label — a user hunting "kick" should not have to know
/// whether it is `audio.kick` or "Kick". An empty filter keeps everything.
pub fn filter_source_keys<'a>(group: &'a SourceGroup, filter: &str) -> Vec<&'a str> {
    if filter.trim().is_empty() {
        return group.keys.iter().map(|k| k.as_str()).collect();
    }
    let needle = filter.trim().to_lowercase();
    group
        .keys
        .iter()
        .filter(|k| {
            k.to_lowercase().contains(&needle)
                || friendly_source_label(k).to_lowercase().contains(&needle)
                || group.label.to_lowercase().contains(&needle)
        })
        .map(|k| k.as_str())
        .collect()
}

/// What a source row is labelled, whichever family it belongs to.
pub fn friendly_source_label(key: &str) -> String {
    if key.starts_with("audio.") {
        audio_source_info(key).friendly
    } else {
        friendly_source(key)
    }
}

/// Whether a target matches `filter`, over its label, its group and its id.
pub fn target_matches(opt: &TargetOption, filter: &str) -> bool {
    if filter.trim().is_empty() {
        return true;
    }
    let needle = filter.trim().to_lowercase();
    opt.label.to_lowercase().contains(&needle)
        || opt.group.to_lowercase().contains(&needle)
        || opt.id.to_lowercase().contains(&needle)
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

/// Whether a binding target still resolves against the layer stack as it is now.
///
/// A target goes dead when the layer it names loses its effect or is given a
/// different one — `param.0.Raster.warp` after layer 0 switches to Frost. Nothing
/// said so: [`target_display_label`] falls back to a prettified version of the id,
/// so a dead binding rendered identically to a working one and simply did nothing.
pub fn target_is_live(target: &str, targets: &[TargetOption]) -> bool {
    if target.is_empty() {
        return false;
    }
    if targets.iter().any(|t| t.id == target) {
        return true;
    }
    // The legacy indexless `param.{effect}.{param}` form resolves against
    // whichever layer currently runs that effect, so it is live if any does.
    let parts: Vec<&str> = target.split('.').collect();
    if parts.len() == 3 && parts[0] == "param" {
        return targets.iter().any(|t| {
            let p: Vec<&str> = t.id.split('.').collect();
            p.len() == 4 && p[0] == "param" && p[2] == parts[1] && p[3] == parts[2]
        });
    }
    false
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

    /// Every audio source the bus publishes, from all three collectors.
    fn all_collected_audio_sources() -> std::collections::HashSet<String> {
        use crate::bindings::sources;
        let mut keys: std::collections::HashSet<String> = std::collections::HashSet::new();
        keys.extend(sources::collect_audio(&Default::default()).into_keys());
        keys.extend(sources::collect_mel_bands(&[0.0; 64]).into_keys());
        keys.extend(sources::collect_dmfcc_bands(&[0.0; 13]).into_keys());
        keys
    }

    /// Sources enumerated per-bin from the live snapshot rather than from the static table.
    fn is_dynamically_enumerated(key: &str) -> bool {
        ["audio.mfcc.", "audio.chroma.", "audio.mel.", "audio.dmfcc."]
            .iter()
            .any(|p| key.starts_with(p))
    }

    #[test]
    fn every_collected_audio_source_is_reachable_from_the_picker() {
        // 28 of the 74 audio features were collected every frame and listed in neither
        // picker, so the only way to bind one was to hand-edit global-bindings.json —
        // while the README promised any of the 74 could drive any parameter.
        let listed: std::collections::HashSet<&str> = AUDIO_SOURCE_ORDER.iter().copied().collect();
        let missing: Vec<String> = all_collected_audio_sources()
            .into_iter()
            .filter(|k| !is_dynamically_enumerated(k))
            .filter(|k| !listed.contains(k.as_str()))
            .collect();
        assert!(
            missing.is_empty(),
            "collected but unreachable in the picker: {missing:?}"
        );
    }

    #[test]
    fn every_picker_audio_source_is_actually_collected() {
        // The reverse direction, which the forward guard alone cannot catch: a key listed in
        // the table that no collector publishes renders a source in the picker that is
        // permanently stuck at zero. One mistyped id is all it takes, and binding to it fails
        // silently — the VJ just sees a control that never moves. This is how the seven A13b
        // band-pan sources (#1801) were found, several releases after they shipped dead.
        let collected = all_collected_audio_sources();
        let dead: Vec<&str> = AUDIO_SOURCE_ORDER
            .iter()
            .copied()
            .filter(|k| !collected.contains(*k))
            .collect();
        assert!(
            dead.is_empty(),
            "listed in the picker but never collected: {dead:?}"
        );
    }

    fn raw() -> SourceRaw {
        SourceRaw {
            display: String::new(),
            numeric: 0.0,
        }
    }

    /// A snapshot holding every key both collectors and the dynamic families emit.
    fn full_snapshot() -> SourceSnapshot {
        let mut snap: SourceSnapshot = SourceSnapshot::new();
        for k in all_collected_audio_sources() {
            snap.insert(k, (0.0, raw()));
        }
        for i in 0..13 {
            snap.insert(format!("audio.mfcc.{i}"), (0.0, raw()));
        }
        for i in 0..12 {
            snap.insert(format!("audio.chroma.{i}"), (0.0, raw()));
        }
        snap.insert("audio.dominant_chroma".to_string(), (0.0, raw()));
        snap.insert("midi.MPD218.cc.0.42".to_string(), (0.0, raw()));
        snap.insert("osc./foo/bar".to_string(), (0.0, raw()));
        snap.insert("ws.mediapipe.left_thumb_y".to_string(), (0.0, raw()));
        snap.insert("ws.mediapipe.right_index_x".to_string(), (0.0, raw()));
        snap
    }

    /// The drift guard the column and the picker never had.
    ///
    /// They used to walk the snapshot separately with their own hardcoded lists,
    /// drifted to 20 keys against 21, and both missed the same 28 features. Now
    /// both read `all_source_groups`, so the only way a source can go unlistable
    /// is if it falls through every group — which is what this asserts.
    #[test]
    fn every_snapshot_key_lands_in_exactly_one_group() {
        let snap = full_snapshot();
        let groups = all_source_groups(&snap);

        let mut seen: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for g in &groups {
            for k in &g.keys {
                *seen.entry(k.as_str()).or_insert(0) += 1;
            }
        }

        let missing: Vec<&String> = snap
            .keys()
            .filter(|k| !seen.contains_key(k.as_str()))
            .collect();
        assert!(missing.is_empty(), "snapshot keys in no group: {missing:?}");

        let duplicated: Vec<&&str> = seen
            .iter()
            .filter(|(_, n)| **n > 1)
            .map(|(k, _)| k)
            .collect();
        assert!(duplicated.is_empty(), "keys listed twice: {duplicated:?}");
    }

    #[test]
    fn group_ids_are_unique_so_collapse_state_cannot_collide() {
        // Two groups sharing an id would collapse and expand as one.
        let groups = all_source_groups(&full_snapshot());
        let mut ids = std::collections::HashSet::new();
        for g in &groups {
            assert!(ids.insert(g.id.clone()), "duplicate group id '{}'", g.id);
        }
        // Each WS bridge gets its own group rather than being lumped together.
        assert!(groups.iter().any(|g| g.id == "ws.mediapipe"));
    }

    #[test]
    fn filtering_matches_key_and_friendly_name() {
        let groups = all_source_groups(&full_snapshot());
        let beat = groups
            .iter()
            .find(|g| g.keys.iter().any(|k| k == "audio.kick"))
            .expect("kick is collected");

        // Empty filter keeps the whole group.
        assert_eq!(filter_source_keys(beat, "").len(), beat.keys.len());
        // The raw id...
        assert!(filter_source_keys(beat, "audio.kick").contains(&"audio.kick"));
        // ...and the friendly label, case-insensitively — a user hunting "Kick"
        // should not have to know the key spelling.
        assert!(filter_source_keys(beat, "KICK").contains(&"audio.kick"));
        // A miss returns nothing rather than everything.
        assert!(filter_source_keys(beat, "zzzznope").is_empty());
    }

    /// Collapse-all used to write seven hardcoded ids, so with more than one layer
    /// the `Layer N • Effect` param groups — the biggest ones — were silently
    /// skipped. Both sides now enumerate the groups actually present.
    #[test]
    fn target_groups_include_per_layer_names() {
        let info = BindingPanelInfo {
            layers: vec![
                LayerParamInfo {
                    index: 0,
                    effect_name: "Raster".to_string(),
                    param_names: vec!["warp".to_string()],
                },
                LayerParamInfo {
                    index: 1,
                    effect_name: "Frost".to_string(),
                    param_names: vec!["bite".to_string()],
                },
            ],
            active_layer: 0,
            layer_count: 2,
            preset_name: String::new(),
        };
        let targets = build_target_options(&info);
        let groups: std::collections::HashSet<&str> =
            targets.iter().map(|t| t.group.as_ref()).collect();
        assert!(groups.contains("Layer 0 \u{2022} Raster"));
        assert!(groups.contains("Layer 1 \u{2022} Frost"));
        // The seven fixed ids alone would not have covered those two.
        assert!(groups.len() > 7);
    }

    /// A binding onto a layer whose effect has changed must be flagged, and a
    /// working one must NOT be — a false warning on every card is as useless as
    /// no warning at all.
    #[test]
    fn a_target_whose_effect_is_gone_reads_as_dead() {
        let info = BindingPanelInfo {
            layers: vec![LayerParamInfo {
                index: 0,
                effect_name: "Frost".to_string(),
                param_names: vec!["bite".to_string()],
            }],
            active_layer: 0,
            layer_count: 1,
            preset_name: String::new(),
        };
        let targets = build_target_options(&info);

        // Live: the layer really does offer this param.
        assert!(target_is_live("param.0.Frost.bite", &targets));
        // Dead: layer 0 used to run Raster. This is what a card showed as a
        // perfectly ordinary "warp intensity" while doing nothing.
        assert!(!target_is_live("param.0.Raster.warp_intensity", &targets));
        // Dead: the layer itself is gone.
        assert!(!target_is_live("param.3.Frost.bite", &targets));
        // Layerless targets never go dead.
        assert!(target_is_live("postfx.vignette", &targets));
        assert!(target_is_live("global.master_opacity", &targets));
        assert!(target_is_live("layer.0.opacity", &targets));
        // An empty target is not "dead", it is unset — the card says so already.
        assert!(!target_is_live("", &targets));
    }

    /// The legacy indexless form resolves against whichever layer runs that
    /// effect, so it must not be flagged just for lacking an index.
    #[test]
    fn a_legacy_indexless_target_is_live_when_its_effect_is_loaded() {
        let info = BindingPanelInfo {
            layers: vec![LayerParamInfo {
                index: 2,
                effect_name: "Frost".to_string(),
                param_names: vec!["bite".to_string()],
            }],
            active_layer: 2,
            layer_count: 3,
            preset_name: String::new(),
        };
        let targets = build_target_options(&info);
        assert!(target_is_live("param.Frost.bite", &targets));
        assert!(!target_is_live("param.Raster.warp", &targets));
    }

    #[test]
    fn target_filter_matches_label_group_and_id() {
        let info = BindingPanelInfo {
            layers: vec![LayerParamInfo {
                index: 0,
                effect_name: "Raster".to_string(),
                param_names: vec!["warp_intensity".to_string()],
            }],
            active_layer: 0,
            layer_count: 1,
            preset_name: String::new(),
        };
        let targets = build_target_options(&info);
        let opt = targets
            .iter()
            .find(|t| t.id == "param.0.Raster.warp_intensity")
            .expect("built above");

        assert!(target_matches(opt, ""));
        assert!(target_matches(opt, "warp")); // label
        // The group names its layer and effect even with one layer loaded, so a
        // user can tell what they are about to bind.
        assert_eq!(opt.group.as_ref(), "Layer 0 \u{2022} Raster");
        assert!(target_matches(opt, "Layer 0")); // group
        assert!(target_matches(opt, "raster")); // id, case-insensitively
        assert!(!target_matches(opt, "zzzznope"));

        // Filtering narrows a real list rather than emptying it.
        let hits: Vec<_> = targets
            .iter()
            .filter(|t| target_matches(t, "bloom"))
            .collect();
        assert!(!hits.is_empty() && hits.len() < targets.len());
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
