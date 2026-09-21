//! The `.fio.json` document: one trama chain as data (handoff §11, I3).
//!
//! A [`ChainDoc`] is the complete authored state of ONE chain — nodes, wires,
//! parameter values, modulation routings, canvas positions. The same document
//! is a standalone `.fio.json` file and the `chain` field of a layer inside a
//! preset, so a chain means the same thing wherever it is stored and carries
//! its own version either way.
//!
//! The document types here are deliberately NOT the runtime types. `NodeGraph`
//! and `NodeInstance` hold caches, runtime modulation state and a version
//! counter, and are free to change shape; the file format is not. Only the
//! small modulation *config* enums are shared, and the golden test below pins
//! their spelling, so renaming a variant breaks a test rather than every
//! saved patch.
//!
//! Loading trusts nothing: a document is a file someone may have edited, made
//! on another machine, or written with a different set of effects installed.
//! What can be repaired is repaired and reported in [`Restored::notes`] (an
//! effect that is not installed, a parameter the effect no longer has, a wire
//! into a pin that no longer exists); what cannot is an error and the chain is
//! left alone. Nothing is dropped silently.

use serde::{Deserialize, Serialize};

use crate::params::{ParamDef, ParamStore, ParamValue};

use super::effect::EffectId;
use super::graph::{GraphError, NodeGraph, Wire};
use super::modulation::{Modulation, ParamMod};
use super::node::{NodeId, NodeInstance, NodeKind};

/// The newest format this build understands. Bump it with a migration in
/// [`migrate`].
pub const TRAMA_VERSION: u32 = 2;

/// The version written into a document is the OLDEST build that can read it
/// back, not the build that wrote it — so a chain using nothing new still
/// says `1` and still opens in an older Fosfora. `.fio.json` files are made
/// to travel between people, and stamping every save with the current build's
/// number would strand anchor-free chains on the release that wrote them for
/// no reason.
const BASE_VERSION: u32 = 1;

/// [`KindDoc::Anchor`] is the first thing that an older build cannot parse.
/// A document holding one says `2`, which that build reports as "saved by a
/// newer Fosfora" instead of failing on an unknown enum variant.
const ANCHOR_VERSION: u32 = 2;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChainDoc {
    pub trama_version: u32,
    pub graph: GraphDoc,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphDoc {
    pub nodes: Vec<NodeDoc>,
    pub wires: Vec<WireDoc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KindDoc {
    Source,
    Effect,
    Feedback,
    LayerInput,
    Anchor,
    Output,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeDoc {
    pub id: u64,
    pub kind: KindDoc,
    /// The effect id, for `source` and `effect` nodes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect: Option<String>,
    /// Input-pin count AS SAVED. For an installed effect the manifest wins on
    /// load; this is what lets a node whose effect is NOT installed keep its
    /// pins, and therefore its wires.
    pub inputs: u8,
    pub pos: [f32; 2],
    #[serde(default)]
    pub bypass: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub params: Vec<ParamDoc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParamDoc {
    pub name: String,
    pub value: ValueDoc,
    #[serde(default, rename = "mod", skip_serializing_if = "Option::is_none")]
    pub modulation: Option<Modulation>,
}

/// A parameter value, told apart by SHAPE rather than by a tag: a number, a
/// bool, `[x, y]`, `[r, g, b, a]`. Friendlier to read and to edit by hand than
/// `{"Float": 0.3}`, and unambiguous — the four shapes cannot be confused.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ValueDoc {
    Float(f32),
    Bool(bool),
    Point2D([f32; 2]),
    Color([f32; 4]),
}

/// `[node id, pin]` at each end. Every node has exactly one output, so the
/// `from` pin is always 0; it is written anyway so the format does not have to
/// change on the day a node grows a second output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireDoc {
    pub from: (u64, u8),
    pub to: (u64, u8),
}

/// What loading needs to know about an installed effect. A plain lookup
/// function rather than `&TramaRegistry`, because the registry holds GPU
/// pipelines and this module's tests should not need a device.
pub struct EffectShape<'a> {
    pub inputs: u8,
    pub params: &'a [ParamDef],
}

#[derive(Debug, PartialEq, thiserror::Error)]
pub enum LoadError {
    #[error("not a trama chain: {0}")]
    Parse(String),
    #[error(
        "this chain was saved by a newer Fosfora (format {found}, this build reads up to {TRAMA_VERSION})"
    )]
    TooNew { found: u32 },
    #[error("unsupported chain format version {0}")]
    BadVersion(u32),
    #[error("node {node} is a {kind:?} node with no effect id")]
    NoEffectId { node: u64, kind: KindDoc },
    #[error("a wire leaves output pin {pin} of node {node}, but nodes have one output")]
    BadOutputPin { node: u64, pin: u8 },
    #[error("the chain is not a valid graph: {0}")]
    Graph(#[from] GraphError),
}

