use wgpu::{CommandEncoder, Device, Queue, TextureFormat};

use crate::effect::format::PassDef;
use crate::effect::EffectLoader;

use super::particle::ParticleSystem;
use super::placeholder::PlaceholderTexture;
use super::render_target::{PingPongTarget, RenderTarget};
use super::uniforms::UniformBuffer;
use super::ShaderPipeline;

/// A compiled pass: pipeline + render target + bind groups.
struct CompiledPass {
    name: String,
    pipeline: ShaderPipeline,
    /// Ping-pong target for this pass (feedback-capable).
    target: PingPongTarget,
    /// Bind groups indexed by ping-pong state.
    bind_groups: [wgpu::BindGroup; 2],
    #[allow(dead_code)]
    scale: f32,
    has_feedback: bool,
}

/// Executes a sequence of render passes for a multi-pass effect.
pub struct PassExecutor {
    passes: Vec<CompiledPass>,
    pub particle_system: Option<ParticleSystem>,
}

impl PassExecutor {
    /// Build a PassExecutor from a list of PassDefs.
    pub fn new(
        device: &Device,
        hdr_format: TextureFormat,
        width: u32,
        height: u32,
        pass_defs: &[PassDef],
        effect_loader: &EffectLoader,
        uniform_buffer: &UniformBuffer,
        placeholder: &PlaceholderTexture,
    ) -> Result<Self, String> {
        let mut passes = Vec::new();

        for def in pass_defs {
            let source = effect_loader
                .load_effect_source(&def.shader)
                .map_err(|e| format!("Failed to load shader '{}': {e}", def.shader))?;

            let pipeline = ShaderPipeline::new(device, hdr_format, &source)
                .map_err(|e| format!("Failed to compile shader '{}': {e}", def.shader))?;

            let target = PingPongTarget::new(device, width, height, hdr_format, def.scale);

            let bind_groups = create_pass_bind_groups(
                device,
                uniform_buffer,
                &pipeline.bind_group_layout,
                &target,
                placeholder,
                def.feedback,
            );

            passes.push(CompiledPass {
                name: def.name.clone(),
                pipeline,
                target,
                bind_groups,
                scale: def.scale,
                has_feedback: def.feedback,
            });
        }

        Ok(Self {
            passes,
            particle_system: None,
        })
    }

    /// Build a single-pass executor (the common case for backward-compatible effects).
    pub fn single_pass(
        pipeline: ShaderPipeline,
        feedback: PingPongTarget,
        uniform_buffer: &UniformBuffer,
        device: &Device,
        placeholder: &PlaceholderTexture,
    ) -> Self {
        let bind_groups = create_pass_bind_groups(
            device,
            uniform_buffer,
            &pipeline.bind_group_layout,
            &feedback,
            placeholder,
            true, // always enable feedback for single-pass mode
        );

        Self {
            passes: vec![CompiledPass {
                name: "main".to_string(),
                pipeline,
                target: feedback,
                bind_groups,
                scale: 1.0,
                has_feedback: true,
            }],
            particle_system: None,
        }
    }

    /// Execute all passes. Returns a reference to the final pass's write target.
    pub fn execute(
        &self,
        encoder: &mut CommandEncoder,
        uniform_buffer: &UniformBuffer,
        queue: &Queue,
        uniforms: &super::ShaderUniforms,
    ) -> &RenderTarget {
        uniform_buffer.update(queue, uniforms);

        // 1. Particle compute dispatch (before fragment passes)
        if let Some(ref ps) = self.particle_system {
            ps.dispatch(encoder, queue);
        }

        // 2. Fragment shader passes
        for pass in &self.passes {
            let write_view = &pass.target.write_target().view;
            let bind_group = &pass.bind_groups[pass.target.current];

            let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(&pass.name),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: write_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            rp.set_pipeline(&pass.pipeline.pipeline);
            rp.set_bind_group(0, bind_group, &[]);
            rp.draw(0..3, 0..1);
        }

        let final_target = self.passes.last().unwrap().target.write_target();

        // 3. Particle render pass — composites on top of last fragment pass with LoadOp::Load
        if let Some(ref ps) = self.particle_system {
            ps.render(encoder, queue, &final_target.view);
        }

        final_target
    }

