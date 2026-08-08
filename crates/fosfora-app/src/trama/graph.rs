//! The trama node graph: nodes, wires, validation, topological order.
//!
//! Invariants held *by construction* rather than checked after the fact:
//! exactly one `Output` node (created in `new_with_output`, refused by
//! `remove_node`), no cycles (refused at `connect` time), at most one wire per
//! input pin (`connect` replaces). `validate()` re-checks them defensively so
//! the executor and tests can assert rather than trust.
//!
//! Every structural edit bumps `version` and drops the cached topo order; the
//! executor keys its plan on `version()`, which is what makes "rewire updates
//! the output next frame" fall out for free.

use std::collections::{HashSet, VecDeque};

use crate::params::{ParamDef, ParamStore};

use super::modulation::{Modulation, ParamMod};
use super::node::{NodeId, NodeInstance, NodeKind};

/// A texture connection. Out pins are not modeled: every node has exactly one
/// output (pin 0), so a wire is fully described by producer + consumer pin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wire {
    pub from: NodeId,
    pub to: NodeId,
    pub to_input: u8,
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum GraphError {
    #[error("connection would create a cycle")]
    Cycle,
    #[error("a node cannot feed itself")]
    SelfLoop,
    #[error("input {input} is out of range for the target node")]
    BadInput { input: u8 },
    #[error("that node has no output pin")]
    NoOutputPin,
    #[error("unknown node")]
    UnknownNode,
    #[error("the Output node cannot be removed")]
    OutputImmortal,
    #[error("graph must contain exactly one Output node")]
    MissingOutput,
    #[error("an input pin has more than one wire")]
    DuplicateWire,
}

/// Projection of one node's *non-structural* state: the manual base values
/// and the modulation slots. Handing out disjoint field borrows (rather
/// than `&mut NodeInstance`) makes it impossible to flip `bypass`/`kind`
/// without going through the version-bumping structural API.
// Consumed by the frame loop and inspector in the follow-up M1 commits; the
// allow keeps this commit green under -D warnings until then.
#[allow(dead_code)]
pub struct NodeParamsMut<'a> {
    pub id: NodeId,
    pub params: &'a mut ParamStore,
    pub mods: &'a mut Vec<ParamMod>,
}

impl<'a> NodeParamsMut<'a> {
    fn of(node: &'a mut NodeInstance) -> Self {
        Self {
            id: node.id,
            params: &mut node.params,
            mods: &mut node.mods,
        }
    }
}

pub struct NodeGraph {
    nodes: Vec<NodeInstance>,
    wires: Vec<Wire>,
    output: NodeId,
    next_id: u64,
    /// Cached Kahn order over all nodes; `None` after any structural edit.
    topo: Option<Vec<NodeId>>,
    /// Bumped by every structural edit (including bypass toggles — they change
    /// the execution plan). The executor replans when this moves.
    version: u64,
}

impl NodeGraph {
    /// A graph containing only its Output node.
    pub fn new_with_output() -> Self {
        let output = NodeId(0);
        Self {
            nodes: vec![NodeInstance {
                id: output,
                kind: NodeKind::Output,
                inputs: 1,
                params: crate::params::ParamStore::new(),
                mods: Vec::new(),
                bypass: false,
            }],
            wires: Vec::new(),
            output,
            next_id: 1,
            topo: None,
            version: 0,
        }
    }

