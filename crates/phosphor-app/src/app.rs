use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use winit::window::Window;

use crate::audio::AudioSystem;
use crate::effect::EffectLoader;
use crate::gpu::particle::ParticleSystem;
use crate::gpu::pass_executor::PassExecutor;
use crate::gpu::placeholder::PlaceholderTexture;
use crate::gpu::postprocess::PostProcessChain;
use crate::gpu::render_target::PingPongTarget;
use crate::gpu::{GpuContext, ShaderPipeline, ShaderUniforms, UniformBuffer};
use crate::params::ParamStore;
use crate::shader::ShaderWatcher;
use crate::ui::EguiOverlay;

pub struct App {
    pub gpu: GpuContext,
    pub uniform_buffer: UniformBuffer,
    pub uniforms: ShaderUniforms,
    pub start_time: Instant,
    pub last_frame: Instant,
    pub frame_count: u32,
    pub shader_watcher: ShaderWatcher,
    pub current_shader_source: String,
    pub shader_error: Option<String>,
    pub param_store: ParamStore,
    pub audio: AudioSystem,
    pub egui_overlay: EguiOverlay,
    pub effect_loader: EffectLoader,
    pub window: Arc<Window>,
    // Multi-pass rendering
    pub pass_executor: PassExecutor,
    pub post_process: PostProcessChain,
    pub placeholder: PlaceholderTexture,
}

impl App {
    pub fn new(window: Arc<Window>) -> Result<Self> {
        let gpu = GpuContext::new(window.clone())?;
        let hdr_format = GpuContext::hdr_format();

        // Load default effect or fall back to default shader
        let mut effect_loader = EffectLoader::new();
        effect_loader.scan_effects_directory();

        // Prefer plasma_wave as default, fall back to first effect
        let default_idx = effect_loader
            .effects
            .iter()
            .position(|e| e.name == "Plasma Wave")
            .or_else(|| {
                if effect_loader.effects.is_empty() {
                    None
                } else {
                    Some(0)
                }
            });
        let (shader_source, param_store) = if let Some(idx) = default_idx {
            let effect = &effect_loader.effects[idx];
            match effect_loader.load_effect_source(&effect.shader) {
                Ok(source) => {
                    let mut store = ParamStore::new();
                    store.load_from_defs(&effect.inputs);
                    effect_loader.current_effect = Some(idx);
                    (source, store)
                }
                Err(e) => {
                    log::warn!("Failed to load effect: {e}, using default shader");
                    let source = std::fs::read_to_string("assets/shaders/default.wgsl")
                        .unwrap_or_else(|_| {
                            include_str!("../../../assets/shaders/default.wgsl").to_string()
                        });
                    (source, ParamStore::new())
                }
            }
        } else {
            let source = std::fs::read_to_string("assets/shaders/default.wgsl")
                .unwrap_or_else(|_| {
                    include_str!("../../../assets/shaders/default.wgsl").to_string()
                });
            (source, ParamStore::new())
        };

        let pipeline = ShaderPipeline::new(&gpu.device, hdr_format, &shader_source)?;
        let uniform_buffer = UniformBuffer::new(&gpu.device);

        // Placeholder 1x1 black texture
        let placeholder = PlaceholderTexture::new(&gpu.device, &gpu.queue, hdr_format);

        // Ping-pong feedback targets for the single-pass default
        let feedback = PingPongTarget::new(
            &gpu.device,
            gpu.surface_config.width,
            gpu.surface_config.height,
            hdr_format,
            1.0,
        );

        // Build PassExecutor for the initial single-pass effect
        let pass_executor = PassExecutor::single_pass(
            pipeline,
            feedback,
            &uniform_buffer,
            &gpu.device,
            &placeholder,
        );

        // Post-processing chain
        let post_process = PostProcessChain::new(
            &gpu.device,
            gpu.format,
            hdr_format,
            gpu.surface_config.width,
            gpu.surface_config.height,
        );

        let shader_watcher = ShaderWatcher::new()?;
        let audio = AudioSystem::new();
        let egui_overlay = EguiOverlay::new(&gpu.device, gpu.format, &window);

        let now = Instant::now();
        Ok(Self {
            gpu,
            uniform_buffer,
            uniforms: ShaderUniforms::zeroed(),
            start_time: now,
            last_frame: now,
            frame_count: 0,
            shader_watcher,
            current_shader_source: shader_source,
            shader_error: None,
            param_store,
            audio,
            egui_overlay,
            effect_loader,
            window,
            pass_executor,
            post_process,
            placeholder,
        })
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.gpu.resize(width, height);
        self.pass_executor.resize(
            &self.gpu.device,
            width,
            height,
            &self.uniform_buffer,
            &self.placeholder,
        );
        self.post_process.resize(&self.gpu.device, width, height);

        self.egui_overlay
            .resize(width, height, self.window.scale_factor() as f32);
    }

