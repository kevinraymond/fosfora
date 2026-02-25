use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use winit::window::Window;

use crate::audio::AudioSystem;
use crate::effect::format::PostProcessDef;
use crate::effect::EffectLoader;
use crate::gpu::particle::ParticleSystem;
use crate::gpu::pass_executor::PassExecutor;
use crate::gpu::placeholder::PlaceholderTexture;
use crate::gpu::postprocess::PostProcessChain;
use crate::gpu::render_target::PingPongTarget;
use crate::gpu::{GpuContext, ShaderPipeline, ShaderUniforms, UniformBuffer};
use crate::midi::types::TriggerAction;
use crate::midi::MidiSystem;
use crate::params::ParamStore;
use crate::preset::PresetStore;
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
    pub current_shader_sources: Vec<String>,
    pub shader_error: Option<String>,
    pub param_store: ParamStore,
    pub audio: AudioSystem,
    pub egui_overlay: EguiOverlay,
    pub effect_loader: EffectLoader,
    pub window: Arc<Window>,
    // MIDI
    pub midi: MidiSystem,
    pub pending_midi_triggers: Vec<TriggerAction>,
    // Presets
    pub preset_store: PresetStore,
    // Multi-pass rendering
    pub pass_executor: PassExecutor,
    pub post_process: PostProcessChain,
    pub current_postprocess: PostProcessDef,
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
        let midi = MidiSystem::new();
        let mut preset_store = PresetStore::new();
        preset_store.scan();
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
            current_shader_sources: vec![shader_source],
            shader_error: None,
            param_store,
            audio,
            midi,
            pending_midi_triggers: Vec::new(),
            preset_store,
            egui_overlay,
            effect_loader,
            window,
            pass_executor,
            post_process,
            current_postprocess: PostProcessDef::default(),
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
            self.uniforms.sub_bass = features.sub_bass;
            self.uniforms.bass = features.bass;
            self.uniforms.low_mid = features.low_mid;
            self.uniforms.mid = features.mid;
            self.uniforms.upper_mid = features.upper_mid;
            self.uniforms.presence = features.presence;
            self.uniforms.brilliance = features.brilliance;
            self.uniforms.rms = features.rms;
            self.uniforms.kick = features.kick;
            self.uniforms.centroid = features.centroid;
            self.uniforms.flux = features.flux;
            self.uniforms.flatness = features.flatness;
            self.uniforms.rolloff = features.rolloff;
            self.uniforms.bandwidth = features.bandwidth;
            self.uniforms.zcr = features.zcr;
            self.uniforms.onset = features.onset;
            self.uniforms.beat = features.beat;
            self.uniforms.beat_phase = features.beat_phase;
            self.uniforms.bpm = features.bpm;
            self.uniforms.beat_strength = features.beat_strength;
        }

        // Drain MIDI and apply mappings
        let defs = self.param_store.defs.clone();
        let midi_result = self.midi.update(&mut self.param_store, &defs);
        self.pending_midi_triggers = midi_result.triggers;

        // Pack params
        self.uniforms.params = self.param_store.pack_to_buffer();

        // Update particle system uniforms
        if let Some(ref mut ps) = self.pass_executor.particle_system {
            ps.update_uniforms(
                dt,
                self.uniforms.time,
                self.uniforms.resolution,
                self.uniforms.beat,
            );
            ps.update_audio(
                self.uniforms.sub_bass,
                self.uniforms.bass,
                self.uniforms.mid,
                self.uniforms.rms,
                self.uniforms.kick,
                self.uniforms.onset,
                self.uniforms.centroid,
                self.uniforms.flux,
                self.uniforms.beat,
                self.uniforms.beat_phase,
            );
        }

        // Check for shader changes — only reload if a relevant file changed
        let changes = self.shader_watcher.drain_changes();
        if !changes.is_empty() {
            if let Some(idx) = self.effect_loader.current_effect {
                let effect = self.effect_loader.effects[idx].clone();
                let passes = effect.normalized_passes();
                let lib_changed = changes.iter().any(|p| p.to_string_lossy().contains("/lib/"));

                // Hot-reload each pass's fragment shader
                let hdr_format = GpuContext::hdr_format();
                for (i, pass_def) in passes.iter().enumerate() {
                    let pass_relevant = lib_changed
                        || changes.iter().any(|p| p.ends_with(&pass_def.shader));
                    if !pass_relevant {
                        continue;
                    }
                    match self.effect_loader.load_effect_source(&pass_def.shader) {
                        Ok(source) => {
                            let changed = self
                                .current_shader_sources
                                .get(i)
                                .map_or(true, |prev| *prev != source);
                            if changed {
                                log::info!("Shader changed: pass {} ({})", i, pass_def.shader);
                                match self.pass_executor.recompile_pass(
                                    i,
                                    &self.gpu.device,
                                    hdr_format,
                                    &source,
                                    &self.uniform_buffer,
                                    &self.placeholder,
                                ) {
                                    Ok(()) => {
                                        if i < self.current_shader_sources.len() {
                                            self.current_shader_sources[i] = source;
                                        }
                                        self.shader_error = None;
                                        log::info!("Pass {} recompiled successfully", i);
                                    }
                                    Err(e) => {
                                        log::error!("Pass {} compilation failed: {e}", i);
                                        self.shader_error = Some(e);
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            log::error!("Failed to reload shader for pass {}: {e}", i);
                            self.shader_error = Some(format!("Read error: {e}"));
                        }
                    }
                }

                // Hot-reload compute shader if applicable
                if let Some(ref particle_def) = effect.particles {
                    if !particle_def.compute_shader.is_empty() {
                        let compute_relevant = changes
                            .iter()
                            .any(|p| p.ends_with(&particle_def.compute_shader));
                        if compute_relevant {
                            if let Some(ref mut ps) = self.pass_executor.particle_system {
                                match self
                                    .effect_loader
                                    .load_compute_source(&particle_def.compute_shader)
                                {
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
                                    Err(e) => {
                                        log::error!("Failed to reload compute shader: {e}")
                                    }
                                }
                            }
                        }
                    }
                }
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
            let passes = effect.normalized_passes();
            if passes.is_empty() {
                log::error!("Effect '{}' has no shader or passes defined", effect.name);
                return;
            }

            match PassExecutor::new(
                &self.gpu.device,
                hdr_format,
                self.gpu.surface_config.width,
                self.gpu.surface_config.height,
                &passes,
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
                    // Apply per-effect postprocess overrides
                    let pp = effect.postprocess.clone().unwrap_or_default();
                    self.post_process.enabled = pp.enabled;
                    self.current_postprocess = pp;
                    // Track shader sources for hot-reload
                    self.current_shader_sources = passes
                        .iter()
                        .filter_map(|p| self.effect_loader.load_effect_source(&p.shader).ok())
                        .collect();
                    log::info!(
                        "Loaded effect: {} ({} pass{})",
                        effect.name,
                        passes.len(),
                        if passes.len() == 1 { "" } else { "es" }
                    );
                }
                Err(e) => {
                    log::error!("Failed to load effect '{}': {e}", effect.name);
                    self.shader_error = Some(format!("Load error: {e}"));
                }
            }
        }
    }

    pub fn save_preset(&mut self, name: &str) {
        let effect_name = self
            .effect_loader
            .current_effect
            .and_then(|i| self.effect_loader.effects.get(i))
            .map(|e| e.name.clone())
            .unwrap_or_default();
        if effect_name.is_empty() {
            log::warn!("No effect loaded, cannot save preset");
            return;
        }
        match self.preset_store.save(
            name,
            &effect_name,
            &self.param_store.values,
            &self.current_postprocess,
        ) {
            Ok(idx) => log::info!("Saved preset '{}' at index {}", name, idx),
            Err(e) => log::error!("Failed to save preset: {e}"),
        }
    }

    pub fn load_preset(&mut self, index: usize) {
        let preset = match self.preset_store.load(index) {
            Some(p) => p.clone(),
            None => return,
        };

        // Find and load the effect by name
        let effect_idx = self
            .effect_loader
            .effects
            .iter()
            .position(|e| e.name == preset.effect_name);
        match effect_idx {
            Some(idx) => {
                self.load_effect(idx);
                // Apply saved params (skip unknown)
                for (name, value) in &preset.params {
                    if self.param_store.values.contains_key(name) {
                        self.param_store.set(name, value.clone());
                    } else {
                        log::warn!("Preset param '{}' not found in effect, skipping", name);
                    }
                }
                // Apply postprocess
                self.current_postprocess = preset.postprocess.clone();
                self.post_process.enabled = preset.postprocess.enabled;
                self.preset_store.current_preset = Some(index);
                log::info!("Loaded preset '{}'", self.preset_store.presets[index].0);
            }
            None => {
                log::warn!(
                    "Effect '{}' not found for preset, skipping load",
                    preset.effect_name
                );
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
            &self.current_postprocess,
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
