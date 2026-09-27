//! The starter chain the Chains tour puts on an empty layer chain (#3128,
//! Kevin's call in #3224): the layer's own picture, through one effect, to
//! Output. A fresh chain is only its Output node, so without this the tour's
//! steps about nodes, wires and a node's controls would point at nothing.

use super::effect::{EffectDef, EffectId, EffectKind};
use super::graph::NodeGraph;
use super::node::{NodeId, NodeKind};
use crate::params::{ParamDef, ParamValue};

/// The effect a starter chain runs: one input, and plain on any picture.
pub const STARTER_EFFECT: &str = "kaleido";
/// If that one is ever missing.
const FALLBACK_EFFECT: &str = "pixelate";

/// How the starter's Kaleidoscope is set (Kevin's pick for the tour): the
/// effect's own defaults stay as they are for anyone adding it by hand.
/// Controls not named keep their defaults (rotate).
pub const STARTER_SETTINGS: &[(&str, f32)] = &[("segments", 15.0), ("spin", -0.10), ("zoom", 4.0)];

/// What seeding made.
#[derive(Debug, PartialEq)]
pub struct Starter {
    /// The effect node: the one worth selecting, so its controls show.
    pub effect: NodeId,
    /// Where the canvas draws each node: left to right, the way it flows.
    pub layout: Vec<(NodeId, [f32; 2])>,
}

/// The effect to seed with, from what the registry loaded.
pub fn pick(effects: &[EffectDef]) -> Option<&EffectDef> {
    let usable = |d: &&EffectDef| d.kind == EffectKind::Effect && d.inputs == 1;
    [STARTER_EFFECT, FALLBACK_EFFECT]
        .iter()
        .find_map(|id| effects.iter().filter(usable).find(|d| d.id.0 == *id))
}

/// Seed `graph` with Layer input → `effect` → Output, wired, if it is empty:
/// nothing but its Output node. A chain with anything in it is the user's,
/// and is left alone (`None`).
pub fn seed(graph: &mut NodeGraph, effect: &EffectId, params: &[ParamDef]) -> Option<Starter> {
    if graph.nodes().len() != 1 {
        return None;
    }
    let output = graph.output_node();
    let input = graph.add_node(NodeKind::ChainInput, 0, &[]);
    let fx = graph.add_node(
        NodeKind::Effect {
            effect: effect.clone(),
        },
        1,
        params,
    );
    if effect.0 == STARTER_EFFECT
        && let Some(node) = graph.params_mut(fx)
    {
        for &(name, value) in STARTER_SETTINGS {
            if node.params.get(name).is_some() {
                node.params.set(name, ParamValue::Float(value));
            }
        }
    }
    graph.connect(input, fx, 0).ok()?;
    graph.connect(fx, output, 0).ok()?;
    Some(Starter {
        effect: fx,
        layout: vec![
            (input, [60.0, 160.0]),
            (fx, [280.0, 160.0]),
            (output, [500.0, 160.0]),
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kaleido() -> EffectId {
        EffectId(STARTER_EFFECT.into())
    }

    #[test]
    fn an_empty_chain_gets_input_effect_output_and_reaches_output() {
        let mut g = NodeGraph::new_with_output();
        assert!(!g.contributes());
        let s = seed(&mut g, &kaleido(), &[]).expect("seeded");
        assert!(g.contributes(), "the picture now reaches Output");
        assert!(g.validate().is_ok());
        assert_eq!(g.nodes().len(), 3);
        assert!(g.chain_input().is_some());
        assert!(matches!(
            g.node(s.effect).map(|n| &n.kind),
            Some(NodeKind::Effect { effect }) if effect.0 == STARTER_EFFECT
        ));
        // Laid out in the order it flows.
        let x = |id| s.layout.iter().find(|(n, _)| *n == id).unwrap().1[0];
        assert!(x(g.chain_input().unwrap()) < x(s.effect));
        assert!(x(s.effect) < x(g.output_node()));
    }

    // Kevin's settings for the tour's Kaleidoscope, over the manifest's
    // defaults; a control not named keeps its default.
    #[test]
    fn the_starter_kaleidoscope_takes_the_tours_settings() {
        let float = |name: &str, default: f32| ParamDef::Float {
            name: name.into(),
            default,
            min: -100.0,
            max: 100.0,
        };
        let defs = [
            float("segments", 6.0),
            float("spin", 0.0),
            float("rotate", 0.0),
            float("zoom", 1.0),
        ];
        let mut g = NodeGraph::new_with_output();
        let s = seed(&mut g, &kaleido(), &defs).unwrap();
        let params = &g.node(s.effect).unwrap().params;
        let get = |n: &str| match params.get(n) {
            Some(ParamValue::Float(v)) => *v,
            other => panic!("{n}: {other:?}"),
        };
        assert_eq!(get("segments"), 15.0);
        assert_eq!(get("spin"), -0.10);
        assert_eq!(get("rotate"), 0.0);
        assert_eq!(get("zoom"), 4.0);
    }

    #[test]
    fn a_chain_with_anything_in_it_is_left_alone() {
        let mut g = NodeGraph::new_with_output();
        g.add_node(NodeKind::Feedback, 1, &[]);
        let before = g.nodes().len();
        assert_eq!(seed(&mut g, &kaleido(), &[]), None);
        assert_eq!(g.nodes().len(), before);
        // And a seeded chain is not seeded twice.
        let mut g = NodeGraph::new_with_output();
        seed(&mut g, &kaleido(), &[]).unwrap();
        assert_eq!(seed(&mut g, &kaleido(), &[]), None);
        assert_eq!(g.nodes().len(), 3);
    }
}
