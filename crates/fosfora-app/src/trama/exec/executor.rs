//! The trama scene executor: turns the node graph into ordered wgpu passes.
//!
//! Per-node uniforms live in one arena buffer at static offsets (stride =
//! `ShaderUniforms` rounded up to the device's uniform-offset alignment); each
//! node gets its own bind group against the effect's ABI v3 layout, cached in
//! an [`ExecPlan`] keyed on the graph's structural version. Steady state does
//! zero allocations and creates zero textures (I8): a frame is N uniform
//! writes + N fullscreen passes in topo order.
//!
//! I4 story: invalid states are mostly unreachable (cycles are refused at
//! `connect`, Output exists by construction, broken effect files never enter
//! the registry). If a plan build still fails, the previous plan keeps
//! rendering and the error is surfaced; with no previous plan the output is
//! deliberately cleared black — never a silent fall-back to the layer stack.

use std::num::NonZeroU64;

use crate::gpu::audio_textures::AudioTextures;
use crate::gpu::placeholder::PlaceholderTexture;
use crate::gpu::render_target::RenderTarget;
use crate::gpu::{GpuContext, ShaderUniforms};

use super::super::effect::TramaRegistry;
use super::super::graph::NodeGraph;
use super::super::node::{NodeId, NodeKind};
use super::textures::TexturePool;

const UNIFORM_SIZE: u64 = std::mem::size_of::<ShaderUniforms>() as u64;
/// Arena capacity floor — grows at plan build if a graph outgrows it.
const INITIAL_CAPACITY: u32 = 16;

/// Round `size` up to a multiple of `align` (a power of two).
pub(crate) fn aligned_stride(size: u64, align: u64) -> u64 {
    debug_assert!(align.is_power_of_two());
    size.div_ceil(align) * align
}

#[derive(Clone, Copy)]
enum TargetSlot {
    Pool(usize),
    Output,
}

struct Step {
    node: NodeId,
    /// Index into `registry.effects`.
    effect: usize,
    target: TargetSlot,
    uniform_offset: u64,
    bind_group: wgpu::BindGroup,
}

struct ExecPlan {
    /// The `NodeGraph::version()` this plan was built for.
    version: u64,
    /// Live, non-bypassed nodes in topological order.
    steps: Vec<Step>,
    /// False ⇒ nothing feeds Output; `execute` clears the output target.
    output_written: bool,
}

pub struct TramaExecutor {
    arena: wgpu::Buffer,
    stride: u64,
    capacity: u32,
    output: RenderTarget,
    pool: TexturePool,
    plan: Option<ExecPlan>,
    // Stable resource handles, cloned once (wgpu handles are ref-counted; the
    // AudioTextures views are fixed-size and never recreated, so bind groups
    // built against them stay valid).
    prev_view: wgpu::TextureView,
    prev_sampler: wgpu::Sampler,
    waveform: wgpu::TextureView,
    spectrum: wgpu::TextureView,
    spectrogram: wgpu::TextureView,
    audio_sampler: wgpu::Sampler,
    width: u32,
    height: u32,
}

impl TramaExecutor {
    pub fn new(
        device: &wgpu::Device,
        placeholder: &PlaceholderTexture,
        audio: &AudioTextures,
        width: u32,
        height: u32,
    ) -> Self {
        let stride = aligned_stride(
            UNIFORM_SIZE,
            u64::from(device.limits().min_uniform_buffer_offset_alignment),
        );
        Self {
            arena: create_arena(device, stride, INITIAL_CAPACITY),
            stride,
            capacity: INITIAL_CAPACITY,
            output: RenderTarget::new(
                device,
                width,
                height,
                GpuContext::hdr_format(),
                1.0,
                "trama-output",
            ),
            pool: TexturePool::new(),
            plan: None,
            prev_view: placeholder.view.clone(),
            prev_sampler: placeholder.sampler.clone(),
            waveform: audio.waveform_view.clone(),
            spectrum: audio.spectrum_view.clone(),
            spectrogram: audio.spectrogram_view.clone(),
            audio_sampler: audio.sampler.clone(),
            width,
            height,
        }
    }

