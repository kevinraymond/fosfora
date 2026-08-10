//! Per-node preview thumbnails (M2, handoff §9.6 / D4).
//!
//! Each executing node owns a persistent 192×108 target the executor blits
//! into — one node every 3rd frame, round-robin, so the amortized cost is a
//! fraction of a pass. Targets are `Rgba8Unorm`, NOT `-srgb`: the egui
//! renderer paints onto the sRGB surface with its linear-framebuffer entry
//! point, which treats sampled *user* textures as gamma-encoded — so the blit
//! shader performs the linear→sRGB encode itself and the texel values are
//! already display-ready. (An `-srgb` view would hardware-decode on sample
//! and get gamma-converted a second time: thumbnails would read too dark.)
//!
//! Texture identity is stable per node — egui registration happens once per
//! texture, not per frame ("re-register on recreate only"). `TextureId`s of
//! removed nodes park in `dead` until the next [`PreviewSet::register`] call
//! frees them, because freeing needs the egui renderer, which the executor
//! (and the GPU tests) never touch.

use std::collections::HashMap;

use crate::gpu::render_target::RenderTarget;

use super::super::node::NodeId;

pub const PREVIEW_W: u32 = 192;
pub const PREVIEW_H: u32 = 108;

struct PreviewEntry {
    target: RenderTarget,
    /// `None` until registered with egui — the canvas draws a placeholder
    /// rect for one frame after a node appears.
    tex_id: Option<egui::TextureId>,
}

#[derive(Default)]
pub struct PreviewSet {
    entries: HashMap<NodeId, PreviewEntry>,
    /// TextureIds owed a `free_texture`, drained on the next `register`.
    dead: Vec<egui::TextureId>,
}

impl PreviewSet {
    /// Create the target for `node` if it doesn't exist yet. Plan-build only
    /// (I8: no texture creation in steady state).
    pub fn ensure(&mut self, device: &wgpu::Device, node: NodeId) {
        self.entries.entry(node).or_insert_with(|| PreviewEntry {
            target: RenderTarget::new(
                device,
                PREVIEW_W,
                PREVIEW_H,
                wgpu::TextureFormat::Rgba8Unorm,
                1.0,
                "trama-preview",
            ),
            tex_id: None,
        });
    }

    /// Drop entries whose node no longer exists; their egui textures are
    /// freed on the next `register`.
    pub fn prune(&mut self, mut alive: impl FnMut(NodeId) -> bool) {
        let dead = &mut self.dead;
        self.entries.retain(|&id, entry| {
            let keep = alive(id);
            if !keep && let Some(tex) = entry.tex_id.take() {
                dead.push(tex);
            }
            keep
        });
    }

    /// The render-attachment view for `node`'s thumbnail, if one exists.
    pub fn view_of(&self, node: NodeId) -> Option<&wgpu::TextureView> {
        self.entries.get(&node).map(|e| &e.target.view)
    }

    /// The egui texture for `node`'s thumbnail, once registered.
    pub fn tex_of(&self, node: NodeId) -> Option<egui::TextureId> {
        self.entries.get(&node).and_then(|e| e.tex_id)
    }

    pub fn count(&self) -> usize {
        self.entries.len()
    }

    /// Register new targets with egui and free dead ones. Called from the
    /// frame loop where the egui renderer lives — never from the executor's
    /// render path, so GPU tests exercise previews without egui.
    pub fn register(&mut self, device: &wgpu::Device, renderer: &mut egui_wgpu::Renderer) {
        for tex in self.dead.drain(..) {
            renderer.free_texture(&tex);
        }
        for entry in self.entries.values_mut() {
            if entry.tex_id.is_none() {
                entry.tex_id = Some(renderer.register_native_texture(
                    device,
                    &entry.target.view,
                    wgpu::FilterMode::Linear,
                ));
            }
        }
    }
}
