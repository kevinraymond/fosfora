use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use winit::window::Window;

use crate::audio::AudioSystem;
use crate::effect::format::PostProcessDef;
use crate::effect::EffectLoader;
use crate::gpu::compositor::Compositor;
use crate::gpu::layer::{BlendMode, Layer, LayerInfo, LayerStack};
use crate::gpu::particle::ParticleSystem;
use crate::gpu::pass_executor::PassExecutor;
use crate::gpu::placeholder::PlaceholderTexture;
use crate::gpu::postprocess::PostProcessChain;
use crate::gpu::render_target::PingPongTarget;
use crate::gpu::{GpuContext, ShaderPipeline, ShaderUniforms, UniformBuffer};
use crate::midi::types::TriggerAction;
use crate::midi::MidiSystem;
use crate::osc::OscSystem;
use crate::params::{ParamStore, ParamValue};
use crate::preset::store::LayerPreset;
use crate::preset::PresetStore;
use crate::shader::ShaderWatcher;
use crate::ui::EguiOverlay;

pub struct App {
    pub gpu: GpuContext,
    pub start_time: Instant,
    pub last_frame: Instant,
    pub frame_count: u32,
    pub shader_watcher: ShaderWatcher,
    pub audio: AudioSystem,
    pub egui_overlay: EguiOverlay,
    pub effect_loader: EffectLoader,
    pub window: Arc<Window>,
    // MIDI
    pub midi: MidiSystem,
    pub pending_midi_triggers: Vec<TriggerAction>,
    // OSC
    pub osc: OscSystem,
    pub pending_osc_triggers: Vec<TriggerAction>,
    pub latest_audio: Option<crate::audio::features::AudioFeatures>,
    // Presets
    pub preset_store: PresetStore,
    // Layers
    pub layer_stack: LayerStack,
    // Compositor + post-processing (separate from layer_stack to avoid borrow conflicts)
    pub compositor: Compositor,
    pub post_process: PostProcessChain,
    pub placeholder: PlaceholderTexture,
    // Global uniforms template (time, audio, etc. — params overwritten per-layer)
    pub uniforms: ShaderUniforms,
}

