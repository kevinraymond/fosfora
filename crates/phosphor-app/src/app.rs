use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use winit::window::Window;

use crate::audio::AudioSystem;
use crate::effect::format::PostProcessDef;
use crate::effect::loader::assets_dir;
use crate::effect::EffectLoader;
use crate::gpu::compositor::Compositor;
use crate::gpu::layer::{BlendMode, EffectLayer, Layer, LayerContent, LayerInfo, LayerStack};
use crate::media::MediaLayer;
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
use crate::web::WebSystem;
use crate::preset::store::LayerPreset;
use crate::preset::PresetStore;
use crate::shader::ShaderWatcher;
use crate::settings::SettingsConfig;
use crate::ui::EguiOverlay;
use crate::ui::panels::shader_editor::ShaderEditorState;

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
    // Web (WebSocket control surface)
    pub web: WebSystem,
    pub pending_web_triggers: Vec<TriggerAction>,
    // Presets
    pub preset_store: PresetStore,
    // Settings
    pub settings: SettingsConfig,
    // Layers
    pub layer_stack: LayerStack,
    // Compositor + post-processing (separate from layer_stack to avoid borrow conflicts)
    pub compositor: Compositor,
    pub post_process: PostProcessChain,
    pub placeholder: PlaceholderTexture,
    // Global uniforms template (time, audio, etc. — params overwritten per-layer)
    pub uniforms: ShaderUniforms,
    // NDI output (feature-gated)
    #[cfg(feature = "ndi")]
    pub ndi: crate::ndi::NdiSystem,
    // Shader editor
    pub shader_editor: ShaderEditorState,
    // Quit confirmation
    pub quit_requested: bool,
    // Transient status error (displayed in status bar, auto-clears)
    pub status_error: Option<(String, Instant)>,
    // Webcam capture (feature-gated)
    #[cfg(feature = "webcam")]
    pub webcam_capture: Option<crate::media::webcam::WebcamCapture>,
}

