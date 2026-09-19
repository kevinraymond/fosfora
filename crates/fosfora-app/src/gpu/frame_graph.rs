//! Layer execution + compositing for one frame, in one place.
//!
//! Extracted from `App::render`, which carried this block **twice** — once for
//! the normal path and a near-verbatim copy inside the dissolve re-render. The
//! copies had already begun to drift in whitespace and were one bugfix away
//! from drifting in behavior; the headless scene renderer (#2027) would have
//! been a third copy. All three call this.

use wgpu::{CommandEncoder, Device, Queue};

use crate::effect::format::{PfxEffect, PostProcessDef};
use crate::gpu::chain_targets::ChainTargets;
use crate::gpu::compositor::{Compositor, LayerComposite};
use crate::gpu::layer::LayerStack;
use crate::gpu::postprocess::AlphaMode;
use crate::gpu::render_target::RenderTarget;
use crate::settings::AlphaOutputMode;
use crate::trama::node::ChainId;

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
///
/// Auto deliberately does not look inside trama chains (decided 2026-09-19): a
/// chain is post-processing that carries coverage through, which every shipped
/// trama effect does, so the layer's own tag still describes the scene. A chain
/// that *creates* transparency on an untagged layer is the user's to flag, by
/// choosing Passthrough.
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

/// Where the master chain's input picture comes from, and how to name it.
///
/// The master chain samples whatever the layer stack produced, and its bind
/// groups are built once at plan time — so the identity of that picture has to
/// be pinned. It is not stable on its own:
///
/// - `Compositor::composite` returns `accumulator.targets[read_idx]`, and
///   `read_idx` depends on how many layers were composited.
/// - The solo fast path returns the *layer's* target, which ping-pongs with
///   that layer's parity, frame to frame.
///
/// So each arm reports both parities of its picture and the generation is
/// taken from the targets' own ids — which move exactly when the textures do,
/// including for the cases nothing else would notice: toggling a layer's
/// `enabled` changes `read_idx` without adding or removing a layer.
struct MasterInput<'a> {
    current: &'a RenderTarget,
    other: &'a RenderTarget,
}

impl<'a> MasterInput<'a> {
    fn stable(target: &'a RenderTarget) -> Self {
        Self {
            current: target,
            other: target,
        }
    }
}

/// Execute every enabled layer and composite the stack; returns the HDR source
/// for post-processing plus the active layer's postprocess settings.
///
/// Layer behavior is unchanged: no layers → the compositor's cleared
/// accumulator with default postprocess; a single fully-opaque layer skips
/// compositing; otherwise bottom-first composite with the list reversed so the
/// top of the UI list renders visually on top.
///
/// On top of that, each layer's trama chain (if it has one that reaches its
/// Output) post-processes that layer *in place* — the chain's output is what
/// gets composited — and the master chain post-processes the composited frame,
/// upstream of `PostProcessDef`, which keeps ownership of tonemapping. A chain
/// runs inside the same loop iteration as its layer, before the next
/// iteration's backdrop snapshot, so an `@backdrop` layer sees the *chained*
/// stack beneath it.
#[allow(clippy::too_many_arguments)]
pub(crate) fn execute_and_composite<'a>(
    layer_stack: &'a LayerStack,
    compositor: &'a mut Compositor,
    trama: Option<&'a mut crate::trama::TramaSystem>,
    chain_targets: &'a ChainTargets,
    device: &Device,
    queue: &Queue,
    encoder: &mut CommandEncoder,
    profiler: crate::gpu::profiler::ProfilerHandle<'_>,
) -> (&'a RenderTarget, PostProcessDef) {
    // Everything below is layer work; scopes opened by layers would nest
    // here if they ever grow their own. Chains open their own "trama" scope
    // under it, so the profiler panel still splits the two.
    let mut layers_scope = profiler.scope("layers", encoder);
    let encoder = layers_scope.encoder();
    // Headless passes None and stays chain-free until #2063.
    let mut trama = trama;

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

    let (source, master_in, postprocess) = if enabled.is_empty() {
        // The accumulator's own parity never advances (nothing calls flip() on
        // it), so this target is stable.
        let target = compositor.accumulator.write_target() as &RenderTarget;
        (
            target,
            MasterInput::stable(target),
            PostProcessDef::default(),
        )
    } else if enabled.len() == 1 && layer_stack.layers[enabled[0]].opacity >= 1.0 {
        // Single-layer fast path: skip compositing entirely (only when fully opaque)
        let layer = &layer_stack.layers[enabled[0]];
        if layer.wants_backdrop() {
            // Nothing beneath a solo layer — @backdrop reads transparent, not stale.
            compositor.clear_backdrop(encoder);
        }
        let raw = layer.execute(encoder, queue);
        let target = run_layer_chain(
            trama.as_deref_mut(),
            layer,
            raw,
            chain_targets,
            device,
            queue,
            encoder,
            profiler,
        );
        // A chain output is a fixed slot; a raw layer target ping-pongs.
        let master_in = if std::ptr::eq(target, raw) {
            let (current, other) = layer.final_targets();
            MasterInput { current, other }
        } else {
            MasterInput::stable(target)
        };
        (target, master_in, active_postprocess())
    } else {
        // Multi-layer: execute visually bottom-first (the UI list's top renders
        // on top, so walk it in reverse), so a layer that samples `@backdrop`
        // (#2061) can be handed the composite of everything beneath it BEFORE
        // it executes. For those layers only, the layers below are composited
        // an extra time into the backdrop snapshot — a few fullscreen draws,
        // bounded by the 8-layer cap — which leaves `composite()`'s
        // well-probed semantics untouched.
        let mut layer_outputs: Vec<LayerComposite<'_>> = Vec::with_capacity(enabled.len());
        for &idx in enabled.iter().rev() {
            let layer = &layer_stack.layers[idx];
            if layer.wants_backdrop() {
                if layer_outputs.is_empty() {
                    compositor.clear_backdrop(encoder);
                } else {
                    let below = compositor.composite(device, queue, encoder, &layer_outputs);
                    compositor.snapshot_backdrop(device, encoder, below);
                }
            }
            let raw = layer.execute(encoder, queue);
            // `run_layer_chain` returns nothing borrowed from `trama`, which
            // is what lets `layer_outputs` keep accumulating while `&mut
            // TramaSystem` is re-borrowed on every iteration.
            layer_outputs.push(LayerComposite {
                target: run_layer_chain(
                    trama.as_deref_mut(),
                    layer,
                    raw,
                    chain_targets,
                    device,
                    queue,
                    encoder,
                    profiler,
                ),
                blend_mode: layer.blend_mode,
                opacity: layer.opacity,
                displace_amount: layer.displace_amount,
            });
        }

        let composited = compositor.composite(device, queue, encoder, &layer_outputs);
        (
            composited,
            MasterInput::stable(composited),
            active_postprocess(),
        )
    };

    // The master chain runs on the composited frame, upstream of postprocess.
    let Some(t) = trama else {
        return (source, postprocess);
    };

    // A chain whose layer is disabled never reached the loop above, so its
    // thumbnails would freeze the moment you selected that layer. Run it for
    // previews only — its host target still holds the last picture the layer
    // rendered — and throw the result away.
    if t.canvas_open {
        if let Some(layer) = layer_stack.layers.iter().enumerate().find_map(|(i, l)| {
            let chain = l.chain.as_deref()?;
            (chain.id == t.active_chain && !enabled.contains(&i)).then_some(l)
        }) {
            let chain = layer.chain.as_deref().expect("found by having a chain");
            if chain_targets.has(chain.id) {
                t.execute_chain(
                    chain.id,
                    &chain.graph,
                    Some(layer.chain_input_source(t.parity())),
                    chain_targets.get(chain.id),
                    chain_targets.generation(chain.id),
                    device,
                    queue,
                    encoder,
                    profiler,
                );
            }
        }
    }

    // Same two-part rule as a layer's chain: an inactive master chain passes
    // the composite through untouched, but still RUNS while it is the chain on
    // the canvas, because its thumbnails have to update as the patch is built.
    let contributes = t.master.contributes();
    if (!contributes && !t.master_on_screen()) || !chain_targets.has(ChainId::Master) {
        return (source, postprocess);
    }
    let out = chain_targets.get(ChainId::Master);
    let parity = t.parity();
    t.execute_master(
        Some(crate::trama::exec::executor::ChainInputSource::paired(
            master_in.current,
            master_in.other,
            parity,
        )),
        out,
        chain_targets.generation(ChainId::Master),
        device,
        queue,
        encoder,
        profiler,
    );
    (if contributes { out } else { source }, postprocess)
}