    /// Flip all feedback-enabled passes for next frame.
    pub fn flip(&mut self) {
        for pass in &mut self.passes {
            if pass.has_feedback {
                pass.target.flip();
            }
        }
        if let Some(ref mut ps) = self.particle_system {
            ps.flip();
        }
    }

    /// Resize all pass targets.
    pub fn resize(
        &mut self,
        device: &Device,
        width: u32,
        height: u32,
        uniform_buffer: &UniformBuffer,
        placeholder: &PlaceholderTexture,
    ) {
        for pass in &mut self.passes {
            pass.target.resize(device, width, height);
            pass.bind_groups = create_pass_bind_groups(
                device,
                uniform_buffer,
                &pass.pipeline.bind_group_layout,
                &pass.target,
                placeholder,
                pass.has_feedback,
            );
        }
    }

    /// Recreate bind groups (e.g., after pipeline rebuild).
    pub fn rebuild_bind_groups(
        &mut self,
        device: &Device,
        uniform_buffer: &UniformBuffer,
        placeholder: &PlaceholderTexture,
    ) {
        for pass in &mut self.passes {
            pass.bind_groups = create_pass_bind_groups(
                device,
                uniform_buffer,
                &pass.pipeline.bind_group_layout,
                &pass.target,
                placeholder,
                pass.has_feedback,
            );
        }
    }

    /// Get the main (first) pass's pipeline for hot-reload.
    pub fn main_pipeline_mut(&mut self) -> &mut ShaderPipeline {
        &mut self.passes[0].pipeline
    }

    /// Get the main pass's bind group layout.
    pub fn main_bind_group_layout(&self) -> &wgpu::BindGroupLayout {
        &self.passes[0].pipeline.bind_group_layout
    }

    /// Number of passes.
    pub fn pass_count(&self) -> usize {
        self.passes.len()
    }

    /// Try to recompile a specific pass's shader (for hot-reload).
    pub fn recompile_pass(
        &mut self,
        pass_index: usize,
        device: &Device,
        hdr_format: TextureFormat,
        source: &str,
        uniform_buffer: &UniformBuffer,
        placeholder: &PlaceholderTexture,
    ) -> Result<(), String> {
        if let Some(pass) = self.passes.get_mut(pass_index) {
            pass.pipeline.recreate_pipeline(device, hdr_format, source)?;
            pass.bind_groups = create_pass_bind_groups(
                device,
                uniform_buffer,
                &pass.pipeline.bind_group_layout,
                &pass.target,
                placeholder,
                pass.has_feedback,
            );
            Ok(())
        } else {
            Err(format!("Pass index {pass_index} out of range"))
        }
    }
}

fn create_pass_bind_groups(
    device: &Device,
    uniform_buffer: &UniformBuffer,
    layout: &wgpu::BindGroupLayout,
    target: &PingPongTarget,
    placeholder: &PlaceholderTexture,
    has_feedback: bool,
) -> [wgpu::BindGroup; 2] {
    if has_feedback {
        // Read from the other target in the pair
        let bg0 = uniform_buffer.create_bind_group(
            device,
            layout,
            &target.targets[1].view,
            &target.targets[1].sampler,
        );
        let bg1 = uniform_buffer.create_bind_group(
            device,
            layout,
            &target.targets[0].view,
            &target.targets[0].sampler,
        );
        [bg0, bg1]
    } else {
        // Use placeholder (1x1 black) for both states
        let bg = uniform_buffer.create_bind_group(
            device,
            layout,
            &placeholder.view,
            &placeholder.sampler,
        );
        let bg2 = uniform_buffer.create_bind_group(
            device,
            layout,
            &placeholder.view,
            &placeholder.sampler,
        );
        [bg, bg2]
    }
}
