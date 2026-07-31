use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use winit::window::Window;

use crate::audio::AudioSystem;
use crate::bindings::bus::BindingBus;
use crate::effect::EffectLoader;
use crate::effect::format::PostProcessDef;
use crate::effect::loader::assets_dir;
use crate::gpu::audio_textures::{AudioTextures, WAVEFORM_PEEK};
use crate::gpu::compositor::Compositor;
use crate::gpu::layer::{EffectLayer, Layer, LayerContent, LayerInfo, LayerStack};
use crate::gpu::layer_builder::read_default_shader;
use crate::gpu::particle::ParticleSystem;
use crate::gpu::pass_executor::PassExecutor;
use crate::gpu::placeholder::PlaceholderTexture;
use crate::gpu::postprocess::PostProcessChain;
use crate::gpu::render_target::PingPongTarget;
use crate::gpu::shader_compiler::{CompileResult, ShaderCompiler};
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
    /// PHOSPHOR_FRAME_LOG=1 — per-frame CSV of both clocks + brightness drivers.
    pub frame_log: bool,
    pub shader_watcher: ShaderWatcher,
    pub shader_compiler: ShaderCompiler,
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
    /// Volumetric Mode (R3): global toggle + params, applied to the active
    /// particle layer each frame. The renderer itself lives inside the layer's
    /// `ParticleSystem` (where the particle buffers are reachable).
    pub volumetric_enabled: bool,
    pub volumetric_params: crate::gpu::volumetric::VolumetricParams,
    pub placeholder: PlaceholderTexture,
    /// A17 audio textures (waveform / spectrum / spectrogram) filling the reserved
    /// bind-group slots; refreshed each frame in `update` (#1468).
    pub audio_textures: AudioTextures,
    /// Wall-clock of the last mel-column commit, and the EMA of the inter-commit
    /// interval — used to extrapolate a fractional scroll phase (0..1) between
    /// commits so the spectrogram terrain scrolls continuously (#1508 Strata Phase 1b).
    mel_last_commit: Option<Instant>,
    mel_commit_interval: f32,
    // Global uniforms template (time, audio, etc. — params overwritten per-layer)
    pub uniforms: ShaderUniforms,
    // NDI output (feature-gated)
    #[cfg(feature = "ndi")]
    pub ndi: crate::ndi::NdiSystem,
    // v4l2 loopback output (Linux virtual camera, feature-gated)
    #[cfg(all(target_os = "linux", feature = "v4l2"))]
    pub v4l2: crate::v4l2::V4l2System,
    // Video recording (always available — ffmpeg is a subprocess)
    pub recording: crate::recording::RecordingSystem,
    // Scenes
    pub scene_store: SceneStore,
    pub timeline: Timeline,
    pub transition_renderer: Option<TransitionRenderer>,
    /// When a dissolve begins, render() captures the outgoing frame then loads
    /// this `(preset index, cue index)` — the cue rides along so its
    /// `param_overrides` apply to the deferred load too.
    pub dissolve_capture_pending: Option<(usize, usize)>,
    /// Cue whose `param_overrides` apply after the next preset finishes
    /// loading. Consumed at the END of `apply_preset_immediately`, which is the
    /// one point every load path funnels through — the sync fast path, the
    /// async media-decode completion, and the dissolve deferred load. Applying
    /// eagerly at the timeline event instead would be clobbered by the async
    /// path, whose decode lands whole frames later.
    pub pending_cue_overrides: Option<usize>,
    pub midi_clock: MidiClock,
    /// Whether MIDI clock was playing last frame (for rising-edge transport detection).
    pub midi_clock_was_playing: bool,
    /// Whether a MIDI clock beat boundary was crossed this frame.
    pub midi_clock_beat_crossed: bool,
    /// Morph transition state: the param/opacity endpoints per layer.
    pub morph_from: Option<crate::scene::cueing::MorphSnapshot>,
    pub morph_to: Option<crate::scene::cueing::MorphSnapshot>,
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
    /// Background Gaussian-splat scene loader (#1800): decodes .ply/.splat off
    /// the main thread; results drained in main.rs → `upload_splat_cloud`.
    pub splat_loader: crate::gpu::particle::SplatSceneLoader,
    /// In-flight Splat demo-scene download (#1800), polled by main.rs; on
    /// completion the cached file is loaded onto the active splat layer.
    pub splat_demo_download: Option<std::sync::Arc<crate::download::DownloadProgress>>,
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
            .or(if effect_loader.effects.is_empty() {
                None
            } else {
                Some(0)
            });
        // Placeholder 1x1 black texture
        let placeholder = PlaceholderTexture::new(&gpu.device, &gpu.queue, hdr_format);
        // A17 audio textures (waveform / spectrum / spectrogram), zero-initialized (#1468).
        let audio_textures = AudioTextures::new(&gpu.device, &gpu.queue);

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
                        &audio_textures,
                        &gpu.queue,
                        gpu.pipeline_cache.as_ref(),
                    ) {
                        Ok(executor) => {
                            let sources: Vec<String> = passes
                                .iter()
                                .filter_map(|p| {
                                    effect_loader
                                        .load_effect_source_with_inputs(&p.shader, p.input_count())
                                        .ok()
                                })
                                .collect();
                            let mut store = ParamStore::new();
                            store.load_from_defs(&effect.inputs);
                            effect_loader.current_effect = Some(idx);
                            (executor, sources, store, Some(idx))
                        }
                        Err(e) => {
                            log::warn!("Failed to load effect: {e}, using default shader");
                            let source = read_default_shader();
                            let pipeline = ShaderPipeline::new(
                                &gpu.device,
                                hdr_format,
                                &source,
                                gpu.pipeline_cache.as_ref(),
                                0,
                            )?;
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
                                &audio_textures,
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
                    let pipeline = ShaderPipeline::new(
                        &gpu.device,
                        hdr_format,
                        &source,
                        gpu.pipeline_cache.as_ref(),
                        0,
                    )?;
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
                        &audio_textures,
                    );
                    (executor, vec![source], ParamStore::new(), None)
                }
            } else {
                let source = read_default_shader();
                let pipeline = ShaderPipeline::new(
                    &gpu.device,
                    hdr_format,
                    &source,
                    gpu.pipeline_cache.as_ref(),
                    0,
                )?;
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
                    &audio_textures,
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
                let mut ps = ParticleSystem::new(
                    &gpu.device,
                    &gpu.queue,
                    hdr_format,
                    pd,
                    &compute_source,
                    pd.interaction,
                );
                log::info!("Particle system created: {} particles", pd.max_count);
                if pd.trail_length >= 2 {
                    ps.setup_trails(&gpu.device, hdr_format, pd.trail_length, pd.trail_width);
                    log::info!("Trail rendering enabled: {} points", pd.trail_length);
                }
                if pd.interaction {
                    log::info!("Spatial hash enabled for particle interaction");
                }
                pass_executor.set_particle_system(
                    Some(ps),
                    &gpu.device,
                    &uniform_buffer,
                    &placeholder,
                    &audio_textures,
                );
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
                pending_rebuild: false,
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
        let shader_compiler = ShaderCompiler::new();
        let settings = SettingsConfig::load();
        #[cfg(feature = "webcam")]
        let webcam_device_from_settings = settings.webcam_device.unwrap_or(0);
        #[cfg(feature = "webcam")]
        let use_ffmpeg_webcam = settings.use_ffmpeg_webcam;
        let mut audio = AudioSystem::new_with_device(
            settings.audio_device.as_deref(),
            settings.band_scale,
            Arc::new(std::sync::Mutex::new(settings.structure_tuning)),
            Arc::new(std::sync::Mutex::new(crate::audio::TempoControl::new(
                settings.tempo,
            ))),
        );
        // A9 (#1460): a setter rather than a 5th `new_with_device` param — the audio thread
        // never sees this value, so threading it through construction would touch every
        // caller for nothing.
        audio.set_auto_reconnect(settings.auto_reconnect);
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
        #[cfg(all(target_os = "linux", feature = "v4l2"))]
        let v4l2 = crate::v4l2::V4l2System::new(
            &gpu.device,
            gpu.format,
            gpu.surface_config.width,
            gpu.surface_config.height,
        );
        let recording = crate::recording::RecordingSystem::new();

        #[cfg(feature = "profiling")]
        let gpu_profiler = crate::gpu::profiler::Profiler::new(&gpu.device);

        let now = Instant::now();
        Ok(Self {
            gpu,
            mel_last_commit: None,
            mel_commit_interval: 1.0 / 43.0, // ~43 Hz audio-hop column rate
            uniforms: ShaderUniforms::zeroed(),
            start_time: now,
            last_frame: now,
            frame_count: 0,
            frame_log: std::env::var("PHOSPHOR_FRAME_LOG").is_ok(),
            shader_watcher,
            shader_compiler,
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
            pending_cue_overrides: None,
            midi_clock: MidiClock::new(),
            midi_clock_was_playing: false,
            midi_clock_beat_crossed: false,
            morph_from: None,
            morph_to: None,
            settings,
            egui_overlay,
            effect_loader,
            window,
            layer_stack,
            compositor,
            post_process,
            volumetric_enabled: false,
            volumetric_params: crate::gpu::volumetric::VolumetricParams::default(),
            placeholder,
            audio_textures,
            #[cfg(feature = "ndi")]
            ndi,
            #[cfg(all(target_os = "linux", feature = "v4l2"))]
            v4l2,
            recording,
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
            splat_loader: crate::gpu::particle::SplatSceneLoader::new(),
            splat_demo_download: None,
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
                &self.audio_textures,
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
        // Surface sender-thread failures (dead NDI runtime, closed device) so the
        // status dot goes off instead of staying green with zero frames sent.
        #[cfg(feature = "ndi")]
        self.ndi.pipeline.poll_health();
        #[cfg(all(target_os = "linux", feature = "v4l2"))]
        self.v4l2.pipeline.poll_health();

        let now = Instant::now();
        // Clamped: a frame hitch (mouse click stall, window drag, effect swap)
        // otherwise integrates as one giant step — particles teleport past
        // kill bounds, trail ribbons smear across the screen for a frame, and
        // the emission accumulator dumps the entire stall's budget at once
        // (#1796 live finding: every real mouse click white-flashed Tide).
        // Momentary slow-motion during a stall beats a white flash.
        let dt = now.duration_since(self.last_frame).as_secs_f32().min(0.05);
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
        if let Some(features) = self.audio.latest_features(dt) {
            self.latest_audio = Some(features);
            crate::gpu::uniforms::mirror_audio_features(&mut self.uniforms, &features);
        }

        // Diagnostic (PHOSPHOR_FRAME_LOG=1): one CSV line per frame covering both
        // clocks plus every uniform that can move overall brightness. Used to find
        // which value actually jumps when the picture reacts to something that is
        // not the music — reasoning from stills cannot see a temporal artefact.
        if self.frame_log {
            let wall = now.duration_since(self.start_time).as_secs_f32();
            log::info!(
                "FRAMELOG {},{:.6},{:.6},{:.6},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4}",
                self.frame_count,
                dt,
                wall,
                self.uniforms.time,
                self.uniforms.rms,
                self.uniforms.kick,
                self.uniforms.onset,
                self.uniforms.beat_phase,
                self.uniforms.beat,
                self.uniforms.buildup,
            );
        }

        // A17 (#1468): refresh the audio textures every frame. The waveform peeks the
        // freshest PCM straight from the recording ring (no audio-thread involvement);
        // the spectrum and spectrogram consume the data `latest_features` just drained
        // (spectrum held newest, mel columns accumulated). Uploads only rewrite texture
        // contents — the bind groups' texture views are stable.
        let mut wav = [0.0f32; WAVEFORM_PEEK];
        self.audio.recording_ring.peek_latest(&mut wav);
        self.audio_textures.upload_waveform(&self.gpu.queue, &wav);
        self.audio_textures
            .upload_spectrum(&self.gpu.queue, self.audio.latest_spectrum());
        let mel_columns = self.audio.take_mel_columns();
        let n_cols = mel_columns.len();
        self.audio_textures
            .upload_spectrogram(&self.gpu.queue, &mel_columns);
        // Fractional scroll phase (0..1) so the spectrogram terrain scrolls
        // continuously instead of snapping one texel per commit (#1508 Strata).
        // Extrapolate from the last commit using an EMA of the inter-commit interval.
        let now = Instant::now();
        if n_cols > 0 {
            if let Some(last) = self.mel_last_commit {
                let per_col = (now - last).as_secs_f32() / n_cols as f32;
                if per_col > 1e-4 && per_col < 0.5 {
                    self.mel_commit_interval = self.mel_commit_interval * 0.9 + per_col * 0.1;
                }
            }
            self.mel_last_commit = Some(now);
        }
        self.uniforms.scroll_phase = match self.mel_last_commit {
            Some(last) => ((now - last).as_secs_f32() / self.mel_commit_interval).clamp(0.0, 1.0),
            None => 0.0,
        };

        // Watchdog: if the device died or stopped delivering data mid-session, surface it and
        // — when auto-reconnect is on (A9 #1460) — reopen it. Safe to drive from here because
        // the teardown is detached: a stalled capture thread may be blocked in a timeout-less
        // read, so joining it inline would hang the render thread.
        if let Some(msg) = self.audio.poll_health() {
            self.status_error = Some((msg, Instant::now()));
        }

        // Drain MIDI and apply to active layer's param_store (skip if locked)
        if let Some(layer) = self.layer_stack.active_mut() {
            let locked = layer.locked;
            if locked {
                // Still drain MIDI messages but only collect triggers, don't apply CC to params
                let midi_result = self.midi.update_triggers_only();
                self.pending_midi_triggers = midi_result.triggers;
            } else {
                let (defs, values, changed) = layer.param_store.split_borrow();
                let midi_result = self.midi.update(values, changed, defs);
                self.pending_midi_triggers = midi_result.triggers;
            }
        }

        // Drain OSC and apply to active layer's param_store (runs after MIDI — last-write-wins)
        if let Some(layer) = self.layer_stack.active_mut() {
            let locked = layer.locked;
            let osc_result = if locked {
                self.osc.update_triggers_only()
            } else {
                let (defs, values, changed) = layer.param_store.split_borrow();
                self.osc.update(values, changed, defs)
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
                        let pv = target_layer
                            .param_store
                            .defs
                            .iter()
                            .find(|d| d.name() == name)
                            .and_then(|def| match def {
                                crate::params::ParamDef::Float { min, max, .. } => Some(
                                    ParamValue::Float(min + (max - min) * value.clamp(0.0, 1.0)),
                                ),
                                crate::params::ParamDef::Bool { .. } => {
                                    Some(ParamValue::Bool(value > 0.5))
                                }
                                _ => None,
                            });
                        if let Some(pv) = pv {
                            target_layer.param_store.set(&name, pv);
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
            for (layer_idx, value) in osc_result.layer_displace {
                if let Some(target_layer) = self.layer_stack.layers.get_mut(layer_idx) {
                    if !target_layer.locked {
                        target_layer.displace_amount = value;
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
            for (layer_idx, value) in osc_result.layer_obstacle_enabled {
                if let Some(ps) = self.osc_obstacle_target(layer_idx) {
                    ps.obstacle_enabled = value;
                }
            }
            for (layer_idx, value) in osc_result.layer_obstacle_mode {
                if let Some(ps) = self.osc_obstacle_target(layer_idx) {
                    // Raw-integer OSC path: 0..3 via from_u32 (bindings use from_normalized).
                    ps.obstacle_mode = crate::gpu::particle::ObstacleMode::from_u32(value);
                }
            }
            for (layer_idx, value) in osc_result.layer_obstacle_threshold {
                if let Some(ps) = self.osc_obstacle_target(layer_idx) {
                    ps.obstacle_threshold = value;
                }
            }
            for (layer_idx, value) in osc_result.layer_obstacle_elasticity {
                if let Some(ps) = self.osc_obstacle_target(layer_idx) {
                    ps.obstacle_elasticity = value;
                }
            }
            if let Some(pp_enabled) = osc_result.postprocess_enabled {
                self.post_process.enabled = pp_enabled;
                if let Some(layer) = self.layer_stack.active_mut() {
                    layer.postprocess.enabled = pp_enabled;
                }
            }
            if let Some(vol_enabled) = osc_result.volumetric_enabled {
                self.volumetric_enabled = vol_enabled;
            }
            for (name, value) in &osc_result.volumetric_params {
                self.volumetric_params.set_param(name, *value);
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
            let web_result = if locked {
                self.web.update_triggers_only()
            } else {
                let (defs, values, changed) = layer.param_store.split_borrow();
                self.web.update(values, changed, defs)
            };
            self.pending_web_triggers = web_result.triggers;

            // Apply layer-targeted WS messages
            for (layer_idx, name, value) in web_result.layer_params {
                if let Some(target_layer) = self.layer_stack.layers.get_mut(layer_idx) {
                    if !target_layer.locked {
                        let pv = target_layer
                            .param_store
                            .defs
                            .iter()
                            .find(|d| d.name() == name)
                            .and_then(|def| match def {
                                crate::params::ParamDef::Float { min, max, .. } => Some(
                                    ParamValue::Float(min + (max - min) * value.clamp(0.0, 1.0)),
                                ),
                                crate::params::ParamDef::Bool { .. } => {
                                    Some(ParamValue::Bool(value > 0.5))
                                }
                                _ => None,
                            });
                        if let Some(pv) = pv {
                            target_layer.param_store.set(&name, pv);
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
            self.audio.latest_mel(),
            self.audio.latest_dmfcc(),
            &self.midi,
            &self.osc,
        );
        for out in bind_results {
            self.apply_binding_target(&out.target, out.value, out.rising);
        }
        self.binding_bus.save_if_dirty();
        // A preset-scoped binding edit persists only on explicit preset save, so
        // surface it as an unsaved change (mark_dirty no-ops with no preset loaded).
        if self.binding_bus.take_preset_scope_dirty() {
            self.preset_store.mark_dirty();
        }

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
            // Tempo for transition_beats resolution. From latest_audio, not
            // uniforms.bpm: a `uniform.bpm` binding evaluated above can have
            // overwritten the uniform mirror by now, and a binding must not be
            // able to warp transition lengths.
            self.timeline.set_beat_period(
                self.latest_audio
                    .as_ref()
                    .map(|a| 60.0 / a.bpm)
                    .filter(|p| p.is_finite() && *p > 0.0),
            );
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
                &self.audio.pulse_counts(),
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
                                if ps.source.is_webcam() {
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

        // Update each layer's uniforms from global template + per-layer params.
        // The body lives in gpu/frame_prep.rs so the headless renderer runs the
        // identical per-frame preparation.
        crate::gpu::frame_prep::prepare_effect_layers(
            &mut self.layer_stack.layers,
            &self.uniforms,
            &self.latest_audio.unwrap_or_default(),
            dt,
            &self.gpu.device,
            &self.gpu.queue,
            self.layer_stack.active_layer,
            self.volumetric_enabled,
            self.volumetric_params,
        );

        // Apply completed background shader compilations
        for result in self.shader_compiler.drain_results() {
            match result {
                CompileResult::RenderPass {
                    layer_idx,
                    pass_idx,
                    result,
                    source,
                } => {
                    let Some(layer) = self.layer_stack.layers.get_mut(layer_idx) else {
                        continue;
                    };
                    let LayerContent::Effect(ref mut e) = layer.content else {
                        continue;
                    };
                    match result {
                        Ok(pipeline) => {
                            match e.pass_executor.swap_pass_pipeline(
                                pass_idx,
                                pipeline,
                                &self.gpu.device,
                                &e.uniform_buffer,
                                &self.placeholder,
                                &self.audio_textures,
                            ) {
                                Ok(()) => {
                                    if pass_idx < e.shader_sources.len() {
                                        e.shader_sources[pass_idx] = source;
                                    }
                                    e.shader_error = None;
                                    log::info!("Pass {} recompiled successfully (bg)", pass_idx);
                                }
                                Err(err) => {
                                    log::error!("Pass {} swap failed: {err}", pass_idx);
                                    e.shader_error = Some(err);
                                }
                            }
                        }
                        Err(err) => {
                            log::error!("Pass {} compilation failed (bg): {err}", pass_idx);
                            e.shader_error = Some(err);
                        }
                    }
                }
                CompileResult::ComputeShader {
                    layer_idx,
                    result,
                    source,
                } => {
                    let Some(layer) = self.layer_stack.layers.get_mut(layer_idx) else {
                        continue;
                    };
                    let LayerContent::Effect(ref mut e) = layer.content else {
                        continue;
                    };
                    match result {
                        Ok(pipeline) => {
                            if let Some(ref mut ps) = e.pass_executor.particle_system {
                                ps.swap_compute_pipeline(pipeline);
                                ps.current_compute_source = source;
                                log::info!("Compute shader recompiled (bg)");
                            }
                        }
                        Err(err) => {
                            log::error!("Compute shader compilation failed (bg): {err}");
                            e.shader_error = Some(err);
                        }
                    }
                }
            }
        }

        // Shader hot-reload — submit changed shaders for background compilation
        let changes = self.shader_watcher.drain_changes();
        if !changes.is_empty() {
            let lib_changed = changes
                .iter()
                .any(|p| p.to_string_lossy().contains("/lib/"));
            if lib_changed {
                self.effect_loader.reload_library();
            }
            let hdr_format = GpuContext::hdr_format();

            // Layers whose last load failed: the executor still belongs to the
            // previous effect, so an incremental pipeline swap would compile the
            // new shader against the wrong bind-group layouts and fail on every
            // change. Collect them here and retry the whole load below, once the
            // immutable borrow of the layer stack ends (#1855).
            let mut rebuilds: Vec<(usize, usize)> = Vec::new();

            for (layer_idx, layer) in self.layer_stack.layers.iter().enumerate() {
                let LayerContent::Effect(ref e) = layer.content else {
                    continue;
                };
                let effect_idx = match e.effect_index {
                    Some(idx) => idx,
                    None => continue,
                };
                let Some(effect) = self.effect_loader.effects.get(effect_idx) else {
                    continue;
                };
                let passes = effect.normalized_passes();

                if e.pending_rebuild {
                    if changes_touch_effect(effect, &changes, lib_changed) {
                        rebuilds.push((layer_idx, effect_idx));
                    }
                    continue;
                }

                // Hot-reload fragment shaders (background compilation)
                for (i, pass_def) in passes.iter().enumerate() {
                    let pass_relevant =
                        lib_changed || changes.iter().any(|p| p.ends_with(&pass_def.shader));
                    if !pass_relevant {
                        continue;
                    }
                    match self
                        .effect_loader
                        .load_effect_source_with_inputs(&pass_def.shader, pass_def.input_count())
                    {
                        Ok(source) => {
                            let changed =
                                e.shader_sources.get(i).map_or(true, |prev| *prev != source);
                            if changed {
                                log::info!(
                                    "Shader changed: pass {} ({}) — compiling in background",
                                    i,
                                    pass_def.shader
                                );
                                self.shader_compiler.compile_render_pass(
                                    layer_idx,
                                    i,
                                    source,
                                    &self.gpu.device,
                                    hdr_format,
                                    pass_def.input_count(),
                                );
                            }
                        }
                        Err(err) => {
                            log::error!("Failed to reload shader for pass {}: {err}", i);
                        }
                    }
                }

                // Hot-reload compute shader (background compilation)
                if let Some(ref particle_def) = effect.particles {
                    if !particle_def.compute_shader.is_empty() {
                        let compute_relevant = changes
                            .iter()
                            .any(|p| p.ends_with(&particle_def.compute_shader));
                        if compute_relevant {
                            if let Some(ref ps) = e.pass_executor.particle_system {
                                match self
                                    .effect_loader
                                    .load_compute_source(&particle_def.compute_shader)
                                {
                                    Ok(src) if src != ps.current_compute_source => {
                                        log::info!(
                                            "Compute shader changed — compiling in background"
                                        );
                                        let layouts = ps.cloned_compute_bind_group_layouts();
                                        self.shader_compiler.compile_compute_shader(
                                            layer_idx,
                                            src,
                                            &self.gpu.device,
                                            layouts,
                                        );
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

            for (layer_idx, effect_idx) in rebuilds {
                log::info!("Layer {layer_idx}: previous load failed — retrying full rebuild");
                self.load_effect_on_layer(layer_idx, effect_idx);
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

            // Preserve the original source_path (may be relative) for consistent future lookups
            let mut new_effect = new_effect;
            new_effect.source_path = self.effect_loader.effects[effect_idx].source_path.clone();
            let diff = self.effect_loader.effects[effect_idx].diff(&new_effect);

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

            // Move new_effect into the loader (no clone needed)
            self.effect_loader.effects[effect_idx] = new_effect;

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

                // A layer whose last load failed has no valid executor to update
                // incrementally, so any .pfx change is a rebuild for it (#1855).
                if diff.needs_rebuild() || eff.pending_rebuild {
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
                            .merge_from_defs(&self.effect_loader.effects[effect_idx].inputs);
                    }
                    if diff.postprocess_changed {
                        let pp = self.effect_loader.effects[effect_idx]
                            .postprocess
                            .clone()
                            .unwrap_or_default();
                        self.layer_stack.layers[layer_idx].postprocess = pp;
                        if layer_idx == self.layer_stack.active_layer {
                            self.post_process.enabled =
                                self.layer_stack.layers[layer_idx].postprocess.enabled;
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
    /// Load an effect on the active layer, as a deliberate user choice —
    /// the effect browser, the next/prev-effect trigger, the web client.
    ///
    /// Picking an effect is a fresh start for that layer, so the preset
    /// bindings aimed at it go with the params `load_effect_on_layer` is about
    /// to reset. This lives here and NOT in `load_effect_on_layer` because
    /// that is also how a preset applies itself, how a shader hot-reload
    /// rebuilds, and how a particle-quality change re-instantiates the layer —
    /// none of which are the user leaving the preset, and one of which would
    /// delete the bindings `load_preset` had just finished loading.
    pub fn load_effect(&mut self, index: usize) {
        let layer_idx = self.layer_stack.active_layer;
        let dropped = self.binding_bus.clear_preset_bindings_for_layer(layer_idx);
        if dropped > 0 {
            log::info!("Dropped {dropped} preset binding(s) targeting layer {layer_idx}");
        }
        self.load_effect_on_layer(layer_idx, index);
    }

    /// Load an effect on a specific layer.
    ///
    /// Deliberately does not touch bindings — see `load_effect` for why.
    pub fn load_effect_on_layer(&mut self, layer_idx: usize, effect_index: usize) {
        let effect = match self.effect_loader.effects.get(effect_index).cloned() {
            Some(e) => e,
            None => return,
        };
        if layer_idx >= self.layer_stack.layers.len() {
            return;
        }

        // Build the particle system first (grid-dims prep + quality scaling in
        // one shared helper), because the splat kick below wants to inspect it
        // before it moves into the executor.
        let particle_system = {
            // Built inline rather than via layer_build_ctx(): that borrows all
            // of self, and prepare_particles needs effect_loader mutably.
            let ctx = crate::gpu::layer_builder::LayerBuildCtx {
                device: &self.gpu.device,
                queue: &self.gpu.queue,
                pipeline_cache: self.gpu.pipeline_cache.as_ref(),
                width: self.gpu.surface_config.width,
                height: self.gpu.surface_config.height,
                placeholder: &self.placeholder,
                audio_textures: &self.audio_textures,
                particle_quality: self.settings.particle_quality,
            };
            crate::gpu::layer_builder::prepare_particles(&ctx, &mut self.effect_loader, &effect)
        };

        // Splat scene load (#1800): kick off the background decode now that
        // the final (quality-scaled) particle budget is known. The effect
        // renders empty until the cloud lands (drained in main.rs), so a
        // slow or failed load can never half-swap the layer (#1855).
        if let (Some(ps), Some(splat)) = (
            particle_system.as_ref(),
            effect.particles.as_ref().and_then(|pd| pd.splat.as_ref()),
        ) {
            match crate::gpu::particle::splat_source::resolve_source(&splat.source) {
                Ok(path) => self
                    .splat_loader
                    .load(path, ps.max_particles, splat.into(), layer_idx),
                Err(e) => log::info!("Splat scene not loaded yet: {e}"),
            }
        }

        let is_media = self.layer_stack.layers[layer_idx].is_media();
        let result = {
            let ctx = crate::gpu::layer_builder::LayerBuildCtx {
                device: &self.gpu.device,
                queue: &self.gpu.queue,
                pipeline_cache: self.gpu.pipeline_cache.as_ref(),
                width: self.gpu.surface_config.width,
                height: self.gpu.surface_config.height,
                placeholder: &self.placeholder,
                audio_textures: &self.audio_textures,
                particle_quality: self.settings.particle_quality,
            };
            crate::gpu::layer_builder::load_effect_into_layer(
                &ctx,
                &self.effect_loader,
                &mut self.layer_stack.layers[layer_idx],
                layer_idx,
                &effect,
                effect_index,
                particle_system,
            )
        };

        match result {
            Ok(()) => {
                // If this is the active layer, update global postprocess + grid
                // selection — UI state, so it stays out of the shared core.
                if layer_idx == self.layer_stack.active_layer {
                    self.post_process.enabled =
                        self.layer_stack.layers[layer_idx].postprocess.enabled;
                    self.effect_loader.current_effect = Some(effect_index);
                }
                self.shader_watcher.drain_changes();
            }
            Err(e) => {
                // Update current_effect so the grid selection reflects the broken effect
                if layer_idx == self.layer_stack.active_layer {
                    self.effect_loader.current_effect = Some(effect_index);
                }
                // Auto-open the editor so the user can fix the shader. Retries land
                // here too, so don't re-open a file the editor already shows — that
                // would reset the user's cursor and scroll on every save.
                let passes = effect.normalized_passes();
                if let Some(pass) = passes.first() {
                    let path = self.effect_loader.resolve_shader_path(&pass.shader);
                    let already_open = self.shader_editor.open
                        && self.shader_editor.file_path.as_ref() == Some(&path);
                    if already_open {
                        self.shader_editor.compile_error = Some(e.clone());
                    } else if let Ok(content) = std::fs::read_to_string(&path) {
                        self.shader_editor.open_file(&effect.name, path, content);
                        self.shader_editor.compile_error = Some(e.clone());
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
        #[cfg(not(feature = "webcam"))]
        let _ = is_media;
    }

    /// Add a new empty layer with the default shader.
    pub fn add_layer(&mut self) {
        let num = self.layer_stack.layers.len();
        if num >= crate::bindings::catalog::MAX_LAYERS {
            // Deliberately quiet for the UI's "+" button, which is already
            // disabled at the cap. It is not quiet enough for
            // `apply_preset_immediately`, which builds a preset's layers by
            // calling this in a loop: an over-tall preset loads with the extras
            // dropped and nothing said. Offline validation catches that before
            // the file gets here.
            return;
        }
        let name = format!("Layer {}", num + 1);
        let layer = {
            let ctx = crate::gpu::layer_builder::LayerBuildCtx {
                device: &self.gpu.device,
                queue: &self.gpu.queue,
                pipeline_cache: self.gpu.pipeline_cache.as_ref(),
                width: self.gpu.surface_config.width,
                height: self.gpu.surface_config.height,
                placeholder: &self.placeholder,
                audio_textures: &self.audio_textures,
                particle_quality: self.settings.particle_quality,
            };
            crate::gpu::layer_builder::new_default_layer(&ctx, name)
        };
        match layer {
            Some(layer) => {
                self.layer_stack.layers.push(layer);
                // Select the new layer
                self.layer_stack.active_layer = self.layer_stack.layers.len() - 1;
                log::info!("Added layer {}", self.layer_stack.layers.len());
            }
            None => log::error!("Failed to create layer: default shader pipeline error"),
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
        if num >= crate::bindings::catalog::MAX_LAYERS {
            log::warn!(
                "Maximum {} layers reached",
                crate::bindings::catalog::MAX_LAYERS
            );
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
        if num >= crate::bindings::catalog::MAX_LAYERS {
            log::warn!(
                "Maximum {} layers reached",
                crate::bindings::catalog::MAX_LAYERS
            );
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

        let capture = self
            .webcam_capture
            .as_ref()
            .expect("webcam_capture set above or returned");
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
    pub fn start_webcam(
        &self,
        device_index: u32,
        resolution: Option<(u32, u32)>,
    ) -> Result<WebcamBackend, String> {
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
                layer.content = LayerContent::Media(Box::new(media_layer));
                layer.param_store = ParamStore::new();
                log::info!("Layer {}: loaded media '{}'", layer_idx, file_name);
            }
            Err(e) => {
                log::error!("Failed to load media '{}': {e}", path.display());
            }
        }
    }

    /// Remove a layer, carrying its bindings with it.
    ///
    /// Wrapped rather than left to each call site: binding targets pin a layer by
    /// index, so a bare `layer_stack.remove_layer` leaves every binding above the
    /// hole pointing one layer too high — silently driving the wrong effect.
    pub fn remove_layer(&mut self, index: usize) {
        let before = self.layer_stack.layers.len();
        self.layer_stack.remove_layer(index);
        if self.layer_stack.layers.len() == before {
            return; // refused (last layer, or out of range)
        }
        self.binding_bus
            .remap_layer_targets(|old| crate::bindings::bus::layer_index_after_remove(old, index));
        self.sync_active_layer();
    }

    /// Move a layer, carrying its bindings with it.
    ///
    /// Reordering used to leave "rms → layer 0 opacity" behind on slot 0 while the
    /// effect that binding was made for moved elsewhere.
    pub fn move_layer(&mut self, from: usize, to: usize) {
        let n = self.layer_stack.layers.len();
        if from >= n || to >= n || from == to {
            return;
        }
        self.layer_stack.move_layer(from, to);
        self.binding_bus.remap_layer_targets(|old| {
            Some(crate::bindings::bus::layer_index_after_move(old, from, to))
        });
        self.sync_active_layer();
    }

    /// Sync effect_loader.current_effect to match active layer.
    pub fn sync_active_layer(&mut self) {
        if let Some(layer) = self.layer_stack.active() {
            self.effect_loader.current_effect = layer.effect_index();
        }
    }

    /// Resolve a per-layer OSC obstacle message to that layer's particle
    /// system, respecting the layer lock (#1793). None for locked, missing,
    /// or non-particle layers.
    fn osc_obstacle_target(
        &mut self,
        layer_idx: usize,
    ) -> Option<&mut crate::gpu::particle::ParticleSystem> {
        let layer = self.layer_stack.layers.get_mut(layer_idx)?;
        if layer.locked {
            return None;
        }
        layer
            .as_effect_mut()?
            .pass_executor
            .particle_system
            .as_mut()
    }

    /// Apply a single binding bus result to its target.
    /// Send one bus value to whatever it drives.
    ///
    /// Used to take a `&str` and re-parse the dotted form on every frame, for
    /// every enabled binding. The shape is now decided once, at load, so this is
    /// a match — and a new layer-bearing variant cannot be added without the
    /// compiler pointing here.
    /// Thin wrapper over [`crate::bindings::apply::apply_binding_target`] —
    /// the dispatch itself lives there so the headless renderer shares it.
    fn apply_binding_target(
        &mut self,
        target: &crate::bindings::types::BindingTarget,
        value: f32,
        rising: bool,
    ) {
        let mut ctx = crate::bindings::apply::BindingTargetCtx {
            layer_stack: &mut self.layer_stack,
            effects: &self.effect_loader.effects,
            uniforms: &mut self.uniforms,
            pending_triggers: &mut self.binding_bus.pending_triggers,
        };
        crate::bindings::apply::apply_binding_target(&mut ctx, target, value, rising);
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
                // One source, so one conversion (#2011) — the preset can no
                // longer disagree with itself about which one is live.
                let source_fields = ps_ref
                    .map(|ps| ps.source.to_preset_fields())
                    .unwrap_or_default();
                let particle_video_path = source_fields.video_path.clone();
                let particle_video_speed = source_fields.video_speed;
                let particle_video_looping = source_fields.video_looping;
                let particle_webcam = source_fields.webcam;
                let particle_image_path = source_fields.image_path.clone();
                let particle_model_path = source_fields.model_path.clone();
                let is_model_source = source_fields.model_path.is_some();
                let particle_model_pose = ps_ref.filter(|_| is_model_source).map(|ps| {
                    [
                        ps.model_sample.yaw_degrees,
                        ps.model_sample.pitch_degrees,
                        ps.model_sample.scale,
                        ps.model_sample.ambient,
                    ]
                });
                // Lighting rides alongside the pose (#1996) — a saved skull that
                // reloads unlit is as wrong a picture as one that reloads front-on.
                let particle_model_light = ps_ref.filter(|_| is_model_source).map(|ps| {
                    [
                        ps.model_sample.light_mix,
                        ps.model_sample.light_x,
                        ps.model_sample.light_y,
                        ps.model_sample.light_z,
                        ps.model_sample.ray_strength,
                    ]
                });
                // Splat scene (#1800): persist the absolute path; restore
                // re-decodes in the background like media layers.
                let splat_scene_path = ps_ref.and_then(|ps| ps.splat_scene_path.clone());
                // Capture obstacle info
                let obstacle_image_path = ps_ref.and_then(|ps| ps.obstacle_image_path.clone());
                let obstacle_mode = ps_ref
                    .filter(|ps| ps.obstacle_enabled)
                    .map(|ps| ps.obstacle_mode as u32);
                let obstacle_fit = ps_ref
                    .filter(|ps| ps.obstacle_enabled)
                    .map(|ps| ps.obstacle_fit as u32);
                let obstacle_threshold = ps_ref
                    .filter(|ps| ps.obstacle_enabled)
                    .map(|ps| ps.obstacle_threshold);
                let obstacle_elasticity = ps_ref
                    .filter(|ps| ps.obstacle_enabled)
                    .map(|ps| ps.obstacle_elasticity);
                let obstacle_depth = ps_ref
                    .filter(|ps| ps.obstacle_enabled && ps.obstacle_source == "depth")
                    .map(|_| true);
                let obstacle_model = ps_ref
                    .filter(|ps| ps.obstacle_enabled && ps.obstacle_source == "model")
                    .map(|_| true);
                // Capture live Lattice / particle-sim panel edits so they
                // round-trip through the preset instead of snapping back to
                // the effect's `.pfx` defaults on reload.
                let lattice = ps_ref
                    .filter(|ps| ps.lattice_enabled)
                    .map(|ps| ps.lattice_params);
                let helix = ps_ref
                    .filter(|ps| ps.helix_enabled)
                    .map(|ps| ps.helix_params);
                let particle_sim = ps_ref.map(|ps| crate::preset::ParticleSimPreset {
                    emit_rate: ps.def.emit_rate,
                    burst_on_beat: ps.def.burst_on_beat,
                    lifetime: ps.def.lifetime,
                    initial_speed: ps.def.initial_speed,
                    initial_size: ps.def.initial_size,
                    drag: ps.def.drag,
                    // Live allocated length (0 when off), so the preset restores
                    // exactly what's on screen.
                    trail_length: Some(ps.trail_length()),
                });
                LayerPreset {
                    effect_name,
                    params: l.param_store.values.clone(),
                    blend_mode: l.blend_mode,
                    opacity: l.opacity,
                    displace_amount: l.displace_amount,
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
                    particle_model_path,
                    particle_model_pose,
                    particle_model_light,
                    splat_scene_path,
                    obstacle_image_path,
                    obstacle_mode,
                    obstacle_fit,
                    obstacle_threshold,
                    obstacle_elasticity,
                    obstacle_depth,
                    obstacle_model,
                    lattice,
                    helix,
                    particle_sim,
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
        // Volumetric (R3) is a global mode, not a per-layer property — persist
        // it at preset scope like `postprocess`.
        let volumetric = Some(crate::preset::VolumetricPreset {
            enabled: self.volumetric_enabled,
            params: self.volumetric_params,
        });
        match self.preset_store.save(
            name,
            layer_presets,
            self.layer_stack.active_layer,
            &postprocess,
            volumetric,
        ) {
            Ok(idx) => {
                log::info!("Saved preset '{}' at index {}", name, idx);
                // Save preset-scoped bindings as sidecar
                self.binding_bus.save_preset_bindings(name);
                self.binding_bus.save_global();
                // Sidecar is now on disk — no longer an unsaved change.
                self.binding_bus.preset_scope_dirty = false;
            }
            Err(e) => log::error!("Failed to save preset: {e}"),
        }
    }

    pub fn load_preset(&mut self, index: usize) {
        // A plain preset load (UI click, OSC) is not a cue: cancel any override
        // still pending from an earlier cue whose async media never finished,
        // or the stale overrides would apply to this unrelated preset.
        self.pending_cue_overrides = None;
        self.load_preset_inner(index);
    }

    /// Load the preset a cue points at, remembering the cue so its
    /// `param_overrides` apply once the load completes (see
    /// `pending_cue_overrides` for why application is deferred).
    pub fn load_preset_for_cue(&mut self, index: usize, cue_index: usize) {
        self.pending_cue_overrides = Some(cue_index);
        self.load_preset_inner(index);
    }

    fn load_preset_inner(&mut self, index: usize) {
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

        // Load preset-scoped bindings and migrate old 3-part targets to 4-part format
        self.binding_bus.load_preset_bindings(&preset_name);
        // Freshly loaded bindings match disk — clear any stale unsaved flag.
        self.binding_bus.preset_scope_dirty = false;
        crate::bindings::apply::upgrade_legacy_targets(&mut self.binding_bus, &preset);

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

            // Set when the preset names an effect that is not installed; the layer
            // is then disabled rather than left holding the previous preset's effect.
            let mut effect_missing = false;

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
                    layer.content = LayerContent::Media(Box::new(media_layer));
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
                        // Leaving the layer as-is meant it kept the *previous* preset's
                        // effect and then got stamped with this preset's opacity, blend
                        // and params — so the same file rendered differently depending on
                        // what was loaded before it. Both shipped presets pointed a layer
                        // at "Swarm" for months after it was deleted and nothing caught
                        // it. A missing layer is debuggable; a wrong one is not.
                        log::warn!(
                            "Effect '{}' not found for layer {}, disabling layer",
                            lp.effect_name,
                            i
                        );
                        effect_missing = true;
                    }
                }
            }

            // Restore the particle source (#2011).
            //
            // This used to be four blocks running in sequence, each guarded on the
            // others' preset fields being absent. That guard was one-directional:
            // a preset naming two sources — which the pre-#2011 save could write,
            // because the live state itself could hold two — half-applied both.
            // `resolve()` settles it once, so exactly one arm runs.
            let source_fields = crate::gpu::particle::SourcePresetFields {
                video_path: lp.particle_video_path.clone(),
                video_speed: lp.particle_video_speed,
                video_looping: lp.particle_video_looping,
                webcam: lp.particle_webcam,
                image_path: lp.particle_image_path.clone(),
                model_path: lp.particle_model_path.clone(),
            };
            // A preset that names no source resets the layer to what its EFFECT
            // declares, rather than leaving whatever happened to be live (#2013).
            // The rebuild is skipped when the layer already runs this effect (see
            // `already_loaded` above, kept that way for morph), so without this a
            // webcam or video loaded by hand outlives every preset after it. The
            // per-arm `already_loaded` checks make the common case a no-op.
            let declared = self
                .effect_loader
                .effects
                .iter()
                .find(|e| e.name == lp.effect_name)
                .and_then(|e| e.particles.as_ref())
                .and_then(|p| {
                    crate::gpu::particle::source::declared_source(&p.emitter, assets_dir())
                });
            match source_fields.resolve().or(declared) {
                Some(crate::gpu::particle::SourceSpec::Video(video_path)) => {
                    #[cfg(feature = "video")]
                    {
                        let path = std::path::PathBuf::from(&video_path);
                        if path.exists() && crate::media::video::ffmpeg_available() {
                            match crate::media::video::probe_video(&path) {
                                Ok(meta) => {
                                    match crate::media::video::decode_all_frames(&path, &meta) {
                                        Ok((frames, delays_ms)) => {
                                            if let Some(ps) = self
                                                .layer_stack
                                                .layers
                                                .get_mut(i)
                                                .and_then(|l| l.as_effect_mut())
                                                .and_then(|e| {
                                                    e.pass_executor.particle_system.as_mut()
                                                })
                                            {
                                                ps.set_video_source(
                                                    &self.gpu.queue,
                                                    frames,
                                                    delays_ms,
                                                    video_path.clone(),
                                                );
                                                // Restore transport settings
                                                if let Some(playback) = ps.source.playback_mut() {
                                                    if let Some(spd) = lp.particle_video_speed {
                                                        playback.speed = spd;
                                                    }
                                                    if let Some(lp_loop) = lp.particle_video_looping
                                                    {
                                                        playback.looping = lp_loop;
                                                    }
                                                }
                                                log::info!(
                                                    "Restored particle video source for layer {i}"
                                                );
                                            }
                                        }
                                        Err(e) => log::warn!(
                                            "Failed to decode particle video for layer {i}: {e}"
                                        ),
                                    }
                                }
                                Err(e) => {
                                    log::warn!("Failed to probe particle video for layer {i}: {e}");
                                }
                            }
                        }
                    }
                    #[cfg(not(feature = "video"))]
                    {
                        let _ = video_path;
                        log::warn!(
                            "Preset for layer {i} names a particle video source, \
                             but this build has no video support"
                        );
                    }
                }
                Some(crate::gpu::particle::SourceSpec::Webcam) => {
                    #[cfg(feature = "webcam")]
                    {
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
                            if let Some(ps) = self
                                .layer_stack
                                .layers
                                .get_mut(i)
                                .and_then(|l| l.as_effect_mut())
                                .and_then(|e| e.pass_executor.particle_system.as_mut())
                            {
                                ps.set_webcam_source(&self.gpu.queue, w, h);
                                log::info!("Restored particle webcam source for layer {i}");
                            }
                        }
                    }
                    #[cfg(not(feature = "webcam"))]
                    log::warn!(
                        "Preset for layer {i} names a particle webcam source, \
                         but this build has no webcam support"
                    );
                }
                Some(crate::gpu::particle::SourceSpec::Model(model_path)) => {
                    if let Some(ps) = self
                        .layer_stack
                        .layers
                        .get_mut(i)
                        .and_then(|l| l.as_effect_mut())
                        .and_then(|e| e.pass_executor.particle_system.as_mut())
                    {
                        crate::gpu::particle::source_restore::restore_model_source(
                            &self.gpu.device,
                            &self.gpu.queue,
                            ps,
                            &model_path,
                            lp.particle_model_pose,
                            lp.particle_model_light,
                            i,
                        );
                    }
                }
                Some(crate::gpu::particle::SourceSpec::Image(img_path)) => {
                    if let Some(ps) = self
                        .layer_stack
                        .layers
                        .get_mut(i)
                        .and_then(|l| l.as_effect_mut())
                        .and_then(|e| e.pass_executor.particle_system.as_mut())
                    {
                        crate::gpu::particle::source_restore::restore_image_source(
                            &self.gpu.device,
                            &self.gpu.queue,
                            ps,
                            &img_path,
                            i,
                        );
                    }
                }
                None => {}
            }

            // Restore the Gaussian-splat scene (#1800) — a BACKGROUND load
            // (scenes reach ~1.5 GB, unlike the synchronous image restore
            // above); the layer renders its .pfx default (or empty) until
            // the decode lands via the main.rs drain.
            if let Some(ref scene_path) = lp.splat_scene_path {
                let ps_ref = self
                    .layer_stack
                    .layers
                    .get(i)
                    .and_then(|l| l.as_effect())
                    .and_then(|e| e.pass_executor.particle_system.as_ref());
                let splat_def = ps_ref.and_then(|ps| ps.def.splat.clone());
                let target = ps_ref.map(|ps| ps.max_particles);
                let already_loaded =
                    ps_ref.and_then(|ps| ps.splat_scene_path.as_ref()) == Some(scene_path);
                if let (Some(splat), Some(target)) = (splat_def, target) {
                    let path = std::path::PathBuf::from(scene_path);
                    if !path.exists() {
                        // Same UX as missing media: warn and keep going.
                        log::warn!("Splat scene '{scene_path}' not found for layer {i}");
                    } else if !already_loaded {
                        self.splat_loader.load(path, target, (&splat).into(), i);
                    }
                }
            }

            // Restore 3D-model obstacle source (#1851): re-load + depth-raster
            // from the stored file. Must precede the image branch — the model
            // path lives in `obstacle_image_path` but is not an image.
            if lp.obstacle_model == Some(true) {
                if let Some(ref model_path) = lp.obstacle_image_path {
                    let path = std::path::PathBuf::from(model_path);
                    if path.exists() {
                        if let Some(layer) = self.layer_stack.layers.get_mut(i) {
                            if let Some(effect) = layer.as_effect_mut() {
                                if let Some(ps) = effect.pass_executor.particle_system.as_mut() {
                                    match ps.set_obstacle_model(&self.gpu.device, &path) {
                                        Ok(()) => {
                                            if let Some(mode) = lp.obstacle_mode {
                                                ps.obstacle_mode =
                                                    crate::gpu::particle::ObstacleMode::from_u32(
                                                        mode,
                                                    );
                                            }
                                            if let Some(fit) = lp.obstacle_fit {
                                                ps.obstacle_fit =
                                                    crate::gpu::particle::ObstacleFit::from_u32(
                                                        fit,
                                                    );
                                            }
                                            if let Some(threshold) = lp.obstacle_threshold {
                                                ps.obstacle_threshold = threshold;
                                            }
                                            if let Some(elasticity) = lp.obstacle_elasticity {
                                                ps.obstacle_elasticity = elasticity;
                                            }
                                            log::info!("Restored obstacle model for layer {i}");
                                        }
                                        Err(e) => log::warn!(
                                            "Failed to load obstacle model for layer {i}: {e}"
                                        ),
                                    }
                                }
                            }
                        }
                    } else {
                        log::warn!("Obstacle model '{model_path}' not found for layer {i}");
                    }
                }
            }
            // Restore obstacle collision state
            else if let Some(ref obstacle_path) = lp.obstacle_image_path {
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
                                        // None (pre-#1790 preset) keeps the constructor
                                        // default Cover — aspect-correct, user-approved.
                                        if let Some(fit) = lp.obstacle_fit {
                                            ps.obstacle_fit =
                                                crate::gpu::particle::ObstacleFit::from_u32(fit);
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
                                if let Some(fit) = lp.obstacle_fit {
                                    ps.obstacle_fit =
                                        crate::gpu::particle::ObstacleFit::from_u32(fit);
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

            // Cloned before the layer borrow below so the lattice rebuild can
            // reach the GPU device while `layer_stack` is mutably borrowed.
            let device = self.gpu.device.clone();
            let hdr = crate::gpu::GpuContext::hdr_format();
            if let Some(layer) = self.layer_stack.layers.get_mut(i) {
                for (name, value) in &lp.params {
                    if layer.param_store.values.contains_key(name) {
                        layer.param_store.set(name, value.clone());
                    }
                }
                layer.blend_mode = lp.blend_mode;
                layer.opacity = lp.opacity;
                layer.displace_amount = lp.displace_amount;
                layer.enabled = lp.enabled && !effect_missing;
                layer.locked = lp.locked;
                layer.pinned = lp.pinned;
                layer.custom_name = lp.custom_name.clone();
                // Restore live particle-sim / Lattice panel edits over the
                // `.pfx` defaults that `ParticleSystem::new` just reset (runs
                // after the effect reload above, so this is the final word).
                if let Some(ps) = layer
                    .as_effect_mut()
                    .and_then(|e| e.pass_executor.particle_system.as_mut())
                {
                    if let Some(sim) = &lp.particle_sim {
                        ps.emit_rate = sim.emit_rate;
                        ps.def.emit_rate = sim.emit_rate;
                        ps.burst_on_beat = sim.burst_on_beat;
                        ps.def.burst_on_beat = sim.burst_on_beat;
                        ps.def.lifetime = sim.lifetime;
                        ps.def.initial_speed = sim.initial_speed;
                        ps.def.initial_size = sim.initial_size;
                        ps.def.drag = sim.drag;
                        // `None` (pre-existing presets) leaves the `.pfx` trail
                        // length; a saved override reallocates the trail buffer so
                        // the length matches on reload, not just `def`.
                        if let Some(len) = sim.trail_length {
                            ps.def.trail_length = len;
                            ps.set_trail_length(&device, hdr, len);
                        }
                    }
                    // Only `lattice_params` — never `lattice_defaults`, which the
                    // panel "Reset" restores from. `init_lattice` rebuilds the
                    // sim buffers if `grid_res` changed, else is a no-op.
                    if let Some(lat) = lp.lattice {
                        ps.lattice_params = lat;
                        ps.init_lattice(&device, hdr);
                    }
                    // Same for Helix: `init_helix` rebuilds the volumes if the
                    // grid or ring length changed, else is a no-op.
                    if let Some(hx) = lp.helix {
                        ps.helix_params = hx;
                        ps.init_helix(&device, hdr);
                    }
                }
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
        // Restore the global Volumetric (R3) mode. Disable when the preset has
        // no volumetric block so an earlier preset's volumetric can't bleed into
        // one saved without it. The per-frame copy onto the active layer then
        // propagates this on the next frame.
        if let Some(vol) = &preset.volumetric {
            self.volumetric_enabled = vol.enabled;
            self.volumetric_params = vol.params;
        } else {
            self.volumetric_enabled = false;
        }
        self.preset_store.current_preset = Some(index);
        self.preset_store.dirty = false;
        // Reset param changed flags so loading doesn't immediately mark dirty
        for layer in &mut self.layer_stack.layers {
            layer.param_store.changed = false;
        }
        // If this load came from a cue, apply the cue's param_overrides on top
        // of the preset values — this is the one funnel every load path exits
        // through, so sync, async-media, and dissolve-deferred loads all get
        // them (see the field's doc for the clobbering hazard this avoids).
        if let Some(cue_idx) = self.pending_cue_overrides.take() {
            if let Some(cue) = self.timeline.cues.get(cue_idx).cloned() {
                crate::scene::cueing::apply_cue_param_overrides(
                    &cue,
                    self.layer_stack.layers.iter_mut().map(|l| {
                        let locked = l.locked;
                        (&mut l.param_store, locked)
                    }),
                );
            }
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
        layer.content = LayerContent::Media(Box::new(media_layer));
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
                        self.load_preset_for_cue(idx, cue_index);
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
                        // The cue index rides along so its param_overrides apply
                        // to that deferred load.
                        let preset_idx = self.timeline.cues.get(to_cue).and_then(|cue| {
                            self.preset_store
                                .presets
                                .iter()
                                .position(|(name, _)| name == &cue.preset_name)
                        });
                        self.dissolve_capture_pending = preset_idx.map(|p| (p, to_cue));
                    }
                    crate::scene::types::TransitionType::ParamMorph => {
                        use crate::scene::cueing::MorphSnapshot;

                        // Snapshot current (outgoing) params
                        self.morph_from = Some(MorphSnapshot::capture(
                            self.layer_stack
                                .layers
                                .iter()
                                .map(|l| (&l.param_store.values, l.opacity)),
                        ));

                        // Load target preset. The cue-aware load applies the
                        // cue's param_overrides before the `to` snapshot below,
                        // so the morph lands ON the overridden values rather
                        // than the preset's saved ones.
                        if let Some(cue) = self.timeline.cues.get(to_cue) {
                            let preset_name = cue.preset_name.clone();
                            let preset_idx = self
                                .preset_store
                                .presets
                                .iter()
                                .position(|(name, _)| name == &preset_name);
                            if let Some(idx) = preset_idx {
                                self.load_preset_for_cue(idx, to_cue);
                            }
                        }

                        // Snapshot target (incoming) params after preset load
                        self.morph_to = Some(MorphSnapshot::capture(
                            self.layer_stack
                                .layers
                                .iter()
                                .map(|l| (&l.param_store.values, l.opacity)),
                        ));
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
                self.morph_from = None;
                self.morph_to = None;
            }
        }
    }

    /// Apply morph interpolation between saved from/to snapshots. The math
    /// lives in `scene::cueing` so the headless renderer runs the identical
    /// interpolation.
    fn apply_morph_interpolation(&mut self, progress: f32) {
        let (Some(from), Some(to)) = (&self.morph_from, &self.morph_to) else {
            return;
        };
        crate::scene::cueing::apply_morph(
            from,
            to,
            progress,
            self.layer_stack
                .layers
                .iter_mut()
                .map(|l| (&mut l.param_store, &mut l.opacity)),
        );
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

        // Poll particle counter readback from previous frame (non-blocking)
        for layer in &mut self.layer_stack.layers {
            if let Some(effect) = layer.as_effect_mut() {
                if let Some(ps) = effect.pass_executor.particle_system.as_mut() {
                    ps.poll_counter_readback();
                    ps.poll_lattice_population();
                }
            }
        }

        // Compute the HDR source from layer execution + compositing — shared
        // with the dissolve re-render below and the headless renderer.
        let (source, postprocess) = crate::gpu::frame_graph::execute_and_composite(
            &self.layer_stack,
            &mut self.compositor,
            &self.gpu.device,
            &self.gpu.queue,
            &mut encoder,
        );

        // Dissolve capture: on the first frame of a dissolve, capture outgoing then load incoming.
        // We must: (1) capture the snapshot from this frame's render, (2) submit those commands,
        // (3) load the new preset (mutates self), (4) re-render layers for the incoming scene.
        if let Some((preset_idx, cue_idx)) = self.dissolve_capture_pending.take() {
            if let Some(ref mut tr) = self.transition_renderer {
                tr.capture_snapshot(&self.gpu.device, &self.gpu.queue, &mut encoder, source);
            }
            // Submit capture commands so snapshot texture is filled
            self.gpu.queue.submit(std::iter::once(encoder.finish()));

            // Load the incoming preset (needs &mut self, no outstanding borrows
            // now). Cue-aware so the cue's param_overrides land on it.
            self.load_preset_for_cue(preset_idx, cue_idx);

            // Create fresh encoder and re-render layers for crossfade
            encoder = self
                .gpu
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("phosphor-encoder-dissolve"),
                });
            let (new_source, new_pp) = crate::gpu::frame_graph::execute_and_composite(
                &self.layer_stack,
                &mut self.compositor,
                &self.gpu.device,
                &self.gpu.queue,
                &mut encoder,
            );
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

            // v4l2 capture
            #[cfg(all(target_os = "linux", feature = "v4l2"))]
            if self.v4l2.is_running() {
                self.v4l2
                    .capture_frame(&self.gpu.device, &mut encoder, &self.post_process, source);
            }

            // Recording capture
            if self.recording.is_recording() {
                self.recording.capture_frame(
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
                        ps.request_lattice_population_readback();
                    }
                }
            }

            #[cfg(feature = "ndi")]
            if self.ndi.is_running() {
                self.ndi.post_submit();
            }

            #[cfg(all(target_os = "linux", feature = "v4l2"))]
            if self.v4l2.is_running() {
                self.v4l2.post_submit();
            }

            if self.recording.is_recording() {
                self.recording.post_submit();
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

        // v4l2 capture
        #[cfg(all(target_os = "linux", feature = "v4l2"))]
        if self.v4l2.is_running() {
            self.v4l2
                .capture_frame(&self.gpu.device, &mut encoder, &self.post_process, source);
        }

        // Recording capture
        if self.recording.is_recording() {
            self.recording.capture_frame(
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

        // Request particle counter + lattice population readback (async, read next
        // frame). The lattice request was previously issued ONLY on the dissolve-
        // transition path above, so on every normal frame the population map was
        // never requested — the auto-reseed then read a perpetually-None population
        // and never fired, so growth rules just filled the domain and parked on a
        // sphere. Requesting it here (alongside the counter) is what makes the
        // reseed work at all.
        for layer in &self.layer_stack.layers {
            if let Some(effect) = layer.as_effect() {
                if let Some(ps) = &effect.pass_executor.particle_system {
                    ps.request_counter_readback();
                    ps.request_lattice_population_readback();
                }
            }
        }
        // Run completed async map callbacks so the readbacks land (wgpu only fires
        // them during a poll). Non-blocking: it processes work the GPU already
        // finished and never stalls the frame.
        let _ = self.gpu.device.poll(wgpu::PollType::Poll);

        // NDI: request async map on staging buffer (must be after queue.submit)
        #[cfg(feature = "ndi")]
        if self.ndi.is_running() {
            self.ndi.post_submit();
        }

        #[cfg(all(target_os = "linux", feature = "v4l2"))]
        if self.v4l2.is_running() {
            self.v4l2.post_submit();
        }

        if self.recording.is_recording() {
            self.recording.post_submit();
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

/// Does a batch of changed shader paths touch this effect?
///
/// Only used to decide whether a layer whose last load *failed* should retry the
/// whole load (#1855). A failed load leaves `effect_index` on the new effect while
/// the executor still belongs to the old one, so there is nothing to patch
/// incrementally — the layer either rebuilds or stays broken.
fn changes_touch_effect(
    effect: &crate::effect::format::PfxEffect,
    changes: &[std::path::PathBuf],
    lib_changed: bool,
) -> bool {
    if lib_changed {
        return true;
    }
    let touches = |name: &str| !name.is_empty() && changes.iter().any(|c| c.ends_with(name));
    effect
        .normalized_passes()
        .iter()
        .any(|p| touches(&p.shader))
        || effect
            .particles
            .as_ref()
            .is_some_and(|pd| touches(&pd.compute_shader))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effect::format::PfxEffect;
    use std::path::PathBuf;

    fn effect(json: &str) -> PfxEffect {
        serde_json::from_str(json).expect("test effect must deserialize")
    }

    #[test]
    fn a_lib_change_retries_every_failed_layer() {
        let e = effect(r#"{"name":"T","author":"Fosfora","shader":"t.wgsl"}"#);
        assert!(changes_touch_effect(&e, &[], true));
    }

    #[test]
    fn only_this_effects_shaders_retry_it() {
        let e = effect(r#"{"name":"T","author":"Fosfora","shader":"tide.wgsl"}"#);
        assert!(changes_touch_effect(
            &e,
            &[PathBuf::from("/assets/shaders/tide.wgsl")],
            false
        ));
        assert!(!changes_touch_effect(
            &e,
            &[PathBuf::from("/assets/shaders/frost.wgsl")],
            false
        ));
        assert!(!changes_touch_effect(&e, &[], false));
    }

    // A particle effect's compute shader is the one most likely to have caused the
    // failed load in the first place, so editing it must retrigger the rebuild.
    #[test]
    fn a_compute_shader_change_retries_the_effect() {
        let e = effect(
            r#"{"name":"T","author":"Fosfora","shader":"t_bg.wgsl",
                "particles":{"compute_shader":"t_sim.wgsl","max_count":1000}}"#,
        );
        assert!(changes_touch_effect(
            &e,
            &[PathBuf::from("/assets/shaders/t_sim.wgsl")],
            false
        ));
    }

    // An empty compute_shader is how a volume effect (Helix, Lattice) declares it
    // has no sim — `ends_with("")` is true for every path, so an unguarded check
    // would retry those layers on any shader edit in the tree.
    #[test]
    fn an_empty_compute_shader_name_matches_nothing() {
        let e = effect(
            r#"{"name":"T","author":"Fosfora","shader":"t_bg.wgsl",
                "particles":{"compute_shader":"","max_count":1000}}"#,
        );
        assert!(!changes_touch_effect(
            &e,
            &[PathBuf::from("/assets/shaders/unrelated.wgsl")],
            false
        ));
    }
}