impl App {
    pub fn new(window: Arc<Window>) -> Result<Self> {
        let gpu = GpuContext::new(window.clone())?;
        let hdr_format = GpuContext::hdr_format();

        // Load default effect or fall back to default shader
        let mut effect_loader = EffectLoader::new();
        effect_loader.scan_effects_directory();

        // Prefer Aurora as default, fall back to first effect
        let default_idx = effect_loader
            .effects
            .iter()
            .position(|e| e.name == "Aurora")
            .or_else(|| {
                if effect_loader.effects.is_empty() {
                    None
                } else {
                    Some(0)
                }
            });
        let (shader_source, param_store, effect_index) = if let Some(idx) = default_idx {
            let effect = &effect_loader.effects[idx];
            match effect_loader.load_effect_source(&effect.shader) {
                Ok(source) => {
                    let mut store = ParamStore::new();
                    store.load_from_defs(&effect.inputs);
                    effect_loader.current_effect = Some(idx);
                    (source, store, Some(idx))
                }
                Err(e) => {
                    log::warn!("Failed to load effect: {e}, using default shader");
                    let source = std::fs::read_to_string("assets/shaders/default.wgsl")
                        .unwrap_or_else(|_| {
                            include_str!("../../../assets/shaders/default.wgsl").to_string()
                        });
                    (source, ParamStore::new(), None)
                }
            }
        } else {
            let source = std::fs::read_to_string("assets/shaders/default.wgsl")
                .unwrap_or_else(|_| {
                    include_str!("../../../assets/shaders/default.wgsl").to_string()
                });
            (source, ParamStore::new(), None)
        };

        let pipeline = ShaderPipeline::new(&gpu.device, hdr_format, &shader_source)?;

        // Placeholder 1x1 black texture
        let placeholder = PlaceholderTexture::new(&gpu.device, &gpu.queue, hdr_format);

        // Build initial layer with default effect
        let uniform_buffer = UniformBuffer::new(&gpu.device);
        let feedback = PingPongTarget::new(
            &gpu.device,
            gpu.surface_config.width,
            gpu.surface_config.height,
            hdr_format,
            1.0,
        );
        let pass_executor = PassExecutor::single_pass(
            pipeline,
            feedback,
            &uniform_buffer,
            &gpu.device,
            &placeholder,
        );

        let initial_layer = Layer {
            name: "Layer 1".to_string(),
            custom_name: None,
            effect_index,
            pass_executor,
            uniform_buffer,
            param_store,
            uniforms: ShaderUniforms::zeroed(),
            blend_mode: BlendMode::Normal,
            opacity: 1.0,
            enabled: true,
            locked: false,
            pinned: false,
            shader_sources: vec![shader_source],
            shader_error: None,
            postprocess: PostProcessDef::default(),
        };

        let mut layer_stack = LayerStack::new();
        layer_stack.layers.push(initial_layer);

        // Compositor
        let compositor = Compositor::new(
            &gpu.device,
            hdr_format,
            gpu.surface_config.width,
            gpu.surface_config.height,
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
        let osc = OscSystem::new();
        let mut preset_store = PresetStore::new();
        preset_store.scan();
        let egui_overlay = EguiOverlay::new(&gpu.device, gpu.format, &window);

        let now = Instant::now();
        Ok(Self {
            gpu,
            uniforms: ShaderUniforms::zeroed(),
            start_time: now,
            last_frame: now,
            frame_count: 0,
            shader_watcher,
            audio,
            midi,
            pending_midi_triggers: Vec::new(),
            osc,
            pending_osc_triggers: Vec::new(),
            latest_audio: None,
            preset_store,
            egui_overlay,
            effect_loader,
            window,
            layer_stack,
            compositor,
            post_process,
            placeholder,
        })
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.gpu.resize(width, height);
        for layer in &mut self.layer_stack.layers {
            layer.resize(&self.gpu.device, width, height, &self.placeholder);
        }
        self.compositor.resize(&self.gpu.device, width, height);
        self.post_process.resize(&self.gpu.device, width, height);
        self.egui_overlay
            .resize(width, height, self.window.scale_factor() as f32);
    }

    pub fn update(&mut self) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32();
        self.last_frame = now;

        // Update global time uniforms
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
            self.latest_audio = Some(features);
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

        // Drain MIDI and apply to active layer's param_store (skip if locked)
        if let Some(layer) = self.layer_stack.active_mut() {
            let locked = layer.locked;
            let defs = layer.param_store.defs.clone();
            if locked {
                // Still drain MIDI messages but only collect triggers, don't apply CC to params
                let midi_result = self.midi.update_triggers_only();
                self.pending_midi_triggers = midi_result.triggers;
            } else {
                let midi_result = self.midi.update(&mut layer.param_store, &defs);
                self.pending_midi_triggers = midi_result.triggers;
            }
        }

        // Drain OSC and apply to active layer's param_store (runs after MIDI — last-write-wins)
        if let Some(layer) = self.layer_stack.active_mut() {
            let locked = layer.locked;
            let defs = layer.param_store.defs.clone();
            let osc_result = if locked {
                self.osc.update_triggers_only()
            } else {
                self.osc.update(&mut layer.param_store, &defs)
            };
            self.pending_osc_triggers = osc_result.triggers;

            // Apply layer-targeted OSC messages
            for (layer_idx, name, value) in osc_result.layer_params {
                if let Some(target_layer) = self.layer_stack.layers.get_mut(layer_idx) {
                    if !target_layer.locked {
                        if let Some(def) = target_layer.param_store.defs.iter().find(|d| d.name() == name) {
                            match def.clone() {
                                crate::params::ParamDef::Float { min, max, .. } => {
                                    let val = min + (max - min) * value.clamp(0.0, 1.0);
                                    target_layer.param_store.set(&name, ParamValue::Float(val));
                                }
                                crate::params::ParamDef::Bool { .. } => {
                                    target_layer.param_store.set(&name, ParamValue::Bool(value > 0.5));
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
            for (layer_idx, value) in osc_result.layer_opacity {
                if let Some(target_layer) = self.layer_stack.layers.get_mut(layer_idx) {
                    if !target_layer.locked {
                        target_layer.opacity = value;
                    }
                }
            }
            for (layer_idx, value) in osc_result.layer_blend {
                if let Some(target_layer) = self.layer_stack.layers.get_mut(layer_idx) {
                    if !target_layer.locked {
                        use crate::gpu::layer::BlendMode;
                        target_layer.blend_mode = match value {
                            0 => BlendMode::Normal,
                            1 => BlendMode::Add,
                            2 => BlendMode::Multiply,
                            3 => BlendMode::Screen,
                            4 => BlendMode::Overlay,
                            5 => BlendMode::SoftLight,
                            6 => BlendMode::Difference,
                            _ => BlendMode::Normal,
                        };
                    }
                }
            }
            for (layer_idx, value) in osc_result.layer_enabled {
                if let Some(target_layer) = self.layer_stack.layers.get_mut(layer_idx) {
                    if !target_layer.locked {
                        target_layer.enabled = value;
                    }
                }
            }
            if let Some(pp_enabled) = osc_result.postprocess_enabled {
                self.post_process.enabled = pp_enabled;
            }
        }

        // OSC TX: send audio features + state (throttled internally)
        if let Some(features) = self.latest_audio {
            let active = self.layer_stack.active_layer;
            let effect_name = self.layer_stack.active()
                .and_then(|l| l.effect_index)
                .and_then(|i| self.effect_loader.effects.get(i))
                .map(|e| e.name.as_str())
                .unwrap_or("");
            self.osc.send_state(&features, active, effect_name);
        }

        // Update each layer's uniforms from global template + per-layer params
        for layer in &mut self.layer_stack.layers {
            layer.uniforms = self.uniforms;
            layer.uniforms.params = layer.param_store.pack_to_buffer();

            // Update particle systems
            if let Some(ref mut ps) = layer.pass_executor.particle_system {
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
        }

        // Shader hot-reload — iterate all layers
        let changes = self.shader_watcher.drain_changes();
        if !changes.is_empty() {
            let lib_changed = changes.iter().any(|p| p.to_string_lossy().contains("/lib/"));
            let hdr_format = GpuContext::hdr_format();

            for layer in &mut self.layer_stack.layers {
                let effect_idx = match layer.effect_index {
                    Some(idx) => idx,
                    None => continue,
                };
                let effect = match self.effect_loader.effects.get(effect_idx) {
                    Some(e) => e.clone(),
                    None => continue,
                };
                let passes = effect.normalized_passes();

                // Hot-reload fragment shaders
                for (i, pass_def) in passes.iter().enumerate() {
                    let pass_relevant =
                        lib_changed || changes.iter().any(|p| p.ends_with(&pass_def.shader));
                    if !pass_relevant {
                        continue;
                    }
                    match self.effect_loader.load_effect_source(&pass_def.shader) {
                        Ok(source) => {
                            let changed = layer
                                .shader_sources
                                .get(i)
                                .map_or(true, |prev| *prev != source);
                            if changed {
                                log::info!("Shader changed: pass {} ({})", i, pass_def.shader);
                                match layer.pass_executor.recompile_pass(
                                    i,
                                    &self.gpu.device,
                                    hdr_format,
                                    &source,
                                    &layer.uniform_buffer,
                                    &self.placeholder,
                                ) {
                                    Ok(()) => {
                                        if i < layer.shader_sources.len() {
                                            layer.shader_sources[i] = source;
                                        }
                                        layer.shader_error = None;
                                        log::info!("Pass {} recompiled successfully", i);
                                    }
                                    Err(e) => {
                                        log::error!("Pass {} compilation failed: {e}", i);
                                        layer.shader_error = Some(e);
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            log::error!("Failed to reload shader for pass {}: {e}", i);
                            layer.shader_error = Some(format!("Read error: {e}"));
                        }
                    }
                }

                // Hot-reload compute shader
                if let Some(ref particle_def) = effect.particles {
                    if !particle_def.compute_shader.is_empty() {
                        let compute_relevant = changes
                            .iter()
                            .any(|p| p.ends_with(&particle_def.compute_shader));
                        if compute_relevant {
                            if let Some(ref mut ps) = layer.pass_executor.particle_system {
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
                                    Ok(_) => {}
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

        let compute_source = if particles.compute_shader.is_empty() {
            include_str!("../../../assets/shaders/builtin/particle_sim.wgsl").to_string()
        } else {
            match self.effect_loader.load_compute_source(&particles.compute_shader) {
                Ok(src) => src,
                Err(e) => {
                    log::error!(
                        "Failed to load compute shader '{}': {e}",
                        particles.compute_shader
                    );
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

    /// Load an effect on the active layer.
    pub fn load_effect(&mut self, index: usize) {
        self.load_effect_on_layer(self.layer_stack.active_layer, index);
    }

    /// Load an effect on a specific layer.
    pub fn load_effect_on_layer(&mut self, layer_idx: usize, effect_index: usize) {
        let effect = match self.effect_loader.effects.get(effect_index).cloned() {
            Some(e) => e,
            None => return,
        };
        if layer_idx >= self.layer_stack.layers.len() {
            return;
        }

        let hdr_format = GpuContext::hdr_format();
        let passes = effect.normalized_passes();
        if passes.is_empty() {
            log::error!("Effect '{}' has no shader or passes defined", effect.name);
            return;
        }

        // Build particle system before borrowing layer mutably (avoids borrow conflict)
        let particle_system = effect
            .particles
            .as_ref()
            .and_then(|pd| self.build_particle_system(pd));

        // Need the layer's uniform buffer reference for PassExecutor::new.
        // Use a temporary reference scope.
        let uniform_buffer_ref = &self.layer_stack.layers[layer_idx].uniform_buffer;
        let executor_result = PassExecutor::new(
            &self.gpu.device,
            hdr_format,
            self.gpu.surface_config.width,
            self.gpu.surface_config.height,
            &passes,
            &self.effect_loader,
            uniform_buffer_ref,
            &self.placeholder,
        );

        match executor_result {
            Ok(mut executor) => {
                executor.particle_system = particle_system;

                let layer = &mut self.layer_stack.layers[layer_idx];
                layer.pass_executor = executor;
                layer.param_store.load_from_defs(&effect.inputs);
                layer.shader_error = None;
                layer.effect_index = Some(effect_index);
                // Apply per-effect postprocess overrides
                let pp = effect.postprocess.clone().unwrap_or_default();
                layer.postprocess = pp.clone();
                // If this is the active layer, update global postprocess
                if layer_idx == self.layer_stack.active_layer {
                    self.post_process.enabled = pp.enabled;
                    self.effect_loader.current_effect = Some(effect_index);
                }
                // Track shader sources for hot-reload
                layer.shader_sources = passes
                    .iter()
                    .filter_map(|p| self.effect_loader.load_effect_source(&p.shader).ok())
                    .collect();
                self.shader_watcher.drain_changes();
                log::info!(
                    "Layer {}: loaded effect '{}' ({} pass{})",
                    layer_idx,
                    effect.name,
                    passes.len(),
                    if passes.len() == 1 { "" } else { "es" }
                );
            }
            Err(e) => {
                log::error!("Failed to load effect '{}': {e}", effect.name);
                self.layer_stack.layers[layer_idx].shader_error =
                    Some(format!("Load error: {e}"));
            }
        }
    }

    /// Add a new empty layer with the default shader.
    pub fn add_layer(&mut self) {
        let num = self.layer_stack.layers.len();
        if num >= 8 {
            return;
        }
        let name = format!("Layer {}", num + 1);
        let hdr_format = GpuContext::hdr_format();

        let source = std::fs::read_to_string("assets/shaders/default.wgsl").unwrap_or_else(|_| {
            include_str!("../../../assets/shaders/default.wgsl").to_string()
        });

        let uniform_buffer = UniformBuffer::new(&self.gpu.device);
        let feedback = PingPongTarget::new(
            &self.gpu.device,
            self.gpu.surface_config.width,
            self.gpu.surface_config.height,
            hdr_format,
            1.0,
        );

        match ShaderPipeline::new(&self.gpu.device, hdr_format, &source) {
            Ok(pipeline) => {
                let executor = PassExecutor::single_pass(
                    pipeline,
                    feedback,
                    &uniform_buffer,
                    &self.gpu.device,
                    &self.placeholder,
                );

                self.layer_stack.layers.push(Layer {
                    name,
                    custom_name: None,
                    effect_index: None,
                    pass_executor: executor,
                    uniform_buffer,
                    param_store: ParamStore::new(),
                    uniforms: ShaderUniforms::zeroed(),
                    blend_mode: BlendMode::Normal,
                    opacity: 1.0,
                    enabled: true,
                    locked: false,
                    pinned: false,
                    shader_sources: vec![source],
                    shader_error: None,
                    postprocess: PostProcessDef::default(),
                });
                // Select the new layer
                self.layer_stack.active_layer = self.layer_stack.layers.len() - 1;
                log::info!("Added layer {}", self.layer_stack.layers.len());
            }
            Err(e) => {
                log::error!("Failed to create layer: {e}");
            }
        }
    }

    /// Remove all layers and create one fresh empty layer.
    pub fn clear_all_layers(&mut self) {
        self.layer_stack.layers.clear();
        self.layer_stack.active_layer = 0;
        self.add_layer();
    }

    /// Sync effect_loader.current_effect to match active layer.
    pub fn sync_active_layer(&mut self) {
        if let Some(layer) = self.layer_stack.active() {
            self.effect_loader.current_effect = layer.effect_index;
        }
    }

    pub fn save_preset(&mut self, name: &str) {
        let layer_presets: Vec<LayerPreset> = self
            .layer_stack
            .layers
            .iter()
            .map(|l| {
                let effect_name = l
                    .effect_index
                    .and_then(|i| self.effect_loader.effects.get(i))
                    .map(|e| e.name.clone())
                    .unwrap_or_default();
                LayerPreset {
                    effect_name,
                    params: l.param_store.values.clone(),
                    blend_mode: l.blend_mode,
                    opacity: l.opacity,
                    enabled: l.enabled,
                    locked: l.locked,
                    pinned: l.pinned,
                    custom_name: l.custom_name.clone(),
                }
            })
            .collect();

        if layer_presets.iter().all(|l| l.effect_name.is_empty()) {
            log::warn!("No effects loaded, cannot save preset");
            return;
        }

        let postprocess = self.current_postprocess();
        match self.preset_store.save(
            name,
            layer_presets,
            self.layer_stack.active_layer,
            &postprocess,
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

        // Remove extra layers or add missing ones to match preset
        while self.layer_stack.layers.len() > preset.layers.len()
            && self.layer_stack.layers.len() > 1
        {
            let last = self.layer_stack.layers.len() - 1;
            self.layer_stack.layers.remove(last);
        }
        while self.layer_stack.layers.len() < preset.layers.len() {
            self.add_layer();
        }

        // Load each layer (skip locked layers)
        for (i, lp) in preset.layers.iter().enumerate() {
            if let Some(layer) = self.layer_stack.layers.get(i) {
                if layer.locked {
                    log::info!("Layer {} is locked, skipping preset load", i);
                    continue;
                }
            }

            if !lp.effect_name.is_empty() {
                let effect_idx = self
                    .effect_loader
                    .effects
                    .iter()
                    .position(|e| e.name == lp.effect_name);
                if let Some(idx) = effect_idx {
                    self.load_effect_on_layer(i, idx);
                } else {
                    log::warn!(
                        "Effect '{}' not found for layer {}, skipping",
                        lp.effect_name,
                        i
                    );
                }
            }

            if let Some(layer) = self.layer_stack.layers.get_mut(i) {
                for (name, value) in &lp.params {
                    if layer.param_store.values.contains_key(name) {
                        layer.param_store.set(name, value.clone());
                    }
                }
                layer.blend_mode = lp.blend_mode;
                layer.opacity = lp.opacity;
                layer.enabled = lp.enabled;
                layer.locked = lp.locked;
                layer.pinned = lp.pinned;
                layer.custom_name = lp.custom_name.clone();
            }
        }

        // Restore active layer + global postprocess
        self.layer_stack.active_layer = preset
            .active_layer
            .min(self.layer_stack.layers.len().saturating_sub(1));
        self.sync_active_layer();
        self.post_process.enabled = preset.postprocess.enabled;
        self.preset_store.current_preset = Some(index);
        self.preset_store.dirty = false;
        // Reset param changed flags so loading doesn't immediately mark dirty
        for layer in &mut self.layer_stack.layers {
            layer.param_store.changed = false;
        }
        log::info!("Loaded preset '{}'", self.preset_store.presets[index].0);
    }

    /// Collect LayerInfo snapshots for UI (avoids borrow conflicts).
    pub fn layer_infos(&self) -> Vec<LayerInfo> {
        self.layer_stack.layer_infos(&self.effect_loader.effects)
    }

    /// Get the current postprocess def from active layer.
    pub fn current_postprocess(&self) -> PostProcessDef {
        self.layer_stack
            .active()
            .map(|l| l.postprocess.clone())
            .unwrap_or_default()
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

        // Execute all enabled layers
        let enabled_layers: Vec<usize> = self
            .layer_stack
            .layers
            .iter()
            .enumerate()
            .filter(|(_, l)| l.enabled)
            .map(|(i, _)| i)
            .collect();

        if enabled_layers.is_empty() {
            // Nothing to render — just clear and present
            self.post_process.render(
                &self.gpu.device,
                &self.gpu.queue,
                &mut encoder,
                &self.compositor.accumulator.write_target(),
                &surface_view,
                self.uniforms.time,
                self.uniforms.rms,
                self.uniforms.onset,
                self.uniforms.flatness,
                &PostProcessDef::default(),
            );
        } else if enabled_layers.len() == 1 && self.layer_stack.layers[enabled_layers[0]].opacity >= 1.0 {
            // Single-layer fast path: skip compositing entirely (only when fully opaque)
            let idx = enabled_layers[0];
            let final_target = self.layer_stack.layers[idx].execute(&mut encoder, &self.gpu.queue);
            let postprocess = self.current_postprocess();

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
                &postprocess,
            );
        } else {
            // Multi-layer: render each layer, then composite
            // Collect layer render results
            let mut layer_outputs: Vec<(&crate::gpu::render_target::RenderTarget, BlendMode, f32)> =
                Vec::new();
            for &idx in &enabled_layers {
                let target = self.layer_stack.layers[idx].execute(&mut encoder, &self.gpu.queue);
                let blend = self.layer_stack.layers[idx].blend_mode;
                let opacity = self.layer_stack.layers[idx].opacity;
                layer_outputs.push((target, blend, opacity));
            }

            // Reverse so top-of-UI-list renders visually on top
            layer_outputs.reverse();

            // Composite layers
            let composited = self.compositor.composite(
                &self.gpu.device,
                &self.gpu.queue,
                &mut encoder,
                &layer_outputs,
            );

            let postprocess = self.current_postprocess();
            self.post_process.render(
                &self.gpu.device,
                &self.gpu.queue,
                &mut encoder,
                composited,
                &surface_view,
                self.uniforms.time,
                self.uniforms.rms,
                self.uniforms.onset,
                self.uniforms.flatness,
                &postprocess,
            );
        }

        // Flip ping-pong for all layers
        for layer in &mut self.layer_stack.layers {
            layer.flip();
        }
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
