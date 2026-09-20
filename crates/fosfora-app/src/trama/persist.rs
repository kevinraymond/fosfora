//! Chains inside presets: capture every chain a layer stack carries, and put
//! a saved set back.
//!
//! Kept out of `App` on purpose. `App` needs a window, so anything that lives
//! only there is reachable only by hand — and this path replaces graphs under
//! an executor that caches plans, bind groups, echo buffers and thumbnails per
//! chain SLOT. The GPU probes in `gpu/frame_graph.rs` drive these two
//! functions directly.

use crate::gpu::layer::LayerStack;

use super::TramaSystem;
use super::graph::NodeGraph;
use super::node::ChainId;
use super::ser::{ChainDoc, EffectShape, Restored};

/// Every chain worth saving: one entry per layer, in stack order, plus the
/// master. A chain holding only the Output node it was born with is `None` —
/// opening the canvas on a layer creates one, and that is not something the
/// user authored.
pub struct SavedChains {
    pub layers: Vec<Option<ChainDoc>>,
    pub master: Option<ChainDoc>,
}

pub fn capture(stack: &LayerStack, trama: &TramaSystem) -> SavedChains {
    let doc = |chain: ChainId, graph: &NodeGraph| {
        (graph.placed_nodes() > 0)
            .then(|| ChainDoc::capture(graph, |node| trama.canvas.position(chain, node)))
    };
    SavedChains {
        layers: stack
            .layers
            .iter()
            .map(|l| l.chain.as_deref().and_then(|c| doc(c.id, &c.graph)))
            .collect(),
        master: doc(ChainId::Master, &trama.master),
    }
}

/// Replace every chain with the saved set; `layers[i]` belongs to
/// `stack.layers[i]`. Returns what had to be repaired or could not be loaded,
/// in words — an empty list means everything came back exactly as saved.
///
/// REPLACE, not merge: a layer the preset saved without a chain ends up
/// without one. Before presets carried chains, loading one left the previous
/// preset's chains attached to whichever layers survived.
///
/// `keep(i)` exempts layer `i` altogether — a locked layer is skipped by a
/// preset load and keeps everything it has, its chain included.
///
/// Every old chain is forgotten — executor state and canvas view — before a
/// new one can land on its slot. Slots are reused, a restored graph starts at
/// version 0 like the last restored graph did, and the plan key would
/// otherwise see nothing change: the executor would go on running the OLD
/// chain's plan, with its echo buffers, under the new chain's name.
pub fn apply(
    stack: &mut LayerStack,
    trama: &mut TramaSystem,
    layers: &[Option<ChainDoc>],
    master: Option<&ChainDoc>,
    keep: impl Fn(usize) -> bool,
) -> Vec<String> {
    let mut notes = Vec::new();

    // Restore first, against the registry, while nothing is borrowed mutably.
    let restore = |doc: &ChainDoc, whose: String, notes: &mut Vec<String>| -> Option<Restored> {
        let result = doc.restore(|id| {
            trama.registry.get(id).map(|def| EffectShape {
                inputs: def.inputs,
                params: &def.params,
            })
        });
        match result {
            Ok(restored) => {
                notes.extend(restored.notes.iter().map(|n| format!("{whose}: {n}")));
                Some(restored)
            }
            Err(e) => {
                notes.push(format!("{whose}: chain not loaded — {e}"));
                None
            }
        }
    };
    let restored_layers: Vec<Option<Restored>> = (0..stack.layers.len())
        .map(|i| {
            if keep(i) {
                return None;
            }
            let doc = layers.get(i)?.as_ref()?;
            restore(doc, format!("layer {}", i + 1), &mut notes)
        })
        .collect();
    let restored_master = master.and_then(|doc| restore(doc, "master".to_string(), &mut notes));

    for (i, layer) in stack.layers.iter_mut().enumerate() {
        if keep(i) {
            continue;
        }
        if let Some(old) = layer.chain.take() {
            trama.drop_chain(old.id);
        }
    }
    for (i, restored) in restored_layers.into_iter().enumerate() {
        let Some(restored) = restored else { continue };
        let Some(id) = stack.ensure_chain(i) else {
            notes.push(format!("layer {}: no free chain slot", i + 1));
            continue;
        };
        let chain = stack.layers[i].chain.as_deref_mut().expect("just ensured");
        chain.graph = restored.graph;
        trama.reset_chain(id, restored.positions);
    }

    let (graph, layout) = match restored_master {
        Some(r) => (r.graph, r.positions),
        None => (NodeGraph::new_with_output(), Vec::new()),
    };
    let has_master = graph.placed_nodes() > 0;
    trama.master = graph;
    trama.reset_chain(ChainId::Master, layout);

    let layer_chains = stack.layers.iter().filter(|l| l.chain.is_some()).count();
    if layer_chains > 0 || has_master {
        log::info!(
            "trama: restored {layer_chains} layer chain(s){}",
            if has_master {
                " and the master chain"
            } else {
                ""
            }
        );
    }
    notes
}

