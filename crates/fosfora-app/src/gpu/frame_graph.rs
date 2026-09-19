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
    let mut per_parity = [
        (&master_in.current.view, &master_in.current.sampler),
        (&master_in.current.view, &master_in.current.sampler),
    ];
    per_parity[1 - parity] = (&master_in.other.view, &master_in.other.sampler);
    t.execute_master(
        Some(crate::trama::exec::executor::ChainInputSource {
            per_parity,
            generation: crate::gpu::render_target::pair_id(master_in.current, master_in.other),
        }),
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
            targets.sync(&device, stack, master_live, |c| trama.drop_chain(c));
            let mut encoder = device.create_command_encoder(&Default::default());
            let (source, _) = execute_and_composite(
                stack,
                compositor,
                Some(trama),
                targets,
                &device,
                &queue,
                &mut encoder,
                crate::gpu::profiler::ProfilerHandle::none(),
            );
            let view = source.view.clone();
            queue.submit([encoder.finish()]);
            snapshot(&device, &queue, &view, DIM)
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
            targets.sync(&device, stack, master_live, |c| trama.drop_chain(c));
            let mut encoder = device.create_command_encoder(&Default::default());
            let (source, _) = execute_and_composite(
                stack,
                compositor,
                Some(trama),
                targets,
                &device,
                &queue,
                &mut encoder,
                crate::gpu::profiler::ProfilerHandle::none(),
            );
            let view = source.view.clone();
            queue.submit([encoder.finish()]);
            snapshot(&device, &queue, &view, DIM)
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
