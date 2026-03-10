use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use winit::window::Window;

use crate::audio::AudioSystem;
use crate::bindings::bus::BindingBus;
use crate::effect::EffectLoader;
use crate::effect::format::PostProcessDef;
use crate::effect::loader::assets_dir;
use crate::gpu::compositor::Compositor;
use crate::gpu::layer::{BlendMode, EffectLayer, Layer, LayerContent, LayerInfo, LayerStack};
use crate::gpu::particle::ParticleSystem;
use crate::gpu::pass_executor::PassExecutor;
use crate::gpu::placeholder::PlaceholderTexture;
use crate::gpu::postprocess::PostProcessChain;
use crate::gpu::render_target::PingPongTarget;
use crate::gpu::{GpuContext, ShaderPipeline, ShaderUniforms, UniformBuffer};
use crate::media::MediaLayer;
#[cfg(feature = "webcam")]
use crate::media::WebcamBackend;
use crate::midi::MidiSystem;
use crate::midi::clock::MidiClock;
use crate::midi::types::TriggerAction;
use crate::osc::OscSystem;
use crate::params::{ParamStore, ParamValue};
use crate::preset::PresetStore;
use crate::preset::loader::{MediaDecodeResult, PresetLoader};
use crate::preset::store::LayerPreset;
use crate::scene::SceneStore;
use crate::scene::timeline::{Timeline, TimelineEvent};
use crate::scene::transition::TransitionRenderer;
use crate::scene::types::AdvanceMode;
use crate::settings::SettingsConfig;
use crate::shader::ShaderWatcher;
use crate::ui::EguiOverlay;
use crate::ui::panels::shader_editor::ShaderEditorState;
use crate::web::WebSystem;

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
    // Binding bus
    pub binding_bus: BindingBus,
    // Presets
    pub preset_store: PresetStore,
    pub preset_loader: PresetLoader,
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
    // Scenes
    pub scene_store: SceneStore,
    pub timeline: Timeline,
    pub transition_renderer: Option<TransitionRenderer>,
    /// When a dissolve begins, render() captures the outgoing frame then loads this preset.
    pub dissolve_capture_pending: Option<usize>,
    pub midi_clock: MidiClock,
    /// Whether MIDI clock was playing last frame (for rising-edge transport detection).
    pub midi_clock_was_playing: bool,
    /// Whether a MIDI clock beat boundary was crossed this frame.
    pub midi_clock_beat_crossed: bool,
    /// Morph transition state: from/to params per layer (layer_idx → param_name → value).
    pub morph_from_params: Option<Vec<std::collections::HashMap<String, ParamValue>>>,
    pub morph_to_params: Option<Vec<std::collections::HashMap<String, ParamValue>>>,
    pub morph_from_opacities: Option<Vec<f32>>,
    pub morph_to_opacities: Option<Vec<f32>>,
    // Shader editor
    pub shader_editor: ShaderEditorState,
    // Binding matrix modal
    pub binding_matrix: crate::ui::panels::binding_matrix::BindingMatrixState,
    // Quit confirmation
    pub quit_requested: bool,
    // Transient status error (displayed in status bar, auto-clears)
    pub status_error: Option<(String, Instant)>,
    // Webcam capture (feature-gated)
    #[cfg(feature = "webcam")]
    pub webcam_capture: Option<WebcamBackend>,
    #[cfg(feature = "webcam")]
    pub webcam_devices: Vec<(u32, String)>,
    #[cfg(feature = "webcam")]
    pub webcam_device_index: u32,
    #[cfg(feature = "webcam")]
    pub use_ffmpeg_webcam: bool,
    // Particle source loader (background image/video decode)
    pub particle_source_loader: crate::gpu::particle::ParticleSourceLoader,
    // Depth estimation (feature-gated)
    #[cfg(feature = "depth")]
    pub depth_thread: Option<crate::depth::thread::DepthThread>,
    #[cfg(feature = "depth")]
    pub depth_download: Option<std::sync::Arc<crate::depth::model::DownloadProgress>>,
    // GPU profiler (feature-gated)
    #[cfg(feature = "profiling")]
    pub gpu_profiler: crate::gpu::profiler::Profiler,
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
        let (pass_executor, shader_sources, param_store, effect_index) =
            if let Some(idx) = default_idx {
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
                        &gpu.queue,
                        gpu.pipeline_cache.as_ref(),
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
                            let pipeline = ShaderPipeline::new(&gpu.device, hdr_format, &source, gpu.pipeline_cache.as_ref())?;
                            let feedback = PingPongTarget::new_cleared(
                                &gpu.device,
                                &gpu.queue,
                                gpu.surface_config.width,
                                gpu.surface_config.height,
                                hdr_format,
                                1.0,
                            );
                            let executor = PassExecutor::single_pass(
                                pipeline,
                                feedback,
                                &uniform_buffer,
                                &gpu.device,
                                &placeholder,
                            );
                            (executor, vec![source], ParamStore::new(), None)
                        }
                    }
                } else {
                    log::warn!(
                        "Effect '{}' has no passes, using default shader",
                        effect.name
                    );
                    let source = read_default_shader();
                    let pipeline = ShaderPipeline::new(&gpu.device, hdr_format, &source, gpu.pipeline_cache.as_ref())?;
                    let feedback = PingPongTarget::new_cleared(
                        &gpu.device,
                        &gpu.queue,
                        gpu.surface_config.width,
                        gpu.surface_config.height,
                        hdr_format,
                        1.0,
                    );
                    let executor = PassExecutor::single_pass(
                        pipeline,
                        feedback,
                        &uniform_buffer,
                        &gpu.device,
                        &placeholder,
                    );
                    (executor, vec![source], ParamStore::new(), None)
                }
            } else {
                let source = read_default_shader();
                let pipeline = ShaderPipeline::new(&gpu.device, hdr_format, &source, gpu.pipeline_cache.as_ref())?;
                let feedback = PingPongTarget::new_cleared(
                    &gpu.device,
                    &gpu.queue,
                    gpu.surface_config.width,
                    gpu.surface_config.height,
                    hdr_format,
                    1.0,
                );
                let executor = PassExecutor::single_pass(
                    pipeline,
                    feedback,
                    &uniform_buffer,
                    &gpu.device,
                    &placeholder,
                );
                (executor, vec![source], ParamStore::new(), None)
            };

        // Build particle system for initial effect (if it has one)
        let mut pass_executor = pass_executor;
        if let Some(idx) = effect_index {
            if let Some(ref pd) = effect_loader.effects[idx].particles {
                if pd.interaction {
                    use crate::gpu::particle::spatial_hash::grid_dims;
                    effect_loader.grid_dims = grid_dims(pd.max_count, pd.grid_max);
                }
                let compute_source = if pd.compute_shader.is_empty() {
                    effect_loader.prepend_compute_libraries(include_str!(
                        "../../../assets/shaders/builtin/particle_sim.wgsl"
                    ))
                } else {
                    effect_loader
                        .load_compute_source(&pd.compute_shader)
                        .unwrap_or_else(|e| {
                            log::warn!("Failed to load compute shader: {e}");
                            effect_loader.prepend_compute_libraries(include_str!(
                                "../../../assets/shaders/builtin/particle_sim.wgsl"
                            ))
                        })
                };
                match ParticleSystem::new(
                    &gpu.device,
                    &gpu.queue,
                    hdr_format,
                    pd,
                    &compute_source,
                    pd.interaction,
                ) {
                    Ok(mut ps) => {
                        log::info!("Particle system created: {} particles", pd.max_count);
                        if pd.trail_length >= 2 {
                            ps.setup_trails(
                                &gpu.device,
                                hdr_format,
                                pd.trail_length,
                                pd.trail_width,
                            );
                            log::info!("Trail rendering enabled: {} points", pd.trail_length);
                        }
                        if pd.interaction {
                            log::info!("Spatial hash enabled for particle interaction");
                        }
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
        #[cfg(feature = "webcam")]
        let webcam_device_from_settings = settings.webcam_device.unwrap_or(0);
        #[cfg(feature = "webcam")]
        let use_ffmpeg_webcam = settings.use_ffmpeg_webcam;
        let audio = AudioSystem::new_with_device(settings.audio_device.as_deref());
        let midi = MidiSystem::new();
        let osc = OscSystem::new();
        let web = WebSystem::new();
        // Migrate legacy MIDI/OSC mappings to binding bus on first launch
        crate::bindings::migration::migrate_legacy_if_needed();
        let binding_bus = BindingBus::new();
        let mut preset_store = PresetStore::new();
        preset_store.scan();
        let mut scene_store = SceneStore::new();
        scene_store.scan();
        let egui_overlay = EguiOverlay::new(&gpu.device, gpu.format, &window, settings.theme);
        #[cfg(feature = "ndi")]
        let ndi = crate::ndi::NdiSystem::new(
            &gpu.device,
            gpu.format,
            gpu.surface_config.width,
            gpu.surface_config.height,
        );

        #[cfg(feature = "profiling")]
        let gpu_profiler = crate::gpu::profiler::Profiler::new(&gpu.device);

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
            binding_bus,
            preset_store,
            preset_loader: PresetLoader::new(),
            scene_store,
            timeline: Timeline::new(Vec::new(), false, AdvanceMode::Manual),
            transition_renderer: None,
            dissolve_capture_pending: None,
            midi_clock: MidiClock::new(),
            midi_clock_was_playing: false,
            midi_clock_beat_crossed: false,
            morph_from_params: None,
            morph_to_params: None,
            morph_from_opacities: None,
            morph_to_opacities: None,
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
            binding_matrix: crate::ui::panels::binding_matrix::BindingMatrixState::new(),
            quit_requested: false,
            status_error: None,
            #[cfg(feature = "webcam")]
            webcam_capture: None,
            #[cfg(feature = "webcam")]
            webcam_devices: if use_ffmpeg_webcam {
                crate::media::webcam_ffmpeg::list_devices().unwrap_or_default()
            } else {
                crate::media::webcam::list_devices().unwrap_or_default()
            },
            #[cfg(feature = "webcam")]
            webcam_device_index: webcam_device_from_settings,
            #[cfg(feature = "webcam")]
            use_ffmpeg_webcam,
            particle_source_loader: crate::gpu::particle::ParticleSourceLoader::new(),
            #[cfg(feature = "depth")]
            depth_thread: None,
            #[cfg(feature = "depth")]
            depth_download: None,
            #[cfg(feature = "profiling")]
            gpu_profiler,
        })
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.gpu.resize(width, height);
        for layer in &mut self.layer_stack.layers {
            layer.resize(
                &self.gpu.device,
                &self.gpu.queue,
                width,
                height,
                &self.placeholder,
            );
            layer.resize_media(&self.gpu.device, &self.gpu.queue, width, height);
        }
        self.compositor.resize(&self.gpu.device, width, height);
        self.post_process.resize(&self.gpu.device, width, height);
        self.egui_overlay
            .resize(width, height, self.window.scale_factor() as f32);
        if let Some(ref mut tr) = self.transition_renderer {
            tr.resize(&self.gpu.device, width, height, GpuContext::hdr_format());
        }
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
            self.uniforms.mfcc[..13].copy_from_slice(&features.mfcc);
            self.uniforms.mfcc[13..].fill(0.0);
            self.uniforms.chroma.copy_from_slice(&features.chroma);
            self.uniforms.dominant_chroma = features.dominant_chroma;
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

            // Extract scene control fields before layer borrow ends
            let scene_goto_cue = osc_result.scene_goto_cue;
            let scene_load_index = osc_result.scene_load_index;
            let scene_load_name = osc_result.scene_load_name;
            let scene_loop_mode = osc_result.scene_loop_mode;
            let scene_advance_mode = osc_result.scene_advance_mode;

            // Apply layer-targeted OSC messages
            for (layer_idx, name, value) in osc_result.layer_params {
                if let Some(target_layer) = self.layer_stack.layers.get_mut(layer_idx) {
                    if !target_layer.locked {
                        if let Some(def) = target_layer
                            .param_store
                            .defs
                            .iter()
                            .find(|d| d.name() == name)
                        {
                            match def.clone() {
                                crate::params::ParamDef::Float { min, max, .. } => {
                                    let val = min + (max - min) * value.clamp(0.0, 1.0);
                                    target_layer.param_store.set(&name, ParamValue::Float(val));
                                }
                                crate::params::ParamDef::Bool { .. } => {
                                    target_layer
                                        .param_store
                                        .set(&name, ParamValue::Bool(value > 0.5));
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

            // Process scene control (outside layer borrow)
            if let Some(index) = scene_goto_cue {
                let event = self.timeline.go_to_cue(index);
                self.process_timeline_event(event);
            }
            if let Some(index) = scene_load_index {
                self.load_scene(index);
            }
            if let Some(name) = scene_load_name {
                if let Some(idx) = self.scene_store.scenes.iter().position(|(n, _)| n == &name) {
                    self.load_scene(idx);
                }
            }
            if let Some(looping) = scene_loop_mode {
                self.timeline.loop_mode = looping;
                self.autosave_scene();
            }
            if let Some(mode) = scene_advance_mode {
                use crate::scene::types::AdvanceMode;
                self.timeline.advance_mode = match mode {
                    0 => AdvanceMode::Manual,
                    1 => AdvanceMode::Timer,
                    _ => AdvanceMode::BeatSync { beats_per_cue: 4 },
                };
                self.autosave_scene();
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
                        if let Some(def) = target_layer
                            .param_store
                            .defs
                            .iter()
                            .find(|d| d.name() == name)
                        {
                            match def.clone() {
                                crate::params::ParamDef::Float { min, max, .. } => {
                                    let val = min + (max - min) * value.clamp(0.0, 1.0);
                                    target_layer.param_store.set(&name, ParamValue::Float(val));
                                }
                                crate::params::ParamDef::Bool { .. } => {
                                    target_layer
                                        .param_store
                                        .set(&name, ParamValue::Bool(value > 0.5));
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
                let layer_data: Vec<_> = self
                    .layer_stack
                    .layers
                    .iter()
                    .map(|l| {
                        (
                            &l.param_store,
                            l.effect_index(),
                            l.blend_mode,
                            l.opacity,
                            l.enabled,
                            l.locked,
                        )
                    })
                    .collect();
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

        // Evaluate binding bus (runs after MIDI/OSC/WS drain — bus overrides direct mappings)
        self.binding_bus.ingest_ws_values(&self.web.bind_values);
        self.web.bind_values.clear();
        // Transfer preview images from WebSystem to binding bus
        for (source, jpeg) in self.web.preview_images.drain() {
            self.binding_bus.ws_preview_images.insert(source, jpeg);
        }
        let bind_results = self.binding_bus.evaluate(
            self.latest_audio.as_ref(),
            &self.midi,
            &self.osc,
        );
        for (target, value) in bind_results {
            self.apply_binding_target(&target, value);
        }
        self.binding_bus.save_if_dirty();

        // Drain async preset decode results
        if let Some(result) = self.preset_loader.try_recv() {
            log::info!(
                "Async preset decode complete, applying preset index {}",
                result.preset_index
            );
            let index = result.preset_index;
            let preset = result.preset;
            self.apply_preset_immediately(index, &preset, result.decoded_media);

            // Broadcast full state to web clients after async preset load
            if self.web.client_count > 0 {
                let layer_infos = self.layer_stack.layer_infos(&self.effect_loader.effects);
                let layer_data: Vec<_> = self
                    .layer_stack
                    .layers
                    .iter()
                    .map(|l| {
                        (
                            &l.param_store,
                            l.effect_index(),
                            l.blend_mode,
                            l.opacity,
                            l.enabled,
                            l.locked,
                        )
                    })
                    .collect();
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

        // Drain MIDI clock bytes into MidiClock
        self.midi_clock_beat_crossed = self.midi.drain_clock(&mut self.midi_clock);

        // Auto-follow MIDI transport → timeline
        if self.midi_clock.playing()
            && !self.midi_clock_was_playing
            && !self.timeline.active
            && !self.timeline.cues.is_empty()
        {
            let event = self.timeline.start(0);
            self.process_timeline_event(event);
        }
        if !self.midi_clock.playing() && self.midi_clock_was_playing && self.timeline.active {
            self.timeline.stop();
        }
        self.midi_clock_was_playing = self.midi_clock.playing();

        // Advance timeline (scene system)
        if self.timeline.active {
            // Feed beat signal for BeatSync mode:
            // prefer MIDI clock beat when playing, fall back to audio beat detector
            let beat_on = if self.midi_clock.playing() {
                self.midi_clock_beat_crossed
            } else {
                self.uniforms.beat > 0.5
            };
            let beat_event = self.timeline.feed_beat(beat_on);
            self.process_timeline_event(beat_event);

            // Tick for timer-based advance
            let tick_event = self.timeline.tick(dt);
            self.process_timeline_event(tick_event);

            // Apply morph interpolation during ParamMorph transitions
            if let crate::scene::timeline::PlaybackState::Transitioning {
                progress,
                transition_type: crate::scene::types::TransitionType::ParamMorph,
                ..
            } = &self.timeline.state
            {
                self.apply_morph_interpolation(*progress);
                // Morph interpolation sets params every frame via param_store.set(),
                // which marks changed=true. Reset it — this is timeline playback,
                // not a user edit, so it should not mark the preset dirty.
                for layer in &mut self.layer_stack.layers {
                    layer.param_store.changed = false;
                }
            }
        }

        // OSC TX: send audio features + state + timeline (throttled internally)
        if let Some(features) = self.latest_audio {
            let active = self.layer_stack.active_layer;
            let effect_name = self
                .layer_stack
                .active()
                .and_then(|l| l.effect_index())
                .and_then(|i| self.effect_loader.effects.get(i))
                .map(|e| e.name.as_str())
                .unwrap_or("");
            let tl_progress =
                if let crate::scene::timeline::PlaybackState::Transitioning { progress, .. } =
                    &self.timeline.state
                {
                    *progress
                } else {
                    0.0
                };
            self.osc.send_state(
                &features,
                active,
                effect_name,
                self.timeline.active,
                self.timeline.current_cue_index(),
                self.timeline.cues.len(),
                tl_progress,
            );

            // Web: broadcast audio at 10Hz
            self.web.broadcast_audio(&features);
        }

        // Web: update latest state for new client initial sync
        if self.web.client_count > 0 || self.web.is_running() {
            let layer_infos = self.layer_stack.layer_infos(&self.effect_loader.effects);
            let layer_data: Vec<_> = self
                .layer_stack
                .layers
                .iter()
                .map(|l| {
                    (
                        &l.param_store,
                        l.effect_index(),
                        l.blend_mode,
                        l.opacity,
                        l.enabled,
                        l.locked,
                    )
                })
                .collect();
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
                    // Feed media layers
                    for layer in &mut self.layer_stack.layers {
                        if let LayerContent::Media(ref mut m) = layer.content {
                            if m.is_live() {
                                m.set_live_frame(frame.data.clone());
                                m.upload_frame(&self.gpu.queue);
                            }
                        }
                    }
                    // Feed particle systems with webcam source
                    for layer in &mut self.layer_stack.layers {
                        if let LayerContent::Effect(ref mut e) = layer.content {
                            if let Some(ref mut ps) = e.pass_executor.particle_system {
                                if ps.image_source.is_webcam() {
                                    ps.update_webcam_frame(
                                        &self.gpu.queue,
                                        &frame.data,
                                        frame.width,
                                        frame.height,
                                    );
                                }
                                // Feed obstacle with webcam frames
                                if ps.obstacle_enabled && ps.obstacle_source == "webcam" {
                                    ps.update_obstacle_webcam(
                                        &self.gpu.device,
                                        &self.gpu.queue,
                                        &frame.data,
                                        frame.width,
                                        frame.height,
                                    );
                                }
                                // Send webcam frame to depth thread for depth-based obstacle
                                #[cfg(feature = "depth")]
                                if ps.obstacle_enabled && ps.obstacle_source == "depth" {
                                    if let Some(ref depth) = self.depth_thread {
                                        depth.send_frame(
                                            frame.data.clone(),
                                            frame.width,
                                            frame.height,
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // Drain depth estimation results → update obstacle texture
        #[cfg(feature = "depth")]
        if let Some(ref depth_thread) = self.depth_thread {
            if let Some(depth_frame) = depth_thread.try_recv_depth() {
                // Convert grayscale depth to RGBA: white RGB with depth as alpha
                let rgba: Vec<u8> = depth_frame
                    .data
                    .iter()
                    .flat_map(|&d| [255u8, 255, 255, d])
                    .collect();
                for layer in &mut self.layer_stack.layers {
                    if let LayerContent::Effect(ref mut e) = layer.content {
                        if let Some(ref mut ps) = e.pass_executor.particle_system {
                            if ps.obstacle_enabled && ps.obstacle_source == "depth" {
                                ps.update_obstacle_webcam(
                                    &self.gpu.device,
                                    &self.gpu.queue,
                                    &rgba,
                                    depth_frame.width,
                                    depth_frame.height,
                                );
                            }
                        }
                    }
                }
            }
        }

        // Update particle image sources (video playback) and transitions
        {
            let dt_f64 = dt as f64;
            for layer in &mut self.layer_stack.layers {
                if let LayerContent::Effect(ref mut e) = layer.content {
                    if let Some(ref mut ps) = e.pass_executor.particle_system {
                        // Advance video source playback
                        ps.update_source(&self.gpu.queue, dt_f64);
                        // Advance source transition animation
                        if ps.source_transition.is_some() {
                            ps.advance_transition(&self.gpu.queue, dt);
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
                    // Forward first 8 effect params to compute shader
                    let p = e.uniforms.params;
                    ps.uniforms.effect_params = [p[0], p[1], p[2], p[3], p[4], p[5], p[6], p[7]];
                    // Advance obstacle video playback
                    if ps.obstacle_source == "video" {
                        ps.advance_obstacle_video(&self.gpu.device, &self.gpu.queue, dt as f64);
                    }
                    // Pack obstacle collision uniforms
                    ps.uniforms.obstacle_enabled = if ps.obstacle_enabled { 1.0 } else { 0.0 };
                    ps.uniforms.obstacle_threshold = ps.obstacle_threshold;
                    ps.uniforms.obstacle_mode = ps.obstacle_mode as u32;
                    ps.uniforms.obstacle_elasticity = ps.obstacle_elasticity;
                    let audio = self.latest_audio.unwrap_or_default();
                    ps.update_audio(&audio);

                    // Symbiosis force matrix management
                    if let Some(ref mut sym) = ps.symbiosis_state {
                        // param(0) = num_species (0-1 maps to 2-8)
                        let ns = (p[0] * 6.0 + 2.0).round() as u32;
                        sym.set_num_species(ns);
                        // param(6) = preset (0-1 maps to preset index)
                        let preset_idx = (p[6] * (crate::gpu::particle::symbiosis::SymbiosisPreset::count() as f32 - 0.01)) as usize;
                        sym.set_preset(preset_idx);
                        sym.update(dt, &audio);
                        ps.uniforms.force_matrix = sym.active_matrix();
                    }

                    // Morph state management
                    if let Some(ref mut morph) = ps.morph_state {
                        morph.update(dt, audio.beat, audio.dominant_chroma);
                    }
                }
            }
        }

        // Shader hot-reload — iterate all layers
        let changes = self.shader_watcher.drain_changes();
        if !changes.is_empty() {
            let lib_changed = changes
                .iter()
                .any(|p| p.to_string_lossy().contains("/lib/"));
            if lib_changed {
                self.effect_loader.reload_library();
            }
            let hdr_format = GpuContext::hdr_format();

            for layer in &mut self.layer_stack.layers {
                let LayerContent::Effect(ref mut e) = layer.content else {
                    continue;
                };
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
                            let changed =
                                e.shader_sources.get(i).map_or(true, |prev| *prev != source);
                            if changed {
                                log::info!("Shader changed: pass {} ({})", i, pass_def.shader);
                                match e.pass_executor.recompile_pass(
                                    i,
                                    &self.gpu.device,
                                    hdr_format,
                                    &source,
                                    &e.uniform_buffer,
                                    &self.placeholder,
                                    self.gpu.pipeline_cache.as_ref(),
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
                                        log::error!("Failed to reload compute shader: {e}");
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // PFX hot-reload — update effect definitions when .pfx files change
        let pfx_changes = self.shader_watcher.drain_pfx_changes();
        for pfx_path in &pfx_changes {
            let json = match std::fs::read_to_string(pfx_path) {
                Ok(s) => s,
                Err(e) => {
                    log::error!("Failed to read .pfx file {}: {e}", pfx_path.display());
                    if self.shader_editor.open {
                        self.shader_editor.compile_error = Some(format!("Read error: {e}"));
                    }
                    continue;
                }
            };
            let new_effect = match serde_json::from_str::<crate::effect::format::PfxEffect>(&json) {
                Ok(mut e) => {
                    e.source_path = Some(pfx_path.clone());
                    e
                }
                Err(e) => {
                    log::error!("Failed to parse .pfx file {}: {e}", pfx_path.display());
                    if self.shader_editor.open {
                        self.shader_editor.compile_error = Some(format!("JSON error: {e}"));
                    }
                    continue;
                }
            };

            // Find matching effect by source_path.
            // Notify delivers absolute paths; source_path is canonicalized at scan time.
            let pfx_canonical = pfx_path.canonicalize().unwrap_or_else(|_| pfx_path.clone());
            let effect_idx = self
                .effect_loader
                .effects
                .iter()
                .position(|e| e.source_path.as_ref() == Some(&pfx_canonical));
            let effect_idx = match effect_idx {
                Some(i) => i,
                None => {
                    log::debug!("No matching effect for {}", pfx_path.display());
                    continue;
                }
            };

            let old_effect = self.effect_loader.effects[effect_idx].clone();
            // Preserve the original source_path (may be relative) for consistent future lookups
            let mut new_effect = new_effect;
            new_effect.source_path = old_effect.source_path.clone();
            let diff = old_effect.diff(&new_effect);

            if diff.is_empty() {
                continue;
            }

            log::info!(
                "PFX hot-reload: {} (inputs={}, passes={}, particles={}, postprocess={}, meta={})",
                new_effect.name,
                diff.inputs_changed,
                diff.passes_changed,
                diff.particles_changed,
                diff.postprocess_changed,
                diff.metadata_changed,
            );

            // Update the effect definition in the loader
            self.effect_loader.effects[effect_idx] = new_effect.clone();

            // Clear editor error on successful parse
            if self.shader_editor.open {
                self.shader_editor.compile_error = None;
            }

            // Update layers using this effect
            for layer_idx in 0..self.layer_stack.layers.len() {
                let layer = &self.layer_stack.layers[layer_idx];
                let LayerContent::Effect(ref eff) = layer.content else {
                    continue;
                };
                if eff.effect_index != Some(effect_idx) {
                    continue;
                }

                if diff.needs_rebuild() {
                    // Full rebuild needed — capture param values, rebuild, restore
                    let saved_values = self.layer_stack.layers[layer_idx]
                        .param_store
                        .values
                        .clone();
                    self.load_effect_on_layer(layer_idx, effect_idx);
                    // Restore params that still exist with matching types
                    let layer = &mut self.layer_stack.layers[layer_idx];
                    for (name, value) in &saved_values {
                        if let Some(def) = layer.param_store.defs.iter().find(|d| d.name() == name)
                        {
                            if def.default_value().float_count() == value.float_count() {
                                layer.param_store.values.insert(name.clone(), value.clone());
                            }
                        }
                    }
                    layer.param_store.changed = true;
                } else {
                    // Incremental update — no GPU rebuild needed
                    if diff.inputs_changed {
                        self.layer_stack.layers[layer_idx]
                            .param_store
                            .merge_from_defs(&new_effect.inputs);
                    }
                    if diff.postprocess_changed {
                        let pp = new_effect.postprocess.clone().unwrap_or_default();
                        self.layer_stack.layers[layer_idx].postprocess = pp.clone();
                        if layer_idx == self.layer_stack.active_layer {
                            self.post_process.enabled = pp.enabled;
                        }
                    }
                }
            }

            // Update editor paired content if open on this effect
            if self.shader_editor.open {
                if let Some(ref paired_path) = self.shader_editor.paired_path {
                    let paired_canonical = paired_path
                        .canonicalize()
                        .unwrap_or_else(|_| paired_path.clone());
                    if paired_canonical == pfx_canonical {
                        self.shader_editor.paired_content = json.clone();
                        self.shader_editor.paired_disk_content = json;
                    }
                }
            }
        }
    }

    /// Build a ParticleSystem from a ParticleDef, or None if the effect doesn't use particles.
    /// Applies the particle quality multiplier to max_count and emit_rate.
    fn build_particle_system(
        &self,
        particles: &crate::gpu::particle::types::ParticleDef,
    ) -> Option<ParticleSystem> {
        let multiplier = self.settings.particle_quality.multiplier();
        let mut particles = particles.clone();
        let original_count = particles.max_count;
        particles.max_count = (particles.max_count as f32 * multiplier).round() as u32;
        particles.emit_rate *= multiplier;

        // Per-effect cap: don't scale past max_scaled_count if set
        if particles.max_scaled_count > 0 && particles.max_count > particles.max_scaled_count {
            let ratio = particles.max_scaled_count as f32 / particles.max_count as f32;
            particles.max_count = particles.max_scaled_count;
            particles.emit_rate *= ratio;
        }

        // Cap particle count to device storage buffer binding limit.
        // The largest buffer is sorted_particles_buffer = max_particles × 9 × 4 bytes
        // (3×3 tile coverage in compute rasterizer scatter pass).
        let max_binding = self.gpu.device.limits().max_storage_buffer_binding_size as u64;
        let max_from_binding = (max_binding / (9 * 4)) as u32;
        if particles.max_count > max_from_binding {
            log::warn!(
                "Capping particles from {} to {} (storage buffer binding limit {}MB)",
                particles.max_count,
                max_from_binding,
                max_binding / (1024 * 1024),
            );
            particles.max_count = max_from_binding;
            particles.emit_rate = particles.emit_rate.min(max_from_binding as f32);
        }

        if particles.max_count != original_count {
            log::info!(
                "Particle quality {}: {} -> {} particles",
                self.settings.particle_quality.display_name(),
                original_count,
                particles.max_count,
            );
        }
        let particles = &particles;

        let hdr_format = GpuContext::hdr_format();
        let is_image_emitter = particles.emitter.shape == "image";

        // For image emitters, use the builtin image_scatter compute shader
        let compute_source = if is_image_emitter && particles.compute_shader.is_empty() {
            self.effect_loader.prepend_compute_libraries(include_str!(
                "../../../assets/shaders/builtin/image_scatter.wgsl"
            ))
        } else if particles.compute_shader.is_empty() {
            self.effect_loader.prepend_compute_libraries(include_str!(
                "../../../assets/shaders/builtin/particle_sim.wgsl"
            ))
        } else {
            match self
                .effect_loader
                .load_compute_source(&particles.compute_shader)
            {
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

        match ParticleSystem::new(
            &self.gpu.device,
            &self.gpu.queue,
            hdr_format,
            particles,
            &compute_source,
            particles.interaction,
        ) {
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
                    ps.sample_def = sample_def.clone();
                    let image_path = assets_dir().join("images").join(&particles.emitter.image);
                    match crate::gpu::particle::image_source::sample_image(
                        &image_path,
                        &sample_def,
                        particles.max_count,
                    ) {
                        Ok(aux_data) => {
                            ps.upload_aux_data(&self.gpu.device, &self.gpu.queue, &aux_data);
                            ps.store_current_aux(aux_data.clone());
                            ps.static_image_path = Some(image_path.to_string_lossy().to_string());
                            log::info!(
                                "Loaded image '{}': {} particles",
                                particles.emitter.image,
                                aux_data.len()
                            );
                        }
                        Err(e) => {
                            log::warn!("Failed to load image '{}': {e}", particles.emitter.image);
                        }
                    }

                    // If a video source is specified, set up video playback
                    #[cfg(feature = "video")]
                    if !particles.emitter.video.is_empty() && particles.emitter.video != "webcam" {
                        let video_path = assets_dir().join("videos").join(&particles.emitter.video);
                        if video_path.exists() {
                            if crate::media::video::ffmpeg_available() {
                                match crate::media::video::probe_video(&video_path) {
                                    Ok(meta) => {
                                        match crate::media::video::decode_all_frames(
                                            &video_path,
                                            &meta,
                                        ) {
                                            Ok((frames, delays_ms)) => {
                                                let path_str =
                                                    video_path.to_string_lossy().to_string();
                                                ps.set_video_source(
                                                    &self.gpu.queue,
                                                    frames,
                                                    delays_ms,
                                                    path_str,
                                                );
                                                log::info!(
                                                    "Particle video source: '{}'",
                                                    particles.emitter.video
                                                );
                                            }
                                            Err(e) => log::warn!(
                                                "Failed to decode particle video '{}': {e}",
                                                particles.emitter.video
                                            ),
                                        }
                                    }
                                    Err(e) => log::warn!(
                                        "Failed to probe particle video '{}': {e}",
                                        particles.emitter.video
                                    ),
                                }
                            }
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

                // Set up trail rendering if trail_length specified
                if particles.trail_length >= 2 {
                    ps.setup_trails(
                        &self.gpu.device,
                        hdr_format,
                        particles.trail_length,
                        particles.trail_width,
                    );
                    log::info!(
                        "Trail rendering enabled: {} points, width {}",
                        particles.trail_length,
                        particles.trail_width
                    );
                }

                if particles.interaction {
                    log::info!("Spatial hash enabled for particle interaction");
                }

                // Morph target loading
                if particles.morph {
                    if let Some(ref targets) = particles.morph_targets {
                        let assets = assets_dir();
                        for (slot, target_def) in targets.iter().take(4).enumerate() {
                            match crate::gpu::particle::morph::load_morph_target(
                                target_def,
                                particles.max_count,
                                particles.initial_size,
                                &assets,
                            ) {
                                Ok(data) => {
                                    log::info!(
                                        "Morph target {}: '{}' ({} particles)",
                                        slot,
                                        target_def.source,
                                        data.len()
                                    );
                                    if let Some(ref mut morph) = ps.morph_state {
                                        morph.load_target(slot as u32, data);
                                    }
                                }
                                Err(e) => {
                                    log::warn!(
                                        "Failed to load morph target {}: {e}",
                                        target_def.source
                                    );
                                }
                            }
                        }
                        ps.upload_morph_targets(&self.gpu.device, &self.gpu.queue);
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

        // Update spatial hash grid dims for this particle count before building
        if let Some(ref pd) = effect.particles {
            if pd.interaction {
                use crate::gpu::particle::spatial_hash::grid_dims;
                self.effect_loader.grid_dims = grid_dims(pd.max_count, pd.grid_max);
            }
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
            let feedback = PingPongTarget::new_cleared(
                &self.gpu.device,
                &self.gpu.queue,
                self.gpu.surface_config.width,
                self.gpu.surface_config.height,
                hdr_format,
                1.0,
            );
            // Temporary pipeline — will be replaced by executor_result below
            let default_source = read_default_shader();
            if let Ok(pipeline) = ShaderPipeline::new(&self.gpu.device, hdr_format, &default_source, self.gpu.pipeline_cache.as_ref())
            {
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
            &self.gpu.queue,
            self.gpu.pipeline_cache.as_ref(),
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
                if let LayerContent::Effect(ref mut eff) =
                    self.layer_stack.layers[layer_idx].content
                {
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
                        self.shader_editor.compile_error = Some(e.to_string());
                        // Load paired .pfx for tab switching
                        if let Some(ref pfx_path) = effect.source_path {
                            if let Ok(pfx_content) = std::fs::read_to_string(pfx_path) {
                                self.shader_editor
                                    .load_paired_pfx(pfx_path.clone(), pfx_content);
                            }
                        }
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
        let feedback = PingPongTarget::new_cleared(
            &self.gpu.device,
            &self.gpu.queue,
            self.gpu.surface_config.width,
            self.gpu.surface_config.height,
            hdr_format,
            1.0,
        );

        match ShaderPipeline::new(&self.gpu.device, hdr_format, &source, self.gpu.pipeline_cache.as_ref()) {
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
        if let Some(idx) = self
            .effect_loader
            .effects
            .iter()
            .position(|e| e.name == "Phosphor")
        {
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
            match self.start_webcam(device_index, Some((1280, 720))) {
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
        let (w, h) = capture.resolution();
        let device_name = capture.device_name().to_string();

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

    /// Stop webcam capture if no live webcam layers or obstacle sources need it.
    #[cfg(feature = "webcam")]
    pub fn cleanup_webcam_if_unused(&mut self) {
        let has_live = self
            .layer_stack
            .layers
            .iter()
            .any(|l| l.as_media().map_or(false, |m| m.is_live()));
        let obstacle_uses_cam = self.layer_stack.layers.iter().any(|l| {
            l.as_effect()
                .and_then(|e| e.pass_executor.particle_system.as_ref())
                .map_or(false, |ps| {
                    matches!(ps.obstacle_source.as_str(), "webcam" | "depth")
                })
        });
        if !has_live && !obstacle_uses_cam {
            if self.webcam_capture.is_some() {
                log::info!("No live webcam layers or obstacle sources remain, stopping capture");
            }
            self.webcam_capture = None;
        }
    }

    /// Start webcam capture using the active backend (native or ffmpeg).
    #[cfg(feature = "webcam")]
    pub fn start_webcam(&self, device_index: u32, resolution: Option<(u32, u32)>) -> Result<WebcamBackend, String> {
        if self.use_ffmpeg_webcam {
            // For ffmpeg, resolve device index to device name
            let device_name = self
                .webcam_devices
                .iter()
                .find(|(idx, _)| *idx == device_index)
                .map(|(_, name)| name.clone())
                .unwrap_or_else(|| format!("Camera {device_index}"));
            WebcamBackend::start_ffmpeg(&device_name, resolution)
        } else {
            WebcamBackend::start_native(device_index, resolution)
        }
    }

    /// Refresh the webcam device list using the active backend.
    #[cfg(feature = "webcam")]
    pub fn refresh_webcam_devices(&mut self) {
        self.webcam_devices = if self.use_ffmpeg_webcam {
            crate::media::webcam_ffmpeg::list_devices().unwrap_or_default()
        } else {
            crate::media::webcam::list_devices().unwrap_or_default()
        };
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

    /// Apply a single binding bus result to its target.
    fn apply_binding_target(&mut self, target: &str, value: f32) {
        let parts: Vec<&str> = target.split('.').collect();
        match parts.first().copied() {
            Some("param") => {
                // param.{effect_or_wildcard}.{param_name}
                if parts.len() >= 3 {
                    let effect_part = parts[1];
                    let param_name = parts[2];
                    // Check effect name matches active layer (wildcard `*` always matches)
                    if effect_part != "*" {
                        let matches = self
                            .layer_stack
                            .active()
                            .and_then(|l| l.effect_index())
                            .and_then(|idx| self.effect_loader.effects.get(idx))
                            .map(|eff| eff.name == effect_part)
                            .unwrap_or(false);
                        if !matches {
                            return;
                        }
                    }
                    // Apply to active layer
                    if let Some(layer) = self.layer_stack.active_mut() {
                        if let Some(def) = layer
                            .param_store
                            .defs
                            .iter()
                            .find(|d| d.name() == param_name)
                            .cloned()
                        {
                            match def {
                                crate::params::ParamDef::Float { min, max, .. } => {
                                    let val = min + (max - min) * value.clamp(0.0, 1.0);
                                    layer.param_store.set(param_name, ParamValue::Float(val));
                                }
                                crate::params::ParamDef::Bool { .. } => {
                                    layer
                                        .param_store
                                        .set(param_name, ParamValue::Bool(value > 0.5));
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
            Some("layer") => {
                // layer.{n}.opacity or layer.{n}.blend or layer.{n}.enabled
                if parts.len() >= 3 {
                    if let Ok(idx) = parts[1].parse::<usize>() {
                        if let Some(layer) = self.layer_stack.layers.get_mut(idx) {
                            match parts[2] {
                                "opacity" => {
                                    layer.opacity = value.clamp(0.0, 1.0);
                                }
                                "blend" => {
                                    use crate::gpu::layer::BlendMode;
                                    layer.blend_mode =
                                        BlendMode::from_u32(value.round() as u32);
                                }
                                "enabled" => {
                                    layer.enabled = value > 0.5;
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
            Some("global") => {
                // global.master_opacity
                if parts.get(1).copied() == Some("master_opacity") {
                    // Apply as global opacity multiplier to all layers
                    let clamped = value.clamp(0.0, 1.0);
                    for layer in &mut self.layer_stack.layers {
                        layer.opacity = clamped;
                    }
                }
            }
            Some("scene") => {
                // scene.transport.go / scene.transport.prev / scene.transport.stop
                if parts.len() >= 3 && parts[1] == "transport" && value > 0.5 {
                    let trigger = format!("scene.transport.{}", parts[2]);
                    self.binding_bus.pending_triggers.push(trigger);
                }
            }
            Some("postfx") => {
                // postfx.bloom_threshold, postfx.bloom_intensity, etc.
                if parts.len() >= 2 {
                    if let Some(layer) = self.layer_stack.active_mut() {
                        match parts[1] {
                            "bloom_threshold" => {
                                layer.postprocess.bloom_threshold = value * 1.5;
                            }
                            "bloom_intensity" => {
                                layer.postprocess.bloom_intensity = value.clamp(0.0, 1.0);
                            }
                            "vignette" => {
                                layer.postprocess.vignette = value.clamp(0.0, 1.0);
                            }
                            "ca_intensity" => {
                                layer.postprocess.ca_intensity = value.clamp(0.0, 1.0);
                            }
                            "grain_intensity" => {
                                layer.postprocess.grain_intensity = value.clamp(0.0, 1.0);
                            }
                            _ => {}
                        }
                    }
                }
            }
            Some("uniform") => {
                // Direct shader uniform override: uniform.{field_name}
                if parts.len() >= 2 {
                    let v = value.clamp(0.0, 1.0);
                    match parts[1] {
                        "sub_bass" => self.uniforms.sub_bass = v,
                        "bass" => self.uniforms.bass = v,
                        "low_mid" => self.uniforms.low_mid = v,
                        "mid" => self.uniforms.mid = v,
                        "upper_mid" => self.uniforms.upper_mid = v,
                        "presence" => self.uniforms.presence = v,
                        "brilliance" => self.uniforms.brilliance = v,
                        "rms" => self.uniforms.rms = v,
                        "kick" => self.uniforms.kick = v,
                        "centroid" => self.uniforms.centroid = v,
                        "flux" => self.uniforms.flux = v,
                        "flatness" => self.uniforms.flatness = v,
                        "rolloff" => self.uniforms.rolloff = v,
                        "bandwidth" => self.uniforms.bandwidth = v,
                        "zcr" => self.uniforms.zcr = v,
                        "onset" => self.uniforms.onset = v,
                        "beat" => self.uniforms.beat = v,
                        "beat_phase" => self.uniforms.beat_phase = v,
                        "bpm" => self.uniforms.bpm = v,
                        "beat_strength" => self.uniforms.beat_strength = v,
                        "dominant_chroma" => self.uniforms.dominant_chroma = v,
                        "feedback_decay" => self.uniforms.feedback_decay = v,
                        "time" => self.uniforms.time = value, // time not clamped
                        _ => {}
                    }
                }
            }
            _ => {}
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
                let media_path = l
                    .as_media()
                    .map(|m| m.file_path.to_string_lossy().to_string());
                let media_speed = l.as_media().map(|m| m.transport.speed);
                let media_looping = l.as_media().map(|m| m.transport.looping);
                let webcam_device = l
                    .as_media()
                    .filter(|m| m.is_live())
                    .map(|m| m.file_name.clone());
                // Capture particle source info
                let ps_ref = l
                    .as_effect()
                    .and_then(|e| e.pass_executor.particle_system.as_ref());
                let particle_video_path = ps_ref.and_then(|ps| ps.video_path.clone());
                let particle_video_speed = ps_ref.and_then(|ps| ps.image_source.video_speed());
                let particle_video_looping = ps_ref.and_then(|ps| {
                    #[cfg(feature = "video")]
                    if let crate::gpu::particle::ParticleImageSource::Video { looping, .. } =
                        &ps.image_source
                    {
                        return Some(*looping);
                    }
                    let _ = ps;
                    None
                });
                let particle_webcam = ps_ref.and_then(|ps| {
                    if ps.image_source.is_webcam() {
                        Some(true)
                    } else {
                        None
                    }
                });
                let particle_image_path = ps_ref.and_then(|ps| ps.static_image_path.clone());
                // Capture obstacle info
                let obstacle_image_path = ps_ref.and_then(|ps| ps.obstacle_image_path.clone());
                let obstacle_mode = ps_ref
                    .filter(|ps| ps.obstacle_enabled)
                    .map(|ps| ps.obstacle_mode as u32);
                let obstacle_threshold = ps_ref
                    .filter(|ps| ps.obstacle_enabled)
                    .map(|ps| ps.obstacle_threshold);
                let obstacle_elasticity = ps_ref
                    .filter(|ps| ps.obstacle_enabled)
                    .map(|ps| ps.obstacle_elasticity);
                let obstacle_depth = ps_ref
                    .filter(|ps| ps.obstacle_enabled && ps.obstacle_source == "depth")
                    .map(|_| true);
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
                    particle_video_path,
                    particle_video_speed,
                    particle_video_looping,
                    particle_webcam,
                    particle_image_path,
                    obstacle_image_path,
                    obstacle_mode,
                    obstacle_threshold,
                    obstacle_elasticity,
                    obstacle_depth,
                }
            })
            .collect();

        if layer_presets.iter().all(|l| {
            l.effect_name.is_empty() && l.media_path.is_none() && l.webcam_device.is_none()
        }) {
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
            Ok(idx) => {
                log::info!("Saved preset '{}' at index {}", name, idx);
                // Save preset-scoped bindings as sidecar
                self.binding_bus.save_preset_bindings(name);
                self.binding_bus.save_global();
            }
            Err(e) => log::error!("Failed to save preset: {e}"),
        }
    }

    pub fn load_preset(&mut self, index: usize) {
        let preset = match self.preset_store.load(index) {
            Some(p) => p.clone(),
            None => return,
        };

        let preset_name = self
            .preset_store
            .presets
            .get(index)
            .map(|(n, _)| n.clone())
            .unwrap_or_default();

        // Load preset-scoped bindings
        self.binding_bus.load_preset_bindings(&preset_name);

        // Scan for media layers that need decoding (skip locked, skip missing files)
        let mut media_jobs: Vec<(usize, std::path::PathBuf)> = Vec::new();
        for (i, lp) in preset.layers.iter().enumerate() {
            // Skip locked layers
            if let Some(layer) = self.layer_stack.layers.get(i) {
                if layer.locked {
                    continue;
                }
            }
            // Skip webcam layers (handled synchronously)
            if lp.webcam_device.is_some() {
                continue;
            }
            if let Some(ref media_path) = lp.media_path {
                let path = std::path::PathBuf::from(media_path);
                if path.exists() {
                    media_jobs.push((i, path));
                } else {
                    log::warn!("Media file '{}' not found for layer {}", media_path, i);
                }
            }
        }

        if media_jobs.is_empty() {
            // Fast path: no media to decode, apply immediately
            self.apply_preset_immediately(index, &preset, std::collections::HashMap::new());
        } else {
            // Async path: decode media in background
            log::info!(
                "Preset '{}' has {} media layer(s), decoding in background",
                preset_name,
                media_jobs.len()
            );
            self.preset_loader
                .request_load(index, preset, media_jobs, preset_name);
        }
    }

    /// Apply a preset immediately, using pre-decoded media from the HashMap.
    /// Called directly for presets with no media (fast path) or when background
    /// decode completes (async path).
    fn apply_preset_immediately(
        &mut self,
        index: usize,
        preset: &crate::preset::Preset,
        mut decoded_media: std::collections::HashMap<usize, MediaDecodeResult>,
    ) {
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
                // Resolve saved device name to current device index
                let device_idx = lp
                    .webcam_device
                    .as_ref()
                    .and_then(|name| {
                        self.webcam_devices
                            .iter()
                            .find(|(_, n)| n == name)
                            .map(|(idx, _)| *idx)
                    })
                    .unwrap_or(self.webcam_device_index);
                // Start webcam capture if not already running
                if self.webcam_capture.is_none() {
                    match self.start_webcam(device_idx, Some((1280, 720))) {
                        Ok(capture) => {
                            self.webcam_capture = Some(capture);
                        }
                        Err(e) => {
                            log::error!("Failed to start webcam for preset layer {i}: {e}");
                            self.status_error =
                                Some((format!("Webcam failed: {e}"), Instant::now()));
                        }
                    }
                }
                if let Some(ref capture) = self.webcam_capture {
                    let (w, h) = capture.resolution();
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
                        std::path::PathBuf::from(capture.device_name()),
                    );
                    let layer = &mut self.layer_stack.layers[i];
                    layer.content = LayerContent::Media(media_layer);
                    layer.param_store = ParamStore::new();
                }
            }

            if !is_webcam_layer {
                if let Some(ref media_path) = lp.media_path {
                    let path = std::path::PathBuf::from(media_path);
                    // Try pre-decoded media first, fall back to sync decode
                    let loaded = if let Some(decode_result) = decoded_media.remove(&i) {
                        match decode_result {
                            MediaDecodeResult::Ok(source) => {
                                self.create_media_layer_from_source(i, source, &path);
                                true
                            }
                            MediaDecodeResult::Err(e) => {
                                log::warn!("Pre-decoded media failed for layer {}: {}", i, e);
                                false
                            }
                        }
                    } else if path.exists() {
                        // Fallback: sync decode (shouldn't happen in normal flow)
                        self.load_media_on_layer(i, path.clone());
                        true
                    } else {
                        log::warn!("Media file '{}' not found for layer {}", media_path, i);
                        false
                    };

                    // Apply transport settings
                    if loaded {
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
                    }
                } else if !lp.effect_name.is_empty() {
                    let effect_idx = self
                        .effect_loader
                        .effects
                        .iter()
                        .position(|e| e.name == lp.effect_name);

                    // Check if this layer already has the same effect loaded.
                    // If so, skip the full reload — keeps particle systems alive for
                    // smooth morph transitions (params will be interpolated by morph).
                    let already_loaded = if let Some(idx) = effect_idx {
                        self.layer_stack
                            .layers
                            .get(i)
                            .and_then(|l| l.effect_index())
                            == Some(idx)
                    } else {
                        false
                    };

                    if already_loaded {
                        log::debug!(
                            "Layer {} already has '{}', skipping reload (morph-safe)",
                            i,
                            lp.effect_name
                        );
                        // Trigger particle source transition if the preset has
                        // different image source than what's currently loaded.
                        // The morph interpolation will handle param blending.
                    } else if let Some(idx) = effect_idx {
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

            // Restore particle source (video or webcam) if saved in preset
            #[cfg(feature = "video")]
            if let Some(ref video_path) = lp.particle_video_path {
                let path = std::path::PathBuf::from(video_path);
                if path.exists() && crate::media::video::ffmpeg_available() {
                    match crate::media::video::probe_video(&path) {
                        Ok(meta) => {
                            match crate::media::video::decode_all_frames(&path, &meta) {
                                Ok((frames, delays_ms)) => {
                                    if let Some(layer) = self.layer_stack.layers.get_mut(i) {
                                        if let Some(effect) = layer.as_effect_mut() {
                                            if let Some(ps) =
                                                effect.pass_executor.particle_system.as_mut()
                                            {
                                                ps.set_video_source(
                                                    &self.gpu.queue,
                                                    frames,
                                                    delays_ms,
                                                    video_path.clone(),
                                                );
                                                // Restore transport settings
                                                if let crate::gpu::particle::ParticleImageSource::Video {
                                                    speed: ref mut s,
                                                    looping: ref mut l,
                                                    ..
                                                } = ps.image_source
                                                {
                                                    if let Some(spd) = lp.particle_video_speed {
                                                        *s = spd;
                                                    }
                                                    if let Some(lp_loop) = lp.particle_video_looping {
                                                        *l = lp_loop;
                                                    }
                                                }
                                                log::info!(
                                                    "Restored particle video source for layer {i}"
                                                );
                                            }
                                        }
                                    }
                                }
                                Err(e) => {
                                    log::warn!("Failed to decode particle video for layer {i}: {e}")
                                }
                            }
                        }
                        Err(e) => log::warn!("Failed to probe particle video for layer {i}: {e}"),
                    }
                }
            }

            #[cfg(feature = "webcam")]
            if lp.particle_webcam == Some(true) {
                // Start webcam capture if not already running
                if self.webcam_capture.is_none() {
                    match self.start_webcam(self.webcam_device_index, Some((1280, 720))) {
                        Ok(capture) => {
                            self.webcam_capture = Some(capture);
                        }
                        Err(e) => {
                            log::error!("Failed to start webcam for particle source: {e}");
                        }
                    }
                }
                if let Some(ref capture) = self.webcam_capture {
                    let (w, h) = capture.resolution();
                    if let Some(layer) = self.layer_stack.layers.get_mut(i) {
                        if let Some(effect) = layer.as_effect_mut() {
                            if let Some(ps) = effect.pass_executor.particle_system.as_mut() {
                                ps.set_webcam_source(&self.gpu.queue, w, h);
                                log::info!("Restored particle webcam source for layer {i}");
                            }
                        }
                    }
                }
            }

            // Restore static particle image source
            if let Some(ref img_path) = lp.particle_image_path {
                // Only restore if no video/webcam source takes priority
                if lp.particle_video_path.is_none() && lp.particle_webcam != Some(true) {
                    let path = std::path::PathBuf::from(img_path);
                    if path.exists() {
                        // Skip if the same image is already loaded
                        let already_loaded = self
                            .layer_stack
                            .layers
                            .get(i)
                            .and_then(|l| l.as_effect())
                            .and_then(|e| e.pass_executor.particle_system.as_ref())
                            .and_then(|ps| ps.static_image_path.as_ref())
                            == Some(img_path);

                        if !already_loaded {
                            if let Some(layer) = self.layer_stack.layers.get_mut(i) {
                                if let Some(effect) = layer.as_effect_mut() {
                                    if let Some(ps) =
                                        effect.pass_executor.particle_system.as_mut()
                                    {
                                        match crate::gpu::particle::image_source::sample_image(
                                            &path,
                                            &ps.sample_def,
                                            ps.max_particles,
                                        ) {
                                            Ok(aux_data) => {
                                                ps.upload_aux_data(
                                                    &self.gpu.device,
                                                    &self.gpu.queue,
                                                    &aux_data,
                                                );
                                                ps.store_current_aux(aux_data);
                                                ps.image_source =
                                                    crate::gpu::particle::ParticleImageSource::Static;
                                                ps.video_path = None;
                                                ps.static_image_path = Some(img_path.clone());
                                                let filename = path
                                                    .file_name()
                                                    .map(|f| f.to_string_lossy().to_string())
                                                    .unwrap_or_default();
                                                ps.def.emitter.image = filename;
                                                log::info!(
                                                    "Restored particle image source for layer {i}: {img_path}"
                                                );
                                            }
                                            Err(e) => {
                                                log::warn!(
                                                    "Failed to restore particle image for layer {i}: {e}"
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    } else {
                        log::warn!("Particle image '{}' not found for layer {}", img_path, i);
                    }
                }
            }

            // Restore obstacle collision state
            if let Some(ref obstacle_path) = lp.obstacle_image_path {
                let path = std::path::PathBuf::from(obstacle_path);
                if path.exists() {
                    match image::open(&path) {
                        Ok(img) => {
                            let rgba = img.to_rgba8();
                            let (w, h) = rgba.dimensions();
                            if let Some(layer) = self.layer_stack.layers.get_mut(i) {
                                if let Some(effect) = layer.as_effect_mut() {
                                    if let Some(ps) = effect.pass_executor.particle_system.as_mut()
                                    {
                                        ps.set_obstacle_image(
                                            &self.gpu.device,
                                            &self.gpu.queue,
                                            &rgba,
                                            w,
                                            h,
                                            Some(obstacle_path.clone()),
                                        );
                                        if let Some(mode) = lp.obstacle_mode {
                                            ps.obstacle_mode =
                                                crate::gpu::particle::ObstacleMode::from_u32(mode);
                                        }
                                        if let Some(threshold) = lp.obstacle_threshold {
                                            ps.obstacle_threshold = threshold;
                                        }
                                        if let Some(elasticity) = lp.obstacle_elasticity {
                                            ps.obstacle_elasticity = elasticity;
                                        }
                                        log::info!("Restored obstacle image for layer {i}");
                                    }
                                }
                            }
                        }
                        Err(e) => log::warn!("Failed to load obstacle image for layer {i}: {e}"),
                    }
                }
            }

            // Restore depth obstacle source
            #[cfg(feature = "depth")]
            if lp.obstacle_depth == Some(true) && lp.obstacle_image_path.is_none() {
                if crate::depth::model::model_exists() {
                    // Start webcam if needed
                    #[cfg(feature = "webcam")]
                    if self.webcam_capture.is_none() {
                        match self.start_webcam(self.webcam_device_index, Some((1280, 720))) {
                            Ok(capture) => {
                                self.webcam_capture = Some(capture);
                            }
                            Err(e) => {
                                log::error!(
                                    "Failed to start webcam for depth obstacle restore: {e}"
                                );
                            }
                        }
                    }
                    // Start depth thread if needed
                    if self.depth_thread.is_none() {
                        let model_path = crate::depth::model::model_path();
                        match crate::depth::thread::DepthThread::start(model_path) {
                            Ok(dt) => {
                                self.depth_thread = Some(dt);
                            }
                            Err(e) => {
                                log::error!("Failed to start depth thread for preset restore: {e}");
                            }
                        }
                    }
                    if let Some(layer) = self.layer_stack.layers.get_mut(i) {
                        if let Some(effect) = layer.as_effect_mut() {
                            if let Some(ps) = effect.pass_executor.particle_system.as_mut() {
                                ps.obstacle_enabled = true;
                                ps.obstacle_source = "depth".to_string();
                                if let Some(mode) = lp.obstacle_mode {
                                    ps.obstacle_mode =
                                        crate::gpu::particle::ObstacleMode::from_u32(mode);
                                }
                                if let Some(threshold) = lp.obstacle_threshold {
                                    ps.obstacle_threshold = threshold;
                                }
                                if let Some(elasticity) = lp.obstacle_elasticity {
                                    ps.obstacle_elasticity = elasticity;
                                }
                                log::info!("Restored depth obstacle for layer {i}");
                            }
                        }
                    }
                } else {
                    log::warn!(
                        "Preset requires depth model but it's not downloaded, skipping depth obstacle for layer {i}"
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
        if let Some((name, _)) = self.preset_store.presets.get(index) {
            log::info!("Loaded preset '{}'", name);
        }
    }

    /// Create a MediaLayer from an already-decoded MediaSource (GPU resource creation only).
    /// Used by apply_preset_immediately to avoid re-decoding media.
    fn create_media_layer_from_source(
        &mut self,
        layer_idx: usize,
        source: crate::media::decoder::MediaSource,
        path: &std::path::Path,
    ) {
        if layer_idx >= self.layer_stack.layers.len() {
            return;
        }

        let hdr_format = GpuContext::hdr_format();
        let media_layer = MediaLayer::new(
            &self.gpu.device,
            &self.gpu.queue,
            hdr_format,
            self.gpu.surface_config.width,
            self.gpu.surface_config.height,
            source,
            path.to_path_buf(),
        );
        let file_name = media_layer.file_name.clone();
        let layer = &mut self.layer_stack.layers[layer_idx];
        layer.content = LayerContent::Media(media_layer);
        layer.param_store = ParamStore::new();
        log::info!(
            "Layer {}: loaded media '{}' (pre-decoded)",
            layer_idx,
            file_name
        );
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

    /// Load a scene and start its timeline.
    pub fn load_scene(&mut self, index: usize) {
        let scene = match self.scene_store.load(index) {
            Some(s) => s.clone(),
            None => return,
        };

        self.scene_store.current_scene = Some(index);

        self.timeline = Timeline::new(scene.cues.clone(), scene.loop_mode, scene.advance_mode);

        // Start at cue 0
        let event = self.timeline.start(0);
        self.process_timeline_event(event);

        log::info!(
            "Loaded scene '{}' with {} cues",
            scene.name,
            scene.cues.len()
        );
    }

    /// Auto-save current timeline state back to the active scene on disk.
    pub fn autosave_scene(&mut self) {
        if let Some(idx) = self.scene_store.current_scene {
            if let Some((name, _)) = self.scene_store.scenes.get(idx) {
                let name = name.clone();
                let set = crate::scene::types::SceneSet {
                    version: 1,
                    name: name.clone(),
                    cues: self.timeline.cues.clone(),
                    loop_mode: self.timeline.loop_mode,
                    advance_mode: self.timeline.advance_mode,
                };
                if let Err(e) = self.scene_store.save(&name, set) {
                    log::error!("Failed to autosave scene: {e}");
                }
            }
        }
    }

    /// Process a timeline event (load cue, begin transition, etc.).
    pub fn process_timeline_event(&mut self, event: TimelineEvent) {
        match event {
            TimelineEvent::None => {}
            TimelineEvent::LoadCue { cue_index } => {
                // Look up the preset by name and load it
                if let Some(cue) = self.timeline.cues.get(cue_index) {
                    let preset_name = cue.preset_name.clone();
                    let preset_idx = self
                        .preset_store
                        .presets
                        .iter()
                        .position(|(name, _)| name == &preset_name);
                    if let Some(idx) = preset_idx {
                        self.load_preset(idx);
                    } else {
                        log::warn!("Preset '{}' not found for cue {}", preset_name, cue_index);
                    }
                }
            }
            TimelineEvent::BeginTransition {
                from_cue: _,
                to_cue,
                transition_type,
                duration: _,
            } => {
                match transition_type {
                    crate::scene::types::TransitionType::Dissolve => {
                        // Ensure TransitionRenderer exists
                        if self.transition_renderer.is_none() {
                            self.transition_renderer = Some(TransitionRenderer::new(
                                &self.gpu.device,
                                GpuContext::hdr_format(),
                            ));
                        }
                        // Defer preset load until render() captures the outgoing frame.
                        // render() will: capture snapshot → load preset → crossfade.
                        let preset_idx = self.timeline.cues.get(to_cue).and_then(|cue| {
                            self.preset_store
                                .presets
                                .iter()
                                .position(|(name, _)| name == &cue.preset_name)
                        });
                        self.dissolve_capture_pending = preset_idx;
                    }
                    crate::scene::types::TransitionType::ParamMorph => {
                        // Snapshot current (outgoing) params
                        let from_params: Vec<std::collections::HashMap<String, ParamValue>> = self
                            .layer_stack
                            .layers
                            .iter()
                            .map(|l| l.param_store.values.clone())
                            .collect();
                        let from_opacities: Vec<f32> =
                            self.layer_stack.layers.iter().map(|l| l.opacity).collect();
                        self.morph_from_params = Some(from_params);
                        self.morph_from_opacities = Some(from_opacities);

                        // Load target preset
                        if let Some(cue) = self.timeline.cues.get(to_cue) {
                            let preset_name = cue.preset_name.clone();
                            let preset_idx = self
                                .preset_store
                                .presets
                                .iter()
                                .position(|(name, _)| name == &preset_name);
                            if let Some(idx) = preset_idx {
                                self.load_preset(idx);
                            }
                        }

                        // Snapshot target (incoming) params after preset load
                        let to_params: Vec<std::collections::HashMap<String, ParamValue>> = self
                            .layer_stack
                            .layers
                            .iter()
                            .map(|l| l.param_store.values.clone())
                            .collect();
                        let to_opacities: Vec<f32> =
                            self.layer_stack.layers.iter().map(|l| l.opacity).collect();
                        self.morph_to_params = Some(to_params);
                        self.morph_to_opacities = Some(to_opacities);
                    }
                    crate::scene::types::TransitionType::Cut => {
                        // Handled by LoadCue
                    }
                }
            }
            TimelineEvent::TransitionProgress { .. } => {
                // Morph interpolation handled in update() loop
                // Dissolve crossfade handled in render() loop
            }
            TimelineEvent::TransitionComplete { cue_index: _ } => {
                // Clear morph state
                self.morph_from_params = None;
                self.morph_to_params = None;
                self.morph_from_opacities = None;
                self.morph_to_opacities = None;
            }
        }
    }

    /// Apply morph interpolation between saved from/to param snapshots.
    fn apply_morph_interpolation(&mut self, progress: f32) {
        let from_params = match &self.morph_from_params {
            Some(p) => p,
            None => return,
        };
        let to_params = match &self.morph_to_params {
            Some(p) => p,
            None => return,
        };

        for (i, layer) in self.layer_stack.layers.iter_mut().enumerate() {
            // Interpolate params using saved from/to snapshots
            if let (Some(from_layer), Some(to_layer)) = (from_params.get(i), to_params.get(i)) {
                for (name, to_val) in to_layer {
                    if let Some(from_val) = from_layer.get(name) {
                        let interpolated = from_val.lerp(to_val, progress);
                        layer.param_store.set(name, interpolated);
                    }
                }
            }

            // Interpolate opacity using saved from/to snapshots
            if let (Some(from_op), Some(to_op)) = (
                self.morph_from_opacities.as_ref(),
                self.morph_to_opacities.as_ref(),
            ) {
                if let (Some(&from_o), Some(&to_o)) = (from_op.get(i), to_op.get(i)) {
                    layer.opacity = from_o + (to_o - from_o) * progress;
                }
            }
        }
    }

    /// Build SceneInfo snapshot for UI.
    pub fn scene_info(&self) -> crate::ui::panels::scene_panel::SceneInfo {
        let scene_store_names: Vec<String> = self
            .scene_store
            .scenes
            .iter()
            .map(|(name, _)| name.clone())
            .collect();
        let timeline = if self.scene_store.current_scene.is_some() {
            Some(self.timeline.info())
        } else {
            None
        };
        let preset_names: Vec<String> = self
            .preset_store
            .presets
            .iter()
            .map(|(name, _)| name.clone())
            .collect();
        let cue_list: Vec<crate::ui::panels::scene_panel::CueDisplayInfo> = self
            .timeline
            .cues
            .iter()
            .map(|c| crate::ui::panels::scene_panel::CueDisplayInfo {
                preset_name: c.display_name().to_string(),
                transition: c.transition,
                transition_secs: c.transition_secs,
                hold_secs: c.hold_secs,
            })
            .collect();
        crate::ui::panels::scene_panel::SceneInfo {
            scene_store_names,
            current_scene: self.scene_store.current_scene,
            timeline,
            preset_names,
            cue_list,
        }
    }

    pub fn render(&mut self) -> Result<(), wgpu::SurfaceError> {
        // Check for GPU device loss
        if self
            .gpu
            .device_lost
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            log::error!("GPU device lost — cannot render");
            return Err(wgpu::SurfaceError::Lost);
        }

        let output = self.gpu.surface.get_current_texture()?;
        let surface_view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .gpu
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

        // Poll particle counter readback from previous frame (non-blocking)
        for layer in &mut self.layer_stack.layers {
            if let Some(effect) = layer.as_effect_mut() {
                if let Some(ps) = effect.pass_executor.particle_system.as_mut() {
                    ps.poll_counter_readback();
                }
            }
        }

        // Compute the HDR source from layer execution + compositing.
        let (source, postprocess) = if enabled_layers.is_empty() {
            (
                self.compositor.accumulator.write_target()
                    as &crate::gpu::render_target::RenderTarget,
                PostProcessDef::default(),
            )
        } else if enabled_layers.len() == 1
            && self.layer_stack.layers[enabled_layers[0]].opacity >= 1.0
        {
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

        // Dissolve capture: on the first frame of a dissolve, capture outgoing then load incoming.
        // We must: (1) capture the snapshot from this frame's render, (2) submit those commands,
        // (3) load the new preset (mutates self), (4) re-render layers for the incoming scene.
        if let Some(preset_idx) = self.dissolve_capture_pending.take() {
            if let Some(ref mut tr) = self.transition_renderer {
                tr.capture_snapshot(&self.gpu.device, &self.gpu.queue, &mut encoder, source);
            }
            // Submit capture commands so snapshot texture is filled
            self.gpu.queue.submit(std::iter::once(encoder.finish()));

            // Load the incoming preset (needs &mut self, no outstanding borrows now)
            self.load_preset(preset_idx);

            // Create fresh encoder and re-render layers for crossfade
            encoder = self
                .gpu
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("phosphor-encoder-dissolve"),
                });
            let enabled_layers2: Vec<usize> = self
                .layer_stack
                .layers
                .iter()
                .enumerate()
                .filter(|(_, l)| l.enabled)
                .map(|(i, _)| i)
                .collect();
            let (new_source, new_pp) = if enabled_layers2.is_empty() {
                (
                    self.compositor.accumulator.write_target()
                        as &crate::gpu::render_target::RenderTarget,
                    PostProcessDef::default(),
                )
            } else if enabled_layers2.len() == 1
                && self.layer_stack.layers[enabled_layers2[0]].opacity >= 1.0
            {
                let idx = enabled_layers2[0];
                let target = self.layer_stack.layers[idx].execute(&mut encoder, &self.gpu.queue);
                (target, self.current_postprocess())
            } else {
                let mut layer_outputs2: Vec<(
                    &crate::gpu::render_target::RenderTarget,
                    BlendMode,
                    f32,
                )> = Vec::new();
                for &idx in &enabled_layers2 {
                    let target =
                        self.layer_stack.layers[idx].execute(&mut encoder, &self.gpu.queue);
                    let blend = self.layer_stack.layers[idx].blend_mode;
                    let opacity = self.layer_stack.layers[idx].opacity;
                    layer_outputs2.push((target, blend, opacity));
                }
                layer_outputs2.reverse();
                let composited = self.compositor.composite(
                    &self.gpu.device,
                    &self.gpu.queue,
                    &mut encoder,
                    &layer_outputs2,
                );
                (composited, self.current_postprocess())
            };
            // Crossfade snapshot (outgoing) + new_source (incoming)
            let source = if let Some(ref tr) = self.transition_renderer {
                if tr.has_snapshot() {
                    if let crate::scene::timeline::PlaybackState::Transitioning {
                        progress, ..
                    } = &self.timeline.state
                    {
                        tr.crossfade(
                            &self.gpu.device,
                            &self.gpu.queue,
                            &mut encoder,
                            new_source,
                            *progress,
                        )
                        .unwrap_or(new_source)
                    } else {
                        new_source
                    }
                } else {
                    new_source
                }
            } else {
                new_source
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
                &new_pp,
                {
                    #[cfg(feature = "ndi")]
                    {
                        self.ndi.config.alpha_from_luma
                    }
                    #[cfg(not(feature = "ndi"))]
                    {
                        false
                    }
                },
            );

            // NDI capture
            #[cfg(feature = "ndi")]
            if self.ndi.is_running() {
                self.ndi
                    .capture_frame(&self.gpu.device, &mut encoder, &self.post_process, source);
            }

            // Flip ping-pong for all layers
            for layer in &mut self.layer_stack.layers {
                layer.flip();
            }
            self.frame_count = self.frame_count.wrapping_add(1);

            // egui overlay
            self.egui_overlay.render(
                &self.gpu.device,
                &self.gpu.queue,
                &mut encoder,
                &surface_view,
            );

            #[cfg(feature = "profiling")]
            self.gpu_profiler.inner.resolve_queries(&mut encoder);

            self.gpu.queue.submit(std::iter::once(encoder.finish()));

            #[cfg(feature = "profiling")]
            self.gpu_profiler.end_frame(&self.gpu.queue);

            // Request particle counter readback (async, read next frame)
            for layer in &self.layer_stack.layers {
                if let Some(effect) = layer.as_effect() {
                    if let Some(ps) = &effect.pass_executor.particle_system {
                        ps.request_counter_readback();
                    }
                }
            }

            #[cfg(feature = "ndi")]
            if self.ndi.is_running() {
                self.ndi.post_submit();
            }

            output.present();
            return Ok(());
        }

        // Dissolve crossfade: if transitioning with dissolve, blend snapshot + current
        let source = if let crate::scene::timeline::PlaybackState::Transitioning {
            transition_type: crate::scene::types::TransitionType::Dissolve,
            progress,
            ..
        } = &self.timeline.state
        {
            if let Some(ref tr) = self.transition_renderer {
                if tr.has_snapshot() {
                    tr.crossfade(
                        &self.gpu.device,
                        &self.gpu.queue,
                        &mut encoder,
                        source,
                        *progress,
                    )
                    .unwrap_or(source)
                } else {
                    source
                }
            } else {
                source
            }
        } else {
            source
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
                {
                    self.ndi.config.alpha_from_luma
                }
                #[cfg(not(feature = "ndi"))]
                {
                    false
                }
            },
        );

        // NDI capture: render composite to capture texture + copy to staging
        #[cfg(feature = "ndi")]
        if self.ndi.is_running() {
            self.ndi
                .capture_frame(&self.gpu.device, &mut encoder, &self.post_process, source);
        }

        // Flip ping-pong for all layers
        for layer in &mut self.layer_stack.layers {
            layer.flip();
        }
        self.frame_count = self.frame_count.wrapping_add(1);

        // egui overlay → surface
        self.egui_overlay.render(
            &self.gpu.device,
            &self.gpu.queue,
            &mut encoder,
            &surface_view,
        );

        // GPU profiler: resolve timestamp queries before submitting
        #[cfg(feature = "profiling")]
        self.gpu_profiler.inner.resolve_queries(&mut encoder);

        self.gpu.queue.submit(std::iter::once(encoder.finish()));

        // GPU profiler: finalize frame and poll results
        #[cfg(feature = "profiling")]
        self.gpu_profiler.end_frame(&self.gpu.queue);

        // Request particle counter readback (async, read next frame)
        for layer in &self.layer_stack.layers {
            if let Some(effect) = layer.as_effect() {
                if let Some(ps) = &effect.pass_executor.particle_system {
                    ps.request_counter_readback();
                }
            }
        }

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
            // Load paired .pfx for tab switching
            if let Some(new_idx) = new_idx {
                if let Some(ref pfx_path) = self.effect_loader.effects[new_idx].source_path {
                    if let Ok(pfx_content) = std::fs::read_to_string(pfx_path) {
                        self.shader_editor
                            .load_paired_pfx(pfx_path.clone(), pfx_content);
                    }
                }
            }
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

        log::info!(
            "Created new effect '{}': {} + {}",
            name,
            pfx_path.display(),
            wgsl_path.display()
        );

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
            // Load paired .pfx for tab switching
            if let Ok(pfx_content) = std::fs::read_to_string(&pfx_path) {
                self.shader_editor.load_paired_pfx(pfx_path, pfx_content);
            }
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