/// Replace ONE chain with a document — importing a `.fio.json` into the chain
/// on the canvas. Returns what had to be repaired. On an error the chain is
/// left exactly as it was: a file that cannot be loaded costs nothing.
pub fn load_into(
    stack: &mut LayerStack,
    trama: &mut TramaSystem,
    chain: ChainId,
    doc: &ChainDoc,
) -> Result<Vec<String>, String> {
    let restored = doc
        .restore(|id| {
            trama.registry.get(id).map(|def| EffectShape {
                inputs: def.inputs,
                params: &def.params,
            })
        })
        .map_err(|e| e.to_string())?;
    let graph = match chain {
        ChainId::Master => &mut trama.master,
        ChainId::Layer(_) => stack
            .layers
            .iter_mut()
            .filter_map(|l| l.chain.as_deref_mut())
            .find(|c| c.id == chain)
            .map(|c| &mut c.graph)
            .ok_or("the layer this chain belonged to is gone")?,
    };
    *graph = restored.graph;
    trama.reset_chain(chain, restored.positions);
    Ok(restored.notes)
}

/// The file dialogs behind the canvas's Export and Import buttons. They run
/// on their own thread, like every other dialog in the app — a native dialog
/// blocks its caller, and the caller here would be the render loop.
pub struct ChainIo {
    tx: std::sync::mpsc::Sender<Imported>,
    rx: std::sync::mpsc::Receiver<Imported>,
}

/// A finished import: which chain asked, and the document or why not.
pub struct Imported {
    pub chain: ChainId,
    pub result: Result<ChainDoc, String>,
}

impl Default for ChainIo {
    fn default() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        Self { tx, rx }
    }
}

impl ChainIo {
    fn dialog() -> rfd::FileDialog {
        rfd::FileDialog::new().add_filter("trama chain", &["json"])
    }

    /// Ask where to save `doc`, then write it. Nothing comes back but a log
    /// line; the document was captured by the caller, so the chain can go on
    /// being edited while the dialog is open.
    pub fn export(&self, doc: ChainDoc) {
        let spawned = std::thread::Builder::new()
            .name("file-dialog".into())
            .spawn(move || {
                let Some(mut path) = Self::dialog()
                    .set_title("Export trama chain")
                    .set_file_name("chain.fio.json")
                    .save_file()
                else {
                    return;
                };
                if path.extension().is_none() {
                    path.set_extension("fio.json");
                }
                match crate::paths::write_atomic(&path, &doc.to_json()) {
                    Ok(()) => log::info!("trama: exported chain to {}", path.display()),
                    Err(e) => log::error!("trama: export to {} failed: {e}", path.display()),
                }
            });
        if let Err(e) = spawned {
            log::error!("trama: could not open the export dialog: {e}");
        }
    }

    /// Ask for a file to load into `chain`. The answer arrives through
    /// [`Self::drain`] on some later frame.
    pub fn import(&self, chain: ChainId) {
        let tx = self.tx.clone();
        let spawned = std::thread::Builder::new()
            .name("file-dialog".into())
            .spawn(move || {
                let Some(path) = Self::dialog().set_title("Import trama chain").pick_file() else {
                    return;
                };
                let result = std::fs::read_to_string(&path)
                    .map_err(|e| format!("{}: {e}", path.display()))
                    .and_then(|text| ChainDoc::from_json(&text).map_err(|e| e.to_string()));
                let _ = tx.send(Imported { chain, result });
            });
        if let Err(e) = spawned {
            log::error!("trama: could not open the import dialog: {e}");
        }
    }

    pub fn drain(&self) -> Vec<Imported> {
        self.rx.try_iter().collect()
    }
}

/// Notices that the chain on the canvas was edited, so the preset can show
/// its "unsaved" marker. Chain edits touch no layer parameter, so nothing
/// else would: structural edits bump the graph version, but a slider, a
/// modulation depth or a dragged node move nothing anyone is watching.
/// Comparing documents catches all of them with one rule, and is the same
/// question saving asks — "would the file be different?"
#[derive(Default)]
pub struct EditWatch {
    baseline: Option<(ChainId, ChainDoc)>,
}

impl EditWatch {
    /// True when `doc` differs from the last document seen for the SAME
    /// chain. The first look at anything, and a look at a different chain,
    /// only move the baseline: switching tabs is not an edit.
    pub fn observe(&mut self, chain: ChainId, doc: ChainDoc) -> bool {
        let edited = matches!(&self.baseline, Some((c, d)) if *c == chain && *d != doc);
        self.baseline = Some((chain, doc));
        edited
    }

