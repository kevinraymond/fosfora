use serde::Serialize;

use crate::audio::features::AudioFeatures;
use crate::effect::format::PfxEffect;
use crate::gpu::layer::{BlendMode, LayerInfo};
use crate::params::{ParamDef, ParamStore, ParamValue};
use crate::preset::PresetStore;

/// Full state snapshot sent on WebSocket connect.
#[derive(Serialize)]
pub struct FullState {
    #[serde(rename = "type")]
    pub msg_type: &'static str,
    pub effects: Vec<EffectInfo>,
    pub layers: Vec<LayerState>,
    pub active_layer: usize,
    pub presets: Vec<PresetInfo>,
    pub current_preset: Option<usize>,
    pub postprocess_enabled: bool,
}

#[derive(Serialize)]
pub struct EffectInfo {
    pub index: usize,
    pub name: String,
}

#[derive(Serialize)]
pub struct LayerState {
    pub index: usize,
    pub name: String,
    pub effect_name: Option<String>,
    pub effect_index: Option<usize>,
    pub blend_mode: u32,
    pub blend_name: &'static str,
    pub opacity: f32,
    pub enabled: bool,
    pub locked: bool,
    pub params: Vec<ParamInfo>,
}

#[derive(Serialize)]
pub struct ParamInfo {
    pub name: String,
    #[serde(rename = "type")]
    pub param_type: &'static str,
    pub value: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
}

#[derive(Serialize)]
pub struct PresetInfo {
    pub index: usize,
    pub name: String,
}

/// Audio snapshot broadcast at 10Hz.
#[derive(Serialize)]
pub struct AudioSnapshot {
    #[serde(rename = "type")]
    pub msg_type: &'static str,
    pub sub_bass: f32,
    pub bass: f32,
    pub low_mid: f32,
    pub mid: f32,
    pub upper_mid: f32,
    pub presence: f32,
    pub brilliance: f32,
    pub rms: f32,
    pub kick: f32,
    pub onset: f32,
    pub beat: f32,
    pub beat_phase: f32,
    pub bpm: f32,
}

/// Param change notification.
#[derive(Serialize)]
pub struct ParamChanged {
    #[serde(rename = "type")]
    pub msg_type: &'static str,
    pub layer: usize,
    pub name: String,
    pub value: f64,
}

/// Layer property change notification.
#[derive(Serialize)]
pub struct LayerChanged {
    #[serde(rename = "type")]
    pub msg_type: &'static str,
    pub index: usize,
    pub effect_name: Option<String>,
    pub effect_index: Option<usize>,
    pub blend_mode: u32,
    pub blend_name: &'static str,
    pub opacity: f32,
    pub enabled: bool,
}

/// Active layer change notification.
#[derive(Serialize)]
pub struct ActiveLayerChanged {
    #[serde(rename = "type")]
    pub msg_type: &'static str,
    pub index: usize,
}

/// Effect loaded notification.
#[derive(Serialize)]
pub struct EffectLoaded {
    #[serde(rename = "type")]
    pub msg_type: &'static str,
    pub layer: usize,
    pub effect_name: String,
    pub effect_index: usize,
    pub params: Vec<ParamInfo>,
}

/// Presets list update.
#[derive(Serialize)]
pub struct PresetsChanged {
    #[serde(rename = "type")]
    pub msg_type: &'static str,
    pub presets: Vec<PresetInfo>,
    pub current: Option<usize>,
}

// -- Builders --

pub fn build_full_state(
    effects: &[PfxEffect],
    layer_infos: &[LayerInfo],
    active_layer: usize,
    layers: &[(&ParamStore, Option<usize>, BlendMode, f32, bool, bool)],
    preset_store: &PresetStore,
    postprocess_enabled: bool,
) -> String {
    let effect_list: Vec<EffectInfo> = effects
        .iter()
        .enumerate()
        .filter(|(_, e)| !e.hidden)
        .map(|(i, e)| EffectInfo {
            index: i,
            name: e.name.clone(),
        })
        .collect();

    let layer_states: Vec<LayerState> = layer_infos
        .iter()
        .enumerate()
        .map(|(i, info)| {
            let params = if i < layers.len() {
                build_params(layers[i].0)
            } else {
                vec![]
            };
            let blend = if i < layers.len() { layers[i].2 } else { BlendMode::Normal };
            LayerState {
                index: i,
                name: info.custom_name.clone().unwrap_or_else(|| info.name.clone()),
                effect_name: info.effect_name.clone(),
                effect_index: info.effect_index,
                blend_mode: blend.as_u32(),
                blend_name: blend.display_name(),
                opacity: info.opacity,
                enabled: info.enabled,
                locked: info.locked,
                params,
            }
        })
        .collect();

    let presets: Vec<PresetInfo> = preset_store
        .presets
        .iter()
        .enumerate()
        .map(|(i, (name, _))| PresetInfo {
            index: i,
            name: name.clone(),
        })
        .collect();

    let state = FullState {
        msg_type: "state",
        effects: effect_list,
        layers: layer_states,
        active_layer,
        presets,
        current_preset: preset_store.current_preset,
        postprocess_enabled,
    };

    serde_json::to_string(&state).unwrap_or_default()
}

