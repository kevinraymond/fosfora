//! What loading a cue *does* to layer state, as shared, GPU-free functions.
//!
//! Two consumers: the live app's timeline path and the headless scene renderer
//! (#2027). Both must treat a cue identically, so the behavior lives here once —
//! duplicating it would recreate the exact bug this module exists to fix:
//! `SceneCue::param_overrides` and `transition_beats` shipped as fields that the
//! validator checked and the scene editor saved, while no runtime code read
//! either one. A cue that overrode layer params played back as if the field were
//! absent. Nothing that shipped ever *wrote* the field, so nothing noticed; the
//! first author to populate it was the #2027 scene generator, whose ranked
//! per-cue intensities silently did nothing.
//!
//! Everything here is plain CPU state — `ParamStore` maps and opacity floats —
//! so it unit-tests in the default build with no GPU.

use std::collections::HashMap;

use super::types::SceneCue;
use crate::params::{ParamStore, ParamValue};

/// Apply a cue's per-layer param overrides on top of the freshly loaded preset.
///
/// `stores` yields `(param_store, locked)` per layer, in layer order —
/// positional, matching `param_overrides`' layer indexing. Locked layers are
/// skipped, consistent with preset load itself. Override entries beyond the
/// layer count are ignored (the offline validator flags them). Unknown param
/// names land in the store's value map but are never packed into the uniform
/// buffer — same silent-drop the preset's own params have, and same reason the
/// validator checks names against the effect.
///
/// Resets `changed` on every store it writes: cue playback is not a user edit
/// and must not mark the preset dirty (same reasoning as the morph path).
pub(crate) fn apply_cue_param_overrides<'a>(
    cue: &SceneCue,
    stores: impl Iterator<Item = (&'a mut ParamStore, bool)>,
) {
    for (overrides, (store, locked)) in cue.param_overrides.iter().zip(stores) {
        if locked || overrides.is_empty() {
            continue;
        }
        for (name, value) in overrides {
            store.set(name, value.clone());
        }
        store.changed = false;
    }
}

/// Param values + opacities for every layer at one instant — the endpoints a
/// ParamMorph transition interpolates between.
#[derive(Debug, Clone)]
pub(crate) struct MorphSnapshot {
    pub params: Vec<HashMap<String, ParamValue>>,
    pub opacities: Vec<f32>,
}

impl MorphSnapshot {
    /// Capture from `(values, opacity)` per layer, in layer order.
    pub(crate) fn capture<'a>(
        layers: impl Iterator<Item = (&'a HashMap<String, ParamValue>, f32)>,
    ) -> Self {
        let (params, opacities) = layers.map(|(v, o)| (v.clone(), o)).unzip();
        Self { params, opacities }
    }
}

