//! The binding targets the app actually acts on, as data.
//!
//! [`parse_target`](super::types::parse_target) tells you whether a target
//! string is *well-formed*. It cannot tell you whether anything *handles* it:
//! `postfx.nonsense` parses cleanly into `BindingTarget::PostFx("nonsense")` and
//! then falls through `apply_binding_target`'s `_ => {}`, so the binding loads,
//! shows up in the UI, and silently does nothing.
//!
//! That gap is invisible to a human authoring bindings by hand — you notice the
//! knob does nothing and fix the spelling. It is not invisible to a generator
//! authoring them in bulk, which is why these lists exist: an offline validator
//! can reject an unhandled leaf name before the file ever reaches the app.
//!
//! The lists are duplicated from the match arms in `app.rs`, so they can drift.
//! `catalog_matches_app` scans those arms and fails if they do — see the test
//! module at the bottom for why the scan asserts non-emptiness first.

#[cfg(any(feature = "analyze", test))]
use super::types::LayerField;

/// Maximum layers the app will hold. `add_layer` bails past this, and
/// `apply_preset_immediately` builds a preset's layers by calling `add_layer` in
/// a loop — so a preset with more layers than this loads with the extras
/// silently dropped, no warning on the path that matters.
pub const MAX_LAYERS: usize = 8;

// The target lists below are consumed by the offline validator and schema dump
// (`analyze`) and by the drift tests. A default build never reads them, so gate
// them rather than blanket-allowing dead code — that way a list which stops
// being used anywhere still shows up as dead.
//
// If the binding-matrix UI ever enumerates its target columns from here instead
// of building its own list, these become load-bearing everywhere and the gate
// can go.
/// `postfx.{leaf}` — applies to the **active** layer's post chain only.
#[cfg(any(feature = "analyze", test))]
pub const POSTFX_TARGETS: &[&str] = &[
    "bloom_threshold",
    "bloom_intensity",
    "vignette",
    "ca_intensity",
    "grain_intensity",
    "grain_rate",
];

/// `particle.{leaf}` — applies to every layer that runs a particle system.
#[cfg(any(feature = "analyze", test))]
pub const PARTICLE_TARGETS: &[&str] = &[
    "emit_rate",
    "burst_on_beat",
    "lifetime",
    "speed",
    "size",
    "drag",
    "turbulence",
    "gravity_x",
    "gravity_y",
    "vortex_strength",
    "obstacle_enabled",
    "obstacle_mode",
    "obstacle_threshold",
    "obstacle_elasticity",
];

/// `uniform.{leaf}` — direct override of a shader uniform field.
#[cfg(any(feature = "analyze", test))]
pub const UNIFORM_TARGETS: &[&str] = &[
    "sub_bass",
    "bass",
    "low_mid",
    "mid",
    "upper_mid",
    "presence",
    "brilliance",
    "rms",
    "kick",
    "centroid",
    "flux",
    "flatness",
    "rolloff",
    "bandwidth",
    "zcr",
    "onset",
    "beat",
    "beat_phase",
    "bpm",
    "beat_strength",
    "dominant_chroma",
    "loudness_m",
    "loudness_s",
    "loudness_trend",
    "key_class",
    "key_is_minor",
    "key_confidence",
    "downbeat",
    "bar_phase",
    "beat_in_bar",
    "pan",
    "stereo_width",
    "stereo_corr",
    "band_pan_sub_bass",
    "band_pan_bass",
    "band_pan_low_mid",
    "band_pan_mid",
    "band_pan_upper_mid",
    "band_pan_presence",
    "band_pan_brilliance",
    "section_novelty",
    "buildup",
    "drop",
    "percussive_energy",
    "harmonic_energy",
    "harmonic_ratio",
    "pitch",
    "pitch_confidence",
    "contrast_0",
    "contrast_1",
    "contrast_2",
    "contrast_3",
    "contrast_4",
    "contrast_5",
    "contrast_mean",
    "timbre_flux",
    "feedback_decay",
    "time",
];

/// `scene.transport.{action}` — edge-triggered timeline control.
///
/// Note these are dispatched in `main.rs`, not `app.rs`: the bus pushes any
/// action string into `pending_triggers` and the event loop drops the ones it
/// does not recognise.
#[cfg(any(feature = "analyze", test))]
pub const SCENE_TRANSPORT_ACTIONS: &[&str] = &["go", "prev", "stop"];