/// A chain brought back from a document.
pub struct Restored {
    pub graph: NodeGraph,
    /// Canvas position per node — owned by the canvas view, not the graph.
    pub positions: Vec<(NodeId, [f32; 2])>,
    /// Everything that was repaired on the way in, in words. Empty for a
    /// document that loaded exactly as written.
    pub notes: Vec<String>,
}

/// serde_json writes a non-finite float as `null`, and `null` does not read
/// back as a float — one NaN parameter would make the whole document (and the
/// preset around it) unloadable. A round-trip test cannot see that, because
/// the failure is in what gets WRITTEN.
fn finite(v: f32) -> f32 {
    if v.is_finite() { v } else { 0.0 }
}

impl From<&ParamValue> for ValueDoc {
    fn from(v: &ParamValue) -> Self {
        match *v {
            ParamValue::Float(f) => ValueDoc::Float(finite(f)),
            ParamValue::Bool(b) => ValueDoc::Bool(b),
            ParamValue::Point2D(p) => ValueDoc::Point2D(p.map(finite)),
            ParamValue::Color(c) => ValueDoc::Color(c.map(finite)),
        }
    }
}

impl From<ValueDoc> for ParamValue {
    fn from(v: ValueDoc) -> Self {
        match v {
            ValueDoc::Float(f) => ParamValue::Float(f),
            ValueDoc::Bool(b) => ParamValue::Bool(b),
            ValueDoc::Point2D(p) => ParamValue::Point2D(p),
            ValueDoc::Color(c) => ParamValue::Color(c),
        }
    }
}

fn same_type(a: &ParamValue, b: &ParamValue) -> bool {
    std::mem::discriminant(a) == std::mem::discriminant(b)
}

