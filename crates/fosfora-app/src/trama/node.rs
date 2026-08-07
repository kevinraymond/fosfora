//! Node identity and per-node instance state for the trama graph.

use crate::params::ParamStore;

use super::effect::EffectId;

/// Stable identity of a node within one graph.
///
/// Ids come from a monotonic counter and are never reused within a graph's
/// lifetime, so later features (serialization, undo) can reference nodes
/// without ambiguity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u64);

/// What a node *is*. `Output` is a graph primitive, not an effect file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeKind {
    /// Generates content; 0 texture inputs.
    Source { effect: EffectId },
    /// Transforms content; 1..=2 texture inputs.
    Effect { effect: EffectId },
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
    /// Per-node parameter values. M0 leaves these at manifest defaults; the
    /// inspector starts editing them in M1.
    pub params: ParamStore,
    /// A bypassed effect forwards its input 0; the executor resolves the
    /// aliasing at plan build, so no pass runs for it.
    pub bypass: bool,
}
