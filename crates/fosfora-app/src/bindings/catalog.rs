//! The binding targets the app actually acts on, as data.
//!
//! [`parse_target`](super::types::parse_target) used to accept any leaf name
//! under a known prefix: `postfx.nonsense` parsed into a variant carrying the
//! string and fell through `apply_binding_target`'s `_ => {}`, so the binding
//! loaded, showed up in the UI and silently did nothing. The leaves are enums
//! now (`target_fields.rs`), so a bad name parses as `Unknown` and the dispatch
//! is exhaustive (#150). These lists are derived from those enums for the
//! offline validator and the schema dump, so they cannot drift from what the
//! app handles.

#[cfg(any(feature = "analyze", test))]
use super::types::{LayerField, ParticleField, PostFxField, TransportAction, UniformField};

/// Maximum layers the app will hold. `add_layer` bails past this, and
/// `apply_preset_immediately` builds a preset's layers by calling `add_layer` in
/// a loop — so a preset with more layers than this loads with the extras
/// silently dropped, no warning on the path that matters.
pub const MAX_LAYERS: usize = 8;

// The lists below are consumed by the offline validator and schema dump
// (`analyze`) and by the tests. A default build never reads them, so gate them
// rather than blanket-allowing dead code.

/// `postfx.{leaf}` — applies to Master's post-processing (#3147).
#[cfg(any(feature = "analyze", test))]
pub fn postfx_targets() -> Vec<&'static str> {
    PostFxField::ALL.iter().map(|f| f.as_str()).collect()
}

/// `particle.{leaf}` — applies to every layer that runs a particle system.
#[cfg(any(feature = "analyze", test))]
pub fn particle_targets() -> Vec<&'static str> {
    ParticleField::ALL.iter().map(|f| f.as_str()).collect()
}

/// `uniform.{leaf}` — direct override of a shader uniform field.
#[cfg(any(feature = "analyze", test))]
pub fn uniform_targets() -> Vec<&'static str> {
    UniformField::ALL.iter().map(|f| f.as_str()).collect()
}

/// `scene.transport.{action}` — edge-triggered timeline control.
#[cfg(any(feature = "analyze", test))]
pub fn scene_transport_actions() -> Vec<&'static str> {
    TransportAction::ALL.iter().map(|a| a.as_str()).collect()
}

/// `layer.{n}.{field}`.
#[cfg(any(feature = "analyze", test))]
pub fn layer_fields() -> [&'static str; LayerField::ALL.len()] {
    LayerField::ALL.map(LayerField::as_str)
}

/// The leaf names valid after `raw`'s prefix, when `raw` is one of the
/// single-leaf forms — so a validator can say what `uniform.kik` should have
/// been instead of only that it is unknown.
#[cfg(any(feature = "analyze", test))]
pub fn leaves_for(raw: &str) -> Option<(&'static str, Vec<&'static str>)> {
    let (kind, leaves) = if raw.starts_with("postfx.") {
        ("postfx", postfx_targets())
    } else if raw.starts_with("particle.") {
        ("particle", particle_targets())
    } else if raw.starts_with("uniform.") {
        ("uniform", uniform_targets())
    } else if raw.starts_with("scene.transport.") {
        ("scene.transport", scene_transport_actions())
    } else {
        return None;
    };
    Some((kind, leaves))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bindings::types::BindingTarget;

    /// The scene-transport actions are dispatched by `main.rs` and the headless
    /// renderer as an exhaustive match on [`TransportAction`]; pin that `main.rs`
    /// still names every variant, so a new action cannot be added to the enum
    /// and wired only into the headless path.
    #[test]
    fn main_dispatches_every_transport_action() {
        const MAIN_RS: &str = include_str!("../main.rs");
        for action in TransportAction::ALL {
            let arm = format!("TransportAction::{action:?} =>");
            assert!(MAIN_RS.contains(&arm), "main.rs has no `{arm}` arm");
        }
    }

    /// Every catalog entry must be a target string that parses back to the
    /// variant it belongs to.
    #[test]
    fn every_catalog_entry_is_a_well_formed_target() {
        for leaf in postfx_targets() {
            let t = BindingTarget::from(format!("postfx.{leaf}"));
            assert!(
                matches!(&t, BindingTarget::PostFx(f) if f.as_str() == leaf),
                "postfx.{leaf} parsed as {t:?}"
            );
        }
        for leaf in particle_targets() {
            let t = BindingTarget::from(format!("particle.{leaf}"));
            assert!(
                matches!(&t, BindingTarget::Particle(f) if f.as_str() == leaf),
                "particle.{leaf} parsed as {t:?}"
            );
        }
        for leaf in uniform_targets() {
            let t = BindingTarget::from(format!("uniform.{leaf}"));
            assert!(
                matches!(&t, BindingTarget::Uniform(f) if f.as_str() == leaf),
                "uniform.{leaf} parsed as {t:?}"
            );
        }
        for action in scene_transport_actions() {
            let t = BindingTarget::from(format!("scene.transport.{action}"));
            assert!(
                matches!(&t, BindingTarget::SceneTransport(a) if a.as_str() == action),
                "scene.transport.{action} parsed as {t:?}"
            );
        }
        for field in layer_fields() {
            let t = BindingTarget::from(format!("layer.0.{field}"));
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

    #[test]
    fn leaves_for_names_the_family() {
        let (kind, leaves) = leaves_for("uniform.kik").expect("uniform family");
        assert_eq!(kind, "uniform");
        assert!(leaves.contains(&"kick"));
        assert!(leaves_for("layer.0.nope").is_none());
    }
}