    /// Output-resolution change: recreate the output target, drop the pool,
    /// force a replan. No-op when the size is unchanged.
    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        if width == self.width && height == self.height {
            return;
        }
        self.width = width;
        self.height = height;
        self.output = RenderTarget::new(
            device,
            width,
            height,
            GpuContext::hdr_format(),
            1.0,
            "trama-output",
        );
        self.pool.clear();
        self.plan = None;
    }

    /// `(in_use, total)` pooled targets — the canvas debug line.
    pub fn pool_stats(&self) -> (usize, usize) {
        self.pool.stats()
    }

    /// Execute the graph for this frame and return the Output node's target.
    ///
    /// Replans when the graph's structural version moved (that is what makes
    /// "rewire updates the output next frame" true); otherwise the frame is
    /// pure uniform writes + passes.
    #[allow(clippy::too_many_arguments)]
    pub fn execute(
        &mut self,
        graph: &mut NodeGraph,
        registry: &TramaRegistry,
        template: &ShaderUniforms,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        last_error: &mut Option<String>,
    ) -> &RenderTarget {
        let version = graph.version();
        if self.plan.as_ref().is_none_or(|p| p.version != version) {
            match self.build_plan(graph, registry, device, version) {
                Ok(plan) => {
                    self.plan = Some(plan);
                    *last_error = None;
                }
                Err(e) => {
                    // Keep the last-good plan rendering (I4). Stamp its
                    // version so the failed build is not retried every frame;
                    // the next structural edit retries naturally.
                    *last_error = Some(e);
                    match self.plan.as_mut() {
                        Some(p) => p.version = version,
                        None => {
                            self.plan = Some(ExecPlan {
                                version,
                                steps: Vec::new(),
                                output_written: false,
                            });
                        }
                    }
                }
            }
        }
        let plan = self.plan.as_ref().expect("plan installed above");

        for step in &plan.steps {
            let node = graph.node(step.node).expect("plan nodes exist in graph");
            let mut u = *template;
            u.params = node.params.pack_to_buffer();
            queue.write_buffer(&self.arena, step.uniform_offset, bytemuck::bytes_of(&u));
        }

        for step in &plan.steps {
            let view = match step.target {
                TargetSlot::Pool(i) => &self.pool.get(i).view,
                TargetSlot::Output => &self.output.view,
            };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("trama-node-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&registry.effects[step.effect].pipeline.pipeline);
            pass.set_bind_group(0, &step.bind_group, &[]);
            pass.draw(0..3, 0..1);
        }

        if !plan.output_written {
            // Nothing feeds Output: deliberate cleared black, never a stale
            // frame and never a silent fall-back to the layer stack.
            encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("trama-output-clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.output.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
        }

        &self.output
    }

    fn build_plan(
        &mut self,
        graph: &mut NodeGraph,
        registry: &TramaRegistry,
        device: &wgpu::Device,
        version: u64,
    ) -> Result<ExecPlan, String> {
        graph.validate().map_err(|e| e.to_string())?;
        let live = graph.live_set();

        // The nodes that actually run: live, not the Output, not bypassed.
        let step_nodes: Vec<NodeId> = live
            .iter()
            .copied()
            .filter(|&id| {
                let node = graph.node(id).expect("live nodes exist");
                !matches!(node.kind, NodeKind::Output) && !node.bypass
            })
            .collect();

        // Bypass aliasing resolves at plan time: the "effective producer" of a
        // pin follows bypassed effects through their input 0 until it lands on
        // a running node (or nothing — placeholder input / cleared output).
        let effective = |mut id: NodeId| -> Option<NodeId> {
            loop {
                let node = graph.node(id)?;
                if !node.bypass {
                    return Some(id);
                }
                match node.kind {
                    NodeKind::Effect { .. } => id = graph.input_source(id, 0)?,
                    // A bypassed source (or Output, unreachable here) yields
                    // nothing.
                    _ => return None,
                }
            }
        };

        let final_producer = graph
            .input_source(graph.output_node(), 0)
            .and_then(effective);

        if step_nodes.len() as u32 > self.capacity {
            self.capacity = (step_nodes.len() as u32).next_power_of_two();
            self.arena = create_arena(device, self.stride, self.capacity);
        }

        // Pass 1: assign targets so pass 2 can resolve inputs to views.
        self.pool.release_all();
        let mut targets: Vec<(NodeId, TargetSlot)> = Vec::with_capacity(step_nodes.len());
        for &id in &step_nodes {
            let slot = if final_producer == Some(id) {
                TargetSlot::Output
            } else {
                TargetSlot::Pool(self.pool.acquire(
                    device,
                    self.width,
                    self.height,
                    GpuContext::hdr_format(),
                ))
            };
            targets.push((id, slot));
        }
        let target_of = |id: NodeId| -> Option<TargetSlot> {
            targets.iter().find(|(n, _)| *n == id).map(|(_, s)| *s)
        };

        // Pass 2: bind groups. Binding 0 is an arena slice at a static offset
        // — the reason `UniformBuffer::create_bind_group` (which binds its own
        // whole buffer) is not reusable here.
        let mut steps = Vec::with_capacity(step_nodes.len());
        for (i, &id) in step_nodes.iter().enumerate() {
            let node = graph.node(id).expect("live nodes exist");
            let effect_id = match &node.kind {
                NodeKind::Source { effect } | NodeKind::Effect { effect } => effect,
                NodeKind::Output => unreachable!("filtered above"),
            };
            let effect_idx = registry
                .effects
                .iter()
                .position(|e| &e.id == effect_id)
                .ok_or_else(|| format!("node references unknown effect `{}`", effect_id.0))?;
            let def = &registry.effects[effect_idx];

            let uniform_offset = i as u64 * self.stride;
            let mut entries = vec![
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &self.arena,
                        offset: uniform_offset,
                        size: Some(NonZeroU64::new(UNIFORM_SIZE).expect("nonzero")),
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&self.prev_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.prev_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&self.waveform),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&self.spectrum),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(&self.spectrogram),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::Sampler(&self.audio_sampler),
                },
            ];
            for pin in 0..def.inputs {
                let producer = graph.input_source(id, pin).and_then(effective);
                let (view, sampler) = match producer.and_then(target_of) {
                    Some(TargetSlot::Pool(p)) => {
                        let rt = self.pool.get(p);
                        (&rt.view, &rt.sampler)
                    }
                    Some(TargetSlot::Output) => (&self.output.view, &self.output.sampler),
                    // Unwired (or a dead bypass chain): 1x1 black.
                    None => (&self.prev_view, &self.prev_sampler),
                };
                let b = 7 + 2 * u32::from(pin);
                entries.push(wgpu::BindGroupEntry {
                    binding: b,
                    resource: wgpu::BindingResource::TextureView(view),
                });
                entries.push(wgpu::BindGroupEntry {
                    binding: b + 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                });
            }
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("trama-node-bind-group"),
                layout: &def.pipeline.bind_group_layout,
                entries: &entries,
            });

            steps.push(Step {
                node: id,
                effect: effect_idx,
                target: targets[i].1,
                uniform_offset,
                bind_group,
            });
        }

        Ok(ExecPlan {
            version,
            steps,
            output_written: final_producer.is_some(),
        })
    }

    #[cfg(test)]
    fn plan_version(&self) -> Option<u64> {
        self.plan.as_ref().map(|p| p.version)
    }
}