/// Run `layer`'s chain and return the target that should be composited for it:
/// the chain's output when the chain reaches its Output node, otherwise the
/// layer's own picture, untouched.
///
/// Pass-through rather than black is the point. A chain whose Output is
/// unwired is *inactive*, not broken: placing a node before wiring it must not
/// blank the layer. (The M0 rule was the opposite, and was right then — trama
/// replaced the whole frame, so falling back to the layer stack would have
/// silently hidden a mistake. Post-processing one layer is a different job.)
///
/// It still *runs* an inactive chain when it is the one on screen, because its
/// thumbnails have to keep updating while the patch is being built — but the
/// picture it produces goes nowhere.
///
/// Returns nothing borrowed from `trama`, which is what lets the caller keep
/// accumulating `LayerComposite`s while re-borrowing `&mut TramaSystem` on
/// every iteration.
#[allow(clippy::too_many_arguments)]
fn run_layer_chain<'a>(
    trama: Option<&mut crate::trama::TramaSystem>,
    layer: &'a crate::gpu::layer::Layer,
    raw: &'a RenderTarget,
    chain_targets: &'a ChainTargets,
    device: &Device,
    queue: &Queue,
    encoder: &mut CommandEncoder,
    profiler: crate::gpu::profiler::ProfilerHandle<'_>,
) -> &'a RenderTarget {
    let (Some(t), Some(chain)) = (trama, layer.chain.as_deref()) else {
        return raw;
    };
    let contributes = chain.graph.contributes();
    let on_screen = t.canvas_open && t.active_chain == chain.id;
    if !contributes && !on_screen {
        return raw;
    }
    // The target sync runs before this and allocates every live chain a slot;
    // a miss would mean the two disagree, so skip rather than panic mid-frame.
    if !chain_targets.has(chain.id) {
        return raw;
    }
    let out = chain_targets.get(chain.id);
    t.execute_chain(
        chain.id,
        &chain.graph,
        Some(layer.chain_input_source(t.parity())),
        out,
        chain_targets.generation(chain.id),
        device,
        queue,
        encoder,
        profiler,
    );
    if contributes { out } else { raw }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::context::GpuContext;
    use crate::gpu::test_gpu::{gpu_guard, snapshot, test_gpu};
    use crate::trama::effect::EffectId;
    use crate::trama::node::NodeKind;

    const DIM: u32 = 64;

    /// Build a one-layer stack running a real `.pfx`, plus everything
    /// `execute_and_composite` needs around it.
    #[allow(clippy::type_complexity)]
    fn scene(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layers: usize,
    ) -> (
        LayerStack,
        Compositor,
        crate::trama::TramaSystem,
        ChainTargets,
    ) {
        // `assets_dir()` resolves CWD-relative and caches in a OnceLock, and
        // cargo runs tests from the package root — the existing convention
        // (see the pfx-layer spike) is to move to the repo root first.
        if !std::path::Path::new("assets/effects").is_dir() {
            let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
            std::env::set_current_dir(&repo).unwrap();
        }
        let mut loader = crate::effect::loader::EffectLoader::new();
        loader.scan_effects_directory();

        let placeholder = crate::gpu::placeholder::PlaceholderTexture::new(
            device,
            queue,
            GpuContext::hdr_format(),
        );
        let audio = crate::gpu::audio_textures::AudioTextures::new(device, queue);
        let ctx = crate::gpu::layer_builder::LayerBuildCtx {
            device,
            queue,
            pipeline_cache: None,
            width: DIM,
            height: DIM,
            placeholder: &placeholder,
            audio_textures: &audio,
            particle_quality: Default::default(),
            backdrop: None,
        };
        // The default layer, not one of the shipped effects: this probe
        // compares whole frames for byte equality, so the host has to render
        // the same picture every time it is asked. A feedback effect
        // accumulates and would drift between runs for reasons that have
        // nothing to do with chains.
        let mut stack = LayerStack::new();
        for i in 0..layers {
            let mut layer = crate::gpu::layer_builder::new_default_layer(&ctx, format!("host{i}"))
                .expect("layer");
            // Zeroed uniforms mean resolution 0, and most shaders divide by it
            // — the picture comes back all-NaN, which compares equal to itself
            // and would let every assertion below pass vacuously.
            if let Some(e) = layer.as_effect_mut() {
                e.uniforms.resolution = [DIM as f32, DIM as f32];
                e.uniforms.time = 1.0;
            }
            stack.layers.push(layer);
        }
        let compositor = Compositor::new(device, GpuContext::hdr_format(), DIM, DIM);
        let trama =
            crate::trama::TramaSystem::new(device, None, &loader, &placeholder, &audio, DIM, DIM);
        let targets = ChainTargets::new(DIM, DIM);
        (stack, compositor, trama, targets)
    }

    /// One frame, driven the way `App::render` drives it: update trama, sync
    /// the chain targets, execute and composite, flip every layer. Returns the
    /// composited picture.
    ///
    /// The flip is not optional. A layer whose last pass has feedback writes
    /// alternate targets on alternate frames, and a chain pairs its bind groups
    /// against that alternation. A harness that never flips only ever sees the
    /// frame a plan was built on: it hid a chain that replanned on every frame
    /// of the live app, and one frame later the chain was sampling a target
    /// the layer never wrote.
    fn frame(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        stack: &mut LayerStack,
        compositor: &mut Compositor,
        trama: &mut crate::trama::TramaSystem,
        targets: &mut ChainTargets,
    ) -> Vec<u8> {
        let template = stack.layers[0]
            .as_effect()
            .map(|e| e.uniforms)
            .unwrap_or_else(crate::gpu::ShaderUniforms::zeroed);
        trama.update(
            stack,
            1.0 / 60.0,
            &template,
            &crate::audio::features::AudioFeatures::default(),
            &[],
        );
        let master_live = trama.master_live();
        targets.sync(device, stack, master_live, |c| trama.drop_chain(c));
        let mut encoder = device.create_command_encoder(&Default::default());
        let (source, _) = execute_and_composite(
            stack,
            compositor,
            Some(trama),
            targets,
            device,
            queue,
            &mut encoder,
            crate::gpu::profiler::ProfilerHandle::none(),
        );
        let view = source.view.clone();
        queue.submit([encoder.finish()]);
        for layer in &mut stack.layers {
            layer.flip();
        }
        snapshot(device, queue, &view, DIM)
    }

    // Run: cargo test -p fosfora-app -- --ignored chain_post_processes_its_own_layer
    //
    // The three states a layer chain can be in, against one real layer:
    // absent, present but not reaching Output, and reaching Output. The middle
    // one is the decision this stage turned over — an inactive chain passes the
    // host's picture through instead of clearing black, so that placing a node
    // before wiring it does not blank the layer. Make `contributes()` return
    // true unconditionally and the pass-through assertion goes red.
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn chain_post_processes_its_own_layer() {
        let _guard = gpu_guard();
        let (device, queue) = test_gpu();
        let (mut stack, mut compositor, mut trama, mut targets) = scene(&device, &queue, 1);

        let run = |stack: &mut LayerStack,
                   compositor: &mut Compositor,
                   trama: &mut crate::trama::TramaSystem,
                   targets: &mut ChainTargets| {
            frame(&device, &queue, stack, compositor, trama, targets)
        };

        device.push_error_scope(wgpu::ErrorFilter::Validation);

        // The floor FIRST: the same scene rendered twice must be identical, or
        // every equality below is measuring the host's own frame-to-frame
        // drift rather than anything about chains.
        let bare = run(&mut stack, &mut compositor, &mut trama, &mut targets);
        let bare_again = run(&mut stack, &mut compositor, &mut trama, &mut targets);
        assert_eq!(bare, bare_again, "unchained host must be reproducible");
        assert!(
            bare.iter().any(|&b| b != 0),
            "the host layer must render something, or this test proves nothing"
        );

        // 1. A chain that exists but reaches nothing. INACTIVE, not broken.
        let slot = stack.ensure_chain(0).expect("a slot is free");
        assert_eq!(slot, ChainId::Layer(0), "first chain takes the first slot");
        {
            let graph = &mut stack.layers[0].chain.as_deref_mut().unwrap().graph;
            graph.add_node(NodeKind::ChainInput, 0, &[]);
        }
        let inactive = run(&mut stack, &mut compositor, &mut trama, &mut targets);
        assert_eq!(
            inactive, bare,
            "an unwired chain must pass the layer's picture through untouched"
        );

        // 1b. The SAME chain, now on screen. This is the case that matters and
        // the one 1a cannot reach: with the canvas open the chain actually
        // runs, so its thumbnails stay live while the patch is built — and its
        // output target gets deliberately cleared, because nothing feeds
        // Output. The layer must still show its own picture.
        //
        // 1a passes even with the pass-through decision reverted, because an
        // off-screen inactive chain is skipped before the decision is reached.
        // Return `out` unconditionally from `run_layer_chain` and THIS
        // assertion is the one that goes red.
        trama.canvas_open = true;
        trama.active_chain = slot;
        let inactive_on_screen = run(&mut stack, &mut compositor, &mut trama, &mut targets);
        assert_eq!(
            inactive_on_screen, bare,
            "an inactive chain must not blank its layer while you are patching it"
        );

        // 2. ChainInput wired straight to Output: the identity chain. Now the
        // chain really does produce the frame — into its own target, through
        // its own blit — and the picture has to survive the round trip. This
        // is what proves step 1 was pass-through and not a skipped chain.
        {
            let graph = &mut stack.layers[0].chain.as_deref_mut().unwrap().graph;
            let ci = graph.chain_input().expect("placed above");
            let out = graph.output_node();
            graph.connect(ci, out, 0).unwrap();
        }
        let identity = run(&mut stack, &mut compositor, &mut trama, &mut targets);
        assert_eq!(
            identity, bare,
            "the identity chain must reproduce the layer's picture exactly"
        );

        // 3. A real effect in the chain changes THIS layer's contribution.
        {
            let hue = trama
                .registry
                .get(&EffectId("hue_drift".into()))
                .expect("hue_drift ships");
            let (hue_id, hue_params) = (hue.id.clone(), hue.params.clone());
            let graph = &mut stack.layers[0].chain.as_deref_mut().unwrap().graph;
            let ci = graph.chain_input().expect("placed above");
            let out = graph.output_node();
            let h = graph.add_node(NodeKind::Effect { effect: hue_id }, 1, &hue_params);
            graph.connect(ci, h, 0).unwrap();
            graph.connect(h, out, 0).unwrap();
        }
        let processed = run(&mut stack, &mut compositor, &mut trama, &mut targets);
        assert_ne!(
            processed, bare,
            "a chain that reaches Output must change what the layer contributes"
        );

        let err = pollster::block_on(device.pop_error_scope());
        assert!(err.is_none(), "validation error: {err:?}");
    }

    // Run: cargo test -p fosfora-app -- --ignored chains_travel_with_their_layers
    //
    // The reason chains live ON the layer and hold an ALLOCATED slot rather
    // than the stack index. Executor state that outlives a replan — feedback
    // ping-pong pairs, preview thumbnails, the uniform arena region — is keyed
    // by that slot, so if the slot were the position, dragging a layer would
    // hand it another layer's echo buffer.
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn chains_travel_with_their_layers() {
        let _guard = gpu_guard();
        let (device, queue) = test_gpu();
        let (mut stack, _compositor, mut trama, mut targets) = scene(&device, &queue, 3);

        // Chains on the outer two only, so the middle layer is a hole in the
        // slot map and the gap-filling half of the allocator is exercised.
        let top = stack.ensure_chain(0).expect("slot");
        let bottom = stack.ensure_chain(2).expect("slot");
        assert_eq!(top, ChainId::Layer(0));
        assert_eq!(bottom, ChainId::Layer(1), "lowest free slot, not the index");
        assert_ne!(top, bottom);

        let sync = |stack: &LayerStack,
                    trama: &mut crate::trama::TramaSystem,
                    targets: &mut ChainTargets| {
            let master_live = trama.master_live();
            targets.sync(&device, stack, master_live, |c| trama.drop_chain(c));
        };
        sync(&stack, &mut trama, &mut targets);
        assert!(targets.has(top) && targets.has(bottom));
        assert_eq!(targets.resident(), 2, "no target for the chainless layer");
        let (top_gen, bottom_gen) = (targets.generation(top), targets.generation(bottom));

        // Drag the bottom layer to the top. Both chains keep their slots, stay
        // attached to the layers they were created on, and — the point — their
        // output targets are not reallocated, so nothing replans.
        stack.move_layer(2, 0);
        assert_eq!(stack.layers[0].name, "host2");
        assert_eq!(
            stack.layers[0].chain.as_deref().map(|c| c.id),
            Some(bottom),
            "the chain moved with its layer"
        );
        assert_eq!(stack.layers[1].chain.as_deref().map(|c| c.id), Some(top));
        assert!(
            stack.layers[2].chain.is_none(),
            "the chainless layer stays so"
        );

        sync(&stack, &mut trama, &mut targets);
        assert_eq!(targets.generation(top), top_gen, "reorder must not realloc");
        assert_eq!(targets.generation(bottom), bottom_gen);

        // Removing a layer takes its chain with it, and the slot comes back.
        stack.remove_layer(1);
        sync(&stack, &mut trama, &mut targets);
        assert!(!targets.has(top), "the removed layer's target is released");
        assert!(targets.has(bottom), "the survivor keeps its own");
        assert_eq!(
            stack.alloc_chain_slot(),
            Some(0),
            "the freed slot is available again"
        );
    }

    // Run: cargo test -p fosfora-app -- --ignored master_chain_passes_through_until_wired
    //
    // The layer probe above, for the master chain. An unwired master patch on
    // the canvas has to RUN (its thumbnails are drawing) without replacing the
    // composited frame with its cleared output.
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn master_chain_passes_through_until_wired() {
        let _guard = gpu_guard();
        let (device, queue) = test_gpu();
        let (mut stack, mut compositor, mut trama, mut targets) = scene(&device, &queue, 1);

        let run = |stack: &mut LayerStack,
                   compositor: &mut Compositor,
                   trama: &mut crate::trama::TramaSystem,
                   targets: &mut ChainTargets| {
            frame(&device, &queue, stack, compositor, trama, targets)
        };

        device.push_error_scope(wgpu::ErrorFilter::Validation);

        // The floor first, or every equality below measures the host's drift.
        let bare = run(&mut stack, &mut compositor, &mut trama, &mut targets);
        let bare_again = run(&mut stack, &mut compositor, &mut trama, &mut targets);
        assert_eq!(bare, bare_again, "unchained host must be reproducible");
        assert!(
            bare.iter().any(|&b| b != 0),
            "the host must render something"
        );

        // An unwired patch, off screen: skipped outright.
        let input = trama.master.add_node(NodeKind::ChainInput, 0, &[]);
        let before = trama.plans_built();
        let off_screen = run(&mut stack, &mut compositor, &mut trama, &mut targets);
        assert_eq!(off_screen, bare);
        assert_eq!(trama.plans_built(), before, "idle and unseen: not run");

        // The same patch with the Master tab open. It must RUN — that is the
        // half an "unchanged picture" assertion cannot see on its own — and
        // the frame must still be the composite, not the chain's cleared
        // output. Return `out` unconditionally and this goes red.
        trama.canvas_open = true;
        trama.active_chain = ChainId::Master;
        let on_screen = run(&mut stack, &mut compositor, &mut trama, &mut targets);
        assert!(
            trama.plans_built() > before,
            "the chain on the canvas runs so its thumbnails stay live"
        );
        assert_eq!(
            on_screen, bare,
            "an inactive master chain must not blank the frame while you patch it"
        );

        // Wired straight through: the identity chain reproduces the frame.
        let out = trama.master.output_node();
        trama.master.connect(input, out, 0).unwrap();
        let identity = run(&mut stack, &mut compositor, &mut trama, &mut targets);
        assert_eq!(identity, bare, "identity master chain reproduces the frame");

        // And again, one frame on. A solo layer takes the fast path, so the
        // master samples the layer's own ping-pong pair: the layer has flipped
        // since the plan was built, and the chain must follow it to the other
        // target WITHOUT replanning.
        let planned = trama.plans_built();
        let steady = run(&mut stack, &mut compositor, &mut trama, &mut targets);
        assert_eq!(
            steady, bare,
            "the pairing holds on the frame after the plan"
        );
        assert_eq!(
            trama.plans_built(),
            planned,
            "a layer flipping its targets is not a reason to replan"
        );

        // A real effect changes it.
        let hue = trama
            .registry
            .get(&EffectId("hue_drift".into()))
            .expect("hue_drift ships");
        let (hue_id, hue_params) = (hue.id.clone(), hue.params.clone());
        let h = trama
            .master
            .add_node(NodeKind::Effect { effect: hue_id }, 1, &hue_params);
        trama.master.connect(input, h, 0).unwrap();
        trama.master.connect(h, out, 0).unwrap();
        let processed = run(&mut stack, &mut compositor, &mut trama, &mut targets);
        assert_ne!(processed, bare, "a wired master chain changes the frame");

        let err = pollster::block_on(device.pop_error_scope());
        assert!(err.is_none(), "validation error: {err:?}");
    }

    /// `ChainInput -> hue_drift (one per shift) -> Output`, left UNWIRED at the
    /// Output so the caller decides when the chain goes live. `speed` is zeroed:
    /// the default drifts with time, and the probe below compares pictures
    /// taken on different frames.
    fn hue_chain(
        graph: &mut crate::trama::graph::NodeGraph,
        registry: &crate::trama::effect::TramaRegistry,
        shifts: &[f32],
    ) -> crate::trama::node::NodeId {
        let hue = registry
            .get(&EffectId("hue_drift".into()))
            .expect("hue_drift ships");
        let mut prev = graph.add_node(NodeKind::ChainInput, 0, &[]);
        for &shift in shifts {
            let h = graph.add_node(
                NodeKind::Effect {
                    effect: hue.id.clone(),
                },
                1,
                &hue.params,
            );
            let p = graph.params_mut(h).expect("just added");
            p.params
                .set("shift", crate::params::ParamValue::Float(shift));
            p.params.set("speed", crate::params::ParamValue::Float(0.0));
            graph.connect(prev, h, 0).unwrap();
            prev = h;
        }
        prev
    }

    // Run: cargo test -p fosfora-app -- --ignored chain_transients_alias_across_chains
    //
    // The stage-E VRAM checkpoint, and H3's falsifiable prediction: the
    // executor's transient pool sits at the LARGEST single chain, not the sum
    // over chains. Every plan build starts with `pool.release_all()`, so each
    // chain re-acquires the same targets from index 0 — sound because chains
    // run one after another and no chain reads another's interior. Delete that
    // `release_all()` and the pool total here reads 10, not 2.
    //
    // What DOES sum, by design, and is asserted next to it so nobody reads the
    // flat pool as "chains are free":
    // - one full-resolution OUTPUT per chain that exists (`ChainTargets`) —
    //   wired or not, a chain holding only its ChainInput still has one;
    // - feedback ping-pong pairs, which hold last frame and cannot alias.
    //
    // The second half checks the aliasing is harmless: every layer chain's
    // output with all five running is byte-identical to the same chain running
    // alone. Point `ChainTargets::get` at one slot for every chain (H2's
    // original bug) and that half goes red.
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn chain_transients_alias_across_chains() {
        const LAYERS: usize = 4;
        let _guard = gpu_guard();
        let (device, queue) = test_gpu();
        let (mut stack, mut compositor, mut trama, mut targets) = scene(&device, &queue, LAYERS);

        let run = |stack: &mut LayerStack,
                   compositor: &mut Compositor,
                   trama: &mut crate::trama::TramaSystem,
                   targets: &mut ChainTargets| {
            frame(&device, &queue, stack, compositor, trama, targets)
        };

        device.push_error_scope(wgpu::ErrorFilter::Validation);

        // Three hue_drifts per chain: two interior targets from the pool, the
        // third writes the chain's output. Shifts differ per chain so that one
        // chain's picture cannot stand in for another's below.
        let shifts =
            |i: usize| -> [f32; 3] { std::array::from_fn(|k| 0.07 * (i * 3 + k + 1) as f32) };
        let mut tails = Vec::new();
        for i in 0..LAYERS {
            let id = stack.ensure_chain(i).expect("a slot is free");
            let graph = &mut stack.layers[i].chain.as_deref_mut().unwrap().graph;
            tails.push((id, hue_chain(graph, &trama.registry, &shifts(i))));
        }
        let wire = |stack: &mut LayerStack, i: usize, tail, on: bool| {
            let graph = &mut stack.layers[i].chain.as_deref_mut().unwrap().graph;
            let out = graph.output_node();
            if on {
                graph.connect(tail, out, 0).unwrap();
            } else {
                graph.disconnect(out, 0);
            }
        };

        // Each chain ALONE: the pool reading, and the reference picture.
        let mut solo = Vec::new();
        for (i, &(id, tail)) in tails.iter().enumerate() {
            wire(&mut stack, i, tail, true);
            let before = trama.plans_built();
            run(&mut stack, &mut compositor, &mut trama, &mut targets);
            run(&mut stack, &mut compositor, &mut trama, &mut targets);
            assert_eq!(
                trama.plans_built(),
                before + 1,
                "chain {i} ran, planned once"
            );
            assert_eq!(trama.pool_stats().1, 2, "P1: one 3-effect chain, alone");
            solo.push(snapshot(&device, &queue, &targets.get(id).view, DIM));
            wire(&mut stack, i, tail, false);
        }
        assert!(
            solo[0] != solo[1],
            "distinct shifts must give distinct pictures"
        );

        // All four layers plus the master, together.
        for (i, &(_, tail)) in tails.iter().enumerate() {
            wire(&mut stack, i, tail, true);
        }
        let master_tail = hue_chain(&mut trama.master, &trama.registry, &shifts(LAYERS));
        let master_out = trama.master.output_node();
        trama.master.connect(master_tail, master_out, 0).unwrap();

        let before = trama.plans_built();
        run(&mut stack, &mut compositor, &mut trama, &mut targets);
        run(&mut stack, &mut compositor, &mut trama, &mut targets);
        assert_eq!(
            trama.plans_built(),
            before + 5,
            "all five chains ran, each planned once"
        );
        assert_eq!(
            trama.pool_stats().1,
            2,
            "P5 == P1: transients alias across chains instead of summing"
        );
        assert_eq!(targets.resident(), 5, "outputs DO sum: one per chain");
        assert_eq!(trama.feedback_stats(), 0, "no feedback node, no pairs");
        for (i, &(id, _)) in tails.iter().enumerate() {
            let together = snapshot(&device, &queue, &targets.get(id).view, DIM);
            assert!(
                together == solo[i],
                "chain {i}'s picture changed when the other chains ran beside it"
            );
        }

        // Lengthen ONE chain to five effects: the pool follows that chain
        // (four interiors), not the total node count.
        {
            let graph = &mut stack.layers[2].chain.as_deref_mut().unwrap().graph;
            let hue = trama
                .registry
                .get(&EffectId("hue_drift".into()))
                .expect("hue_drift ships");
            let out = graph.output_node();
            let mut prev = tails[2].1;
            for _ in 0..2 {
                let h = graph.add_node(
                    NodeKind::Effect {
                        effect: hue.id.clone(),
                    },
                    1,
                    &hue.params,
                );
                graph.connect(prev, h, 0).unwrap();
                prev = h;
            }
            graph.connect(prev, out, 0).unwrap();
        }
        run(&mut stack, &mut compositor, &mut trama, &mut targets);
        run(&mut stack, &mut compositor, &mut trama, &mut targets);
        assert_eq!(
            trama.pool_stats().1,
            4,
            "the pool follows the longest chain"
        );

        let err = pollster::block_on(device.pop_error_scope());
        assert!(err.is_none(), "validation error: {err:?}");
    }

    // Run: cargo test -p fosfora-app -- --ignored a_reloaded_manifest_reaches_every_chain
    //
    // The glue between the registry and the graphs: when an effect's manifest
    // changes on disk, EVERY live node of it is brought in line — in each
    // layer's chain and in the master — and a chain whose pins moved has its
    // canvas view rebuilt from the graph, since the view keeps its own copy of
    // the wire set and would go on drawing a wire the graph no longer has.
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn a_reloaded_manifest_reaches_every_chain() {
        let _guard = gpu_guard();
        let (device, queue) = test_gpu();
        let (mut stack, mut compositor, mut trama, mut targets) = scene(&device, &queue, 2);
        let loader = crate::effect::loader::EffectLoader::new();

        // A scratch effect, outside the shipped set: two inputs, one param.
        let dir = std::env::temp_dir().join(format!("fosfora-trama-glue-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("scratch_blend.wgsl");
        let file = |inputs: u8, body: &str| {
            format!(
                "/*! trama\n{{ \"name\": \"Scratch\", \"id\": \"scratch_blend\", \"kind\": \"effect\", \
                 \"inputs\": {inputs}, \"params\": [ {{ \"type\": \"Float\", \"name\": \"amount\", \
                 \"default\": 0.5, \"min\": 0.0, \"max\": 1.0 }} ] }}\n*/\n\
                 @fragment\nfn fs_main(@builtin(position) p: vec4f) -> @location(0) vec4f {{\n\
                 let uv = p.xy / u.resolution;\n{body}\n}}\n"
            )
        };
        std::fs::write(
            &path,
            file(2, "return mix(input0(uv), input1(uv), param(0u));"),
        )
        .unwrap();
        trama.reload_effects(
            &device,
            None,
            &loader,
            std::slice::from_ref(&path),
            false,
            &mut stack,
        );
        assert!(
            trama.registry.errors.is_empty(),
            "{:?}",
            trama.registry.errors
        );
        let scratch = EffectId("scratch_blend".into());

        // Layer input into BOTH pins of a scratch node, on a layer and on the master.
        let wire_up = |graph: &mut crate::trama::graph::NodeGraph,
                       registry: &crate::trama::effect::TramaRegistry| {
            let def = registry.get(&scratch).expect("just loaded");
            let input = graph.add_node(NodeKind::ChainInput, 0, &[]);
            let node = graph.add_node(
                NodeKind::Effect {
                    effect: def.id.clone(),
                },
                def.inputs,
                &def.params,
            );
            let out = graph.output_node();
            graph.connect(input, node, 0).unwrap();
            graph.connect(input, node, 1).unwrap();
            graph.connect(node, out, 0).unwrap();
            node
        };
        let id = stack.ensure_chain(1).expect("a slot is free");
        let on_layer = wire_up(
            &mut stack.layers[1].chain.as_deref_mut().unwrap().graph,
            &trama.registry,
        );
        let on_master = wire_up(&mut trama.master, &trama.registry);
        trama
            .canvas
            .open_view(id, &stack.layers[1].chain.as_deref().unwrap().graph);
        let placed = trama
            .canvas
            .position(id, on_layer)
            .expect("the view knows it");

        device.push_error_scope(wgpu::ErrorFilter::Validation);
        frame(
            &device,
            &queue,
            &mut stack,
            &mut compositor,
            &mut trama,
            &mut targets,
        );

        // The effect loses its second input.
        std::fs::write(&path, file(1, "return input0(uv) * param(0u);")).unwrap();
        trama.reload_effects(
            &device,
            None,
            &loader,
            std::slice::from_ref(&path),
            false,
            &mut stack,
        );

        let layer_graph = &stack.layers[1].chain.as_deref().unwrap().graph;
        for (graph, node, whose) in [
            (layer_graph, on_layer, "layer"),
            (&trama.master, on_master, "master"),
        ] {
            assert_eq!(
                graph.node(node).unwrap().inputs,
                1,
                "{whose}: pin count follows the manifest"
            );
            assert_eq!(
                graph.wires().len(),
                2,
                "{whose}: the wire into the lost pin is gone"
            );
            graph.validate().unwrap_or_else(|e| panic!("{whose}: {e}"));
        }
        assert!(!trama.canvas.has_view(id), "the stale view was thrown away");
        assert_eq!(
            trama.canvas.position(id, on_layer),
            Some(placed),
            "and the node stays where it was put"
        );
        // It still renders, against the new pipeline and the new pin count.
        frame(
            &device,
            &queue,
            &mut stack,
            &mut compositor,
            &mut trama,
            &mut targets,
        );
        frame(
            &device,
            &queue,
            &mut stack,
            &mut compositor,
            &mut trama,
            &mut targets,
        );

        let err = pollster::block_on(device.pop_error_scope());
        assert!(err.is_none(), "validation error: {err:?}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    // Run: cargo test -p fosfora-app -- --ignored chains_survive_a_save_and_a_load
    //
    // M3's acceptance, on the per-layer shape: save every chain of a stack,
    // push the documents through JSON TEXT (what a preset file is), load them
    // into a scene that has never seen them — a restart — and the picture is
    // byte-identical. Then the two ways loading over a LIVE stack goes wrong:
    //
    // - Slots are reused and a restored graph starts at version 0 like the last
    //   restored graph did, so the plan key sees nothing change. Without
    //   forgetting the slot first, the executor keeps running the OLD chain's
    //   plan. Make `TramaSystem::reset_chain` skip `executor.drop_chain` and
    //   the "second preset" assertion goes red.
    // - A preset saved WITHOUT a chain must leave the layer without one.
    //   Before presets carried chains, the previous preset's stayed attached.
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn chains_survive_a_save_and_a_load() {
        use crate::trama::persist;
        use crate::trama::ser::ChainDoc;

        let _guard = gpu_guard();
        let (device, queue) = test_gpu();
        device.push_error_scope(wgpu::ErrorFilter::Validation);

        let render = |stack: &mut LayerStack,
                      compositor: &mut Compositor,
                      trama: &mut crate::trama::TramaSystem,
                      targets: &mut ChainTargets| {
            let mut shot = Vec::new();
            for _ in 0..2 {
                shot = frame(&device, &queue, stack, compositor, trama, targets);
            }
            shot
        };
        // Layer 0: Layer input -> hue_drift(shift) -> Output. Master: Layer
        // input -> Transform at half size -> Output, so the two are told apart
        // in the picture and neither can stand in for the other.
        let author = |stack: &mut LayerStack, trama: &mut crate::trama::TramaSystem, shift: f32| {
            let id = stack.ensure_chain(0).expect("a slot is free");
            {
                let graph = &mut stack.layers[0].chain.as_deref_mut().unwrap().graph;
                let tail = hue_chain(graph, &trama.registry, &[shift]);
                let out = graph.output_node();
                graph.connect(tail, out, 0).unwrap();
            }
            let transform = trama
                .registry
                .get(&EffectId("transform".into()))
                .expect("transform ships");
            let (kind, params) = (
                NodeKind::Effect {
                    effect: transform.id.clone(),
                },
                transform.params.clone(),
            );
            let input = trama.master.add_node(NodeKind::ChainInput, 0, &[]);
            let t = trama.master.add_node(kind, 1, &params);
            trama
                .master
                .params_mut(t)
                .unwrap()
                .params
                .set("scale", crate::params::ParamValue::Float(0.5));
            let out = trama.master.output_node();
            trama.master.connect(input, t, 0).unwrap();
            trama.master.connect(t, out, 0).unwrap();
            id
        };
        let through_text = |saved: &persist::SavedChains| {
            let reread = |d: &ChainDoc| ChainDoc::from_json(&d.to_json()).expect("reads back");
            (
                saved
                    .layers
                    .iter()
                    .map(|d| d.as_ref().map(reread))
                    .collect::<Vec<_>>(),
                saved.master.as_ref().map(reread),
            )
        };

        // Author, render, save.
        let (mut stack, mut compositor, mut trama, mut targets) = scene(&device, &queue, 2);
        let bare = render(&mut stack, &mut compositor, &mut trama, &mut targets);
        let id = author(&mut stack, &mut trama, 0.37);
        // A view, as if the canvas had been opened — positions live there.
        trama
            .canvas
            .open_view(id, &stack.layers[0].chain.as_deref().unwrap().graph);
        let authored = render(&mut stack, &mut compositor, &mut trama, &mut targets);
        assert!(
            authored != bare,
            "the chains must show, or nothing is proved"
        );
        let saved = persist::capture(&stack, &trama);
        assert!(saved.layers[0].is_some() && saved.layers[1].is_none() && saved.master.is_some());
        let (layer_docs, master_doc) = through_text(&saved);

        // "Restart": a scene that has never seen them.
        let (mut stack, mut compositor, mut trama, mut targets) = scene(&device, &queue, 2);
        let notes = persist::apply(
            &mut stack,
            &mut trama,
            &layer_docs,
            master_doc.as_ref(),
            |_| false,
        );
        assert_eq!(notes, Vec::<String>::new(), "nothing needed repair");
        let reloaded = render(&mut stack, &mut compositor, &mut trama, &mut targets);
        assert!(
            reloaded == authored,
            "a reloaded patch renders the same picture"
        );
        // Saved again WITHOUT the canvas ever opening, the layout survives.
        let resaved = persist::capture(&stack, &trama);
        assert_eq!(resaved.layers, saved.layers, "positions kept with no view");
        assert_eq!(resaved.master, saved.master);

        // A second preset over the live stack, landing on the same slots — with
        // a different TOPOLOGY on both chains. A different parameter value
        // would prove nothing: values are re-read every frame, so a stale plan
        // renders them correctly. What it is compared against is the same
        // preset rendered in a scene of its own.
        let (mut other_stack, mut other_comp, mut other_trama, mut other_targets) =
            scene(&device, &queue, 2);
        {
            other_stack.ensure_chain(0).expect("a slot is free");
            let graph = &mut other_stack.layers[0].chain.as_deref_mut().unwrap().graph;
            let tail = hue_chain(graph, &other_trama.registry, &[0.2, 0.3]);
            let out = graph.output_node();
            graph.connect(tail, out, 0).unwrap();
            let tail = hue_chain(&mut other_trama.master, &other_trama.registry, &[0.45]);
            let out = other_trama.master.output_node();
            other_trama.master.connect(tail, out, 0).unwrap();
        }
        let expected = render(
            &mut other_stack,
            &mut other_comp,
            &mut other_trama,
            &mut other_targets,
        );
        assert!(
            expected != reloaded,
            "the second preset must look different"
        );
        let second = persist::capture(&other_stack, &other_trama);
        let (layer_docs, master_doc) = through_text(&second);
        persist::apply(
            &mut stack,
            &mut trama,
            &layer_docs,
            master_doc.as_ref(),
            |_| false,
        );
        let replaced = render(&mut stack, &mut compositor, &mut trama, &mut targets);
        assert!(
            replaced == expected,
            "the second preset's chains must be the ones running, not the first's plans"
        );

        // And a preset with no chains at all clears them.
        // A LOCKED layer is skipped by a preset load and keeps what it has.
        let kept = persist::apply(&mut stack, &mut trama, &[None, None], None, |i| i == 0);
        assert!(kept.is_empty());
        assert!(
            stack.layers[0].chain.is_some(),
            "a locked layer keeps its chain"
        );
        persist::apply(&mut stack, &mut trama, &[None, None], None, |_| false);
        assert!(stack.layers.iter().all(|l| l.chain.is_none()));
        assert_eq!(trama.master.placed_nodes(), 0);
        let cleared = render(&mut stack, &mut compositor, &mut trama, &mut targets);
        assert!(
            cleared == bare,
            "no chain left behind from the preset before"
        );

        let err = pollster::block_on(device.pop_error_scope());
        assert!(err.is_none(), "validation error: {err:?}");
    }

    // Run: cargo test -p fosfora-app -- --ignored a_chain_reads_the_frame_its_layer_just_rendered
    //
    // A chain must sample the picture its layer rendered THIS frame — from the
    // first frame ever, and on every frame after. A layer whose last pass has
    // feedback writes alternate targets on alternate frames, so every step fed
    // by the Layer input needs a bind group per parity. When only feedback
    // inputs counted as parity-dependent, such a step carried the parity-0 bind
    // group twice: on every other frame it read the target the layer had
    // written the frame BEFORE (the layer ran at half rate inside its chain),
    // and on the first frame ever it read a target nothing had written yet —
    // one frame of transparent black (#2680), which is what loading a preset
    // that carries chains would have flashed.
    //
    // A static host hides all of it after frame one, because both of its
    // targets end up holding the same picture. This host moves.
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn a_chain_reads_the_frame_its_layer_just_rendered() {
        let _guard = gpu_guard();
        let (device, queue) = test_gpu();
        // Layer 0 is left alone: `frame` takes the chain's uniform template
        // from it, and the host's own uniforms are about to be abused.
        let (mut stack, mut compositor, mut trama, mut targets) = scene(&device, &queue, 2);
        const HOST: usize = 1;

        // Layer input -> Transform at its defaults -> Output. An identity that
        // is still an EFFECT step, which is the kind that was mis-paired (a
        // bare Layer input -> Output is a copy step, and was always right).
        let id = stack.ensure_chain(HOST).expect("a slot is free");
        {
            let transform = trama
                .registry
                .get(&EffectId("transform".into()))
                .expect("transform ships");
            let graph = &mut stack.layers[HOST].chain.as_deref_mut().unwrap().graph;
            let input = graph.add_node(NodeKind::ChainInput, 0, &[]);
            let t = graph.add_node(
                NodeKind::Effect {
                    effect: transform.id.clone(),
                },
                1,
                &transform.params,
            );
            let out = graph.output_node();
            graph.connect(input, t, 0).unwrap();
            graph.connect(t, out, 0).unwrap();
        }

        device.push_error_scope(wgpu::ErrorFilter::Validation);

        let mut previous: Option<Vec<u8>> = None;
        for k in 1..=6u32 {
            // The default layer draws a gradient of `uv = pos / resolution`
            // and ignores time, so stretching its resolution is what makes it
            // render a different picture every frame.
            if let Some(e) = stack.layers[HOST].as_effect_mut() {
                e.uniforms.resolution = [(DIM * (k + 1)) as f32, DIM as f32];
            }
            frame(
                &device,
                &queue,
                &mut stack,
                &mut compositor,
                &mut trama,
                &mut targets,
            );
            // `frame` has flipped the layer: what it rendered is now `other`.
            let (_, rendered) = stack.layers[HOST].final_targets();
            let host = snapshot(&device, &queue, &rendered.view, DIM);
            let chain = snapshot(&device, &queue, &targets.get(id).view, DIM);

            assert!(host.iter().any(|&b| b != 0), "frame {k}: the host rendered");
            if let Some(previous) = &previous {
                assert!(
                    *previous != host,
                    "frame {k}: the host must move, or a stale read is invisible"
                );
            }
            assert!(
                chain == host,
                "frame {k}: the chain did not read the picture its layer rendered \
                 this frame (first byte off at {:?})",
                chain.iter().zip(&host).position(|(a, b)| a != b)
            );
            previous = Some(host);
        }
        assert_eq!(
            trama.plans_built(),
            1,
            "and it never had to replan to do it"
        );

        let err = pollster::block_on(device.pop_error_scope());
        assert!(err.is_none(), "validation error: {err:?}");
    }

    // Run: cargo test -p fosfora-app -- --ignored a_shrunk_layer_reveals_the_layer_beneath
    //
    // The picture-in-picture recipe in docs/TUTORIALS.md, as a probe. Transform
    // writes TRANSPARENT where it pulls from outside its input, and the
    // compositor weights a layer by its alpha — so shrinking the top layer has
    // to show the layer under it around the edges, not a black border. Make
    // transform.wgsl return opaque black outside and the corner assertion goes
    // red.
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn a_shrunk_layer_reveals_the_layer_beneath() {
        let _guard = gpu_guard();
        let (device, queue) = test_gpu();
        let (mut stack, mut compositor, mut trama, mut targets) = scene(&device, &queue, 2);
        // Tell the two layers apart. Both hosts render the same picture (the
        // default layer ignores `time`), so the one beneath gets a fixed hue
        // rotation from a chain of its own.
        stack.ensure_chain(1).expect("a slot is free");
        {
            let graph = &mut stack.layers[1].chain.as_deref_mut().unwrap().graph;
            let tail = hue_chain(graph, &trama.registry, &[0.5]);
            let out = graph.output_node();
            graph.connect(tail, out, 0).unwrap();
        }
        // Half size about the center leaves the middle half of each axis
        // covered; stay a few pixels clear of that edge on both sides, where
        // bilinear filtering blends the two.
        let differing = |a: &[u8], b: &[u8], inside: bool| -> usize {
            let (lo, hi) = (DIM / 4, DIM * 3 / 4);
            let mut n = 0;
            for y in 0..DIM {
                for x in 0..DIM {
                    let within = |m: u32| x >= lo + m && x < hi - m && y >= lo + m && y < hi - m;
                    let wanted = if inside {
                        within(3)
                    } else {
                        !(x + 3 >= lo && x < hi + 3 && y + 3 >= lo && y < hi + 3)
                    };
                    let i = ((y * DIM + x) * 8) as usize;
                    if wanted && a[i..i + 8] != b[i..i + 8] {
                        n += 1;
                    }
                }
            }
            n
        };

        device.push_error_scope(wgpu::ErrorFilter::Validation);

        // The layer beneath, alone: what the border must show afterwards. This
        // is the first frame ever rendered, with a chain already on the layer —
        // what loading a preset that carries chains looks like.
        stack.layers[0].enabled = false;
        let beneath = frame(
            &device,
            &queue,
            &mut stack,
            &mut compositor,
            &mut trama,
            &mut targets,
        );
        assert!(
            beneath.iter().any(|&b| b != 0),
            "the reference must be a picture, or every comparison below is empty"
        );
        stack.layers[0].enabled = true;
        let full = frame(
            &device,
            &queue,
            &mut stack,
            &mut compositor,
            &mut trama,
            &mut targets,
        );
        assert!(
            differing(&full, &beneath, false) > 100,
            "unchained, the opaque top layer must cover the border with a \
             DIFFERENT picture, or the border assertion below proves nothing"
        );

        // Top layer: Layer input -> Transform (half size) -> Output.
        stack.ensure_chain(0).expect("a slot is free");
        {
            let transform = trama
                .registry
                .get(&EffectId("transform".into()))
                .expect("transform ships");
            let graph = &mut stack.layers[0].chain.as_deref_mut().unwrap().graph;
            let input = graph.add_node(NodeKind::ChainInput, 0, &[]);
            let t = graph.add_node(
                NodeKind::Effect {
                    effect: transform.id.clone(),
                },
                1,
                &transform.params,
            );
            graph
                .params_mut(t)
                .expect("just added")
                .params
                .set("scale", crate::params::ParamValue::Float(0.5));
            let out = graph.output_node();
            graph.connect(input, t, 0).unwrap();
            graph.connect(t, out, 0).unwrap();
        }
        let mut shrunk = Vec::new();
        for _ in 0..2 {
            shrunk = frame(
                &device,
                &queue,
                &mut stack,
                &mut compositor,
                &mut trama,
                &mut targets,
            );
        }
        assert_eq!(
            differing(&shrunk, &beneath, false),
            0,
            "outside the shrunk picture, the layer beneath shows through"
        );
        assert!(
            differing(&shrunk, &beneath, true) > 100,
            "inside it, the top layer still covers"
        );

        let err = pollster::block_on(device.pop_error_scope());
        assert!(err.is_none(), "validation error: {err:?}");
    }

    // Run: cargo test -p fosfora-app -- --ignored master_chain_survives_being_unwired
    //
    // The master chain's output target comes and goes with whether anything
    // reaches Output, and releasing a target tells trama to forget the chain.
    // For a layer chain that is right — the layer is gone. The master GRAPH
    // never goes away, so forgetting its canvas view threw away every node
    // position the moment the last wire was pulled.
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn master_chain_survives_being_unwired() {
        let _guard = gpu_guard();
        let (device, _queue) = test_gpu();
        let (stack, _compositor, mut trama, mut targets) = scene(&device, &_queue, 1);

        let sync = |trama: &mut crate::trama::TramaSystem, targets: &mut ChainTargets| {
            let master_live = trama.master_live();
            targets.sync(&device, &stack, master_live, |c| trama.drop_chain(c));
        };

        // Master tab open on a patch that reaches Output.
        trama.canvas_open = true;
        trama.active_chain = ChainId::Master;
        let input = trama.master.add_node(NodeKind::ChainInput, 0, &[]);
        let out = trama.master.output_node();
        trama.master.connect(input, out, 0).unwrap();
        trama.canvas.open_view(ChainId::Master, &trama.master);
        sync(&mut trama, &mut targets);
        assert!(targets.has(ChainId::Master));

        // Pull the wire while still looking at the master canvas. The chain is
        // on screen, so it must keep a target (its thumbnails are still
        // drawing) and keep its view.
        trama.master.disconnect(out, 0);
        sync(&mut trama, &mut targets);
        assert!(
            targets.has(ChainId::Master),
            "the chain on screen keeps running for its thumbnails"
        );
        assert!(trama.canvas.has_view(ChainId::Master));

        // Look away. Now the target really is released — and the view must
        // survive that, because the graph did.
        trama.active_chain = ChainId::Layer(0);
        sync(&mut trama, &mut targets);
        assert!(
            !targets.has(ChainId::Master),
            "idle and off screen: released"
        );
        assert!(
            trama.canvas.has_view(ChainId::Master),
            "the master graph outlives its target, so its view must too"
        );
    }
}
