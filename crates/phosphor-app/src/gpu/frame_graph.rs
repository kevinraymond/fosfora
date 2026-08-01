//! Layer execution + compositing for one frame, in one place.
//!
//! Extracted from `App::render`, which carried this block **twice** — once for
//! the normal path and a near-verbatim copy inside the dissolve re-render. The
//! copies had already begun to drift in whitespace and were one bugfix away
//! from drifting in behavior; the headless scene renderer (#2027) would have
//! been a third copy. All three call this.

use wgpu::{CommandEncoder, Device, Queue};

use crate::effect::format::{PfxEffect, PostProcessDef};
use crate::gpu::compositor::{Compositor, LayerComposite};
use crate::gpu::layer::LayerStack;
use crate::gpu::postprocess::AlphaMode;
use crate::gpu::render_target::RenderTarget;
use crate::settings::AlphaOutputMode;

/// Resolve the frame's output-alpha mode (overlay initiative; docs/alpha.md).
///
/// Shared by the live path (both `App::render` branches) and the headless
/// renderer so screen, capture and offline output can never disagree. Auto
/// resolves to Passthrough exactly when the scene *is* an overlay — at least
/// one enabled layer, and every enabled layer is an effect tagged
/// `alpha: true` (a media layer or any ordinary effect means the content
/// underneath is the picture, so the frame stays opaque). Otherwise the NDI
/// "Alpha from brightness" checkbox keeps selecting the legacy luma key —
/// existing setups see no change.
pub(crate) fn resolve_output_alpha(
    setting: AlphaOutputMode,
    layer_stack: &LayerStack,
    effects: &[PfxEffect],
    ndi_alpha_from_luma: bool,
) -> AlphaMode {
    match setting {
        AlphaOutputMode::Opaque => AlphaMode::Opaque,
        AlphaOutputMode::Luma => AlphaMode::Luma,
        AlphaOutputMode::Passthrough => AlphaMode::Passthrough,
        AlphaOutputMode::Auto => {
            let enabled: Vec<_> = layer_stack.layers.iter().filter(|l| l.enabled).collect();
            let all_overlay = !enabled.is_empty()
                && enabled.iter().all(|l| {
                    l.as_effect()
                        .and_then(|e| e.effect_index)
                        .and_then(|i| effects.get(i))
                        .is_some_and(|fx| fx.alpha)
                });
            if all_overlay {
                AlphaMode::Passthrough
            } else if ndi_alpha_from_luma {
                AlphaMode::Luma
            } else {
                AlphaMode::Opaque
            }
        }
    }
}

/// Execute every enabled layer and composite the stack; returns the HDR source
/// for post-processing plus the active layer's postprocess settings.
///
/// Mirrors the shipped behavior exactly: no layers → the compositor's cleared
/// accumulator with default postprocess; a single fully-opaque layer skips
/// compositing; otherwise bottom-first composite with the list reversed so the
/// top of the UI list renders visually on top.
pub(crate) fn execute_and_composite<'a>(
    layer_stack: &'a LayerStack,
    compositor: &'a mut Compositor,
    device: &Device,
    queue: &Queue,
    encoder: &mut CommandEncoder,
) -> (&'a RenderTarget, PostProcessDef) {
    let enabled: Vec<usize> = layer_stack
        .layers
        .iter()
        .enumerate()
        .filter(|(_, l)| l.enabled)
        .map(|(i, _)| i)
        .collect();

    let active_postprocess = || {
        layer_stack
            .active()
            .map(|l| l.postprocess.clone())
            .unwrap_or_default()
    };

    if enabled.is_empty() {
        (
            compositor.accumulator.write_target() as &RenderTarget,
            PostProcessDef::default(),
        )
    } else if enabled.len() == 1 && layer_stack.layers[enabled[0]].opacity >= 1.0 {
        // Single-layer fast path: skip compositing entirely (only when fully opaque)
        let target = layer_stack.layers[enabled[0]].execute(encoder, queue);
        (target, active_postprocess())
    } else {
        // Multi-layer: render each layer, then composite
        let mut layer_outputs: Vec<LayerComposite<'_>> = Vec::with_capacity(enabled.len());
        for &idx in &enabled {
            let layer = &layer_stack.layers[idx];
            layer_outputs.push(LayerComposite {
                target: layer.execute(encoder, queue),
                blend_mode: layer.blend_mode,
                opacity: layer.opacity,
                displace_amount: layer.displace_amount,
            });
        }
        // Reverse so top-of-UI-list renders visually on top
        layer_outputs.reverse();

        let composited = compositor.composite(device, queue, encoder, &layer_outputs);
        (composited, active_postprocess())
    }
}