    pub fn update(&mut self) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32();
        self.last_frame = now;

        // Update time uniforms
        self.uniforms.time = now.duration_since(self.start_time).as_secs_f32();
        self.uniforms.delta_time = dt;
        self.uniforms.resolution = [
            self.gpu.surface_config.width as f32,
            self.gpu.surface_config.height as f32,
        ];

        // Feedback uniforms
        self.uniforms.feedback_decay = 0.88;
        self.uniforms.frame_index = self.frame_count as f32;

        // Drain audio features
        if let Some(features) = self.audio.latest_features() {
            self.uniforms.bass = features.bass;
            self.uniforms.mid = features.mid;
            self.uniforms.treble = features.treble;
            self.uniforms.rms = features.rms;
            self.uniforms.phase = features.phase;
            self.uniforms.onset = features.onset;
            self.uniforms.centroid = features.centroid;
            self.uniforms.flux = features.flux;
            self.uniforms.flatness = features.flatness;
            self.uniforms.rolloff = features.rolloff;
            self.uniforms.bandwidth = features.bandwidth;
            self.uniforms.zcr = features.zcr;
        }

        // Pack params
        self.uniforms.params = self.param_store.pack_to_buffer();

        // Update particle system uniforms
        if let Some(ref mut ps) = self.pass_executor.particle_system {
            ps.update_uniforms(
                dt,
                self.uniforms.time,
                self.uniforms.resolution,
                self.uniforms.onset,
            );
            ps.update_audio(
                self.uniforms.bass,
                self.uniforms.mid,
                self.uniforms.treble,
                self.uniforms.rms,
                self.uniforms.onset,
                self.uniforms.centroid,
                self.uniforms.flux,
                self.uniforms.flatness,
            );
        }