impl App {
    pub fn new(window: Arc<Window>) -> Result<Self> {
        let gpu = GpuContext::new(window.clone())?;
        let hdr_format = GpuContext::hdr_format();

        // Load default effect or fall back to default shader
        let mut effect_loader = EffectLoader::new();
        effect_loader.scan_effects_directory();

        // Prefer Phosphor as default, fall back to first effect
        let default_idx = effect_loader
            .effects
            .iter()
            .position(|e| e.name == "Phosphor")
            .or_else(|| {
                if effect_loader.effects.is_empty() {
                    None
                } else {
                    Some(0)
                }
            });
        // Placeholder 1x1 black texture
        let placeholder = PlaceholderTexture::new(&gpu.device, &gpu.queue, hdr_format);

        // Build initial layer with default effect (use normalized_passes for multi-pass effects)
        let uniform_buffer = UniformBuffer::new(&gpu.device);
        let (pass_executor, shader_sources, param_store, effect_index) = if let Some(idx) = default_idx {
            let effect = &effect_loader.effects[idx];
            let passes = effect.normalized_passes();
            if !passes.is_empty() {
                match PassExecutor::new(
                    &gpu.device,
                    hdr_format,
                    gpu.surface_config.width,
                    gpu.surface_config.height,
                    &passes,
                    &effect_loader,
                    &uniform_buffer,
                    &placeholder,
                ) {
                    Ok(executor) => {
                        let sources: Vec<String> = passes
                            .iter()
                            .filter_map(|p| effect_loader.load_effect_source(&p.shader).ok())
                            .collect();
                        let mut store = ParamStore::new();
                        store.load_from_defs(&effect.inputs);
                        effect_loader.current_effect = Some(idx);
                        (executor, sources, store, Some(idx))
                    }
                    Err(e) => {
                        log::warn!("Failed to load effect: {e}, using default shader");
                        let source = read_default_shader();
                        let pipeline = ShaderPipeline::new(&gpu.device, hdr_format, &source)?;
                        let feedback = PingPongTarget::new(
                            &gpu.device, gpu.surface_config.width, gpu.surface_config.height,
                            hdr_format, 1.0,
                        );
                        let executor = PassExecutor::single_pass(
                            pipeline, feedback, &uniform_buffer, &gpu.device, &placeholder,
                        );
                        (executor, vec![source], ParamStore::new(), None)
                    }
                }
            } else {
                log::warn!("Effect '{}' has no passes, using default shader", effect.name);
                let source = read_default_shader();
                let pipeline = ShaderPipeline::new(&gpu.device, hdr_format, &source)?;
                let feedback = PingPongTarget::new(
                    &gpu.device, gpu.surface_config.width, gpu.surface_config.height,
                    hdr_format, 1.0,
                );
                let executor = PassExecutor::single_pass(
                    pipeline, feedback, &uniform_buffer, &gpu.device, &placeholder,
                );
                (executor, vec![source], ParamStore::new(), None)
            }
        } else {
            let source = read_default_shader();
            let pipeline = ShaderPipeline::new(&gpu.device, hdr_format, &source)?;
            let feedback = PingPongTarget::new(
                &gpu.device, gpu.surface_config.width, gpu.surface_config.height,
                hdr_format, 1.0,
            );
            let executor = PassExecutor::single_pass(
                pipeline, feedback, &uniform_buffer, &gpu.device, &placeholder,
            );
            (executor, vec![source], ParamStore::new(), None)
        };

        // Build particle system for initial effect (if it has one)
        let mut pass_executor = pass_executor;
        if let Some(idx) = effect_index {
            if let Some(ref pd) = effect_loader.effects[idx].particles {
                let compute_source = if pd.compute_shader.is_empty() {
                    include_str!("../../../assets/shaders/builtin/particle_sim.wgsl").to_string()
                } else {
                    effect_loader
                        .load_compute_source(&pd.compute_shader)
                        .unwrap_or_else(|e| {
                            log::warn!("Failed to load compute shader: {e}");
                            include_str!("../../../assets/shaders/builtin/particle_sim.wgsl")
                                .to_string()
                        })
                };
                match ParticleSystem::new(
                    &gpu.device,
                    &gpu.queue,
                    hdr_format,
                    pd,
                    &compute_source,
                ) {
                    Ok(ps) => {
                        log::info!("Particle system created: {} particles", pd.max_count);
                        pass_executor.particle_system = Some(ps);
                    }
                    Err(e) => log::error!("Failed to create particle system: {e}"),
                }
            }
        }

        let initial_layer = Layer::new_effect(
            "Layer 1".to_string(),
            EffectLayer {
                pass_executor,
                uniform_buffer,
                uniforms: ShaderUniforms::zeroed(),
                effect_index,
                shader_sources,
                shader_error: None,
            },
            param_store,
        );

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
        let settings = SettingsConfig::load();
        let audio = AudioSystem::new_with_device(settings.audio_device.as_deref());
        let midi = MidiSystem::new();
        let osc = OscSystem::new();
        let web = WebSystem::new();
        let mut preset_store = PresetStore::new();
        preset_store.scan();
        let egui_overlay = EguiOverlay::new(&gpu.device, gpu.format, &window, settings.theme);
        #[cfg(feature = "ndi")]
        let ndi = crate::ndi::NdiSystem::new(
            &gpu.device,
            gpu.format,
            gpu.surface_config.width,
            gpu.surface_config.height,
        );

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
            web,
            pending_web_triggers: Vec::new(),
            preset_store,
            settings,
            egui_overlay,
            effect_loader,
            window,
            layer_stack,
            compositor,
            post_process,
            placeholder,
            #[cfg(feature = "ndi")]
            ndi,
            shader_editor: ShaderEditorState::default(),
            quit_requested: false,
            status_error: None,
            #[cfg(feature = "webcam")]
            webcam_capture: None,
        })
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.gpu.resize(width, height);
        for layer in &mut self.layer_stack.layers {
            layer.resize(&self.gpu.device, width, height, &self.placeholder);
            layer.resize_media(&self.gpu.device, &self.gpu.queue, width, height);
        }
        self.compositor.resize(&self.gpu.device, width, height);
        self.post_process.resize(&self.gpu.device, width, height);
        self.egui_overlay
            .resize(width, height, self.window.scale_factor() as f32);
        #[cfg(feature = "ndi")]
        self.ndi.resize(&self.gpu.device, width, height);
    }

    pub fn update(&mut self) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32();
        self.last_frame = now;

        // Auto-clear status error after 6 seconds
        if let Some((_, when)) = &self.status_error {
            if when.elapsed().as_secs_f64() > 6.0 {
                self.status_error = None;
            }
        }

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
                        target_layer.blend_mode = BlendMode::from_u32(value);
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
                if let Some(layer) = self.layer_stack.active_mut() {
                    layer.postprocess.enabled = pp_enabled;
                }
            }
        }

        // Drain WebSocket messages (runs after OSC — last-write-wins)
        if let Some(layer) = self.layer_stack.active_mut() {
            let locked = layer.locked;
            let defs = layer.param_store.defs.clone();
            let web_result = if locked {
                self.web.update_triggers_only()
            } else {
                self.web.update(&mut layer.param_store, &defs)
            };
            self.pending_web_triggers = web_result.triggers;

            // Apply layer-targeted WS messages
            for (layer_idx, name, value) in web_result.layer_params {
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
            for (layer_idx, value) in web_result.layer_opacity {
                if let Some(target_layer) = self.layer_stack.layers.get_mut(layer_idx) {
                    if !target_layer.locked {
                        target_layer.opacity = value;
                    }
                }
            }
            for (layer_idx, value) in web_result.layer_blend {
                if let Some(target_layer) = self.layer_stack.layers.get_mut(layer_idx) {
                    if !target_layer.locked {
                        use crate::gpu::layer::BlendMode;
                        target_layer.blend_mode = BlendMode::from_u32(value);
                    }
                }
            }
            for (layer_idx, value) in web_result.layer_enabled {
                if let Some(target_layer) = self.layer_stack.layers.get_mut(layer_idx) {
                    if !target_layer.locked {
                        target_layer.enabled = value;
                    }
                }
            }
            if let Some(pp_enabled) = web_result.postprocess_enabled {
                self.post_process.enabled = pp_enabled;
                if let Some(layer) = self.layer_stack.active_mut() {
                    layer.postprocess.enabled = pp_enabled;
                }
            }

            // Handle effect loads from web
            for effect_idx in web_result.effect_loads {
                let active_locked = self.layer_stack.active().map_or(false, |l| l.locked);
                if !active_locked {
                    self.load_effect(effect_idx);
                }
            }

            // Handle layer selection from web
            if let Some(idx) = web_result.select_layer {
                if idx < self.layer_stack.layers.len() {
                    self.layer_stack.active_layer = idx;
                    self.sync_active_layer();
                    let msg = crate::web::state::build_active_layer_changed(idx);
                    self.web.broadcast_json(&msg);
                }
            }

            // Handle preset loads from web
            let had_preset_loads = !web_result.preset_loads.is_empty();
            for preset_idx in web_result.preset_loads {
                self.load_preset(preset_idx);
            }

            // After preset load, broadcast full state so all clients update
            if had_preset_loads && self.web.client_count > 0 {
                let layer_infos = self.layer_stack.layer_infos(&self.effect_loader.effects);
                let layer_data: Vec<_> = self.layer_stack.layers.iter().map(|l| {
                    (&l.param_store, l.effect_index(), l.blend_mode, l.opacity, l.enabled, l.locked)
                }).collect();
                let state_json = crate::web::state::build_full_state(
                    &self.effect_loader.effects,
                    &layer_infos,
                    self.layer_stack.active_layer,
                    &layer_data,
                    &self.preset_store,
                    self.post_process.enabled,
                );
                self.web.broadcast_json(&state_json);
            }
        }

        // OSC TX: send audio features + state (throttled internally)
        if let Some(features) = self.latest_audio {
            let active = self.layer_stack.active_layer;
            let effect_name = self.layer_stack.active()
                .and_then(|l| l.effect_index())
                .and_then(|i| self.effect_loader.effects.get(i))
                .map(|e| e.name.as_str())
                .unwrap_or("");
            self.osc.send_state(&features, active, effect_name);

            // Web: broadcast audio at 10Hz
            self.web.broadcast_audio(&features);
        }

        // Web: update latest state for new client initial sync
        if self.web.client_count > 0 || self.web.is_running() {
            let layer_infos = self.layer_stack.layer_infos(&self.effect_loader.effects);
            let layer_data: Vec<_> = self.layer_stack.layers.iter().map(|l| {
                (&l.param_store, l.effect_index(), l.blend_mode, l.opacity, l.enabled, l.locked)
            }).collect();
            let state_json = crate::web::state::build_full_state(
                &self.effect_loader.effects,
                &layer_infos,
                self.layer_stack.active_layer,
                &layer_data,
                &self.preset_store,
                self.post_process.enabled,
            );
            self.web.update_latest_state(&state_json);
        }

        // Advance media playback + upload frames for media layers
        for layer in &mut self.layer_stack.layers {
            if let LayerContent::Media(ref mut m) = layer.content {
                m.advance(dt);
                m.upload_frame(&self.gpu.queue);
            }
        }

        // Drain webcam frames into live media layers; detect dead capture thread
        #[cfg(feature = "webcam")]
        {
            let webcam_dead = self
                .webcam_capture
                .as_ref()
                .map_or(false, |c| !c.is_running());
            if webcam_dead {
                log::warn!("Webcam capture thread died unexpectedly");
                self.status_error =
                    Some(("Webcam capture stopped unexpectedly".into(), Instant::now()));
                self.webcam_capture = None;
            }
            if let Some(ref capture) = self.webcam_capture {
                if let Some(frame) = capture.try_recv_frame() {
                    for layer in &mut self.layer_stack.layers {
                        if let LayerContent::Media(ref mut m) = layer.content {
                            if m.is_live() {
                                m.set_live_frame(frame.data.clone());
                                m.upload_frame(&self.gpu.queue);
                            }
                        }
                    }
                }
            }
        }

        // Update each layer's uniforms from global template + per-layer params
        for layer in &mut self.layer_stack.layers {
            if let LayerContent::Effect(ref mut e) = layer.content {
                e.uniforms = self.uniforms;
                e.uniforms.params = layer.param_store.pack_to_buffer();

                // Update particle systems
                if let Some(ref mut ps) = e.pass_executor.particle_system {
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
        }

        // Shader hot-reload — iterate all layers
        let changes = self.shader_watcher.drain_changes();
        if !changes.is_empty() {
            let lib_changed = changes.iter().any(|p| p.to_string_lossy().contains("/lib/"));
            if lib_changed {
                self.effect_loader.reload_library();
            }
            let hdr_format = GpuContext::hdr_format();

            for layer in &mut self.layer_stack.layers {
                let LayerContent::Effect(ref mut e) = layer.content else { continue };
                let effect_idx = match e.effect_index {
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
                            let changed = e
                                .shader_sources
                                .get(i)
                                .map_or(true, |prev| *prev != source);
                            if changed {
                                log::info!("Shader changed: pass {} ({})", i, pass_def.shader);
                                match e.pass_executor.recompile_pass(
                                    i,
                                    &self.gpu.device,
                                    hdr_format,
                                    &source,
                                    &e.uniform_buffer,
                                    &self.placeholder,
                                ) {
                                    Ok(()) => {
                                        if i < e.shader_sources.len() {
                                            e.shader_sources[i] = source;
                                        }
                                        e.shader_error = None;
                                        log::info!("Pass {} recompiled successfully", i);
                                    }
                                    Err(err) => {
                                        log::error!("Pass {} compilation failed: {err}", i);
                                        e.shader_error = Some(err);
                                    }
                                }
                            }
                        }
                        Err(err) => {
                            log::error!("Failed to reload shader for pass {}: {err}", i);
                            e.shader_error = Some(format!("Read error: {err}"));
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
                            if let Some(ref mut ps) = e.pass_executor.particle_system {
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
        let is_image_emitter = particles.emitter.shape == "image";

        // For image emitters, use the builtin image_scatter compute shader
        let compute_source = if is_image_emitter && particles.compute_shader.is_empty() {
            include_str!("../../../assets/shaders/builtin/image_scatter.wgsl").to_string()
        } else if particles.compute_shader.is_empty() {
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

        match ParticleSystem::new(&self.gpu.device, &self.gpu.queue, hdr_format, particles, &compute_source) {
            Ok(mut ps) => {
                log::info!("Particle system created: {} particles", particles.max_count);

                // Load image data for image emitters
                if is_image_emitter && !particles.emitter.image.is_empty() {
                    let sample_def = particles.image_sample.clone().unwrap_or(
                        crate::gpu::particle::types::ImageSampleDef {
                            mode: "grid".to_string(),
                            threshold: 0.1,
                            scale: 1.0,
                        },
                    );
                    let image_path = assets_dir().join("images").join(&particles.emitter.image);
                    match crate::gpu::particle::image_source::sample_image(
                        &image_path,
                        &sample_def,
                        particles.max_count,
                    ) {
                        Ok(aux_data) => {
                            ps.upload_aux_data(&self.gpu.device, &self.gpu.queue, &aux_data);
                            log::info!(
                                "Loaded image '{}': {} particles",
                                particles.emitter.image,
                                aux_data.len()
                            );
                        }
                        Err(e) => {
                            log::warn!(
                                "Failed to load image '{}': {e}",
                                particles.emitter.image
                            );
                        }
                    }
                }

                // Load sprite texture if defined
                if let Some(ref sprite_def) = particles.sprite {
                    let sprite_path = assets_dir().join("images").join(&sprite_def.texture);
                    match crate::gpu::particle::sprite::SpriteAtlas::load_with_def(
                        &self.gpu.device,
                        &self.gpu.queue,
                        &sprite_path,
                        sprite_def.cols,
                        sprite_def.rows,
                        sprite_def.animated,
                        sprite_def.frames,
                    ) {
                        Ok(atlas) => {
                            ps.set_sprite(&self.gpu.device, atlas);
                            log::info!("Loaded sprite atlas: {}", sprite_def.texture);
                        }
                        Err(e) => {
                            log::warn!("Failed to load sprite '{}': {e}", sprite_def.texture);
                        }
                    }
                }

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

        // If layer is currently Media, convert to Effect first
        let is_media = self.layer_stack.layers[layer_idx].is_media();
        if is_media {
            let uniform_buffer = UniformBuffer::new(&self.gpu.device);
            let feedback = PingPongTarget::new(
                &self.gpu.device,
                self.gpu.surface_config.width,
                self.gpu.surface_config.height,
                hdr_format,
                1.0,
            );
            // Temporary pipeline — will be replaced by executor_result below
            let default_source = read_default_shader();
            if let Ok(pipeline) = ShaderPipeline::new(&self.gpu.device, hdr_format, &default_source) {
                let pass_executor = PassExecutor::single_pass(
                    pipeline,
                    feedback,
                    &uniform_buffer,
                    &self.gpu.device,
                    &self.placeholder,
                );
                self.layer_stack.layers[layer_idx].content = LayerContent::Effect(EffectLayer {
                    pass_executor,
                    uniform_buffer,
                    uniforms: ShaderUniforms::zeroed(),
                    effect_index: None,
                    shader_sources: vec![],
                    shader_error: None,
                });
            }
        }

        // Need the layer's uniform buffer reference for PassExecutor::new.
        let LayerContent::Effect(ref eff) = self.layer_stack.layers[layer_idx].content else {
            return;
        };
        let uniform_buffer_ref = &eff.uniform_buffer;
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
                let LayerContent::Effect(ref mut e) = layer.content else {
                    return;
                };
                e.pass_executor = executor;
                layer.param_store.load_from_defs(&effect.inputs);
                e.shader_error = None;
                e.effect_index = Some(effect_index);
                // Apply per-effect postprocess overrides
                let pp = effect.postprocess.clone().unwrap_or_default();
                layer.postprocess = pp.clone();
                // If this is the active layer, update global postprocess
                if layer_idx == self.layer_stack.active_layer {
                    self.post_process.enabled = pp.enabled;
                    self.effect_loader.current_effect = Some(effect_index);
                }
                // Track shader sources for hot-reload
                e.shader_sources = passes
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
                if let LayerContent::Effect(ref mut eff) = self.layer_stack.layers[layer_idx].content {
                    eff.shader_error = Some(format!("Load error: {e}"));
                    // Still track the effect index so "Edit Shader" can find the source file
                    eff.effect_index = Some(effect_index);
                }
                // Update current_effect so the grid selection reflects the broken effect
                if layer_idx == self.layer_stack.active_layer {
                    self.effect_loader.current_effect = Some(effect_index);
                }
                // Auto-open the editor so the user can fix the shader
                if let Some(pass) = passes.first() {
                    let path = self.effect_loader.resolve_shader_path(&pass.shader);
                    if let Ok(content) = std::fs::read_to_string(&path) {
                        self.shader_editor.open_file(&effect.name, path, content);
                        self.shader_editor.compile_error = Some(format!("{e}"));
                    }
                }
            }
        }

        // If we converted a live webcam layer, clean up capture if no live layers remain
        #[cfg(feature = "webcam")]
        if is_media {
            self.cleanup_webcam_if_unused();
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

        let source = read_default_shader();

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

                self.layer_stack.layers.push(Layer::new_effect(
                    name,
                    EffectLayer {
                        pass_executor: executor,
                        uniform_buffer,
                        uniforms: ShaderUniforms::zeroed(),
                        effect_index: None,
                        shader_sources: vec![source],
                        shader_error: None,
                    },
                    ParamStore::new(),
                ));
                // Select the new layer
                self.layer_stack.active_layer = self.layer_stack.layers.len() - 1;
                log::info!("Added layer {}", self.layer_stack.layers.len());
            }
            Err(e) => {
                log::error!("Failed to create layer: {e}");
            }
        }
    }

    /// Remove all layers and create one fresh layer with the Phosphor default effect.
    pub fn clear_all_layers(&mut self) {
        self.layer_stack.layers.clear();
        self.layer_stack.active_layer = 0;
        self.add_layer();
        // Load Phosphor as default on the fresh layer
        if let Some(idx) = self.effect_loader.effects.iter().position(|e| e.name == "Phosphor") {
            self.load_effect(idx);
        }
    }

    /// Add a new media layer from a file path.
    pub fn add_media_layer(&mut self, path: std::path::PathBuf) {
        let num = self.layer_stack.layers.len();
        if num >= 8 {
            log::warn!("Maximum 8 layers reached");
            return;
        }

        match crate::media::decoder::load_media(&path) {
            Ok(source) => {
                let hdr_format = GpuContext::hdr_format();
                let media_layer = MediaLayer::new(
                    &self.gpu.device,
                    &self.gpu.queue,
                    hdr_format,
                    self.gpu.surface_config.width,
                    self.gpu.surface_config.height,
                    source,
                    path.clone(),
                );
                let file_name = media_layer.file_name.clone();
                let name = format!("Layer {}", num + 1);
                self.layer_stack
                    .layers
                    .push(Layer::new_media(name, media_layer));
                self.layer_stack.active_layer = self.layer_stack.layers.len() - 1;
                self.sync_active_layer();
                log::info!("Added media layer: {}", file_name);
            }
            Err(e) => {
                log::error!("Failed to load media '{}': {e}", path.display());
                self.status_error = Some((e, Instant::now()));
            }
        }
    }

    /// Add a webcam layer. Starts capture if not already running.
    #[cfg(feature = "webcam")]
    pub fn add_webcam_layer(&mut self, device_index: u32) {
        let num = self.layer_stack.layers.len();
        if num >= 8 {
            log::warn!("Maximum 8 layers reached");
            return;
        }

        // Start capture if not already running
        if self.webcam_capture.is_none() {
            match crate::media::webcam::WebcamCapture::start(device_index, Some((1280, 720))) {
                Ok(capture) => {
                    self.webcam_capture = Some(capture);
                }
                Err(e) => {
                    log::error!("Failed to start webcam: {e}");
                    self.status_error = Some((format!("Webcam failed: {e}"), Instant::now()));
                    return;
                }
            }
        }

        let capture = self.webcam_capture.as_ref().unwrap();
        let (w, h) = capture.resolution;
        let device_name = capture.device_name.clone();

        let source = crate::media::decoder::MediaSource::Live {
            width: w,
            height: h,
        };
        let hdr_format = GpuContext::hdr_format();
        let media_layer = MediaLayer::new(
            &self.gpu.device,
            &self.gpu.queue,
            hdr_format,
            self.gpu.surface_config.width,
            self.gpu.surface_config.height,
            source,
            std::path::PathBuf::from(&device_name),
        );
        let name = format!("Layer {}", num + 1);
        self.layer_stack
            .layers
            .push(Layer::new_media(name, media_layer));
        self.layer_stack.active_layer = self.layer_stack.layers.len() - 1;
        self.sync_active_layer();
        log::info!("Added webcam layer: {device_name}");
    }

    /// Stop webcam capture if no live webcam layers remain.
    #[cfg(feature = "webcam")]
    pub fn cleanup_webcam_if_unused(&mut self) {
        let has_live = self
            .layer_stack
            .layers
            .iter()
            .any(|l| l.as_media().map_or(false, |m| m.is_live()));
        if !has_live {
            if self.webcam_capture.is_some() {
                log::info!("No live webcam layers remain, stopping capture");
            }
            self.webcam_capture = None;
        }
    }

    /// Replace active layer content with media from a file path.
    pub fn load_media_on_layer(&mut self, layer_idx: usize, path: std::path::PathBuf) {
        if layer_idx >= self.layer_stack.layers.len() {
            return;
        }

        match crate::media::decoder::load_media(&path) {
            Ok(source) => {
                let hdr_format = GpuContext::hdr_format();
                let media_layer = MediaLayer::new(
                    &self.gpu.device,
                    &self.gpu.queue,
                    hdr_format,
                    self.gpu.surface_config.width,
                    self.gpu.surface_config.height,
                    source,
                    path.clone(),
                );
                let file_name = media_layer.file_name.clone();
                let layer = &mut self.layer_stack.layers[layer_idx];
                layer.content = LayerContent::Media(media_layer);
                layer.param_store = ParamStore::new();
                log::info!("Layer {}: loaded media '{}'", layer_idx, file_name);
            }
            Err(e) => {
                log::error!("Failed to load media '{}': {e}", path.display());
            }
        }
    }

    /// Sync effect_loader.current_effect to match active layer.
    pub fn sync_active_layer(&mut self) {
        if let Some(layer) = self.layer_stack.active() {
            self.effect_loader.current_effect = layer.effect_index();
        }
    }

    pub fn save_preset(&mut self, name: &str) {
        let layer_presets: Vec<LayerPreset> = self
            .layer_stack
            .layers
            .iter()
            .map(|l| {
                let effect_name = l
                    .effect_index()
                    .and_then(|i| self.effect_loader.effects.get(i))
                    .map(|e| e.name.clone())
                    .unwrap_or_default();
                let media_path = l.as_media().map(|m| m.file_path.to_string_lossy().to_string());
                let media_speed = l.as_media().map(|m| m.transport.speed);
                let media_looping = l.as_media().map(|m| m.transport.looping);
                let webcam_device = l.as_media()
                    .filter(|m| m.is_live())
                    .map(|m| m.file_name.clone());
                LayerPreset {
                    effect_name,
                    params: l.param_store.values.clone(),
                    blend_mode: l.blend_mode,
                    opacity: l.opacity,
                    enabled: l.enabled,
                    locked: l.locked,
                    pinned: l.pinned,
                    custom_name: l.custom_name.clone(),
                    media_path,
                    media_speed,
                    media_looping,
                    webcam_device,
                }
            })
            .collect();

        if layer_presets.iter().all(|l| l.effect_name.is_empty() && l.media_path.is_none() && l.webcam_device.is_none()) {
            log::warn!("No effects or media loaded, cannot save preset");
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

            // Determine what to load for this layer
            let is_webcam_layer = lp.webcam_device.is_some();

            #[cfg(feature = "webcam")]
            if is_webcam_layer {
                // Webcam preset: reconnect to first available camera
                self.add_webcam_layer(0);
            }

            if !is_webcam_layer {
                if let Some(ref media_path) = lp.media_path {
                    // Load media layer
                    let path = std::path::PathBuf::from(media_path);
                    if path.exists() {
                        self.load_media_on_layer(i, path);
                        // Apply transport settings
                        if let Some(layer) = self.layer_stack.layers.get_mut(i) {
                            if let Some(ref mut m) = layer.as_media_mut() {
                                if let Some(speed) = lp.media_speed {
                                    m.transport.speed = speed;
                                }
                                if let Some(looping) = lp.media_looping {
                                    m.transport.looping = looping;
                                }
                            }
                        }
                    } else {
                        log::warn!("Media file '{}' not found for layer {}", media_path, i);
                    }
                } else if !lp.effect_name.is_empty() {
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
        if let Some(layer) = self.layer_stack.active_mut() {
            layer.postprocess = preset.postprocess.clone();
        }
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

        // Compute the HDR source from layer execution + compositing.
        let (source, postprocess) = if enabled_layers.is_empty() {
            (
                self.compositor.accumulator.write_target() as &crate::gpu::render_target::RenderTarget,
                PostProcessDef::default(),
            )
        } else if enabled_layers.len() == 1 && self.layer_stack.layers[enabled_layers[0]].opacity >= 1.0 {
            // Single-layer fast path: skip compositing entirely (only when fully opaque)
            let idx = enabled_layers[0];
            let target = self.layer_stack.layers[idx].execute(&mut encoder, &self.gpu.queue);
            (target, self.current_postprocess())
        } else {
            // Multi-layer: render each layer, then composite
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

            let composited = self.compositor.composite(
                &self.gpu.device,
                &self.gpu.queue,
                &mut encoder,
                &layer_outputs,
            );
            (composited, self.current_postprocess())
        };

        // Post-process → surface
        self.post_process.render(
            &self.gpu.device,
            &self.gpu.queue,
            &mut encoder,
            source,
            &surface_view,
            self.uniforms.time,
            self.uniforms.rms,
            self.uniforms.onset,
            self.uniforms.flatness,
            &postprocess,
            {
                #[cfg(feature = "ndi")]
                { self.ndi.config.alpha_from_luma }
                #[cfg(not(feature = "ndi"))]
                { false }
            },
        );

        // NDI capture: render composite to capture texture + copy to staging
        #[cfg(feature = "ndi")]
        if self.ndi.is_running() {
            self.ndi.capture_frame(
                &self.gpu.device,
                &mut encoder,
                &self.post_process,
                source,
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

        // NDI: request async map on staging buffer (must be after queue.submit)
        #[cfg(feature = "ndi")]
        if self.ndi.is_running() {
            self.ndi.post_submit();
        }

        output.present();

        Ok(())
    }

    /// Create a new effect from template (.pfx + .wgsl), scan, load, and open in editor.
    pub fn copy_builtin_effect(&mut self, new_name: &str) -> Result<()> {
        let idx = self
            .effect_loader
            .current_effect
            .ok_or_else(|| anyhow::anyhow!("No effect selected"))?;

        let (_pfx_path, wgsl_path) = self.effect_loader.copy_builtin_effect(idx, new_name)?;

        // Rescan effects
        self.effect_loader.scan_effects_directory();

        // Find and load the new effect
        let new_idx = self
            .effect_loader
            .effects
            .iter()
            .position(|e| e.name == new_name);
        if let Some(new_idx) = new_idx {
            self.load_effect(new_idx);
        }

        // Open in editor
        if wgsl_path.exists() {
            let content = std::fs::read_to_string(&wgsl_path)?;
            self.shader_editor.open_file(new_name, wgsl_path, content);
        }

        Ok(())
    }

    pub fn create_new_effect(&mut self, name: &str) -> Result<()> {
        use std::io::Write;

        let name = name.trim();
        if name.is_empty() {
            anyhow::bail!("Effect name cannot be empty");
        }

        // Sanitize to snake_case filename
        let snake: String = name
            .chars()
            .map(|c| {
                if c.is_alphanumeric() {
                    c.to_ascii_lowercase()
                } else {
                    '_'
                }
            })
            .collect();
        let snake = snake.trim_matches('_').to_string();
        if snake.is_empty() {
            anyhow::bail!("Invalid effect name");
        }

        let effects_dir = assets_dir().join("effects");
        let shaders_dir = assets_dir().join("shaders");
        let pfx_path = effects_dir.join(format!("{snake}.pfx"));
        let wgsl_path = shaders_dir.join(format!("{snake}.wgsl"));

        if pfx_path.exists() {
            anyhow::bail!("Effect '{}' already exists: {}", name, pfx_path.display());
        }
        if wgsl_path.exists() {
            anyhow::bail!("Shader '{}' already exists: {}", name, wgsl_path.display());
        }

        // Write template .pfx
        let pfx_json = serde_json::json!({
            "name": name,
            "author": "",
            "description": "",
            "shader": format!("{snake}.wgsl"),
            "inputs": [
                {
                    "type": "Float",
                    "name": "speed",
                    "default": 0.5,
                    "min": 0.0,
                    "max": 1.0
                },
                {
                    "type": "Float",
                    "name": "intensity",
                    "default": 0.7,
                    "min": 0.0,
                    "max": 1.0
                }
            ],
            "postprocess": {
                "enabled": true
            }
        });
        let mut f = std::fs::File::create(&pfx_path)?;
        f.write_all(serde_json::to_string_pretty(&pfx_json)?.as_bytes())?;

        // Write template .wgsl
        let wgsl_template = format!(
            r#"// {name} — audio-reactive shader
// param(0) = speed, param(1) = intensity

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {{
    let res = u.resolution;
    let uv = frag_coord.xy / res;
    let aspect = res.x / res.y;
    let p = (uv - 0.5) * vec2f(aspect, 1.0);
    let t = u.time * (0.2 + param(0u) * 0.8);
    let intensity = param(1u);

    let r = length(p);
    let angle = atan2(p.y, p.x);

    // Animated gradient with audio reactivity
    let wave = sin(r * 8.0 - t * 2.0) * 0.5 + 0.5;
    let audio_pulse = 1.0 + u.rms * 0.5 + u.bass * 0.3;
    let glow = (1.0 - r * 1.2) * intensity * audio_pulse;

    let col = vec3f(
        0.2 + 0.3 * sin(t + angle),
        0.4 + 0.3 * sin(t * 0.7 + r * 4.0),
        0.7 + 0.3 * cos(t * 0.5 + angle * 2.0),
    ) * wave * glow;

    let alpha = clamp(max(col.r, max(col.g, col.b)) * 2.0, 0.0, 1.0);
    return vec4f(max(col, vec3f(0.0)), alpha);
}}
"#
        );
        std::fs::write(&wgsl_path, &wgsl_template)?;

        log::info!("Created new effect '{}': {} + {}", name, pfx_path.display(), wgsl_path.display());

        // Rescan effects directory
        self.effect_loader.scan_effects_directory();

        // Find and load the new effect
        let idx = self
            .effect_loader
            .effects
            .iter()
            .position(|e| e.name == name);
        if let Some(idx) = idx {
            self.load_effect(idx);
        }

        // Open in editor
        if wgsl_path.exists() {
            let content = std::fs::read_to_string(&wgsl_path)?;
            self.shader_editor.open_file(name, wgsl_path, content);
        }

        Ok(())
    }
}

impl ShaderUniforms {
    pub fn zeroed() -> Self {
        bytemuck::Zeroable::zeroed()
    }
}

/// Read default.wgsl from assets dir, falling back to embedded copy.
fn read_default_shader() -> String {
    let path = assets_dir().join("shaders/default.wgsl");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|_| include_str!("../../../assets/shaders/default.wgsl").to_string())
}