    fn touch(&mut self) {
        self.version += 1;
        self.topo = None;
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn output_node(&self) -> NodeId {
        self.output
    }

    pub fn node(&self, id: NodeId) -> Option<&NodeInstance> {
        self.nodes.iter().find(|n| n.id == id)
    }

    #[cfg(test)]
    pub fn wires(&self) -> &[Wire] {
        &self.wires
    }

    /// Add a node. `inputs` and `defs` come from the effect manifest; params
    /// are instantiated at their defaults.
    pub fn add_node(&mut self, kind: NodeKind, inputs: u8, defs: &[ParamDef]) -> NodeId {
        let id = NodeId(self.next_id);
        self.next_id += 1;
        let mut params = crate::params::ParamStore::new();
        params.load_from_defs(defs);
        self.nodes.push(NodeInstance {
            id,
            kind,
            inputs,
            params,
            mods: Vec::new(),
            bypass: false,
        });
        self.touch();
        id
    }

    /// Remove a node and every wire touching it. The Output node is refused.
    pub fn remove_node(&mut self, id: NodeId) -> Result<(), GraphError> {
        if id == self.output {
            return Err(GraphError::OutputImmortal);
        }
        if self.node(id).is_none() {
            return Err(GraphError::UnknownNode);
        }
        self.nodes.retain(|n| n.id != id);
        self.wires.retain(|w| w.from != id && w.to != id);
        self.touch();
        Ok(())
    }

    /// Wire `from`'s output into `to`'s input pin. An occupied pin is
    /// replaced (VJ-friendly rewiring); a connection that would close a cycle
    /// is refused and the graph is left untouched.
    pub fn connect(&mut self, from: NodeId, to: NodeId, to_input: u8) -> Result<(), GraphError> {
        if from == to {
            return Err(GraphError::SelfLoop);
        }
        let from_node = self.node(from).ok_or(GraphError::UnknownNode)?;
        if matches!(from_node.kind, NodeKind::Output) {
            return Err(GraphError::NoOutputPin);
        }
        let to_node = self.node(to).ok_or(GraphError::UnknownNode)?;
        if to_input >= to_node.inputs {
            return Err(GraphError::BadInput { input: to_input });
        }
        // The new edge from→to closes a cycle iff `from` is already reachable
        // downstream of `to`. DFS over existing wires; O(nodes + wires) per
        // edit is nothing at canvas scale.
        if self.reaches(to, from) {
            return Err(GraphError::Cycle);
        }
        self.wires
            .retain(|w| !(w.to == to && w.to_input == to_input));
        self.wires.push(Wire { from, to, to_input });
        self.touch();
        Ok(())
    }

    /// Remove the wire into `to`'s input pin, if any.
    pub fn disconnect(&mut self, to: NodeId, to_input: u8) {
        let before = self.wires.len();
        self.wires
            .retain(|w| !(w.to == to && w.to_input == to_input));
        if self.wires.len() != before {
            self.touch();
        }
    }

    /// The producer wired into `node`'s input pin, if any.
    pub fn input_source(&self, node: NodeId, input: u8) -> Option<NodeId> {
        self.wires
            .iter()
            .find(|w| w.to == node && w.to_input == input)
            .map(|w| w.from)
    }

    /// Toggle-style bypass setter; a structural edit because the execution
    /// plan changes (the node's pass disappears and its consumers re-alias).
    pub fn set_bypass(&mut self, id: NodeId, bypass: bool) -> Result<(), GraphError> {
        let node = self
            .nodes
            .iter_mut()
            .find(|n| n.id == id)
            .ok_or(GraphError::UnknownNode)?;
        if node.bypass != bypass {
            node.bypass = bypass;
            self.touch();
        }
        Ok(())
    }

    /// Mutable access to one node's non-structural state (base values +
    /// modulations). Deliberately NOT a `node_mut()`: structural fields
    /// (kind, inputs, bypass) stay behind the versioned API, because
    /// param/mod edits must not bump `version` — a replan rebuilds every
    /// bind group and reassigns the texture pool (I8).
    #[allow(dead_code)] // inspector, commit 4
    pub fn params_mut(&mut self, id: NodeId) -> Option<NodeParamsMut<'_>> {
        self.nodes
            .iter_mut()
            .find(|n| n.id == id)
            .map(NodeParamsMut::of)
    }

