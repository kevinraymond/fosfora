//! Node identity and per-node instance state for the trama graph.

use crate::params::ParamStore;

use super::effect::EffectId;
use super::modulation::ParamMod;

/// Stable identity of a node within one graph.
///
/// Ids come from a monotonic counter and are never reused within a graph's
/// lifetime, so later features (serialization, undo) can reference nodes
/// without ambiguity.
///
/// **Unique within ONE graph only.** Every [`super::graph::NodeGraph`] starts
/// its counter at zero, so once there is a chain per layer, `NodeId(3)` names
/// a different node in every one of them. Anything that outlives a single
/// chain's plan must be keyed by [`ChainNode`] instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u64);

/// Which chain a graph is: one per layer, plus the master chain that runs on
/// the composited frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ChainId {
    /// Post-processes layer `n`'s rendered target, before compositing.
    // Constructed by the frame-graph integration (stage C); until then only
    // the tests build one.
    #[allow(dead_code)]
    Layer(u8),
    /// Post-processes the composited frame, upstream of `PostProcessDef`.
    Master,
}

impl ChainId {
    /// Dense index, used to give each chain a disjoint arena region. Master
    /// sits above every layer slot so adding a layer never renumbers it.
    pub fn index(self) -> u32 {
        match self {
            ChainId::Layer(n) => u32::from(n),
            ChainId::Master => crate::bindings::catalog::MAX_LAYERS as u32,
        }
    }
}

/// A node's identity across the whole system: the pair that is actually
/// unique. Every piece of executor state that survives a plan rebuild —
/// feedback ping-pong pairs, preview targets — is keyed by this, because
/// [`NodeId`] alone would let one layer's chain claim another's resources.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ChainNode {
    pub chain: ChainId,
    pub node: NodeId,
}

impl ChainNode {
    pub fn new(chain: ChainId, node: NodeId) -> Self {
        Self { chain, node }
    }
}

/// What a node *is*. `Output` is a graph primitive, not an effect file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeKind {
    /// Generates content; 0 texture inputs.
    Source { effect: EffectId },
    /// Transforms content; 1..=2 texture inputs.
    Effect { effect: EffectId },
    /// Explicit one-frame delay — the only temporal recursion (I9). Outputs
    /// the buffer its input wrote *last* frame; 1 texture input, no params.
    /// A wire INTO a Feedback node is not a dataflow edge for cycle purposes,
    /// which is what lets `mix → transform → feedback → mix` exist in a DAG.
    Feedback,
    /// What this chain's host handed in: the layer's rendered target for a
    /// layer chain, the composited frame for the master chain. 0 texture
    /// inputs, no params — it is a source whose content comes from outside
    /// the graph.
    ///
    /// Leaving it out (or leaving it unwired) is what keeps a chain a pure
    /// generator, which is exactly how trama behaved before chains existed.
    /// At most one per chain.
    ChainInput,
    /// The single sink; whatever feeds its one input reaches the screen.
    Output,
}

/// One placed node: kind + instance state.
pub struct NodeInstance {
    pub id: NodeId,
    pub kind: NodeKind,
    /// Input-pin count, denormalized from the effect manifest at add time so
    /// graph logic never needs the registry (`Output` = 1, sources = 0).
    pub inputs: u8,
    /// Per-node parameter values — the manual base half of the triple.
    pub params: ParamStore,
    /// Per-parameter modulation slots (name-keyed, at most one per param).
    /// The embedded runtime state never serializes — M3 skips it.
    pub mods: Vec<ParamMod>,
    /// A bypassed effect forwards its input 0; the executor resolves the
    /// aliasing at plan build, so no pass runs for it.
    pub bypass: bool,
    /// Running integrals of this node's RATE parameters (the effect
    /// manifest's `rates`). Runtime-only, like modulation state: never
    /// serialized, starts at zero.
    pub phases: Vec<RatePhase>,
}

/// The running integral of one rate parameter: `phase += value · dt`, once per
/// frame. The shader is handed this in the parameter's slot INSTEAD of the
/// value.
///
/// Why it exists: a shader that computes `u.time * speed` multiplies every
/// CHANGE in speed by how long the app has been running. Five minutes in, a
/// modulation wobbling speed by 0.05 moved Hue Drift's hue 15 turns between
/// frames — a strobe, and worse with every minute of uptime. Integrated, a
/// change in speed changes how fast the phase moves from here on, and nothing
/// about where it has been. `f64`, because it only ever grows.
#[derive(Debug, Clone, PartialEq)]
pub struct RatePhase {
    pub param: String,
    pub phase: f64,
}