        // Check for shader changes — only reload if a relevant file changed
        let changes = self.shader_watcher.drain_changes();
        if !changes.is_empty() {
            if let Some(idx) = self.effect_loader.current_effect {
                // Clone what we need to avoid borrow conflicts with self
                let effect = self.effect_loader.effects[idx].clone();
                let main_shader = if !effect.passes.is_empty() {
                    effect.passes[0].shader.clone()
                } else {
                    effect.shader.clone()
                };
                let is_relevant = changes.iter().any(|p| {
                    p.ends_with(&main_shader) || p.to_string_lossy().contains("/lib/")
                });
                if is_relevant {
                    match self.effect_loader.load_effect_source(&main_shader) {
                        Ok(source) if source != self.current_shader_source => {
                            log::info!(
                                "Shader file changed: {}",
                                changes
                                    .iter()
                                    .map(|p| p.display().to_string())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            );
                            self.try_recompile_shader(&source);
                        }
                        Ok(_) => {} // content unchanged, skip
                        Err(e) => {
                            log::error!("Failed to reload shader: {e}");
                            self.shader_error = Some(format!("Read error: {e}"));
                        }
                    }
                }

                // Hot-reload compute shader if applicable
                if let Some(ref particle_def) = effect.particles {
                    if !particle_def.compute_shader.is_empty() {
                        let compute_relevant = changes.iter().any(|p| {
                            p.ends_with(&particle_def.compute_shader)
                        });
                        if compute_relevant {
                            if let Some(ref mut ps) = self.pass_executor.particle_system {
                                match self.effect_loader.load_compute_source(&particle_def.compute_shader) {
                                    Ok(src) if src != ps.current_compute_source => {
                                        match ps.recompile_compute(&self.gpu.device, &src) {
                                            Ok(()) => {
                                                ps.current_compute_source = src;
                                                log::info!("Compute shader recompiled");
                                            }
                                            Err(e) => log::error!("Compute shader error: {e}"),
                                        }
                                    }
                                    Ok(_) => {} // content unchanged, skip
                                    Err(e) => log::error!("Failed to reload compute shader: {e}"),
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    pub fn try_recompile_shader(&mut self, source: &str) {
        let hdr_format = GpuContext::hdr_format();
        match self.pass_executor.main_pipeline_mut().recreate_pipeline(
            &self.gpu.device,
            hdr_format,
            source,
        ) {
            Ok(()) => {
                self.current_shader_source = source.to_string();
                self.shader_error = None;
                self.pass_executor.rebuild_bind_groups(
                    &self.gpu.device,
                    &self.uniform_buffer,
                    &self.placeholder,
                );
                log::info!("Shader recompiled successfully");
            }
            Err(e) => {
                log::error!("Shader compilation failed: {e}");
                self.shader_error = Some(e);
            }
        }
    }

    /// Build a ParticleSystem from a ParticleDef, or None if the effect doesn't use particles.
    fn build_particle_system(
        &self,
        particles: &crate::gpu::particle::types::ParticleDef,
    ) -> Option<ParticleSystem> {
        let hdr_format = GpuContext::hdr_format();

        // Load compute shader source
        let compute_source = if particles.compute_shader.is_empty() {
            // Use builtin default
            include_str!("../../../assets/shaders/builtin/particle_sim.wgsl").to_string()
        } else {
            match self.effect_loader.load_compute_source(&particles.compute_shader) {
                Ok(src) => src,
                Err(e) => {
                    log::error!("Failed to load compute shader '{}': {e}", particles.compute_shader);
                    return None;
                }
            }
        };

        match ParticleSystem::new(&self.gpu.device, hdr_format, particles, &compute_source) {
            Ok(ps) => {
                log::info!("Particle system created: {} particles", particles.max_count);
                Some(ps)
            }
            Err(e) => {
                log::error!("Failed to create particle system: {e}");
                None
            }
        }
    }

    pub fn load_effect(&mut self, index: usize) {
        if let Some(effect) = self.effect_loader.effects.get(index).cloned() {
            let hdr_format = GpuContext::hdr_format();
            let uses_passes = !effect.passes.is_empty();

            if uses_passes {
                // Effect defines explicit passes — build a new PassExecutor
                match PassExecutor::new(
                    &self.gpu.device,
                    hdr_format,
                    self.gpu.surface_config.width,
                    self.gpu.surface_config.height,
                    &effect.passes,
                    &self.effect_loader,
                    &self.uniform_buffer,
                    &self.placeholder,
                ) {
                    Ok(mut executor) => {
                        // Build particle system if effect defines one
                        if let Some(ref particle_def) = effect.particles {
                            executor.particle_system = self.build_particle_system(particle_def);
                        }
                        self.pass_executor = executor;
                        self.param_store.load_from_defs(&effect.inputs);
                        self.shader_error = None;
                        self.effect_loader.current_effect = Some(index);
                        self.shader_watcher.drain_changes();
                        // Track the first pass's shader source for hot-reload
                        if let Ok(src) = self.effect_loader.load_effect_source(&effect.passes[0].shader) {
                            self.current_shader_source = src;
                        }
                        log::info!("Loaded effect: {} ({} passes)", effect.name, effect.passes.len());
                    }
                    Err(e) => {
                        log::error!("Failed to load effect '{}': {e}", effect.name);
                        self.shader_error = Some(format!("Load error: {e}"));
                    }
                }
            } else {
                // Single-pass effect (backward compatible): hot-reloadable path
                match self.effect_loader.load_effect_source(&effect.shader) {
                    Ok(source) => {
                        self.param_store.load_from_defs(&effect.inputs);
                        self.try_recompile_shader(&source);
                        // Build particle system if effect defines one
                        if let Some(ref particle_def) = effect.particles {
                            self.pass_executor.particle_system =
                                self.build_particle_system(particle_def);
                        } else {
                            self.pass_executor.particle_system = None;
                        }
                        self.effect_loader.current_effect = Some(index);
                        self.shader_watcher.drain_changes();
                        log::info!("Loaded effect: {}", effect.name);
                    }
                    Err(e) => {
                        log::error!("Failed to load effect '{}': {e}", effect.name);
                        self.shader_error = Some(format!("Load error: {e}"));
                    }
                }
            }
        }
    }

    pub fn render(&mut self) -> Result<(), wgpu::SurfaceError> {
        let output = self.gpu.surface.get_current_texture()?;
        let surface_view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder =
            self.gpu
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("phosphor-encoder"),
                });

        // Execute all effect passes (single or multi-pass)
        let final_target = self.pass_executor.execute(
            &mut encoder,
            &self.uniform_buffer,
            &self.gpu.queue,
            &self.uniforms,
        );

        // Post-processing → surface
        self.post_process.render(
            &self.gpu.device,
            &self.gpu.queue,
            &mut encoder,
            final_target,
            &surface_view,
            self.uniforms.time,
            self.uniforms.rms,
            self.uniforms.onset,
            self.uniforms.flatness,
        );

        // Flip ping-pong for next frame
        self.pass_executor.flip();
        self.frame_count = self.frame_count.wrapping_add(1);

        // egui overlay → surface
        self.egui_overlay
            .render(&self.gpu.device, &self.gpu.queue, &mut encoder, &surface_view);

        self.gpu.queue.submit(std::iter::once(encoder.finish()));
        output.present();

        Ok(())
    }
}

impl ShaderUniforms {
    pub fn zeroed() -> Self {
        bytemuck::Zeroable::zeroed()
    }
}