impl ChainDoc {
    /// Capture a chain. `position` answers for the canvas, which owns node
    /// positions; a node it does not know lands at the origin.
    pub fn capture(graph: &NodeGraph, position: impl Fn(NodeId) -> Option<[f32; 2]>) -> Self {
        let nodes: Vec<NodeDoc> = graph
            .nodes()
            .iter()
            .map(|n| {
                let (kind, effect) = match &n.kind {
                    NodeKind::Source { effect } => (KindDoc::Source, Some(effect.0.clone())),
                    NodeKind::Effect { effect } => (KindDoc::Effect, Some(effect.0.clone())),
                    NodeKind::Feedback => (KindDoc::Feedback, None),
                    NodeKind::ChainInput => (KindDoc::LayerInput, None),
                    NodeKind::Anchor => (KindDoc::Anchor, None),
                    NodeKind::Output => (KindDoc::Output, None),
                };
                // Manifest order first, so a file diffs cleanly; then any
                // value the manifest does not name (a missing effect has no
                // manifest at all), sorted, so the order is never the
                // HashMap's.
                let mut names: Vec<&str> = n.params.defs.iter().map(|d| d.name()).collect();
                let mut extra: Vec<&str> = n
                    .params
                    .values
                    .keys()
                    .map(String::as_str)
                    .filter(|k| !names.contains(k))
                    .collect();
                extra.sort_unstable();
                names.extend(extra);
                let params = names
                    .into_iter()
                    .filter_map(|name| {
                        let value = n.params.values.get(name)?;
                        Some(ParamDoc {
                            name: name.to_string(),
                            value: value.into(),
                            modulation: n.mods.iter().find(|m| m.param == name).map(|m| {
                                let mut c = m.config;
                                c.amount = finite(c.amount);
                                c.smoothing = finite(c.smoothing);
                                c
                            }),
                        })
                    })
                    .collect();
                NodeDoc {
                    id: n.id.0,
                    kind,
                    effect,
                    inputs: n.inputs,
                    pos: position(n.id).unwrap_or_default().map(finite),
                    bypass: n.bypass,
                    params,
                }
            })
            .collect();
        let wires = graph
            .wires()
            .iter()
            .map(|w| WireDoc {
                from: (w.from.0, 0),
                to: (w.to.0, w.to_input),
            })
            .collect();
        let trama_version = if nodes.iter().any(|n| n.kind == KindDoc::Anchor) {
            ANCHOR_VERSION
        } else {
            BASE_VERSION
        };
        Self {
            trama_version,
            graph: GraphDoc { nodes, wires },
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("a ChainDoc is plain data")
    }

    pub fn from_json(text: &str) -> Result<Self, LoadError> {
        let doc: Self = serde_json::from_str(text).map_err(|e| LoadError::Parse(e.to_string()))?;
        migrate(doc)
    }

    /// Rebuild the chain. `lookup` reports an installed effect's pin count and
    /// parameters, or `None` for one that is not installed — that node comes
    /// back as a placeholder with its saved pins, values and modulations kept
    /// verbatim, so saving again loses nothing.
    pub fn restore<'a>(
        &self,
        lookup: impl Fn(&EffectId) -> Option<EffectShape<'a>>,
    ) -> Result<Restored, LoadError> {
        let doc = migrate(self.clone())?;
        let mut notes = Vec::new();
        let mut nodes = Vec::with_capacity(doc.graph.nodes.len());
        let mut positions = Vec::with_capacity(doc.graph.nodes.len());

        for nd in &doc.graph.nodes {
            let id = NodeId(nd.id);
            let effect_id = || {
                nd.effect
                    .clone()
                    .map(EffectId)
                    .ok_or(LoadError::NoEffectId {
                        node: nd.id,
                        kind: nd.kind,
                    })
            };
            let kind = match nd.kind {
                KindDoc::Source => NodeKind::Source {
                    effect: effect_id()?,
                },
                KindDoc::Effect => NodeKind::Effect {
                    effect: effect_id()?,
                },
                KindDoc::Feedback => NodeKind::Feedback,
                KindDoc::LayerInput => NodeKind::ChainInput,
                KindDoc::Anchor => NodeKind::Anchor,
                KindDoc::Output => NodeKind::Output,
            };

            let mut params = ParamStore::new();
            for p in &nd.params {
                params.values.insert(p.name.clone(), p.value.into());
            }
            let inputs = match &kind {
                NodeKind::Output | NodeKind::Feedback | NodeKind::Anchor => 1,
                NodeKind::ChainInput => 0,
                NodeKind::Source { effect } | NodeKind::Effect { effect } => {
                    match lookup(effect) {
                        Some(shape) => {
                            // Manifest merge: a value survives if the effect
                            // still has a parameter of that name and type.
                            for p in &nd.params {
                                let def = shape.params.iter().find(|d| d.name() == p.name);
                                match def {
                                    None => notes.push(format!(
                                        "{}: `{}` is no longer a parameter; its value was dropped",
                                        effect.0, p.name
                                    )),
                                    Some(d) if !same_type(&d.default_value(), &p.value.into()) => {
                                        notes.push(format!(
                                            "{}: `{}` changed type; reset to its default",
                                            effect.0, p.name
                                        ));
                                    }
                                    Some(_) => {}
                                }
                            }
                            params.merge_from_defs(shape.params);
                            params.changed = false;
                            shape.inputs
                        }
                        None => {
                            notes.push(format!(
                                "effect `{}` is not installed; the node is kept as a placeholder",
                                effect.0
                            ));
                            nd.inputs
                        }
                    }
                }
            };

            let mods = nd
                .params
                .iter()
                .filter_map(|p| Some((p, p.modulation?)))
                .filter(|(p, _)| {
                    let kept = params.values.contains_key(&p.name);
                    if !kept {
                        notes.push(format!(
                            "the modulation on `{}` went with its parameter",
                            p.name
                        ));
                    }
                    kept
                })
                .map(|(p, config)| ParamMod::new(id, p.name.clone(), config))
                .collect();

            positions.push((id, nd.pos));
            nodes.push(NodeInstance {
                id,
                kind,
                inputs,
                params,
                mods,
                bypass: nd.bypass,
                phases: Vec::new(),
            });
        }

        let mut wires = Vec::with_capacity(doc.graph.wires.len());
        for w in &doc.graph.wires {
            if w.from.1 != 0 {
                return Err(LoadError::BadOutputPin {
                    node: w.from.0,
                    pin: w.from.1,
                });
            }
            // An effect that lost an input pin since the save: the wire has
            // nowhere to go. Every other wire problem is a corrupt document
            // and is left for `from_parts` to reject.
            let pins = nodes.iter().find(|n| n.id.0 == w.to.0).map(|n| n.inputs);
            if pins.is_some_and(|pins| w.to.1 >= pins) {
                notes.push(format!(
                    "a wire into input {} of node {} was dropped: the effect no longer has that input",
                    w.to.1, w.to.0
                ));
                continue;
            }
            wires.push(Wire {
                from: NodeId(w.from.0),
                to: NodeId(w.to.0),
                to_input: w.to.1,
            });
        }

        Ok(Restored {
            graph: NodeGraph::from_parts(nodes, wires)?,
            positions,
            notes,
        })
    }
}