    /// Per-frame iteration over every node's non-structural state, for the
    /// modulation resolve pass. Orphans included — their oscillator phases
    /// stay warm across rewires (and `live_set()` would allocate).
    pub fn params_iter_mut(&mut self) -> impl Iterator<Item = NodeParamsMut<'_>> {
        self.nodes.iter_mut().map(NodeParamsMut::of)
    }

    /// Install, replace, or (with `None`) remove the modulation on one
    /// param. At most one slot per param; replacing keeps the runtime state
    /// so oscillator phase stays warm across config tweaks. Non-structural:
    /// no version bump.
    #[allow(dead_code)] // inspector, commit 4
    pub fn set_modulation(
        &mut self,
        id: NodeId,
        param: &str,
        config: Option<Modulation>,
    ) -> Result<(), GraphError> {
        let node = self
            .nodes
            .iter_mut()
            .find(|n| n.id == id)
            .ok_or(GraphError::UnknownNode)?;
        match config {
            Some(config) => {
                if let Some(existing) = node.mods.iter_mut().find(|m| m.param == param) {
                    existing.config = config;
                } else {
                    node.mods.push(ParamMod::new(node.id, param, config));
                }
            }
            None => node.mods.retain(|m| m.param != param),
        }
        Ok(())
    }

    /// True if `target` is reachable from `start` walking producer→consumer.
    fn reaches(&self, start: NodeId, target: NodeId) -> bool {
        let mut seen: HashSet<NodeId> = HashSet::new();
        let mut queue = VecDeque::from([start]);
        while let Some(n) = queue.pop_front() {
            if n == target {
                return true;
            }
            for w in self.wires.iter().filter(|w| w.from == n) {
                if seen.insert(w.to) {
                    queue.push_back(w.to);
                }
            }
        }
        false
    }

    /// Topological order over *all* nodes (orphans included), cached until the
    /// next structural edit. Kahn's algorithm; insertion order breaks ties so
    /// the result is deterministic.
    pub fn topo_order(&mut self) -> &[NodeId] {
        if self.topo.is_none() {
            let mut indegree: Vec<usize> = self
                .nodes
                .iter()
                .map(|n| self.wires.iter().filter(|w| w.to == n.id).count())
                .collect();
            let mut queue: VecDeque<usize> = (0..self.nodes.len())
                .filter(|&i| indegree[i] == 0)
                .collect();
            let mut order = Vec::with_capacity(self.nodes.len());
            while let Some(i) = queue.pop_front() {
                let id = self.nodes[i].id;
                order.push(id);
                for w in self.wires.iter().filter(|w| w.from == id) {
                    let j = self
                        .nodes
                        .iter()
                        .position(|n| n.id == w.to)
                        .expect("wire references a node that exists");
                    indegree[j] -= 1;
                    if indegree[j] == 0 {
                        queue.push_back(j);
                    }
                }
            }
            // `connect` refuses cycles, so a partial order here would mean a
            // broken invariant, not user input.
            debug_assert_eq!(order.len(), self.nodes.len(), "cycle in wire graph");
            self.topo = Some(order);
        }
        self.topo.as_deref().expect("just filled")
    }

    /// The nodes that actually feed the Output, in topological order. Orphan
    /// subgraphs are legal to author but excluded from execution (nothing
    /// renders them until previews land in M2).
    pub fn live_set(&mut self) -> Vec<NodeId> {
        let mut live: HashSet<NodeId> = HashSet::from([self.output]);
        let mut queue = VecDeque::from([self.output]);
        while let Some(n) = queue.pop_front() {
            for w in self.wires.iter().filter(|w| w.to == n) {
                if live.insert(w.from) {
                    queue.push_back(w.from);
                }
            }
        }
        self.topo_order()
            .iter()
            .copied()
            .filter(|id| live.contains(id))
            .collect()
    }

    /// Defensive re-check of the by-construction invariants, for the executor
    /// and tests.
    pub fn validate(&self) -> Result<(), GraphError> {
        let outputs = self
            .nodes
            .iter()
            .filter(|n| matches!(n.kind, NodeKind::Output))
            .count();
        if outputs != 1 || self.node(self.output).is_none() {
            return Err(GraphError::MissingOutput);
        }
        let mut pins: HashSet<(NodeId, u8)> = HashSet::new();
        for w in &self.wires {
            let from = self.node(w.from).ok_or(GraphError::UnknownNode)?;
            if matches!(from.kind, NodeKind::Output) {
                return Err(GraphError::NoOutputPin);
            }
            let to = self.node(w.to).ok_or(GraphError::UnknownNode)?;
            if w.to_input >= to.inputs {
                return Err(GraphError::BadInput { input: w.to_input });
            }
            if !pins.insert((w.to, w.to_input)) {
                return Err(GraphError::DuplicateWire);
            }
        }
        // Kahn without the cache: a leftover node means a cycle.
        let mut indegree: Vec<usize> = self
            .nodes
            .iter()
            .map(|n| self.wires.iter().filter(|w| w.to == n.id).count())
            .collect();
        let mut queue: VecDeque<usize> = (0..self.nodes.len())
            .filter(|&i| indegree[i] == 0)
            .collect();
        let mut visited = 0usize;
        while let Some(i) = queue.pop_front() {
            visited += 1;
            let id = self.nodes[i].id;
            for w in self.wires.iter().filter(|w| w.from == id) {
                let j = self
                    .nodes
                    .iter()
                    .position(|n| n.id == w.to)
                    .expect("checked above");
                indegree[j] -= 1;
                if indegree[j] == 0 {
                    queue.push_back(j);
                }
            }
        }
        if visited != self.nodes.len() {
            return Err(GraphError::Cycle);
        }
        Ok(())
    }

    #[cfg(test)]
    fn topo_cached(&self) -> bool {
        self.topo.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::super::effect::EffectId;
    use super::*;

    fn src(g: &mut NodeGraph) -> NodeId {
        g.add_node(
            NodeKind::Source {
                effect: EffectId("s".into()),
            },
            0,
            &[],
        )
    }

    fn eff(g: &mut NodeGraph, inputs: u8) -> NodeId {
        g.add_node(
            NodeKind::Effect {
                effect: EffectId("e".into()),
            },
            inputs,
            &[],
        )
    }

    #[test]
    fn topo_linear_chain_orders_source_first() {
        let mut g = NodeGraph::new_with_output();
        let s = src(&mut g);
        let e = eff(&mut g, 1);
        let out = g.output_node();
        g.connect(s, e, 0).unwrap();
        g.connect(e, out, 0).unwrap();
        let order = g.topo_order();
        let pos = |id| order.iter().position(|&n| n == id).unwrap();
        assert!(pos(s) < pos(e));
        assert!(pos(e) < pos(out));
    }

    #[test]
    fn topo_diamond_orders_dependencies_before_dependents() {
        let mut g = NodeGraph::new_with_output();
        let s = src(&mut g);
        let a = eff(&mut g, 1);
        let b = eff(&mut g, 1);
        let mix = eff(&mut g, 2);
        let out = g.output_node();
        g.connect(s, a, 0).unwrap();
        g.connect(s, b, 0).unwrap();
        g.connect(a, mix, 0).unwrap();
        g.connect(b, mix, 1).unwrap();
        g.connect(mix, out, 0).unwrap();
        let order = g.topo_order();
        let pos = |id| order.iter().position(|&n| n == id).unwrap();
        assert!(pos(s) < pos(a) && pos(s) < pos(b));
        assert!(pos(a) < pos(mix) && pos(b) < pos(mix));
        assert!(pos(mix) < pos(out));
    }

    #[test]
    fn connect_rejects_cycle() {
        let mut g = NodeGraph::new_with_output();
        let a = eff(&mut g, 1);
        let b = eff(&mut g, 1);
        g.connect(a, b, 0).unwrap();
        assert_eq!(g.connect(b, a, 0), Err(GraphError::Cycle));
        // Longer loop through a third node.
        let c = eff(&mut g, 1);
        g.connect(b, c, 0).unwrap();
        assert_eq!(g.connect(c, a, 0), Err(GraphError::Cycle));
        g.validate().unwrap();
    }

    #[test]
    fn connect_rejects_self_loop() {
        let mut g = NodeGraph::new_with_output();
        let a = eff(&mut g, 1);
        assert_eq!(g.connect(a, a, 0), Err(GraphError::SelfLoop));
    }

    #[test]
    fn connect_rejects_out_of_range_input() {
        let mut g = NodeGraph::new_with_output();
        let s = src(&mut g);
        let e = eff(&mut g, 1);
        assert_eq!(g.connect(s, e, 1), Err(GraphError::BadInput { input: 1 }));
    }

    #[test]
    fn connect_rejects_wire_into_source() {
        let mut g = NodeGraph::new_with_output();
        let s1 = src(&mut g);
        let s2 = src(&mut g);
        assert_eq!(g.connect(s1, s2, 0), Err(GraphError::BadInput { input: 0 }));
    }

    #[test]
    fn connect_rejects_output_as_producer() {
        let mut g = NodeGraph::new_with_output();
        let e = eff(&mut g, 1);
        let out = g.output_node();
        assert_eq!(g.connect(out, e, 0), Err(GraphError::NoOutputPin));
    }

    #[test]
    fn connect_replaces_existing_wire_on_input() {
        let mut g = NodeGraph::new_with_output();
        let s1 = src(&mut g);
        let s2 = src(&mut g);
        let out = g.output_node();
        g.connect(s1, out, 0).unwrap();
        g.connect(s2, out, 0).unwrap();
        assert_eq!(g.input_source(out, 0), Some(s2));
        assert_eq!(g.wires().len(), 1);
        g.validate().unwrap();
    }

    #[test]
    fn remove_output_refused() {
        let mut g = NodeGraph::new_with_output();
        let out = g.output_node();
        assert_eq!(g.remove_node(out), Err(GraphError::OutputImmortal));
        assert!(g.node(out).is_some());
    }

    #[test]
    fn remove_node_drops_adjacent_wires() {
        let mut g = NodeGraph::new_with_output();
        let s = src(&mut g);
        let e = eff(&mut g, 1);
        let out = g.output_node();
        g.connect(s, e, 0).unwrap();
        g.connect(e, out, 0).unwrap();
        g.remove_node(e).unwrap();
        assert!(g.wires().is_empty());
        g.validate().unwrap();
    }

    #[test]
    fn orphan_subgraph_validates_but_excluded_from_live_set() {
        let mut g = NodeGraph::new_with_output();
        let s = src(&mut g);
        let e = eff(&mut g, 1);
        let out = g.output_node();
        g.connect(s, out, 0).unwrap();
        // `e` dangles, wired to nothing.
        let _ = e;
        g.validate().unwrap();
        let live = g.live_set();
        assert!(live.contains(&s) && live.contains(&out));
        assert!(!live.contains(&e));
    }

    #[test]
    fn version_bumps_on_every_structural_edit() {
        let mut g = NodeGraph::new_with_output();
        let v0 = g.version();
        let s = src(&mut g);
        assert!(g.version() > v0);
        let out = g.output_node();
        let v1 = g.version();
        g.connect(s, out, 0).unwrap();
        assert!(g.version() > v1);
        let v2 = g.version();
        g.disconnect(out, 0);
        assert!(g.version() > v2);
        let v3 = g.version();
        g.set_bypass(s, true).unwrap();
        assert!(g.version() > v3);
        // Idempotent bypass write is not an edit.
        let v4 = g.version();
        g.set_bypass(s, true).unwrap();
        assert_eq!(g.version(), v4);
        // A refused edit is not an edit.
        assert_eq!(g.connect(s, s, 0), Err(GraphError::SelfLoop));
        assert_eq!(g.version(), v4);
    }

    #[test]
    fn topo_cache_reused_when_clean() {
        let mut g = NodeGraph::new_with_output();
        let s = src(&mut g);
        let out = g.output_node();
        g.connect(s, out, 0).unwrap();
        assert!(!g.topo_cached());
        let first: Vec<NodeId> = g.topo_order().to_vec();
        assert!(g.topo_cached());
        assert_eq!(g.topo_order(), first.as_slice());
        g.disconnect(out, 0);
        assert!(!g.topo_cached());
    }

    fn any_modulation() -> Modulation {
        use crate::trama::audio::AudioFeature;
        use crate::trama::modulation::{ModMode, ModSource};
        Modulation {
            source: ModSource::Audio(AudioFeature::Rms),
            amount: 0.5,
            mode: ModMode::Add,
            smoothing: 0.0,
        }
    }

    #[test]
    fn param_and_modulation_edits_do_not_bump_version() {
        let mut g = NodeGraph::new_with_output();
        let s = src(&mut g);
        let v = g.version();
        let node = g.params_mut(s).unwrap();
        node.params.set("x", crate::params::ParamValue::Float(0.9));
        g.set_modulation(s, "x", Some(any_modulation())).unwrap();
        g.set_modulation(s, "x", None).unwrap();
        for _ in g.params_iter_mut() {}
        assert_eq!(g.version(), v, "non-structural edits must not replan");
    }

    #[test]
    fn set_modulation_keeps_one_slot_per_param() {
        let mut g = NodeGraph::new_with_output();
        let s = src(&mut g);
        g.set_modulation(s, "x", Some(any_modulation())).unwrap();
        g.set_modulation(s, "x", Some(any_modulation())).unwrap();
        assert_eq!(g.params_mut(s).unwrap().mods.len(), 1);
        g.set_modulation(s, "y", Some(any_modulation())).unwrap();
        assert_eq!(g.params_mut(s).unwrap().mods.len(), 2);
        assert_eq!(
            g.set_modulation(NodeId(999), "x", Some(any_modulation())),
            Err(GraphError::UnknownNode)
        );
    }

    #[test]
    fn remove_node_drops_its_modulations() {
        let mut g = NodeGraph::new_with_output();
        let s = src(&mut g);
        g.set_modulation(s, "x", Some(any_modulation())).unwrap();
        g.remove_node(s).unwrap();
        assert!(g.params_mut(s).is_none(), "mods die with the node");
    }
}