/// Interpolate layer state between two snapshots at `progress` in 0..=1.
///
/// Only params present in *both* snapshots morph (a param that appears on one
/// side has no counterpart to lerp from), matching the shipped behavior this
/// was extracted from. Sets `changed = false` afterwards — timeline playback is
/// not a user edit.
pub(crate) fn apply_morph<'a>(
    from: &MorphSnapshot,
    to: &MorphSnapshot,
    progress: f32,
    layers: impl Iterator<Item = (&'a mut ParamStore, &'a mut f32)>,
) {
    for (i, (store, opacity)) in layers.enumerate() {
        if let (Some(from_layer), Some(to_layer)) = (from.params.get(i), to.params.get(i)) {
            for (name, to_val) in to_layer {
                if let Some(from_val) = from_layer.get(name) {
                    store.set(name, from_val.lerp(to_val, progress));
                }
            }
            store.changed = false;
        }
        if let (Some(&from_o), Some(&to_o)) = (from.opacities.get(i), to.opacities.get(i)) {
            *opacity = from_o + (to_o - from_o) * progress;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_with(pairs: &[(&str, f32)]) -> ParamStore {
        let mut s = ParamStore::new();
        for (name, v) in pairs {
            s.set(name, ParamValue::Float(*v));
        }
        s.changed = false;
        s
    }

    fn cue_with_overrides(overrides: Vec<HashMap<String, ParamValue>>) -> SceneCue {
        let mut cue = SceneCue::new("P");
        cue.param_overrides = overrides;
        cue
    }

    fn float(store: &ParamStore, name: &str) -> f32 {
        match store.values.get(name) {
            Some(ParamValue::Float(v)) => *v,
            other => panic!("{name} = {other:?}"),
        }
    }

    #[test]
    fn overrides_write_values_and_do_not_dirty() {
        let mut s0 = store_with(&[("speed", 0.5)]);
        let mut s1 = store_with(&[("glow", 0.2)]);
        let cue = cue_with_overrides(vec![
            HashMap::from([("speed".to_string(), ParamValue::Float(0.9))]),
            HashMap::from([("glow".to_string(), ParamValue::Float(0.7))]),
        ]);

        apply_cue_param_overrides(&cue, [(&mut s0, false), (&mut s1, false)].into_iter());

        assert_eq!(float(&s0, "speed"), 0.9);
        assert_eq!(float(&s1, "glow"), 0.7);
        // Playback must not mark the preset dirty — that is what debounced
        // autosave keys on.
        assert!(!s0.changed && !s1.changed);
    }

    #[test]
    fn locked_layers_keep_their_values() {
        let mut s0 = store_with(&[("speed", 0.5)]);
        let cue = cue_with_overrides(vec![HashMap::from([(
            "speed".to_string(),
            ParamValue::Float(0.9),
        )])]);

        apply_cue_param_overrides(&cue, [(&mut s0, true)].into_iter());

        assert_eq!(float(&s0, "speed"), 0.5, "locked layer was overridden");
    }

    /// More override groups than layers: the extras must be ignored, not panic
    /// and not wrap around.
    #[test]
    fn overflow_override_groups_are_ignored() {
        let mut s0 = store_with(&[("speed", 0.5)]);
        let cue = cue_with_overrides(vec![
            HashMap::from([("speed".to_string(), ParamValue::Float(0.9))]),
            HashMap::from([("ghost".to_string(), ParamValue::Float(1.0))]),
            HashMap::from([("ghost".to_string(), ParamValue::Float(1.0))]),
        ]);

        apply_cue_param_overrides(&cue, [(&mut s0, false)].into_iter());

        assert_eq!(float(&s0, "speed"), 0.9);
        assert!(!s0.values.contains_key("ghost"));
    }

    #[test]
    fn empty_override_group_leaves_layer_untouched() {
        let mut s0 = store_with(&[("speed", 0.5)]);
        s0.changed = true; // pre-existing dirty state must survive a no-op group
        let cue = cue_with_overrides(vec![HashMap::new()]);

        apply_cue_param_overrides(&cue, [(&mut s0, false)].into_iter());

        assert_eq!(float(&s0, "speed"), 0.5);
        assert!(
            s0.changed,
            "a no-op group must not clear an unrelated dirty flag"
        );
    }

    #[test]
    fn morph_lerps_params_and_opacity() {
        let from = MorphSnapshot {
            params: vec![HashMap::from([("x".to_string(), ParamValue::Float(0.0))])],
            opacities: vec![0.0],
        };
        let to = MorphSnapshot {
            params: vec![HashMap::from([("x".to_string(), ParamValue::Float(1.0))])],
            opacities: vec![1.0],
        };
        let mut store = store_with(&[("x", 0.0)]);
        let mut opacity = 0.0f32;

        apply_morph(&from, &to, 0.25, [(&mut store, &mut opacity)].into_iter());

        assert!((float(&store, "x") - 0.25).abs() < 1e-6);
        assert!((opacity - 0.25).abs() < 1e-6);
        assert!(!store.changed);
    }

    /// The morph-to snapshot is taken AFTER cue overrides are applied, so a
    /// morph into an overridden cue must land on the overridden value. This is
    /// the end-to-end contract for "same preset, different intensity per cue".
    #[test]
    fn morph_targets_overridden_values() {
        // Preset default 0.2; cue overrides to 0.8.
        let mut store = store_with(&[("intensity", 0.2)]);
        let from = MorphSnapshot::capture([(&store.values, 1.0f32)].into_iter());

        let cue = cue_with_overrides(vec![HashMap::from([(
            "intensity".to_string(),
            ParamValue::Float(0.8),
        )])]);
        apply_cue_param_overrides(&cue, [(&mut store, false)].into_iter());
        let to = MorphSnapshot::capture([(&store.values, 1.0f32)].into_iter());

        // Morph fully: must end at the OVERRIDDEN value, not the preset default.
        let mut opacity = 1.0f32;
        apply_morph(&from, &to, 1.0, [(&mut store, &mut opacity)].into_iter());
        assert!((float(&store, "intensity") - 0.8).abs() < 1e-6);
    }
}