/// Bring a document up to [`TRAMA_VERSION`]. v1 is the first format, so there
/// is nothing to lift yet — the scaffold is here so that the first real
/// migration is one match arm, not a redesign under deadline. Each arm lifts
/// ONE version and recurses: `1 => migrate(v1_to_v2(doc))`.
fn migrate(doc: ChainDoc) -> Result<ChainDoc, LoadError> {
    match doc.trama_version {
        // v1 and v2 describe the same structure; v2 only marks a document
        // that uses a node kind v1 readers do not know, so neither needs
        // lifting to be read HERE. A real migration takes an arm of its own.
        BASE_VERSION | ANCHOR_VERSION => Ok(doc),
        found if found > TRAMA_VERSION => Err(LoadError::TooNew { found }),
        v => Err(LoadError::BadVersion(v)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trama::audio::AudioFeature;
    use crate::trama::modulation::{BeatDiv, ModMode, ModSource, Osc, OscRate, OscShape};

    fn float(name: &str, default: f32) -> ParamDef {
        serde_json::from_value(serde_json::json!({
            "type": "Float", "name": name, "default": default, "min": 0.0, "max": 4.0
        }))
        .expect("a Float ParamDef")
    }

    /// A stand-in registry: `hue_drift` (1 input), `noise_field` (source),
    /// `mix` (2 inputs). Anything else is not installed.
    struct Fake {
        hue: Vec<ParamDef>,
        noise: Vec<ParamDef>,
        mix: Vec<ParamDef>,
    }

    impl Fake {
        fn new() -> Self {
            Self {
                hue: vec![float("shift", 0.0), float("speed", 0.2)],
                noise: vec![float("scale", 3.0)],
                mix: vec![float("amount", 0.5)],
            }
        }

        fn lookup(&self, id: &EffectId) -> Option<EffectShape<'_>> {
            let (inputs, params) = match id.0.as_str() {
                "hue_drift" => (1, &self.hue),
                "noise_field" => (0, &self.noise),
                "mix" => (2, &self.mix),
                _ => return None,
            };
            Some(EffectShape { inputs, params })
        }
    }

    fn effect(id: &str) -> NodeKind {
        NodeKind::Effect {
            effect: EffectId(id.into()),
        }
    }

    /// noise -> hue (shift 0.3, driven by bass) -> output.
    fn small_patch(fake: &Fake) -> (NodeGraph, NodeId, NodeId) {
        let mut g = NodeGraph::new_with_output();
        let n = g.add_node(
            NodeKind::Source {
                effect: EffectId("noise_field".into()),
            },
            0,
            &fake.noise,
        );
        let h = g.add_node(effect("hue_drift"), 1, &fake.hue);
        g.params_mut(h)
            .unwrap()
            .params
            .set("shift", ParamValue::Float(0.3));
        g.set_modulation(
            h,
            "shift",
            Some(Modulation {
                source: ModSource::Audio(AudioFeature::Bass),
                amount: 0.6,
                mode: ModMode::Add,
                smoothing: 0.4,
            }),
        )
        .unwrap();
        let out = g.output_node();
        g.connect(n, h, 0).unwrap();
        g.connect(h, out, 0).unwrap();
        (g, n, h)
    }

    #[allow(clippy::unnecessary_wraps)] // the shape `capture` asks for
    fn layout(id: NodeId) -> Option<[f32; 2]> {
        Some([100.0 * id.0 as f32, 50.0])
    }

    /// Everything authored about a graph, in a comparable form.
    fn authored(g: &NodeGraph) -> Vec<String> {
        let mut out: Vec<String> = g
            .nodes()
            .iter()
            .map(|n| {
                let mut values: Vec<String> = n
                    .params
                    .values
                    .iter()
                    .map(|(k, v)| format!("{k}={v:?}"))
                    .collect();
                values.sort();
                let mods: Vec<String> = n
                    .mods
                    .iter()
                    .map(|m| format!("{}:{:?}", m.param, m.config))
                    .collect();
                format!(
                    "{:?} {:?} in={} bypass={} {values:?} {mods:?}",
                    n.id, n.kind, n.inputs, n.bypass
                )
            })
            .collect();
        out.extend(g.wires().iter().map(|w| format!("{w:?}")));
        out
    }

    #[test]
    fn a_chain_round_trips_through_json() {
        let fake = Fake::new();
        let (mut g, _, h) = small_patch(&fake);
        g.set_bypass(h, true).unwrap();
        let doc = ChainDoc::capture(&g, layout);
        let back = ChainDoc::from_json(&doc.to_json()).expect("reads its own writing");
        assert_eq!(back, doc, "the document survives the text");

        let restored = back.restore(|id| fake.lookup(id)).expect("restores");
        assert_eq!(authored(&restored.graph), authored(&g));
        assert_eq!(
            restored.notes,
            Vec::<String>::new(),
            "nothing needed repair"
        );
        assert_eq!(
            restored.positions,
            g.nodes()
                .iter()
                .map(|n| (n.id, layout(n.id).unwrap()))
                .collect::<Vec<_>>()
        );
        // And a second trip is a fixed point: saving what was loaded writes
        // the same bytes.
        let again = ChainDoc::capture(&restored.graph, layout);
        assert_eq!(again.to_json(), doc.to_json());
    }

    // The WRITER's shape, pinned. A round trip proves only that the reader
    // understands the writer; it says nothing about whether either of them
    // still speaks the format other people's files are in. Renaming a variant
    // of ModMode or AudioFeature passes every round-trip test and breaks every
    // saved patch — it fails here instead.
    #[test]
    fn the_written_format_is_pinned() {
        let fake = Fake::new();
        let (g, _, _) = small_patch(&fake);
        let expected = r#"{
  "trama_version": 1,
  "graph": {
    "nodes": [
      {
        "id": 0,
        "kind": "output",
        "inputs": 1,
        "pos": [
          0.0,
          50.0
        ],
        "bypass": false
      },
      {
        "id": 1,
        "kind": "source",
        "effect": "noise_field",
        "inputs": 0,
        "pos": [
          100.0,
          50.0
        ],
        "bypass": false,
        "params": [
          {
            "name": "scale",
            "value": 3.0
          }
        ]
      },
      {
        "id": 2,
        "kind": "effect",
        "effect": "hue_drift",
        "inputs": 1,
        "pos": [
          200.0,
          50.0
        ],
        "bypass": false,
        "params": [
          {
            "name": "shift",
            "value": 0.3,
            "mod": {
              "source": {
                "audio": "bass"
              },
              "amount": 0.6,
              "mode": "add",
              "smoothing": 0.4
            }
          },
          {
            "name": "speed",
            "value": 0.2
          }
        ]
      }
    ],
    "wires": [
      {
        "from": [
          1,
          0
        ],
        "to": [
          2,
          0
        ]
      },
      {
        "from": [
          2,
          0
        ],
        "to": [
          0,
          0
        ]
      }
    ]
  }
}"#;
        assert_eq!(ChainDoc::capture(&g, layout).to_json(), expected);
    }

    // Every modulation source and value shape, spelled the way files spell
    // them — read from a literal, not from our own writer.
    #[test]
    fn every_source_and_value_shape_reads_from_a_literal() {
        let text = r#"{"trama_version":1,"graph":{"nodes":[
          {"id":0,"kind":"output","inputs":1,"pos":[0,0]},
          {"id":3,"kind":"effect","effect":"elsewhere","inputs":2,"pos":[1,2],"params":[
            {"name":"a","value":1.5,"mod":{"source":{"audio":{"band":7}},"amount":-1.0,"mode":"multiply","smoothing":0.0}},
            {"name":"b","value":true},
            {"name":"c","value":[0.25,0.75]},
            {"name":"d","value":[1,0,1,1],"mod":{"source":{"oscillator":{"shape":"sample_hold","rate":{"beat_sync":"four_bars"},"phase":0.5}},"amount":1.0,"mode":"replace","smoothing":1.0}},
            {"name":"e","value":0,"mod":{"source":{"oscillator":{"shape":"drift","rate":{"hz":0.25},"phase":0.0}},"amount":0.1,"mode":"add","smoothing":0.2}},
            {"name":"f","value":0,"mod":{"source":{"audio":"beat_phase"},"amount":0.1,"mode":"add","smoothing":0.2}}
          ]}],"wires":[]}}"#;
        let doc = ChainDoc::from_json(text).expect("the documented spelling parses");
        let r = doc.restore(|_| None).expect("restores");
        let n = r.graph.node(NodeId(3)).unwrap();
        assert!(matches!(n.params.get("a"), Some(ParamValue::Float(v)) if *v == 1.5));
        assert!(matches!(n.params.get("b"), Some(ParamValue::Bool(true))));
        assert!(
            matches!(n.params.get("c"), Some(ParamValue::Point2D([x, y])) if (*x, *y) == (0.25, 0.75))
        );
        assert!(matches!(
            n.params.get("d"),
            Some(ParamValue::Color([1.0, 0.0, 1.0, 1.0]))
        ));
        let src = |name: &str| n.mods.iter().find(|m| m.param == name).unwrap().config;
        assert_eq!(src("a").source, ModSource::Audio(AudioFeature::Band(7)));
        assert_eq!(src("a").mode, ModMode::Multiply);
        assert_eq!(
            src("d").source,
            ModSource::Oscillator(Osc {
                shape: OscShape::SampleHold,
                rate: OscRate::BeatSync(BeatDiv::FourBars),
                phase: 0.5
            })
        );
        assert_eq!(src("d").mode, ModMode::Replace);
        assert_eq!(
            src("e").source,
            ModSource::Oscillator(Osc {
                shape: OscShape::Drift,
                rate: OscRate::Hz(0.25),
                phase: 0.0
            })
        );
        assert_eq!(src("f").source, ModSource::Audio(AudioFeature::BeatPhase));
    }

    // I3/I4 on load: an effect that is not installed must not cost the user
    // anything they authored. The node keeps its saved pins (so its wires
    // survive), its values and its modulations, verbatim — and saving the
    // chain again writes the same document, so passing a patch through a
    // machine without the effect does not damage it.
    #[test]
    fn a_missing_effect_is_kept_verbatim() {
        let fake = Fake::new();
        let mut g = NodeGraph::new_with_output();
        let n = g.add_node(
            NodeKind::Source {
                effect: EffectId("noise_field".into()),
            },
            0,
            &fake.noise,
        );
        // Authored on a machine that HAS `kaleido`: two inputs, two params.
        let k = g.add_node(
            effect("kaleido"),
            2,
            // Deliberately NOT alphabetical: without a manifest the values
            // come back name-sorted, so the comparison at the end has to be
            // about content and must not pass by an accident of spelling.
            &[float("spin", 0.0), float("segments", 6.0)],
        );
        g.params_mut(k)
            .unwrap()
            .params
            .set("segments", ParamValue::Float(9.0));
        g.set_modulation(
            k,
            "spin",
            Some(Modulation {
                source: ModSource::Audio(AudioFeature::Rms),
                amount: 0.5,
                mode: ModMode::Add,
                smoothing: 0.1,
            }),
        )
        .unwrap();
        let out = g.output_node();
        g.connect(n, k, 1).unwrap();
        g.connect(k, out, 0).unwrap();
        let doc = ChainDoc::capture(&g, layout);

        // Loaded on a machine that does not.
        let r = doc.restore(|id| fake.lookup(id)).expect("still loads");
        let node = r.graph.node(k).expect("the node is kept");
        assert_eq!(
            node.inputs, 2,
            "pins come from the file when there is no manifest"
        );
        assert!(matches!(node.params.get("segments"), Some(ParamValue::Float(v)) if *v == 9.0));
        assert_eq!(node.mods.len(), 1, "its modulation is kept");
        assert_eq!(
            r.graph.wires().len(),
            2,
            "and so is the wire into its second pin"
        );
        assert_eq!(r.notes.len(), 1, "{:?}", r.notes);
        assert!(r.notes[0].contains("kaleido"), "{:?}", r.notes);

        let by_name = |mut d: ChainDoc| {
            for n in &mut d.graph.nodes {
                n.params.sort_by(|a, b| a.name.cmp(&b.name));
            }
            d
        };
        let resaved = ChainDoc::capture(&r.graph, layout);
        assert_ne!(resaved, doc, "parameter ORDER is not kept, only content");
        assert_eq!(
            by_name(resaved),
            by_name(doc),
            "nothing was lost in transit"
        );
    }

    // The manifest-merge half (handoff §12): the effect is installed, but its
    // manifest moved on since the save.
    #[test]
    fn a_changed_manifest_is_merged_by_name() {
        let fake = Fake::new();
        let text = r#"{"trama_version":1,"graph":{"nodes":[
          {"id":0,"kind":"output","inputs":1,"pos":[0,0]},
          {"id":1,"kind":"source","effect":"noise_field","inputs":0,"pos":[0,0]},
          {"id":2,"kind":"effect","effect":"hue_drift","inputs":3,"pos":[0,0],"params":[
            {"name":"shift","value":0.7},
            {"name":"gone","value":1.0,"mod":{"source":{"audio":"rms"},"amount":0.5,"mode":"add","smoothing":0.0}},
            {"name":"speed","value":true}
          ]}],
          "wires":[{"from":[1,0],"to":[2,0]},{"from":[1,0],"to":[2,2]},{"from":[2,0],"to":[0,0]}]}}"#;
        let r = ChainDoc::from_json(text)
            .unwrap()
            .restore(|id| fake.lookup(id))
            .expect("loads, repaired");
        let n = r.graph.node(NodeId(2)).unwrap();
        assert_eq!(n.inputs, 1, "the installed manifest wins on pin count");
        assert!(
            matches!(n.params.get("shift"), Some(ParamValue::Float(v)) if *v == 0.7),
            "kept"
        );
        assert!(
            matches!(n.params.get("speed"), Some(ParamValue::Float(v)) if *v == 0.2),
            "wrong type: default"
        );
        assert!(n.params.get("gone").is_none(), "removed parameter dropped");
        assert!(n.mods.is_empty(), "and its modulation with it");
        assert_eq!(
            r.graph.wires().len(),
            2,
            "the wire into the pin that no longer exists is dropped"
        );
        // Four repairs, four notes — nothing dropped silently.
        assert_eq!(r.notes.len(), 4, "{:#?}", r.notes);
    }

    #[test]
    fn a_non_finite_value_never_reaches_the_file() {
        let fake = Fake::new();
        let (mut g, _, h) = small_patch(&fake);
        g.params_mut(h)
            .unwrap()
            .params
            .set("shift", ParamValue::Float(f32::NAN));
        let text = ChainDoc::capture(&g, |_| Some([f32::INFINITY, 0.0])).to_json();
        assert!(
            !text.contains("null"),
            "serde_json writes NaN as null:\n{text}"
        );
        ChainDoc::from_json(&text).expect("and null does not read back as a float");
    }

    #[test]
    fn documents_that_are_not_graphs_are_refused() {
        let fake = Fake::new();
        let load = |text: &str| {
            ChainDoc::from_json(text).and_then(|d| d.restore(|id| fake.lookup(id)).map(|_| ()))
        };
        let wrap = |nodes: &str, wires: &str| {
            format!(r#"{{"trama_version":1,"graph":{{"nodes":[{nodes}],"wires":[{wires}]}}}}"#)
        };
        let out = r#"{"id":0,"kind":"output","inputs":1,"pos":[0,0]}"#;
        let hue = |id: u32| {
            format!(r#"{{"id":{id},"kind":"effect","effect":"hue_drift","inputs":1,"pos":[0,0]}}"#)
        };

        assert_eq!(
            load(&wrap("", "")),
            Err(LoadError::Graph(GraphError::MissingOutput))
        );
        assert_eq!(
            load(&wrap(&format!("{out},{out}"), "")),
            Err(LoadError::Graph(GraphError::DuplicateNode))
        );
        assert_eq!(
            load(&wrap(
                &format!("{out},{},{}", hue(1), hue(2)),
                r#"{"from":[1,0],"to":[2,0]},{"from":[2,0],"to":[1,0]}"#
            )),
            Err(LoadError::Graph(GraphError::Cycle))
        );
        assert_eq!(
            load(&wrap(out, r#"{"from":[9,0],"to":[0,0]}"#)),
            Err(LoadError::Graph(GraphError::UnknownNode))
        );
        assert_eq!(
            load(&wrap(
                &format!("{out},{}", hue(1)),
                r#"{"from":[1,1],"to":[0,0]}"#
            )),
            Err(LoadError::BadOutputPin { node: 1, pin: 1 })
        );
        assert_eq!(
            load(&wrap(
                &format!(r#"{out},{{"id":1,"kind":"effect","inputs":1,"pos":[0,0]}}"#),
                ""
            )),
            Err(LoadError::NoEffectId {
                node: 1,
                kind: KindDoc::Effect
            })
        );
        assert!(matches!(load("{}"), Err(LoadError::Parse(_))));
        // v2 is readable here (it only marks a document holding an Anchor),
        // so the "newer Fosfora" case is the one after it.
        assert_eq!(
            load(r#"{"trama_version":3,"graph":{"nodes":[],"wires":[]}}"#),
            Err(LoadError::TooNew { found: 3 })
        );
        assert_eq!(
            load(r#"{"trama_version":0,"graph":{"nodes":[],"wires":[]}}"#),
            Err(LoadError::BadVersion(0))
        );
    }

    // Ids are kept, not re-allocated: a graph with a gap from a removed node
    // comes back with the same ids, and the next node placed does not collide.
    // An anchor has to survive the round trip like any other node, and the
    // document has to ADMIT it holds one — a v1 reader cannot parse the
    // `anchor` kind, so a file carrying one must say 2 and get the "saved by
    // a newer Fosfora" message rather than a serde error about an unknown
    // variant. A chain without anchors must keep saying 1, or every plain
    // patch would stop opening in the previous release for nothing.
    #[test]
    fn a_document_says_2_only_when_it_actually_holds_an_anchor() {
        let fake = Fake::new();
        let mut g = NodeGraph::new_with_output();
        let out = g.output_node();
        let h = g.add_node(effect("hue_drift"), 1, &fake.hue);
        g.connect(h, out, 0).unwrap();
        assert_eq!(
            ChainDoc::capture(&g, layout).trama_version,
            BASE_VERSION,
            "nothing new in this chain, so it stays readable by older builds"
        );

        let a = g.add_node(NodeKind::Anchor, 1, &[]);
        g.connect(h, a, 0).unwrap();
        g.connect(a, out, 0).unwrap();
        let doc = ChainDoc::capture(&g, layout);
        assert_eq!(doc.trama_version, ANCHOR_VERSION);
        let json = doc.to_json();
        assert!(json.contains(r#""kind": "anchor""#), "{json}");

        // Round trip: the anchor comes back an anchor, with one input pin and
        // its wires, and is still not counted.
        let r = ChainDoc::from_json(&json)
            .unwrap()
            .restore(|id| fake.lookup(id))
            .unwrap();
        assert!(r.notes.is_empty(), "nothing to repair: {:?}", r.notes);
        let back = r.graph.node(a).expect("the anchor survived");
        assert!(matches!(back.kind, NodeKind::Anchor));
        assert_eq!(back.inputs, 1);
        assert_eq!(r.graph.placed_nodes(), 1, "hue_drift only");
        assert!(r.graph.contributes());
    }

    #[test]
    fn saved_ids_are_kept_and_the_allocator_moves_past_them() {
        let fake = Fake::new();
        let mut g = NodeGraph::new_with_output();
        let a = g.add_node(effect("hue_drift"), 1, &fake.hue);
        let b = g.add_node(effect("hue_drift"), 1, &fake.hue);
        g.remove_node(a).unwrap();
        let mut r = ChainDoc::capture(&g, layout)
            .restore(|id| fake.lookup(id))
            .unwrap();
        assert!(r.graph.node(a).is_none() && r.graph.node(b).is_some());
        let c = r.graph.add_node(effect("hue_drift"), 1, &fake.hue);
        assert!(c.0 > b.0, "a fresh id, past every saved one");
    }
}