fn create_arena(device: &wgpu::Device, stride: u64, capacity: u32) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("trama-uniform-arena"),
        size: stride * u64::from(capacity),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effect::loader::{EffectLoader, probe_libs};
    use crate::gpu::test_gpu::{gpu_guard, test_gpu};
    use crate::trama::effect::{EffectId, TramaRegistry};
    use crate::trama::graph::NodeGraph;
    use crate::trama::node::NodeKind;

    #[test]
    fn aligned_stride_rounds_448_up_to_alignment() {
        assert_eq!(aligned_stride(448, 256), 512);
        assert_eq!(aligned_stride(448, 64), 448);
        assert_eq!(aligned_stride(448, 32), 448);
        assert_eq!(aligned_stride(256, 256), 256);
    }

    fn effects_dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/trama/effects")
    }

    fn registry(device: &wgpu::Device) -> TramaRegistry {
        let loader = EffectLoader::for_test(&probe_libs());
        TramaRegistry::load(device, None, &loader, &effects_dir())
    }

    // Run: cargo test -p fosfora-app -- --ignored trama_registry_builds_pipelines_for_builtins
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn trama_registry_builds_pipelines_for_builtins() {
        let _guard = gpu_guard();
        let (device, _queue) = test_gpu();
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let reg = registry(&device);
        let err = pollster::block_on(device.pop_error_scope());
        assert!(err.is_none(), "validation error: {err:?}");
        assert!(
            reg.errors.is_empty(),
            "registry load errors: {:?}",
            reg.errors
        );
        let ids: Vec<&str> = reg.effects.iter().map(|e| e.id.0.as_str()).collect();
        assert_eq!(ids, ["hue_drift", "noise_field"], "sorted by id");
    }

    // Run: cargo test -p fosfora-app -- --ignored trama_executor_renders_noise_hue_output_chain
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn trama_executor_renders_noise_hue_output_chain() {
        let _guard = gpu_guard();
        let (device, queue) = test_gpu();
        let reg = registry(&device);
        assert!(reg.errors.is_empty(), "{:?}", reg.errors);

        let noise = reg.get(&EffectId("noise_field".into())).unwrap();
        let hue = reg.get(&EffectId("hue_drift".into())).unwrap();
        let (noise_id, noise_params) = (noise.id.clone(), noise.params.clone());
        let (hue_id, hue_params) = (hue.id.clone(), hue.params.clone());

        let mut graph = NodeGraph::new_with_output();
        let n = graph.add_node(NodeKind::Source { effect: noise_id }, 0, &noise_params);
        let h = graph.add_node(NodeKind::Effect { effect: hue_id }, 1, &hue_params);
        let out = graph.output_node();
        graph.connect(n, h, 0).unwrap();
        graph.connect(h, out, 0).unwrap();

        let placeholder = PlaceholderTexture::new(&device, &queue, GpuContext::hdr_format());
        let audio = AudioTextures::new(&device, &queue);
        let mut exec = TramaExecutor::new(&device, &placeholder, &audio, 256, 144);

        let mut template = ShaderUniforms::zeroed();
        template.resolution = [256.0, 144.0];
        let mut last_error = None;

        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        let _ = exec.execute(
            &mut graph,
            &reg,
            &template,
            &device,
            &queue,
            &mut encoder,
            &mut last_error,
        );
        queue.submit([encoder.finish()]);
        let err = pollster::block_on(device.pop_error_scope());
        assert!(err.is_none(), "validation error: {err:?}");
        assert!(last_error.is_none(), "{last_error:?}");
        let v = exec.plan_version();
        let stats = exec.pool_stats();
        // noise renders to a pooled target; hue renders straight into the
        // output target — one pooled texture total.
        assert_eq!(stats, (1, 1), "pool stats");

        // Second frame with unchanged topology: same plan, no new targets.
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        let _ = exec.execute(
            &mut graph,
            &reg,
            &template,
            &device,
            &queue,
            &mut encoder,
            &mut last_error,
        );
        queue.submit([encoder.finish()]);
        assert_eq!(exec.plan_version(), v, "plan reused");
        assert_eq!(exec.pool_stats(), stats, "no new pool targets");
    }

    // Run: cargo test -p fosfora-app -- --ignored trama_executor_black_on_unwired_output
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn trama_executor_black_on_unwired_output() {
        let _guard = gpu_guard();
        let (device, queue) = test_gpu();
        let reg = registry(&device);
        let mut graph = NodeGraph::new_with_output();
        let placeholder = PlaceholderTexture::new(&device, &queue, GpuContext::hdr_format());
        let audio = AudioTextures::new(&device, &queue);
        let mut exec = TramaExecutor::new(&device, &placeholder, &audio, 64, 64);
        let template = ShaderUniforms::zeroed();
        let mut last_error = None;

        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        let _ = exec.execute(
            &mut graph,
            &reg,
            &template,
            &device,
            &queue,
            &mut encoder,
            &mut last_error,
        );
        queue.submit([encoder.finish()]);
        let err = pollster::block_on(device.pop_error_scope());
        assert!(err.is_none(), "validation error: {err:?}");
        assert!(last_error.is_none(), "{last_error:?}");
    }
}