/// `layer.{n}.{field}` — derived from [`LayerField`] rather than duplicated, so
/// a new variant is a compile error here instead of a silent omission.
#[cfg(any(feature = "analyze", test))]
pub fn layer_fields() -> [&'static str; LayerField::ALL.len()] {
    LayerField::ALL.map(LayerField::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// The dispatch source, so the catalog can be checked against the arms it
    /// mirrors. A scan rather than a refactor: the alternative is threading a
    /// `-> bool` "handled" return through three nested matches, which is churn
    /// in the hottest dispatch path to prove one list is complete. The dispatch
    /// moved from app.rs to bindings/apply.rs for the headless renderer; this
    /// include moved with it.
    const APPLY_RS: &str = include_str!("apply.rs");
    const MAIN_RS: &str = include_str!("../main.rs");

    /// Pull the `"leaf" =>` arm names out of one `match rest { .. }` block,
    /// bounded by the variant marker and the `_ => {}` that closes it.
    fn arms_after(src: &str, marker: &str) -> BTreeSet<String> {
        let start = src
            .find(marker)
            .unwrap_or_else(|| panic!("marker not found in source: {marker}"));
        let rest = &src[start..];
        let end = rest
            .find("_ => {}")
            .unwrap_or_else(|| panic!("no `_ => {{}}` closing the match after {marker}"));
        let region = &rest[..end];

        region
            .lines()
            .filter_map(|line| {
                let line = line.trim();
                let name = line.strip_prefix('"')?;
                let (name, tail) = name.split_once('"')?;
                tail.trim_start()
                    .starts_with("=>")
                    .then(|| name.to_string())
            })
            .collect()
    }

    fn catalog(entries: &[&str]) -> BTreeSet<String> {
        entries.iter().map(|s| s.to_string()).collect()
    }

    // A scan that silently stops matching would make every assertion below
    // pass against an empty set, which is how a guard rots into decoration.
    // Each case asserts a concrete count first, so a regex that stops finding
    // arms fails loudly here rather than quietly approving anything.
    #[test]
    fn catalog_matches_app() {
        for (marker, entries, label) in [
            (
                "BindingTarget::PostFx(rest) => {",
                POSTFX_TARGETS,
                "POSTFX_TARGETS",
            ),
            (
                "BindingTarget::Particle(rest) => {",
                PARTICLE_TARGETS,
                "PARTICLE_TARGETS",
            ),
            (
                "BindingTarget::Uniform(rest) => {",
                UNIFORM_TARGETS,
                "UNIFORM_TARGETS",
            ),
        ] {
            let scanned = arms_after(APPLY_RS, marker);
            assert!(
                !scanned.is_empty(),
                "{label}: the arm scan found nothing — the scan itself is broken, \
                 not the catalog"
            );
            assert_eq!(
                scanned.len(),
                entries.len(),
                "{label}: app.rs handles {} leaf names, catalog lists {}. \
                 Missing from catalog: {:?}. Listed but unhandled: {:?}",
                scanned.len(),
                entries.len(),
                scanned.difference(&catalog(entries)).collect::<Vec<_>>(),
                catalog(entries).difference(&scanned).collect::<Vec<_>>(),
            );
            assert_eq!(scanned, catalog(entries), "{label} disagrees with app.rs");
        }
    }

    /// The transport actions are dispatched on the full dotted string in
    /// `main.rs`, so they need their own scan.
    #[test]
    fn scene_transport_catalog_matches_main() {
        let scanned: BTreeSet<String> = MAIN_RS
            .lines()
            .filter_map(|line| {
                let line = line.trim();
                let inner = line.strip_prefix("\"scene.transport.")?;
                let (action, tail) = inner.split_once('"')?;
                tail.trim_start()
                    .starts_with("=>")
                    .then(|| action.to_string())
            })
            .collect();

        assert!(
            !scanned.is_empty(),
            "the transport scan found nothing — the scan is broken, not the catalog"
        );
        assert_eq!(scanned, catalog(SCENE_TRANSPORT_ACTIONS));
    }

    /// Every catalog entry must be a target string that parses back to the
    /// variant it belongs to. Catches a leaf name that is handled but is not a
    /// legal target — e.g. one containing a dot, which would re-parse as a
    /// different variant or as `Unknown`.
    #[test]
    fn every_catalog_entry_is_a_well_formed_target() {
        use crate::bindings::types::BindingTarget;

        for leaf in POSTFX_TARGETS {
            let t: BindingTarget = format!("postfx.{leaf}").as_str().into();
            assert!(
                matches!(&t, BindingTarget::PostFx(f) if f == leaf),
                "postfx.{leaf} parsed as {t:?}"
            );
        }
        for leaf in PARTICLE_TARGETS {
            let t: BindingTarget = format!("particle.{leaf}").as_str().into();
            assert!(
                matches!(&t, BindingTarget::Particle(f) if f == leaf),
                "particle.{leaf} parsed as {t:?}"
            );
        }
        for leaf in UNIFORM_TARGETS {
            let t: BindingTarget = format!("uniform.{leaf}").as_str().into();
            assert!(
                matches!(&t, BindingTarget::Uniform(f) if f == leaf),
                "uniform.{leaf} parsed as {t:?}"
            );
        }
        for action in SCENE_TRANSPORT_ACTIONS {
            let t: BindingTarget = format!("scene.transport.{action}").as_str().into();
            assert!(
                matches!(&t, BindingTarget::SceneTransport(a) if a == action),
                "scene.transport.{action} parsed as {t:?}"
            );
        }
        for field in layer_fields() {
            let t: BindingTarget = format!("layer.0.{field}").as_str().into();
            assert!(
                matches!(&t, BindingTarget::Layer { layer: 0, field: f } if f.as_str() == field),
                "layer.0.{field} parsed as {t:?}"
            );
        }
    }

    #[test]
    fn layer_fields_round_trip() {
        for field in LayerField::ALL {
            assert_eq!(
                LayerField::parse(field.as_str()),
                Some(field),
                "{} did not round-trip",
                field.as_str()
            );
        }
    }
}