    /// A chain was replaced by a load: whatever comes next is not an edit.
    pub fn forget(&mut self) {
        self.baseline = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trama::node::NodeKind;

    // The capture demos (scripts/capture/demos/*.json) are what the gallery and
    // the tutorial clips are filmed from, and a bad one fails SILENTLY: a chain
    // that needs repair, or does not reach its Output, films the plain layer
    // and quietly lies about what it shows. The Python demo checker would have
    // to re-implement the chain format to notice, and it has drifted from the
    // app's grammar before — so ask the app's own loader instead, against the
    // real effect manifests.
    #[test]
    fn every_capture_demo_chain_loads_clean_and_does_something() {
        use crate::params::{ParamDef, ParamValue};
        use crate::preset::Preset;
        use crate::trama::effect::{TramaManifest, parse_effect_file};
        use crate::trama::ser::EffectShape;

        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let manifests: Vec<TramaManifest> = std::fs::read_dir(repo.join("assets/trama/effects"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|x| x == "wgsl"))
            .map(|p| parse_effect_file(&std::fs::read_to_string(&p).unwrap()).unwrap())
            .collect();
        let lookup = |id: &crate::trama::effect::EffectId| {
            manifests
                .iter()
                .find(|m| m.id == id.0)
                .map(|m| EffectShape {
                    inputs: m.inputs,
                    params: &m.params,
                })
        };

        let mut chains = 0;
        for entry in std::fs::read_dir(repo.join("scripts/capture/demos")).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let is_json = path
                .extension()
                .is_some_and(|x| x.eq_ignore_ascii_case("json"));
            // `_scene.json` is not a preset, and `*.bindings.json` is a sidecar.
            if !is_json || name.starts_with('_') || name.contains(".bindings.") {
                continue;
            }
            let preset: Preset = serde_json::from_str(&std::fs::read_to_string(&path).unwrap())
                .unwrap_or_else(|e| panic!("{name}: not a preset: {e}"));
            let docs = preset
                .layers
                .iter()
                .enumerate()
                .filter_map(|(i, l)| Some((format!("layer {}", i + 1), l.chain.as_ref()?)))
                .chain(
                    preset
                        .master_chain
                        .as_ref()
                        .map(|d| ("master".to_string(), d)),
                );
            for (whose, doc) in docs {
                chains += 1;
                let r = doc
                    .restore(lookup)
                    .unwrap_or_else(|e| panic!("{name}, {whose}: {e}"));
                assert!(
                    r.notes.is_empty(),
                    "{name}, {whose}: needed repair: {:#?}",
                    r.notes
                );
                assert!(
                    r.graph.contributes(),
                    "{name}, {whose}: nothing reaches Output, so the clip would show the plain picture"
                );
                // A declared range is a slider hint, not a clamp: a value
                // outside it reaches the shader as written.
                for node in r.graph.nodes() {
                    for def in &node.params.defs {
                        if let (
                            ParamDef::Float {
                                name: p, min, max, ..
                            },
                            Some(ParamValue::Float(v)),
                        ) = (def, node.params.get(def.name()))
                        {
                            assert!(
                                (*min..=*max).contains(v),
                                "{name}, {whose}: `{p}` = {v} is outside {min}..={max}"
                            );
                        }
                    }
                }
            }
        }
        // Zero chains would mean this test has stopped looking at anything.
        assert!(chains >= 1, "no capture demo carries a trama chain");
    }

    #[test]
    fn only_a_change_to_the_same_chain_counts_as_an_edit() {
        let mut g = NodeGraph::new_with_output();
        let doc = |g: &NodeGraph, x: f32| ChainDoc::capture(g, |_| Some([x, 0.0]));
        let mut watch = EditWatch::default();
        let (a, b) = (ChainId::Layer(0), ChainId::Master);

        assert!(
            !watch.observe(a, doc(&g, 0.0)),
            "the first look is a baseline"
        );
        assert!(!watch.observe(a, doc(&g, 0.0)), "nothing changed");
        g.add_node(NodeKind::ChainInput, 0, &[]);
        assert!(watch.observe(a, doc(&g, 0.0)), "a placed node is an edit");
        assert!(watch.observe(a, doc(&g, 40.0)), "so is a dragged one");
        assert!(!watch.observe(a, doc(&g, 40.0)), "and then it settles");

        assert!(
            !watch.observe(b, doc(&g, 0.0)),
            "switching tabs is not an edit"
        );
        watch.forget();
        assert!(!watch.observe(b, doc(&g, 99.0)), "nor is a load");
    }
}