pub fn build_audio_snapshot(f: &AudioFeatures) -> String {
    let snap = AudioSnapshot {
        msg_type: "audio",
        sub_bass: f.sub_bass,
        bass: f.bass,
        low_mid: f.low_mid,
        mid: f.mid,
        upper_mid: f.upper_mid,
        presence: f.presence,
        brilliance: f.brilliance,
        rms: f.rms,
        kick: f.kick,
        onset: f.onset,
        beat: f.beat,
        beat_phase: f.beat_phase,
        bpm: f.bpm * 300.0, // raw BPM
    };
    serde_json::to_string(&snap).unwrap_or_default()
}

pub fn build_active_layer_changed(index: usize) -> String {
    serde_json::to_string(&ActiveLayerChanged {
        msg_type: "active_layer",
        index,
    })
    .unwrap_or_default()
}

pub fn build_layer_changed(info: &LayerInfo, index: usize) -> String {
    serde_json::to_string(&LayerChanged {
        msg_type: "layer_changed",
        index,
        effect_name: info.effect_name.clone(),
        effect_index: info.effect_index,
        blend_mode: info.blend_mode.as_u32(),
        blend_name: info.blend_mode.display_name(),
        opacity: info.opacity,
        enabled: info.enabled,
    })
    .unwrap_or_default()
}

pub fn build_effect_loaded(
    layer: usize,
    effect: &PfxEffect,
    effect_index: usize,
    param_store: &ParamStore,
) -> String {
    serde_json::to_string(&EffectLoaded {
        msg_type: "effect_loaded",
        layer,
        effect_name: effect.name.clone(),
        effect_index,
        params: build_params(param_store),
    })
    .unwrap_or_default()
}

pub fn build_presets_changed(preset_store: &PresetStore) -> String {
    let presets: Vec<PresetInfo> = preset_store
        .presets
        .iter()
        .enumerate()
        .map(|(i, (name, _))| PresetInfo {
            index: i,
            name: name.clone(),
        })
        .collect();
    serde_json::to_string(&PresetsChanged {
        msg_type: "presets",
        presets,
        current: preset_store.current_preset,
    })
    .unwrap_or_default()
}

fn build_params(store: &ParamStore) -> Vec<ParamInfo> {
    store
        .defs
        .iter()
        .map(|def| {
            let value = store.values.get(def.name());
            match def {
                ParamDef::Float { name, min, max, .. } => {
                    let v = match value {
                        Some(ParamValue::Float(f)) => *f as f64,
                        _ => *min as f64,
                    };
                    // Normalize to 0-1
                    let range = *max - *min;
                    let normalized = if range > 0.0 { (v as f32 - *min) / range } else { 0.0 };
                    ParamInfo {
                        name: name.clone(),
                        param_type: "float",
                        value: normalized as f64,
                        min: Some(*min as f64),
                        max: Some(*max as f64),
                    }
                }
                ParamDef::Bool { name, .. } => {
                    let v = match value {
                        Some(ParamValue::Bool(b)) => if *b { 1.0 } else { 0.0 },
                        _ => 0.0,
                    };
                    ParamInfo {
                        name: name.clone(),
                        param_type: "bool",
                        value: v,
                        min: None,
                        max: None,
                    }
                }
                ParamDef::Color { name, .. } => {
                    ParamInfo {
                        name: name.clone(),
                        param_type: "color",
                        value: 0.0,
                        min: None,
                        max: None,
                    }
                }
                ParamDef::Point2D { name, .. } => {
                    ParamInfo {
                        name: name.clone(),
                        param_type: "point2d",
                        value: 0.0,
                        min: None,
                        max: None,
                    }
                }
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::types::ParamDef;

    #[test]
    fn audio_snapshot_contains_type() {
        let f = AudioFeatures::default();
        let json = build_audio_snapshot(&f);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["type"], "audio");
    }

    #[test]
    fn audio_snapshot_raw_bpm() {
        let mut f = AudioFeatures::default();
        f.bpm = 0.4; // normalized, display = 120 BPM
        let json = build_audio_snapshot(&f);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let bpm = v["bpm"].as_f64().unwrap();
        assert!((bpm - 120.0).abs() < 0.1);
    }

    #[test]
    fn active_layer_changed_message() {
        let json = build_active_layer_changed(3);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["type"], "active_layer");
        assert_eq!(v["index"], 3);
    }

    #[test]
    fn build_params_float_normalized() {
        let mut store = ParamStore::new();
        let defs = vec![ParamDef::Float { name: "x".into(), default: 0.5, min: 0.0, max: 1.0 }];
        store.load_from_defs(&defs);
        store.set("x", ParamValue::Float(0.75));
        let params = build_params(&store);
        assert_eq!(params.len(), 1);
        assert_eq!(params[0].param_type, "float");
        // value = (0.75 - 0.0) / (1.0 - 0.0) = 0.75 normalized
        assert!((params[0].value - 0.75).abs() < 0.01);
    }

    #[test]
    fn build_params_bool() {
        let mut store = ParamStore::new();
        let defs = vec![ParamDef::Bool { name: "flag".into(), default: false }];
        store.load_from_defs(&defs);
        store.set("flag", ParamValue::Bool(true));
        let params = build_params(&store);
        assert_eq!(params[0].param_type, "bool");
        assert!((params[0].value - 1.0).abs() < 0.01);
    }
}
