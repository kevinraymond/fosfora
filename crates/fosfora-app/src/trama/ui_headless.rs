//! Stand-in for the canvas editor (`trama/ui/`) in builds without the `desktop`
//! feature, which have no egui-snarl.
//!
//! What the runtime needs from the canvas is its node layouts: a chain loaded
//! from a preset keeps its saved positions, and saving it again writes them
//! back. With no editor there are no views, so the loaded layout is the only
//! copy — the same path the desktop canvas takes for a chain it has not drawn.

pub mod canvas {
    use std::collections::HashMap;

    use super::super::node::{ChainId, NodeId};

    #[derive(Default)]
    pub struct CanvasState {
        layouts: HashMap<ChainId, Vec<(NodeId, [f32; 2])>>,
        /// Last refused edit. Nothing shows it without the editor.
        pub status: Option<String>,
    }

    impl CanvasState {
        pub fn drop_chain(&mut self, chain: ChainId) {
            if chain != ChainId::Master {
                self.layouts.remove(&chain);
            }
        }

        pub fn replace_chain(&mut self, chain: ChainId, layout: Vec<(NodeId, [f32; 2])>) {
            self.layouts.insert(chain, layout);
        }

        pub fn position(&self, chain: ChainId, node: NodeId) -> Option<[f32; 2]> {
            self.layouts
                .get(&chain)?
                .iter()
                .find(|(n, _)| *n == node)
                .map(|(_, p)| *p)
        }
    }
}
