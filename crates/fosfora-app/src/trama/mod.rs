//! trama — the node-graph effect-chain system.
//!
//! Phase-0 survey and design record: `docs/trama/INTEGRATION.md`; running
//! decision log: `docs/trama/DECISIONS.md`. M0 builds the graph model, the
//! manifest registry, the scene-level executor behind the
//! `execute_and_composite` seam, and the canvas.
#![allow(dead_code)] // M0 lands in stages; removed when the canvas commit wires everything up.

pub mod effect;
pub mod graph;
pub mod node;

/// Which pipeline produces the frame: the 8-layer stack or the trama graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RenderMode {
    #[default]
    Layers,
    Trama,
}
