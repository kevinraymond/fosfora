use std::sync::Arc;
use std::sync::atomic::Ordering;
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
use crate::gpu::render_target::{PingPongTarget, RenderTarget};
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

/// The outgoing preset of a live Dissolve, with the volumetric settings it
/// rendered with (the incoming preset's load replaces App's).
pub struct RetiringStack {
    pub stack: LayerStack,
    pub volumetric_enabled: bool,
    pub volumetric_params: crate::gpu::volumetric::VolumetricParams,
}

/// The index of the default camera in a device list: the camera saved by
/// name if it is connected, else the saved index if the list has it, else
/// the first camera. A saved index alone goes stale, since the OS renumbers
/// cameras as they come and go; one that named no camera left "+ Webcam"
/// failing with nothing to pick another from.
#[cfg(feature = "webcam")]
fn resolve_default_webcam(
    devices: &[(u32, String)],
    saved_name: Option<&str>,
    saved_index: Option<u32>,
) -> u32 {
    let by_name = saved_name.and_then(|name| devices.iter().find(|(_, n)| n == name));
    let by_index = || saved_index.and_then(|index| devices.iter().find(|(i, _)| *i == index));
    by_name
        .or_else(by_index)
        .or(devices.first())
        .map_or(0, |(index, _)| *index)
}

/// The cameras to pick from: the connected ones as the active backend lists
/// them, then the network streams switched on in settings. Also returns the
/// names among them that are streams.
#[cfg(feature = "webcam")]
fn list_webcam_devices(
    use_ffmpeg: bool,
    streams: &[crate::settings::RtmpStream],
) -> (Vec<(u32, String)>, Vec<String>) {
    let mut devices = if use_ffmpeg {
        crate::media::webcam_ffmpeg::list_devices().unwrap_or_default()
    } else {
        crate::media::webcam::list_devices().unwrap_or_default()
    };
    let cameras: Vec<&str> = devices.iter().map(|(_, name)| name.as_str()).collect();
    let stream_names: Vec<String> = crate::settings::stream_names(streams, &cameras)
        .into_iter()
        .map(str::to_string)
        .collect();
    let first_free = devices
        .iter()
        .map(|(index, _)| index + 1)
        .max()
        .unwrap_or(0);
    devices.extend((first_free..).zip(stream_names.iter().cloned()));
    (devices, stream_names)
}

pub struct App {
    pub gpu: GpuContext,
    pub start_time: Instant,
    pub last_frame: Instant,
    pub frame_count: u32,
    /// FOSFORA_FRAME_LOG=1 — per-frame CSV of both clocks + brightness drivers.
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
    /// The OS asks for reduced motion (#109), read once at startup. Auto flash
    /// limiting goes Strict and interface animations are switched off.
    pub reduce_motion: bool,
    /// Theme files from the themes folder (#3125), read at startup and on
    /// Reload in Appearance.
    pub custom_themes: Vec<crate::ui::theme::custom::CustomTheme>,
    // Layers
    pub layer_stack: LayerStack,
    // Compositor + post-processing (separate from layer_stack to avoid borrow conflicts)
    pub compositor: Compositor,
    pub post_process: PostProcessChain,
    /// Master post-processing (#3147): one setting for the whole preset,
    /// saved with it. Each layer still carries its effect's own
    /// `postprocess` block, but only as a suggestion — selecting a layer
    /// never changes this. See [`App::adopt_layer_postprocess`].
    pub master_postprocess: PostProcessDef,
    /// What Master's post-processing was before it was last replaced by a
    /// reset, for the inspector's way back. Cleared when a preset loads.
    pub master_postprocess_previous: Option<PostProcessDef>,
    /// The finished frame, off-screen (#3122). Post-processing renders here
    /// instead of straight onto the swapchain, and the window gets a blit of
    /// it. That indirection is what lets the v2 interface show the output as a
    /// preview inside a panel, and lets a second window present the same frame,
    /// without either of them re-running the post chain.
    pub display: RenderTarget,
    /// The v2 layer rows' pictures (#3123): each layer alone, and the stack
    /// blended up to it. Tapped only while the workspace shell is on screen.
    pub layer_thumbs: crate::gpu::layer_thumbs::LayerThumbs,
    /// The v2 catalog's effect pictures, decoded on first use (#3124).
    pub catalog_thumbs: crate::ui::catalog_thumbs::CatalogThumbs,
    /// The second output window, when one is open (#3122). It presents the
    /// display target above and carries no interface. Opened and closed from
    /// `main.rs`, where the `ActiveEventLoop` that can create a window lives.
    pub output_window: Option<crate::output_window::OutputWindow>,
    /// The displays the window system reports, refreshed periodically from the
    /// event loop so the picker lists a projector plugged in mid-session.
    pub displays: Vec<crate::output_window::DisplayInfo>,
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
    // Spout output (Windows texture sharing, feature-gated)
    #[cfg(all(target_os = "windows", feature = "spout"))]
    pub spout: crate::spout::SpoutSystem,
    // Syphon output (macOS texture sharing, feature-gated)
    #[cfg(all(target_os = "macos", feature = "syphon"))]
    pub syphon: crate::syphon::SyphonSystem,
    // Ableton Link session sync (feature-gated)
    #[cfg(feature = "link")]
    pub link: crate::link::LinkSystem,
    // Video recording (always available — ffmpeg is a subprocess)
    pub recording: crate::recording::RecordingSystem,
    // Scenes
    pub scene_store: SceneStore,
    pub timeline: Timeline,
    pub transition_renderer: Option<TransitionRenderer>,
    /// A preset switch waiting for its media to decode or for render() to
    /// capture the outgoing frame (`scene::switch`). Cues and plain switches
    /// both stage here.
    pub staged_switch: Option<crate::scene::switch::StagedSwitch>,
    /// The switch in flight: the frame crossfade and the param morph read
    /// their progress from it, whether a cue or a preset click started it.
    pub active_transition: Option<crate::scene::switch::ActiveTransition>,
    /// The outgoing preset of a live Dissolve, still animating until the fade
    /// ends ("Keep moving"). Its own field so OSC, bindings, the web remote
    /// and the UI, which all address layers by index, only ever reach the
    /// live stack.
    pub retiring: Option<RetiringStack>,
    /// Composites `retiring`. A second compositor, not the live one reused:
    /// each composite writes its blend uniforms with `queue.write_buffer`,
    /// and two composites through one compositor in a frame would both read
    /// the last write. Made on the first live fade and kept.
    pub retire_compositor: Option<Compositor>,
    /// Cue whose `param_overrides` apply after the next preset finishes
    /// loading. Consumed at the END of `apply_preset_immediately`, which is the
    /// one point every load path funnels through — the sync fast path, the
    /// async media-decode completion, and a staged switch (`apply_staged_switch`). Applying
    /// eagerly at the timeline event instead would be clobbered by the async
    /// path, whose decode lands whole frames later.
    pub pending_cue_overrides: Option<usize>,
    /// Media files decoding for new layers, oldest first. A video pre-decodes
    /// every frame, which took ~15 s with the app frozen and nothing on
    /// screen while it ran on this thread.
    pub media_loads: Vec<MediaLoad>,
    pub midi_clock: MidiClock,
    /// Whether MIDI clock was playing last frame (for rising-edge transport detection).
    pub midi_clock_was_playing: bool,
    /// Whether a MIDI clock beat boundary was crossed this frame.
    pub midi_clock_beat_crossed: bool,
    // Shader editor
    pub shader_editor: ShaderEditorState,
    // Trama node-graph system (M0) — graph, registry, executor, canvas state
    pub trama: crate::trama::TramaSystem,
    /// Chain output targets, owned here rather than by the executor: the
    /// composite loop holds shared references to them across iterations while
    /// re-borrowing `&mut trama` on each one.
    pub chain_targets: crate::gpu::chain_targets::ChainTargets,
    // Binding matrix modal
    pub binding_matrix: crate::ui::panels::binding_matrix::BindingMatrixState,
    // Quit confirmation
    pub quit_requested: bool,
    // Transient status error (displayed in status bar, auto-clears)
    pub status_error: Option<(String, Instant)>,
    // Webcam capture (feature-gated)
    /// One running capture per camera in use, shared by every layer that
    /// shows that camera.
    #[cfg(feature = "webcam")]
    pub webcam_captures: Vec<WebcamBackend>,
    #[cfg(feature = "webcam")]
    pub webcam_devices: Vec<(u32, String)>,
    /// The entries of `webcam_devices` that are network streams.
    #[cfg(feature = "webcam")]
    webcam_stream_names: Vec<String>,
    /// The default camera, as an index into `webcam_devices`: what a
    /// particle source, an obstacle and the first camera layer use.
    #[cfg(feature = "webcam")]
    pub webcam_device_index: u32,
    #[cfg(feature = "webcam")]
    pub use_ffmpeg_webcam: bool,
    /// A camera layer waiting on the macOS camera prompt: the answer, and the
    /// device to add once it is yes (GH #212).
    #[cfg(all(target_os = "macos", feature = "webcam"))]
    webcam_access_pending: Option<(crossbeam_channel::Receiver<bool>, String)>,
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

        // Prefer the launch effect, fall back to the first one
        let default_idx = effect_loader
            .effects
            .iter()
            .position(|e| e.name == crate::effect::loader::LAUNCH_EFFECT)
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
                rates: Default::default(),
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

        let layer_thumbs = crate::gpu::layer_thumbs::LayerThumbs::new(&gpu.device);

        // Post-processing chain
        let post_process = PostProcessChain::new(
            &gpu.device,
            gpu.format,
            hdr_format,
            gpu.surface_config.width,
            gpu.surface_config.height,
        );

        // Display target: the finished frame, in the surface's own format so the
        // blit to the swapchain is a straight copy.
        let display = Self::new_display(
            &gpu.device,
            gpu.surface_config.width,
            gpu.surface_config.height,
            gpu.format,
        );

        let shader_watcher = ShaderWatcher::new();
        let shader_compiler = ShaderCompiler::new();
        let settings = SettingsConfig::load();
        #[cfg(feature = "webcam")]
        let use_ffmpeg_webcam = settings.use_ffmpeg_webcam;
        #[cfg(feature = "webcam")]
        let (webcam_devices, webcam_stream_names) =
            list_webcam_devices(use_ffmpeg_webcam, &settings.rtmp_streams);
        #[cfg(feature = "webcam")]
        let webcam_device_from_settings = resolve_default_webcam(
            &webcam_devices,
            settings.webcam_device_name.as_deref(),
            settings.webcam_device,
        );
        let mut audio = AudioSystem::new_with_device(
            settings.audio_device.as_deref(),
            settings.band_scale,
            Arc::new(std::sync::Mutex::new(settings.structure_tuning)),
            Arc::new(std::sync::Mutex::new(crate::audio::TempoControl::new(
                settings.tempo,
            ))),
        )?;
        // A9 (#1460): a setter rather than a 5th `new_with_device` param — the audio thread
        // never sees this value, so threading it through construction would touch every
        // caller for nothing.
        audio.set_auto_reconnect(settings.auto_reconnect);
        audio.set_input_trim_db(settings.input_trim_db);
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
        let custom_themes =
            crate::ui::theme::custom::load_dir(&crate::ui::theme::custom::themes_dir());
        let egui_overlay = EguiOverlay::new(
            &gpu.device,
            gpu.format,
            &window,
            settings.theme.palette(&custom_themes),
            settings.ui_scale,
        );
        crate::ui::theme::custom::publish(&egui_overlay.context(), &custom_themes);
        let reduce_motion =
            crate::ui::accessibility::motion::ReducedMotion::detect().should_reduce();
        if reduce_motion {
            log::info!("System asks for reduced motion: strict flash limit, no UI animation");
            // configure() clones the current style, so this survives theme changes.
            egui_overlay.context().style_mut(|s| s.animation_time = 0.0);
        }
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
        #[cfg(all(target_os = "windows", feature = "spout"))]
        let spout = crate::spout::SpoutSystem::new(
            &gpu.device,
            gpu.format,
            gpu.surface_config.width,
            gpu.surface_config.height,
        );
        #[cfg(all(target_os = "macos", feature = "syphon"))]
        let syphon = crate::syphon::SyphonSystem::new(
            &gpu.device,
            gpu.format,
            gpu.surface_config.width,
            gpu.surface_config.height,
        );
        let recording = crate::recording::RecordingSystem::new();

        let trama = crate::trama::TramaSystem::new(
            &gpu.device,
            gpu.pipeline_cache.as_ref(),
            &effect_loader,
            &placeholder,
            &audio_textures,
            gpu.surface_config.width,
            gpu.surface_config.height,
        );

        let chain_targets = crate::gpu::chain_targets::ChainTargets::new(
            gpu.surface_config.width,
            gpu.surface_config.height,
        );

        #[cfg(feature = "profiling")]
        let gpu_profiler = crate::gpu::profiler::Profiler::new(&gpu.device);

        let now = Instant::now();
        let mut app = Self {
            gpu,
            mel_last_commit: None,
            mel_commit_interval: 1.0 / 43.0, // ~43 Hz audio-hop column rate
            uniforms: ShaderUniforms::zeroed(),
            start_time: now,
            last_frame: now,
            frame_count: 0,
            frame_log: std::env::var("FOSFORA_FRAME_LOG").is_ok(),
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
            staged_switch: None,
            active_transition: None,
            retiring: None,
            retire_compositor: None,
            pending_cue_overrides: None,
            media_loads: Vec::new(),
            midi_clock: MidiClock::new(),
            midi_clock_was_playing: false,
            midi_clock_beat_crossed: false,
            settings,
            reduce_motion,
            custom_themes,
            egui_overlay,
            effect_loader,
            window,
            layer_stack,
            compositor,
            post_process,
            master_postprocess: PostProcessDef::default(),
            master_postprocess_previous: None,
            display,
            layer_thumbs,
            catalog_thumbs: crate::ui::catalog_thumbs::CatalogThumbs::shipped(),
            output_window: None,
            displays: Vec::new(),
            volumetric_enabled: false,
            volumetric_params: crate::gpu::volumetric::VolumetricParams::default(),
            placeholder,
            audio_textures,
            #[cfg(feature = "ndi")]
            ndi,
            #[cfg(all(target_os = "linux", feature = "v4l2"))]
            v4l2,
            #[cfg(all(target_os = "windows", feature = "spout"))]
            spout,
            #[cfg(all(target_os = "macos", feature = "syphon"))]
            syphon,
            #[cfg(feature = "link")]
            link: crate::link::LinkSystem::new(crate::link::LinkConfig::load()),
            recording,
            shader_editor: ShaderEditorState::default(),
            trama,
            chain_targets,
            binding_matrix: crate::ui::panels::binding_matrix::BindingMatrixState::new(),
            quit_requested: false,
            status_error: None,
            #[cfg(feature = "webcam")]
            webcam_captures: Vec::new(),
            #[cfg(feature = "webcam")]
            webcam_devices,
            #[cfg(feature = "webcam")]
            webcam_stream_names,
            #[cfg(feature = "webcam")]
            webcam_device_index: webcam_device_from_settings,
            #[cfg(feature = "webcam")]
            use_ffmpeg_webcam,
            #[cfg(all(target_os = "macos", feature = "webcam"))]
            webcam_access_pending: None,
            particle_source_loader: crate::gpu::particle::ParticleSourceLoader::new(),
            splat_loader: crate::gpu::particle::SplatSceneLoader::new(),
            splat_demo_download: None,
            #[cfg(feature = "depth")]
            depth_thread: None,
            #[cfg(feature = "depth")]
            depth_download: None,
            #[cfg(feature = "profiling")]
            gpu_profiler,
        };
        app.open_launch_stack();
        Ok(app)
    }

    /// Close the second output window, if one is open (#3122). Dropping it
    /// drops its surface with it — the surface holds the window alive, so the
    /// two can only go together.
    pub fn close_output_window(&mut self) {
        if let Some(ow) = self.output_window.take() {
            log::info!("Output window closed ({})", ow.display_name);
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        // Windows delivers minimize as a 0x0 resize; zero-size render targets
        // are invalid (validation-error spam) and zero-size capture staging
        // buffers panic on map. Skip entirely — restore sends the real size.
        if width == 0 || height == 0 {
            return;
        }
        // A fade's pictures are the old size: the snapshot is reallocated
        // below, and the outgoing stack would need a full resize for a second
        // at most. End the fade. (A lost surface "resizes" to the same size;
        // that keeps it.)
        if (width, height)
            != (
                self.gpu.surface_config.width,
                self.gpu.surface_config.height,
            )
        {
            self.active_transition = None;
            self.retiring = None;
        }
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
        if let Some(c) = self.retire_compositor.as_mut() {
            c.resize(&self.gpu.device, width, height);
        }
        // The compositor just recreated the @backdrop texture; every executor
        // holds a cloned handle to the OLD one. Refresh + rebind (set_backdrop
        // rebuilds bind groups only for layers that actually sample it).
        for layer in &mut self.layer_stack.layers {
            if let Some(e) = layer.as_effect_mut() {
                e.pass_executor.set_backdrop(
                    Some((
                        self.compositor.backdrop.view.clone(),
                        self.compositor.backdrop.sampler.clone(),
                    )),
                    &self.gpu.device,
                    &e.uniform_buffer,
                    &self.placeholder,
                    &self.audio_textures,
                );
            }
        }
        self.post_process.resize(&self.gpu.device, width, height);
        self.display = Self::new_display(&self.gpu.device, width, height, self.gpu.format);
        let display_view = self.display_view_for_egui();
        self.egui_overlay
            .set_display_texture(&self.gpu.device, &display_view);
        self.trama.resize(width, height);
        self.chain_targets.resize(width, height);
        self.egui_overlay
            .resize(width, height, self.window.scale_factor() as f32);
        if let Some(ref mut tr) = self.transition_renderer {
            tr.resize(&self.gpu.device, width, height, GpuContext::hdr_format());
        }
        #[cfg(feature = "ndi")]
        self.ndi.resize(&self.gpu.device, width, height);
        // v4l2 deliberately does not resize (readers can't tolerate mid-stream
        // geometry changes); Spout and Syphon receivers adapt, so they follow
        // the window.
        #[cfg(all(target_os = "windows", feature = "spout"))]
        self.spout.resize(&self.gpu.device, width, height);
        #[cfg(all(target_os = "macos", feature = "syphon"))]
        self.syphon.resize(&self.gpu.device, width, height);
    }

    /// The full state snapshot web clients sync from.
    fn web_full_state(&self) -> String {
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
        // Every stream switched on, with the light and words the settings
        // page shows.
        #[cfg(feature = "webcam")]
        let streams = self
            .settings
            .rtmp_streams
            .iter()
            .filter(|s| s.is_usable())
            .map(|s| {
                use crate::media::stream::{NOT_LISTENING, StreamLight};
                let (light, status) = self
                    .webcam_captures
                    .iter()
                    .filter(|c| c.device_name() == s.name)
                    .find_map(|c| c.stream_status())
                    .unwrap_or((StreamLight::Down, NOT_LISTENING.to_string()));
                crate::web::state::StreamInfo {
                    name: s.name.clone(),
                    light: match light {
                        StreamLight::Connected => "connected",
                        StreamLight::Waiting => "waiting",
                        StreamLight::Down => "down",
                    },
                    status,
                }
            })
            .collect();
        #[cfg(not(feature = "webcam"))]
        let streams = Vec::new();
        crate::web::state::build_full_state(
            &self.effect_loader.effects,
            &layer_infos,
            self.layer_stack.active_layer,
            &layer_data,
            &self.preset_store,
            self.post_process.enabled,
            streams,
        )
    }

    pub fn update(&mut self) {
        // Surface sender-thread failures (dead NDI runtime, closed device) so the
        // status dot goes off instead of staying green with zero frames sent.
        #[cfg(feature = "ndi")]
        self.ndi.pipeline.poll_health();
        #[cfg(all(target_os = "linux", feature = "v4l2"))]
        self.v4l2.pipeline.poll_health();
        #[cfg(all(target_os = "windows", feature = "spout"))]
        self.spout.pipeline.poll_health();
        #[cfg(all(target_os = "macos", feature = "syphon"))]
        self.syphon.pipeline.poll_health();

        let now = Instant::now();
        // Clamped: a frame hitch (mouse click stall, window drag, effect swap)
        // otherwise integrates as one giant step — particles teleport past
        // kill bounds, trail ribbons smear across the screen for a frame, and
        // the emission accumulator dumps the entire stall's budget at once
        // (#1796 live finding: every real mouse click white-flashed Tide).
        // Momentary slow-motion during a stall beats a white flash.
        let dt = now.duration_since(self.last_frame).as_secs_f32().min(0.05);
        self.last_frame = now;

        // A watch that failed at startup is noted once, on the first frame,
        // so the six seconds start when there is a window to show them in.
        if let Some(msg) = self.shader_watcher.take_degraded_notice() {
            self.status_error = Some((msg, now));
        }

        // The macOS camera prompt was answered: add the layer that waited on it.
        #[cfg(all(target_os = "macos", feature = "webcam"))]
        if let Some((answer, device_name)) = &self.webcam_access_pending {
            let device_name = device_name.clone();
            match answer.try_recv() {
                Ok(true) => {
                    log::info!("Camera access granted at the prompt; adding the camera layer");
                    self.webcam_access_pending = None;
                    self.add_webcam_layer(&device_name);
                }
                Ok(false) => {
                    self.webcam_access_pending = None;
                    self.status_error =
                        Some((crate::media::webcam::CAMERA_DENIED.into(), Instant::now()));
                }
                Err(crossbeam_channel::TryRecvError::Empty) => {}
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    self.webcam_access_pending = None;
                }
            }
        }

        // Auto-clear status error after 6 seconds
        if let Some((_, when)) = &self.status_error {
            if when.elapsed().as_secs_f64() > 6.0 {
                self.status_error = None;
            }
        }

        // Update global time uniforms
        self.uniforms.time =
            crate::gpu::uniforms::shader_time(now.duration_since(self.start_time).as_secs_f64());
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

        // Diagnostic (FOSFORA_FRAME_LOG=1): one CSV line per frame covering both
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
            if let Some(visible) = osc_result.overlay_visible {
                self.egui_overlay.visible = visible;
            }
            if let Some(pp_enabled) = osc_result.postprocess_enabled {
                self.post_process.enabled = pp_enabled;
                self.master_postprocess.enabled = pp_enabled;
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
            if osc_result.preset_transition.is_some() || osc_result.preset_transition_secs.is_some()
            {
                use crate::scene::types::TransitionType;
                let kind = osc_result.preset_transition.map(|k| match k {
                    0 => TransitionType::Cut,
                    1 => TransitionType::Dissolve,
                    _ => TransitionType::ParamMorph,
                });
                self.set_preset_transition(kind, osc_result.preset_transition_secs, None);
            }
        }

        // Set when a preset load should reach web clients this frame rather
        // than at the next 10 Hz state refresh.
        let mut web_state_changed = false;

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
                self.master_postprocess.enabled = pp_enabled;
            }

            // Handle effect load from web (coalesced to the last one this frame)
            if let Some(effect_idx) = web_result.effect_load {
                let active_locked = self.layer_stack.active().map_or(false, |l| l.locked);
                if !crate::web::state::remote_loadable(&self.effect_loader.effects, effect_idx) {
                    log::warn!(
                        "Web remote asked for effect {effect_idx}, which it does not list; ignored"
                    );
                } else if !active_locked {
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

            // Handle preset load from web (coalesced to the last one this frame).
            // A switch with a transition applies a frame or more later; the
            // staged apply below marks web state changed when it does.
            let had_preset_loads = web_result.preset_load.is_some();
            if let Some(preset_idx) = web_result.preset_load {
                self.switch_preset(preset_idx);
            }

            // After preset load, push full state so all clients update
            web_state_changed |= had_preset_loads;
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
        // Same for a trama chain: it is saved with the preset, and editing one
        // touches no layer parameter.
        if self.trama.take_edited() {
            self.preset_store.mark_dirty();
        }

        // Drain async preset decode results
        if let Some(result) = self.preset_loader.try_recv() {
            log::info!(
                "Async preset decode complete, applying preset index {}",
                result.preset_index
            );
            match self.staged_switch.as_mut() {
                // A switch with a transition: its fade starts from the frame
                // on screen now, not from when it was asked for.
                Some(staged) if staged.stage == crate::scene::switch::Stage::Decoding => {
                    staged.decode_landed(result);
                }
                _ => {
                    let index = result.preset_index;
                    let preset = result.preset;
                    self.apply_preset_immediately(index, &preset, result.decoded_media);
                    // Push full state to web clients after async preset load
                    web_state_changed = true;
                }
            }
        }
        // A staged switch applies once its frame is captured (see
        // `scene::switch` for the stages).
        if let Some(captured) = self.staged_switch.as_mut().and_then(|s| s.poll()) {
            if let Some(staged) = self.staged_switch.take() {
                self.apply_staged_switch(staged, captured);
                web_state_changed = true;
            }
        }

        // Drain MIDI clock bytes into MidiClock
        self.midi_clock_beat_crossed = self.midi.drain_clock(&mut self.midi_clock);

        // Ableton Link: poll the session and route tempo per the configured
        // mode (Follow pins the tracker's prior, Lead commits our detected
        // BPM). The tick's beat/transport edges feed the timeline below.
        #[cfg(feature = "link")]
        let link_tick = {
            let detected_bpm = self.latest_audio.map(|a| a.raw_bpm()).unwrap_or(0.0);
            self.link.drive(self.audio.tempo(), detected_bpm, dt)
        };

        // Auto-follow Link transport → timeline (opt-in via start/stop sync),
        // same contract as the MIDI transport follow below.
        #[cfg(feature = "link")]
        if self.link.config.start_stop_sync {
            if let Some(t) = link_tick {
                if t.playing_started && !self.timeline.active && !self.timeline.cues.is_empty() {
                    let event = self.timeline.start(0);
                    self.process_timeline_event(event);
                }
                if t.playing_stopped && self.timeline.active {
                    self.timeline.stop();
                }
            }
        }

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

        // Advance timeline (scene system). Held while a cue's switch is
        // staged: its media may take seconds to decode, and the cue's hold
        // time must not run out before the cue is even on screen.
        let cue_staged = self.staged_switch.as_ref().is_some_and(|s| s.cue.is_some());
        if self.timeline.active && !cue_staged {
            // Tempo for transition_beats resolution. From latest_audio, not
            // uniforms.bpm: a `uniform.bpm` binding evaluated above can have
            // overwritten the uniform mirror by now, and a binding must not be
            // able to warp transition lengths.
            self.timeline.set_beat_period(
                self.latest_audio
                    .as_ref()
                    .and_then(|a| a.beat_period_secs()),
            );
            // Feed beat signal for BeatSync mode. Precedence: Link session
            // grid (while peers are connected — the shared grid is the point,
            // and it holds through breakdowns where the detector goes quiet) >
            // MIDI clock while its transport plays > our own beat detector.
            #[cfg(feature = "link")]
            let link_beat = link_tick.filter(|t| t.peers > 0).map(|t| t.beat_crossed);
            #[cfg(not(feature = "link"))]
            let link_beat: Option<bool> = None;
            let beat_on = if let Some(crossed) = link_beat {
                crossed
            } else if self.midi_clock.playing() {
                self.midi_clock_beat_crossed
            } else {
                self.uniforms.beat > 0.5
            };
            let beat_event = self.timeline.feed_beat(beat_on);
            self.process_timeline_event(beat_event);

            // Tick for timer-based advance
            let tick_event = self.timeline.tick(dt);
            self.process_timeline_event(tick_event);
        }

        // The switch in flight, a cue's or a preset click's: the morph runs
        // here, the frame crossfade in render().
        if let Some(t) = self.active_transition.as_mut() {
            let finished = t.advance(dt);
            if let Some((from, to)) = t.morph() {
                // `apply_morph` resets `changed`: a transition is not a user
                // edit and must not mark the preset dirty.
                crate::scene::cueing::apply_morph(
                    from,
                    to,
                    t.progress(),
                    self.layer_stack
                        .layers
                        .iter_mut()
                        .map(|l| (&mut l.param_store, &mut l.opacity)),
                );
            }
            if finished {
                self.active_transition = None;
                // Frees the outgoing preset's GPU resources.
                self.retiring = None;
                // And the cameras only it was showing.
                #[cfg(feature = "webcam")]
                self.cleanup_webcam_if_unused();
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

        // Web: the one place full state is built — stored for new-client initial
        // sync and broadcast to connected clients, at 10 Hz or at once after a
        // preset load.
        if self.web.state_due(web_state_changed) {
            let state_json = self.web_full_state();
            self.web.update_latest_state(state_json);
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
        self.pump_webcams();

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

        // Advance trama modulation and capture this frame's fully-mirrored
        // template. Reads the features App already drained this frame —
        // `latest_features` consumes a single-consumer pulse latch and must
        // not be called again.
        let trama_audio = self.latest_audio.unwrap_or_default();
        // The canvas edits the SELECTED layer's chain, so opening it on a
        // layer is what brings that layer's chain into existence. A fresh
        // chain holds only its Output node, reaches nothing, and therefore
        // changes nothing about what the layer renders.
        //
        // No layer to select falls back to the master chain rather than
        // leaving last frame's id in place: that id may name a layer that is
        // gone, and the canvas would file the master graph under it.
        self.trama.resolve_active_chain(&mut self.layer_stack);
        self.trama.update(
            &mut self.layer_stack,
            dt,
            &self.uniforms,
            &trama_audio,
            self.audio.latest_mel(),
        );

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
        // The outgoing preset of a live Dissolve keeps animating: the same
        // per-frame work as the live stack's media, particle sources and
        // uniforms above. Its bindings were replaced at the switch, so values
        // they drove hold; audio and time still move it.
        if let Some(r) = self.retiring.as_mut() {
            for layer in &mut r.stack.layers {
                match layer.content {
                    LayerContent::Media(ref mut m) => {
                        m.advance(dt);
                        m.upload_frame(&self.gpu.queue);
                    }
                    LayerContent::Effect(ref mut e) => {
                        if let Some(ref mut ps) = e.pass_executor.particle_system {
                            ps.update_source(&self.gpu.queue, dt as f64);
                            if ps.source_transition.is_some() {
                                ps.advance_transition(&self.gpu.queue, dt);
                            }
                        }
                    }
                }
            }
            crate::gpu::frame_prep::prepare_effect_layers(
                &mut r.stack.layers,
                &self.uniforms,
                &self.latest_audio.unwrap_or_default(),
                dt,
                &self.gpu.device,
                &self.gpu.queue,
                r.stack.active_layer,
                r.volumetric_enabled,
                r.volumetric_params,
            );
        }

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
        let trama_changes = self.shader_watcher.drain_trama_changes();
        let lib_changed = changes
            .iter()
            .any(|p| p.to_string_lossy().contains("/lib/"));
        if lib_changed {
            self.effect_loader.reload_library();
        }
        // trama effects have their own directory and their own registry. A
        // library change reloads all of them: every one is compiled with the
        // library prepended. After `reload_library`, so they see the new text.
        if lib_changed || !trama_changes.is_empty() {
            self.trama.reload_effects(
                &self.gpu.device,
                self.gpu.pipeline_cache.as_ref(),
                &self.effect_loader,
                &trama_changes,
                lib_changed,
                &mut self.layer_stack,
            );
        }
        if !changes.is_empty() {
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
                        // Master follows an edited .pfx only while it is
                        // still exactly what that effect suggested — someone
                        // tuning the effect's file sees the change; someone
                        // who has since tuned Master keeps their own.
                        let old = std::mem::replace(
                            &mut self.layer_stack.layers[layer_idx].postprocess,
                            pp.clone(),
                        );
                        if self.master_postprocess == old {
                            self.master_postprocess = pp;
                            self.post_process.enabled = self.master_postprocess.enabled;
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
        // Same guards `load_effect_on_layer` returns on, checked first: an
        // out-of-range index (OSC, web, a stale index after the effect list
        // reloads) would otherwise drop the bindings and load nothing.
        if index >= self.effect_loader.effects.len() || layer_idx >= self.layer_stack.layers.len() {
            log::warn!("load_effect: no effect {index} or layer {layer_idx}; ignored");
            return;
        }
        let dropped = self.binding_bus.clear_preset_bindings_for_layer(layer_idx);
        if dropped > 0 {
            log::info!("Dropped {dropped} preset binding(s) targeting layer {layer_idx}");
        }
        self.load_effect_on_layer(layer_idx, index);
    }

    /// Put a catalog effect on the stack (#3124): onto a layer, replacing its
    /// effect, or as a new layer at an index. Either way the layer it lands on
    /// is selected, and — a user's pick, like [`Self::load_effect`] — starts
    /// without the preset bindings that aimed at the effect it replaced.
    /// Returns false when nothing changed: a locked layer, a full stack.
    pub fn place_effect(
        &mut self,
        effect_index: usize,
        at: crate::ui::panels::catalog_panel::CatalogDrop,
    ) -> bool {
        use crate::ui::panels::catalog_panel::CatalogDrop;
        if effect_index >= self.effect_loader.effects.len() {
            return false;
        }
        match at {
            CatalogDrop::Replace(i) => {
                if self.layer_stack.layers.get(i).is_none_or(|l| l.locked) {
                    return false;
                }
                self.layer_stack.active_layer = i;
            }
            CatalogDrop::Insert(pos) => {
                let n = self.layer_stack.layers.len();
                self.add_layer();
                if self.layer_stack.layers.len() == n {
                    return false;
                }
                // add_layer appends at the bottom and selects it; the move
                // carries the selection along.
                self.move_layer(n, pos.min(n));
                self.layer_stack.active_layer = pos.min(n);
            }
        }
        self.load_effect(effect_index);
        true
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
                backdrop: Some((
                    &self.compositor.backdrop.view,
                    &self.compositor.backdrop.sampler,
                )),
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
                backdrop: Some((
                    &self.compositor.backdrop.view,
                    &self.compositor.backdrop.sampler,
                )),
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
                // An effect loaded onto the only layer IS the output, so its
                // own post-processing becomes Master's. On a stack of several
                // it stays a suggestion the Master inspector offers (#3147):
                // an overlay on layer 5 asking for a linear tonemap must not
                // restyle the four layers beneath it.
                if self.layer_stack.layers.len() == 1 {
                    self.adopt_layer_postprocess(layer_idx);
                }
                // Grid selection is UI state, so it stays out of the shared core.
                if layer_idx == self.layer_stack.active_layer {
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
                backdrop: Some((
                    &self.compositor.backdrop.view,
                    &self.compositor.backdrop.sampler,
                )),
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

    /// The stack a launch opens with (#3126), and Clear stack returns to: an
    /// empty Layer 1 over the launch effect as Layer 2, with Layer 1
    /// selected. Expects the launch effect to be the only layer. The first effect
    /// picked in the catalog lands on top of the F, so the stack shows a
    /// blend (and the First run tour has one to explain) from the start.
    fn open_launch_stack(&mut self) {
        self.add_layer();
        let n = self.layer_stack.layers.len();
        if n < 2 {
            return;
        }
        // add_layer appends at the bottom; the empty layer goes on top.
        self.move_layer(n - 1, 0);
        for (i, layer) in self.layer_stack.layers.iter_mut().enumerate() {
            layer.name = format!("Layer {}", i + 1);
        }
        self.layer_stack.active_layer = 0;
        self.sync_active_layer();
    }

    /// Remove all layers and start again from the launch stack (Clear
    /// stack, New preset): the launch effect, and an empty Layer 1 over it.
    pub fn clear_all_layers(&mut self) {
        self.cancel_media_loads();
        self.layer_stack.layers.clear();
        self.layer_stack.active_layer = 0;
        self.add_layer();
        // Load the launch effect on the fresh layer
        if let Some(idx) = self
            .effect_loader
            .effects
            .iter()
            .position(|e| e.name == crate::effect::loader::LAUNCH_EFFECT)
        {
            self.load_effect(idx);
        }
        self.open_launch_stack();
    }

    /// Add a new media layer from a file path.
    /// Start decoding a media file for a new layer, off this thread. The
    /// layer appears when the decode finishes ([`Self::poll_media_loads`]);
    /// until then [`Self::media_loads`] says how far it has got.
    pub fn start_media_layer(&mut self, path: std::path::PathBuf) {
        let max = crate::bindings::catalog::MAX_LAYERS;
        if self.layer_stack.layers.len() + self.media_loads.len() >= max {
            log::warn!("Maximum {max} layers reached");
            return;
        }
        let progress = std::sync::Arc::new(crate::media::decoder::MediaProgress::default());
        let (tx, rx) = crossbeam_channel::bounded(1);
        let (p, job) = (progress.clone(), path.clone());
        let spawned = std::thread::Builder::new()
            .name("media-decode".into())
            .spawn(move || {
                let _ = tx.send(crate::media::decoder::load_media_with(&job, &p));
            });
        if let Err(e) = spawned {
            self.status_error = Some((format!("Could not start loading: {e}"), Instant::now()));
            return;
        }
        let file_name = path.file_name().map_or_else(
            || path.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        log::info!("Loading media: {}", path.display());
        self.media_loads.push(MediaLoad {
            path,
            file_name,
            progress,
            rx,
            started: Instant::now(),
        });
    }

    /// Add the layers whose media finished decoding. Returns how many.
    pub fn poll_media_loads(&mut self) -> usize {
        let mut added = 0;
        let mut i = 0;
        while i < self.media_loads.len() {
            match self.media_loads[i].rx.try_recv() {
                Ok(result) => {
                    let load = self.media_loads.remove(i);
                    match result {
                        Ok(source) => {
                            log::info!(
                                "Decoded {} in {:.1} s",
                                load.file_name,
                                load.started.elapsed().as_secs_f32()
                            );
                            self.add_media_layer_from_source(load.path, source);
                            added += 1;
                        }
                        Err(e) if load.progress.cancel.load(Ordering::Relaxed) => {
                            log::info!("Cancelled loading {}: {e}", load.file_name);
                        }
                        Err(e) => {
                            log::error!("Failed to load media '{}': {e}", load.path.display());
                            self.status_error = Some((e, Instant::now()));
                        }
                    }
                }
                Err(crossbeam_channel::TryRecvError::Empty) => i += 1,
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    self.media_loads.remove(i);
                }
            }
        }
        added
    }

    /// Stop decoding `index` of [`Self::media_loads`] (the UI's Cancel).
    pub fn cancel_media_load(&mut self, index: usize) {
        if index < self.media_loads.len() {
            let load = self.media_loads.remove(index);
            load.progress.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Stop every pending decode: the stack it was meant for is being
    /// replaced (a preset load, Clear stack).
    pub fn cancel_media_loads(&mut self) {
        for load in self.media_loads.drain(..) {
            load.progress.cancel.store(true, Ordering::Relaxed);
        }
    }

    fn add_media_layer_from_source(
        &mut self,
        path: std::path::PathBuf,
        source: crate::media::decoder::MediaSource,
    ) {
        let num = self.layer_stack.layers.len();
        if num >= crate::bindings::catalog::MAX_LAYERS {
            log::warn!(
                "Maximum {} layers reached",
                crate::bindings::catalog::MAX_LAYERS
            );
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
            path,
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

    /// Add a layer showing the named camera. Starts its capture if it is
    /// not already running; cameras already running keep running.
    #[cfg(feature = "webcam")]
    pub fn add_webcam_layer(&mut self, device_name: &str) {
        let num = self.layer_stack.layers.len();
        if num >= crate::bindings::catalog::MAX_LAYERS {
            log::warn!(
                "Maximum {} layers reached",
                crate::bindings::catalog::MAX_LAYERS
            );
            return;
        }

        // On macOS, ask for the camera first. While the prompt is up the
        // layer waits in `webcam_access_pending` and `update` adds it on a
        // yes, so the user does not have to add the camera a second time.
        // A network stream is not a camera to macOS.
        #[cfg(target_os = "macos")]
        if self.stream_for(device_name).is_none() {
            use crate::media::webcam::{CAMERA_DENIED, CameraAccess, request_camera_access};
            match request_camera_access() {
                CameraAccess::Granted => {}
                CameraAccess::Denied => {
                    self.status_error = Some((CAMERA_DENIED.into(), Instant::now()));
                    return;
                }
                CameraAccess::Asking(answer) => {
                    self.webcam_access_pending = Some((answer, device_name.to_string()));
                    self.status_error = Some((
                        "Allow Fosfora to use the camera when macOS asks; the camera layer \
                         appears once you do."
                            .into(),
                        Instant::now(),
                    ));
                    return;
                }
            }
        }

        let resolution = match self.ensure_webcam(device_name) {
            Ok(resolution) => resolution,
            Err(e) => {
                log::error!("Failed to start webcam: {e}");
                self.status_error = Some((format!("Webcam failed: {e}"), Instant::now()));
                return;
            }
        };

        let media_layer = self.new_webcam_media_layer(device_name, resolution);
        let name = format!("Layer {}", num + 1);
        self.layer_stack
            .layers
            .push(Layer::new_media(name, media_layer));
        self.layer_stack.active_layer = self.layer_stack.layers.len() - 1;
        self.sync_active_layer();
        log::info!("Added webcam layer: {device_name}");
    }

    /// A live media layer for the named camera, black until its first frame.
    #[cfg(feature = "webcam")]
    fn new_webcam_media_layer(&self, device_name: &str, (width, height): (u32, u32)) -> MediaLayer {
        let mut media_layer = MediaLayer::new(
            &self.gpu.device,
            &self.gpu.queue,
            GpuContext::hdr_format(),
            self.gpu.surface_config.width,
            self.gpu.surface_config.height,
            crate::media::decoder::MediaSource::Live { width, height },
            std::path::PathBuf::from(device_name),
        );
        // The whole name, where a path's last component would be taken.
        media_layer.set_live_device(device_name);
        media_layer
    }

    /// The name a listed camera goes by.
    #[cfg(feature = "webcam")]
    pub fn webcam_device_name(&self, device_index: u32) -> Option<String> {
        self.webcam_devices
            .iter()
            .find(|(idx, _)| *idx == device_index)
            .map(|(_, name)| name.clone())
    }

    /// Where a camera is in the device list, if it is connected.
    #[cfg(feature = "webcam")]
    pub fn webcam_device_index_of(&self, device_name: &str) -> Option<u32> {
        self.webcam_devices
            .iter()
            .find(|(_, name)| name == device_name)
            .map(|(idx, _)| *idx)
    }

    /// The default camera: the one last picked, or the first listed.
    #[cfg(feature = "webcam")]
    pub fn default_webcam_name(&self) -> Option<String> {
        self.webcam_device_name(self.webcam_device_index)
            .or_else(|| self.webcam_devices.first().map(|(_, name)| name.clone()))
    }

    /// Make a listed camera the default, and remember it.
    #[cfg(feature = "webcam")]
    pub fn set_default_webcam(&mut self, device_index: u32) {
        self.webcam_device_index = device_index;
        self.settings.webcam_device = Some(device_index);
        self.settings.webcam_device_name = self.webcam_device_name(device_index);
        self.settings.save();
    }

    /// The camera a new camera layer shows: the default, or once a layer
    /// already shows that one, the first camera no layer shows yet.
    #[cfg(feature = "webcam")]
    pub fn webcam_for_new_layer(&self) -> Option<String> {
        let shown: Vec<&str> = self
            .layer_stack
            .layers
            .iter()
            .filter_map(|l| l.as_media().and_then(|m| m.live_device()))
            .collect();
        let default = self.default_webcam_name()?;
        std::iter::once(&default)
            .chain(self.webcam_devices.iter().map(|(_, name)| name))
            .find(|name| !shown.contains(&name.as_str()))
            .cloned()
            .or(Some(default))
    }

    /// The running capture of a camera.
    #[cfg(feature = "webcam")]
    pub fn webcam_capture(&self, device_name: &str) -> Option<&WebcamBackend> {
        self.webcam_captures
            .iter()
            .find(|c| c.device_name() == device_name)
    }

    /// The settings of the network stream listed under this name, if the
    /// name is a stream's.
    #[cfg(feature = "webcam")]
    fn stream_for(&self, device_name: &str) -> Option<&crate::settings::RtmpStream> {
        self.webcam_stream_names
            .iter()
            .any(|name| name == device_name)
            .then(|| {
                self.settings
                    .rtmp_streams
                    .iter()
                    .find(|s| s.is_usable() && s.name == device_name)
            })
            .flatten()
    }

    /// Replace the network streams offered as cameras, and remember them.
    /// A stream on screen whose settings changed is opened again with the new
    /// ones; one that was removed or switched off stops.
    #[cfg(feature = "webcam")]
    pub fn set_rtmp_streams(&mut self, streams: Vec<crate::settings::RtmpStream>) {
        self.settings.rtmp_streams = streams;
        self.settings.save();
        self.refresh_webcam_devices();
        let current: Vec<bool> = self
            .webcam_captures
            .iter()
            .map(|c| {
                c.stream_config()
                    .is_none_or(|config| self.stream_for(&config.name) == Some(config))
            })
            .collect();
        let mut current = current.into_iter();
        self.webcam_captures
            .retain(|_| current.next().unwrap_or(true));
        self.start_rtmp_streams();
    }

    /// Open every network stream switched on in settings. They run for as
    /// long as they are switched on, shown or not, so a sender can connect
    /// before anything uses its stream and a layer picks it up live.
    #[cfg(feature = "webcam")]
    pub fn start_rtmp_streams(&mut self) {
        for name in self.webcam_stream_names.clone() {
            if let Err(e) = self.ensure_webcam(&name) {
                log::error!("Failed to open stream '{name}': {e}");
                self.status_error = Some((format!("Stream failed: {e}"), Instant::now()));
            }
        }
    }

    /// Make sure the named camera is capturing. Returns its frame size.
    #[cfg(feature = "webcam")]
    pub fn ensure_webcam(&mut self, device_name: &str) -> Result<(u32, u32), String> {
        if let Some(capture) = self.webcam_capture(device_name) {
            if capture.is_running() {
                return Ok(capture.resolution());
            }
        }
        self.webcam_captures
            .retain(|c| c.device_name() != device_name);
        let capture = if let Some(stream) = self.stream_for(device_name) {
            WebcamBackend::start_stream(stream)
        } else if self.use_ffmpeg_webcam {
            WebcamBackend::start_ffmpeg(device_name, Some((1280, 720)))
        } else {
            WebcamBackend::start_native(device_name, Some((1280, 720)))
        }?;
        let resolution = capture.resolution();
        self.webcam_captures.push(capture);
        Ok(resolution)
    }

    /// Make sure the default camera is capturing. Returns its frame size.
    #[cfg(feature = "webcam")]
    pub fn ensure_default_webcam(&mut self) -> Result<(u32, u32), String> {
        if self.default_webcam_name().is_none() {
            self.refresh_webcam_devices();
        }
        let name = self
            .default_webcam_name()
            .ok_or_else(|| "No camera found".to_string())?;
        self.ensure_webcam(&name)
    }

    /// Every layer on screen: the live stack and, during a live Dissolve,
    /// the outgoing preset's.
    #[cfg(feature = "webcam")]
    fn layers_on_screen(&self) -> impl Iterator<Item = &Layer> {
        self.layer_stack
            .layers
            .iter()
            .chain(self.retiring.iter().flat_map(|r| r.stack.layers.iter()))
    }

    /// Whether a particle source or an obstacle is fed by the default camera.
    #[cfg(feature = "webcam")]
    pub fn default_webcam_in_use(&self) -> bool {
        self.layers_on_screen().any(|l| {
            l.as_effect()
                .and_then(|e| e.pass_executor.particle_system.as_ref())
                .map_or(false, |ps| {
                    (ps.source.is_webcam() && ps.webcam_device.is_none())
                        || matches!(ps.obstacle_source.as_str(), "webcam" | "depth")
                })
        })
    }

    /// The camera a preset's particle source names, or `None` (the default
    /// camera) where that camera is not connected here.
    #[cfg(feature = "webcam")]
    pub fn particle_webcam_or_default(&self, saved: Option<&str>) -> Option<String> {
        let saved = saved?;
        if self.webcam_device_index_of(saved).is_some() {
            return Some(saved.to_string());
        }
        log::warn!(
            "Particle source wants camera '{saved}', which is not connected; using the default"
        );
        None
    }

    /// Make the camera `device` (the default one for `None`) the particle
    /// source of a layer, starting its capture if need be.
    #[cfg(feature = "webcam")]
    pub fn set_particle_webcam(
        &mut self,
        layer: usize,
        device: Option<String>,
    ) -> Result<(), String> {
        let (w, h) = match device.as_deref() {
            Some(name) => self.ensure_webcam(name),
            None => self.ensure_default_webcam(),
        }?;
        if let Some(ps) = self
            .layer_stack
            .layers
            .get_mut(layer)
            .and_then(|l| l.as_effect_mut())
            .and_then(|e| e.pass_executor.particle_system.as_mut())
        {
            ps.set_webcam_source(&self.gpu.queue, w, h);
            ps.webcam_device = device;
        }
        Ok(())
    }

    /// Every camera something on screen is fed by, and every network stream:
    /// those stay open whether shown or not.
    #[cfg(feature = "webcam")]
    fn webcams_needed(&self) -> Vec<String> {
        let particle_systems = self.layers_on_screen().filter_map(|l| {
            l.as_effect()
                .and_then(|e| e.pass_executor.particle_system.as_ref())
        });
        let mut needed: Vec<String> = self
            .layers_on_screen()
            .filter_map(|l| l.as_media().and_then(|m| m.live_device()))
            .map(str::to_string)
            .chain(
                particle_systems
                    .filter(|ps| ps.source.is_webcam())
                    .filter_map(|ps| ps.webcam_device.clone()),
            )
            .collect();
        if self.default_webcam_in_use() {
            needed.extend(self.default_webcam_name());
        }
        needed.extend(self.webcam_stream_names.iter().cloned());
        needed
    }

    /// Stop the capture of every camera nothing shows any more. Network
    /// streams are left running.
    #[cfg(feature = "webcam")]
    pub fn cleanup_webcam_if_unused(&mut self) {
        let needed = self.webcams_needed();
        self.webcam_captures.retain(|c| {
            let keep = needed.iter().any(|n| n == c.device_name());
            if !keep {
                log::info!(
                    "Camera '{}' is no longer shown, stopping capture",
                    c.device_name()
                );
            }
            keep
        });
    }

    /// Hand each camera's newest frame to the layers showing that camera
    /// and the particle sources naming it, and the default camera's to the
    /// other particle sources and to obstacles.
    #[cfg(feature = "webcam")]
    fn pump_webcams(&mut self) {
        if self.webcam_captures.is_empty() {
            return;
        }
        if let Some(dead) = self.webcam_captures.iter().position(|c| !c.is_running()) {
            let capture = self.webcam_captures.remove(dead);
            log::warn!(
                "Capture of camera '{}' died unexpectedly",
                capture.device_name()
            );
            self.status_error = Some((
                format!("Camera '{}' stopped unexpectedly", capture.device_name()),
                Instant::now(),
            ));
        }
        let default = self.default_webcam_name();
        for capture in &self.webcam_captures {
            let Some(frame) = capture.try_recv_frame() else {
                continue;
            };
            let device_name = capture.device_name();
            let is_default = default.as_deref() == Some(device_name);
            let layers = self.layer_stack.layers.iter_mut().chain(
                self.retiring
                    .iter_mut()
                    .flat_map(|r| r.stack.layers.iter_mut()),
            );
            for layer in layers {
                match layer.content {
                    LayerContent::Media(ref mut m) => {
                        if m.live_device() == Some(device_name) {
                            m.set_live_frame(
                                &self.gpu.device,
                                &self.gpu.queue,
                                frame.data.clone(),
                                frame.width,
                                frame.height,
                            );
                            m.upload_frame(&self.gpu.queue);
                        }
                    }
                    LayerContent::Effect(ref mut e) => {
                        let Some(ref mut ps) = e.pass_executor.particle_system else {
                            continue;
                        };
                        let is_source = ps
                            .webcam_device
                            .as_deref()
                            .map_or(is_default, |d| d == device_name);
                        if ps.source.is_webcam() && is_source {
                            ps.update_webcam_frame(
                                &self.gpu.queue,
                                &frame.data,
                                frame.width,
                                frame.height,
                            );
                        }
                        if !is_default {
                            continue;
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
                                depth.send_frame(frame.data.clone(), frame.width, frame.height);
                            }
                        }
                    }
                }
            }
        }
    }

    /// Refresh the webcam device list using the active backend. The default
    /// camera stays the same camera, wherever it is listed now.
    #[cfg(feature = "webcam")]
    pub fn refresh_webcam_devices(&mut self) {
        let default = self
            .webcam_device_name(self.webcam_device_index)
            .or_else(|| self.settings.webcam_device_name.clone());
        (self.webcam_devices, self.webcam_stream_names) =
            list_webcam_devices(self.use_ffmpeg_webcam, &self.settings.rtmp_streams);
        self.webcam_device_index = resolve_default_webcam(
            &self.webcam_devices,
            default.as_deref(),
            Some(self.webcam_device_index),
        );
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
                self.status_error = Some((e, Instant::now()));
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
            postprocess: &mut self.master_postprocess,
            pending_triggers: &mut self.binding_bus.pending_triggers,
        };
        crate::bindings::apply::apply_binding_target(&mut ctx, target, value, rising);
    }

    pub fn save_preset(&mut self, name: &str) {
        let mut chains = crate::trama::persist::capture(&self.layer_stack, &self.trama);
        let mut layer_chains = std::mem::take(&mut chains.layers).into_iter();
        let layer_presets: Vec<LayerPreset> = self
            .layer_stack
            .layers
            .iter()
            .map(|l| {
                let chain = layer_chains.next().flatten();
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
                #[cfg(feature = "webcam")]
                let particle_webcam_device = ps_ref
                    .filter(|ps| ps.source.is_webcam())
                    .and_then(|ps| ps.webcam_device.clone());
                #[cfg(not(feature = "webcam"))]
                let particle_webcam_device = None;
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
                    particle_webcam_device,
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
                    chain,
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
            chains.master,
        ) {
            Ok(idx) => {
                log::info!("Saved preset '{}' at index {}", name, idx);
                // Save preset-scoped bindings as sidecar
                self.binding_bus.save_preset_bindings(name);
                self.binding_bus.save_global();
                // Sidecar is now on disk — no longer an unsaved change.
                self.binding_bus.preset_scope_dirty = false;
            }
            Err(e) => {
                log::error!("Failed to save preset: {e}");
                self.status_error = Some((format!("Failed to save preset: {e}"), Instant::now()));
            }
        }
    }

    /// Switch to a preset the way a plain switch does (a click, Next/Prev
    /// Preset, the web remote): with the default transition from Settings
    /// (#217).
    pub fn switch_preset(&mut self, index: usize) {
        let style = crate::scene::switch::TransitionStyle {
            kind: self.settings.preset_transition,
            secs: self.settings.preset_transition_secs,
        };
        self.begin_switch(index, None, style);
    }

    /// Change the default preset-switch transition, and whether a Dissolve
    /// keeps the outgoing preset moving, saving them if they moved. The length
    /// is held to the cue editor's 0.1–30 s.
    pub fn set_preset_transition(
        &mut self,
        kind: Option<crate::scene::types::TransitionType>,
        secs: Option<f32>,
        keep_moving: Option<bool>,
    ) {
        let mut changed = false;
        if let Some(keep) = keep_moving.filter(|k| *k != self.settings.dissolve_keeps_moving) {
            self.settings.dissolve_keeps_moving = keep;
            changed = true;
        }
        if let Some(kind) = kind.filter(|k| *k != self.settings.preset_transition) {
            self.settings.preset_transition = kind;
            changed = true;
        }
        if let Some(secs) = secs.filter(|v| v.is_finite()).map(|v| v.clamp(0.1, 30.0)) {
            #[expect(
                clippy::float_cmp,
                reason = "change detection: any edit, however small, is stored"
            )]
            if secs != self.settings.preset_transition_secs {
                self.settings.preset_transition_secs = secs;
                changed = true;
            }
        }
        if changed {
            self.settings.save();
        }
    }

    /// The preset being switched to, or else the current one. Next/Prev
    /// Preset step from here, so a second press while a switch is still
    /// staged moves on instead of asking for the same preset again.
    pub fn target_preset(&self) -> Option<usize> {
        self.staged_switch
            .as_ref()
            .map(|s| s.preset)
            .or(self.preset_store.current_preset)
    }

    /// Load a preset with a cut: no transition, and any transition in flight
    /// or staged ends here.
    pub fn load_preset(&mut self, index: usize) {
        // A plain preset load (UI click, OSC) is not a cue: cancel any override
        // still pending from an earlier cue whose async media never finished,
        // or the stale overrides would apply to this unrelated preset.
        self.pending_cue_overrides = None;
        self.end_transitions();
        self.load_preset_inner(index);
    }

    /// Load the preset a cue points at, remembering the cue so its
    /// `param_overrides` apply once the load completes (see
    /// `pending_cue_overrides` for why application is deferred).
    pub fn load_preset_for_cue(&mut self, index: usize, cue_index: usize) {
        self.end_transitions();
        self.pending_cue_overrides = Some(cue_index);
        self.load_preset_inner(index);
    }

    /// Drop the staged switch and the transition in flight, if any.
    fn end_transitions(&mut self) {
        self.staged_switch = None;
        self.active_transition = None;
        self.retiring = None;
    }

    /// Start switching to a preset with a transition. A Cut loads at once;
    /// anything else is staged (`scene::switch`) and applies once its media
    /// has decoded and the frame on screen has been captured.
    fn begin_switch(
        &mut self,
        index: usize,
        cue: Option<usize>,
        style: crate::scene::switch::TransitionStyle,
    ) {
        if style.is_cut() {
            match cue {
                Some(cue_index) => self.load_preset_for_cue(index, cue_index),
                None => self.load_preset(index),
            }
            return;
        }
        let Some(preset) = self.preset_store.load(index).cloned() else {
            return;
        };
        self.cancel_media_loads();
        // A decode an earlier switch started is for a preset no longer wanted.
        self.preset_loader.cancel();
        self.pending_cue_overrides = None;
        let media_jobs = self.preset_media_jobs(&preset);
        let decoding = !media_jobs.is_empty();
        if decoding {
            let name = self.preset_name(index);
            log::info!(
                "Preset '{}' has {} media layer(s), decoding before its transition",
                name,
                media_jobs.len()
            );
            self.preset_loader
                .request_load(index, preset, media_jobs, name);
        }
        // Targets now, so the capture in render() needs only shared access.
        let (w, h) = (
            self.gpu.surface_config.width,
            self.gpu.surface_config.height,
        );
        let tr = self.transition_renderer.get_or_insert_with(|| {
            TransitionRenderer::new(&self.gpu.device, GpuContext::hdr_format())
        });
        tr.ensure_targets(&self.gpu.device, w, h, GpuContext::hdr_format());
        self.staged_switch = Some(crate::scene::switch::StagedSwitch::new(
            index, cue, style, decoding,
        ));
    }

    /// Apply a staged switch and start its transition. `captured` says the
    /// snapshot holds the frame shown just before, so a dissolve may start
    /// from it.
    fn apply_staged_switch(&mut self, staged: crate::scene::switch::StagedSwitch, captured: bool) {
        use crate::scene::cueing::MorphSnapshot;
        use crate::scene::switch::{ActiveTransition, dissolves_frame, keep_changed_layers_still};
        use crate::scene::types::TransitionType;

        // A live fade already running is superseded: this switch fades from
        // the still captured of it (`Outgoing::fade_running_live`).
        let fade_running_live = self.retiring.take().is_some();
        let live = crate::scene::switch::keeps_outgoing_moving(
            staged.style.kind,
            self.settings.dissolve_keeps_moving,
            crate::scene::switch::Outgoing {
                has_chains: self.trama.master_live()
                    || self.layer_stack.layers.iter().any(|l| l.chain.is_some()),
                has_locked: self.layer_stack.layers.iter().any(|l| l.locked),
                fade_running_live,
            },
        );
        if live {
            self.retire_live_stack();
        }

        let before = self.layer_keys();
        let mut from = MorphSnapshot::capture(
            self.layer_stack
                .layers
                .iter()
                .map(|l| (&l.param_store.values, l.opacity)),
        );
        let (preset, decoded_media) = match staged.decoded {
            Some(result) => (result.preset, result.decoded_media),
            None => match self.preset_store.load(staged.preset) {
                Some(p) => (p.clone(), std::collections::HashMap::new()),
                None => return,
            },
        };
        let name = self.preset_name(staged.preset);
        self.load_preset_scope_bindings(&name, &preset);
        // Applied inside the load, before the `to` snapshot below, so a morph
        // lands ON the cue's overridden values rather than the preset's.
        self.pending_cue_overrides = staged.cue;
        self.apply_preset_immediately(staged.preset, &preset, decoded_media);

        let after = self.layer_keys();
        let kind = staged.style.kind;
        let morph = (kind == TransitionType::ParamMorph).then(|| {
            keep_changed_layers_still(&mut from, &before, &after);
            let to = MorphSnapshot::capture(
                self.layer_stack
                    .layers
                    .iter()
                    .map(|l| (&l.param_store.values, l.opacity)),
            );
            (from, to)
        });
        let dissolve_frame = live || (captured && dissolves_frame(kind, &before, &after));
        log::info!(
            "Preset switch: {} over {:.1} s (frame dissolve: {}, outgoing: {}, param morph: {}{})",
            kind,
            staged.style.secs,
            dissolve_frame,
            if live { "moving" } else { "still" },
            morph.is_some(),
            if captured || live {
                ""
            } else {
                ", no frame was captured"
            },
        );
        self.active_transition = Some(ActiveTransition::new(
            staged.style.secs,
            dissolve_frame,
            morph,
        ));
    }

    /// Move the live layers aside to keep animating through a Dissolve, and
    /// leave an empty stack for the incoming preset to build fresh layers in
    /// (a reused layer could not be in both pictures).
    fn retire_live_stack(&mut self) {
        let (w, h) = (
            self.gpu.surface_config.width,
            self.gpu.surface_config.height,
        );
        let compositor = self.retire_compositor.get_or_insert_with(|| {
            Compositor::new(&self.gpu.device, GpuContext::hdr_format(), w, h)
        });
        let mut stack = std::mem::replace(&mut self.layer_stack, LayerStack::new());
        // Their executors sample the live compositor's @backdrop; the
        // outgoing picture composites through its own.
        for layer in &mut stack.layers {
            if let Some(e) = layer.as_effect_mut() {
                e.pass_executor.set_backdrop(
                    Some((
                        compositor.backdrop.view.clone(),
                        compositor.backdrop.sampler.clone(),
                    )),
                    &self.gpu.device,
                    &e.uniform_buffer,
                    &self.placeholder,
                    &self.audio_textures,
                );
            }
        }
        self.retiring = Some(RetiringStack {
            stack,
            volumetric_enabled: self.volumetric_enabled,
            volumetric_params: self.volumetric_params,
        });
    }

    /// What each layer shows, for deciding whether a Morph must also dissolve.
    fn layer_keys(&self) -> Vec<crate::scene::switch::LayerKey> {
        use crate::scene::switch::{ContentKey, LayerKey};
        self.layer_stack
            .layers
            .iter()
            .map(|l| LayerKey {
                content: match &l.content {
                    LayerContent::Effect(_) => ContentKey::Effect(l.effect_index()),
                    LayerContent::Media(m) => {
                        ContentKey::Media(m.file_path.clone(), m.current_frame)
                    }
                },
                blend: l.blend_mode,
                chain: l.chain.as_deref().map(|c| {
                    (
                        c.graph.nodes().iter().map(|n| n.kind.clone()).collect(),
                        c.graph.wires().len(),
                    )
                }),
            })
            .collect()
    }

    fn preset_name(&self, index: usize) -> String {
        self.preset_store
            .presets
            .get(index)
            .map(|(n, _)| n.clone())
            .unwrap_or_default()
    }

    /// Load the preset's own bindings, upgrading legacy targets.
    fn load_preset_scope_bindings(&mut self, name: &str, preset: &crate::preset::Preset) {
        self.binding_bus.load_preset_bindings(name);
        // Freshly loaded bindings match disk — clear any stale unsaved flag.
        self.binding_bus.preset_scope_dirty = false;
        crate::bindings::apply::upgrade_legacy_targets(&mut self.binding_bus, preset);
    }

    /// Media layers that need decoding (skip locked, skip missing files).
    fn preset_media_jobs(
        &self,
        preset: &crate::preset::Preset,
    ) -> Vec<(usize, std::path::PathBuf)> {
        let mut media_jobs = Vec::new();
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
        media_jobs
    }

    fn load_preset_inner(&mut self, index: usize) {
        self.cancel_media_loads();
        // A decode still running for an earlier load must not land on top of
        // this one. (The async path below starts a new generation anyway.)
        self.preset_loader.cancel();
        let preset = match self.preset_store.load(index) {
            Some(p) => p.clone(),
            None => return,
        };

        let preset_name = self.preset_name(index);

        // Load preset-scoped bindings and migrate old 3-part targets to 4-part format
        self.load_preset_scope_bindings(&preset_name, &preset);

        let media_jobs = self.preset_media_jobs(&preset);

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
        // A camera the preset names may have been connected (or a virtual
        // camera started) since the devices were last listed.
        #[cfg(feature = "webcam")]
        if preset.layers.iter().any(|lp| {
            [&lp.webcam_device, &lp.particle_webcam_device]
                .into_iter()
                .flatten()
                .any(|name| self.webcam_device_index_of(name).is_none())
        }) {
            self.refresh_webcam_devices();
        }

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

        // A layer locked NOW is skipped below and keeps everything it has, its
        // trama chain included. Taken before the loop: a layer that is loaded
        // takes its `locked` flag from the preset, and must not be mistaken
        // afterwards for one that was skipped.
        let kept_layers: Vec<bool> = self.layer_stack.layers.iter().map(|l| l.locked).collect();

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
            if let Some(saved) = lp.webcam_device.as_deref() {
                // The camera the preset names; the default one where that
                // camera is not connected here.
                let device_name = if self.webcam_device_index_of(saved).is_some() {
                    saved.to_string()
                } else {
                    let fallback = self.default_webcam_name();
                    log::warn!(
                        "Preset layer {i} wants camera '{saved}', which is not connected; \
                         using {fallback:?}"
                    );
                    fallback.unwrap_or_else(|| saved.to_string())
                };
                // Each camera has its own capture, so layers naming
                // different cameras each get their own picture.
                let resolution = match self.ensure_webcam(&device_name) {
                    Ok(resolution) => resolution,
                    Err(e) => {
                        log::error!("Failed to start webcam for preset layer {i}: {e}");
                        self.status_error = Some((format!("Webcam failed: {e}"), Instant::now()));
                        // The layer is still the camera's, dark until it is back.
                        (1280, 720)
                    }
                };
                // A layer already showing this camera keeps its picture
                // instead of going black until the next frame.
                let already_showing = self.layer_stack.layers[i]
                    .as_media()
                    .and_then(|m| m.live_device())
                    == Some(device_name.as_str());
                if !already_showing {
                    let media_layer = self.new_webcam_media_layer(&device_name, resolution);
                    self.layer_stack.layers[i].content = LayerContent::Media(Box::new(media_layer));
                }
                self.layer_stack.layers[i].param_store = ParamStore::new();
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
                                    match crate::media::video::decode_all_frames(
                                        &path,
                                        &meta,
                                        &Default::default(),
                                    ) {
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
                        // The camera the preset names, started if it is
                        // not already running.
                        let device =
                            self.particle_webcam_or_default(lp.particle_webcam_device.as_deref());
                        match self.set_particle_webcam(i, device) {
                            Ok(()) => log::info!("Restored particle webcam source for layer {i}"),
                            Err(e) => {
                                log::error!("Failed to start webcam for particle source: {e}");
                                self.status_error =
                                    Some((format!("Webcam failed: {e}"), Instant::now()));
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
                    if let Err(e) = self.ensure_default_webcam() {
                        log::error!("Failed to start webcam for depth obstacle restore: {e}");
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
        self.master_postprocess = preset.postprocess.clone();
        self.master_postprocess_previous = None;
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
        // trama chains: REPLACE, for the same reason as volumetric above — a
        // preset saved without a chain must not inherit the last preset's.
        let layer_chains: Vec<Option<crate::trama::ser::ChainDoc>> =
            preset.layers.iter().map(|lp| lp.chain.clone()).collect();
        let notes = crate::trama::persist::apply(
            &mut self.layer_stack,
            &mut self.trama,
            &layer_chains,
            preset.master_chain.as_ref(),
            |i| kept_layers.get(i).copied().unwrap_or(false),
        );
        for note in &notes {
            log::warn!("trama: preset load: {note}");
        }
        if !notes.is_empty() {
            self.trama.canvas.status = Some(format!(
                "{} thing(s) repaired while loading chains — see the log",
                notes.len()
            ));
        }
        self.preset_store.current_preset = Some(index);
        self.preset_store.dirty = false;
        // Reset param changed flags so loading doesn't immediately mark dirty
        for layer in &mut self.layer_stack.layers {
            layer.param_store.changed = false;
        }
        // If this load came from a cue, apply the cue's param_overrides on top
        // of the preset values — this is the one funnel every load path exits
        // through, so sync, async-media, and staged-switch loads all get
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
        // Cameras the previous preset showed and this one does not.
        #[cfg(feature = "webcam")]
        self.cleanup_webcam_if_unused();
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

    /// The master chain's badge for the layer panel — `None` while the master
    /// chain holds nothing, by the same rule as a layer row's. Without it a
    /// master patch left running post-processes the whole output with nothing
    /// in the main window to say so: its tab only shows with the canvas open.
    pub fn master_chain_badge(&self) -> Option<crate::gpu::layer::ChainBadge> {
        crate::gpu::layer::ChainBadge::of(&self.trama.master)
    }

    /// The display target: the finished frame, in the surface's own format so
    /// the blit to the swapchain is a straight copy — and viewable without its
    /// `-srgb` suffix, for [`Self::display_view_for_egui`].
    fn new_display(
        device: &wgpu::Device,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
    ) -> RenderTarget {
        let plain = format.remove_srgb_suffix();
        let extra: &[wgpu::TextureFormat] = if plain == format { &[] } else { &[plain] };
        RenderTarget::new_with_view_formats(device, width, height, format, 1.0, "display", extra)
    }

    /// The finished frame as egui must sample it: WITHOUT the `-srgb`
    /// decode. egui-wgpu treats every sampled texture as gamma-encoded and
    /// converts it to linear itself (`fs_main_linear_framebuffer`), so an
    /// `-srgb` view is decoded twice — the output preview read darker and
    /// more contrasty than the real output, much like the tonemap. The same
    /// trap the trama previews document.
    pub fn display_view_for_egui(&self) -> wgpu::TextureView {
        self.display
            .view_as(self.display.format.remove_srgb_suffix())
    }

    /// Master post-processing, as a preset saves it.
    pub fn current_postprocess(&self) -> PostProcessDef {
        self.master_postprocess.clone()
    }

    /// Make layer `idx`'s effect's own post-processing Master's.
    pub fn adopt_layer_postprocess(&mut self, idx: usize) {
        if let Some(layer) = self.layer_stack.layers.get(idx) {
            self.replace_master_postprocess(layer.postprocess.clone());
        }
    }

    /// Replace Master's post-processing, keeping the settings it replaces as
    /// the inspector's "Previous settings".
    pub fn replace_master_postprocess(&mut self, pp: PostProcessDef) {
        replace_keeping_previous(
            &mut self.master_postprocess,
            &mut self.master_postprocess_previous,
            pp,
        );
        self.post_process.enabled = self.master_postprocess.enabled;
    }

    /// Go back to the settings the last replacement replaced. That keeps the
    /// ones it leaves, so pressing it again comes back.
    pub fn restore_previous_postprocess(&mut self) {
        if let Some(prev) = self.master_postprocess_previous.clone() {
            self.replace_master_postprocess(prev);
        }
    }

    /// Load a scene and start its timeline.
    pub fn load_scene(&mut self, index: usize) {
        if !self.open_scene(index) {
            return;
        }
        // Start at cue 0
        let event = self.timeline.start(0);
        self.process_timeline_event(event);
    }

    /// Make scene `index` the one being edited, without playing it: the
    /// workspace opens a scene with a click and plays it with a double-click
    /// (#3173). Returns false when there is no such scene.
    pub fn open_scene(&mut self, index: usize) -> bool {
        let Some(scene) = self.scene_store.load(index).cloned() else {
            return false;
        };
        self.scene_store.current_scene = Some(index);
        self.timeline = Timeline::new(scene.cues.clone(), scene.loop_mode, scene.advance_mode);
        log::info!(
            "Opened scene '{}' with {} cues",
            scene.name,
            scene.cues.len()
        );
        true
    }

    /// A new cue for `preset_name`, as the cue list adds one: a cut, and in
    /// Timer mode a hold so the timer can advance.
    pub fn new_cue(&self, preset_name: String) -> crate::scene::types::SceneCue {
        let hold_secs = matches!(
            self.timeline.advance_mode,
            crate::scene::types::AdvanceMode::Timer
        )
        .then_some(4.0);
        crate::scene::types::SceneCue {
            preset_name,
            transition: crate::scene::types::TransitionType::Cut,
            transition_secs: 1.0,
            hold_secs,
            label: None,
            param_overrides: Vec::new(),
            transition_beats: None,
        }
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
                duration,
            } => {
                let preset_idx = self.timeline.cues.get(to_cue).and_then(|cue| {
                    self.preset_store
                        .presets
                        .iter()
                        .position(|(name, _)| name == &cue.preset_name)
                });
                match preset_idx {
                    Some(idx) => self.begin_switch(
                        idx,
                        Some(to_cue),
                        crate::scene::switch::TransitionStyle {
                            kind: transition_type,
                            secs: duration,
                        },
                    ),
                    None => log::warn!("Preset not found for cue {}", to_cue),
                }
            }
            // The transition runs on `active_transition`'s clock: the morph
            // in update(), the crossfade in render().
            TimelineEvent::TransitionProgress { .. } | TimelineEvent::TransitionComplete { .. } => {
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
            .map(|c| {
                let preset = self
                    .preset_store
                    .presets
                    .iter()
                    .find(|(n, _)| *n == c.preset_name)
                    .map(|(_, p)| p);
                crate::ui::panels::scene_panel::CueDisplayInfo {
                    preset_name: c.display_name().to_string(),
                    transition: c.transition,
                    transition_secs: c.transition_secs,
                    hold_secs: c.hold_secs,
                    label: c.label.clone(),
                    effect: preset.and_then(|p| {
                        p.layers
                            .iter()
                            .find(|l| l.media_path.is_none() && l.webcam_device.is_none())
                            .map(|l| l.effect_name.clone())
                    }),
                    layers: preset.map_or(0, |p| p.layers.len()),
                }
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

    /// The frame's output-alpha mode; both render branches and the capture path
    /// (shared post-params buffer) see this one resolution.
    fn resolve_output_alpha(&self) -> crate::gpu::postprocess::AlphaMode {
        crate::gpu::frame_graph::resolve_output_alpha(
            self.settings.output_alpha,
            &self.layer_stack,
            &self.effect_loader.effects,
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
        )
    }

    /// Put the finished frame on the window — or don't.
    ///
    /// The workspace shell shows the output in a preview, and blitting it
    /// full-window as well left the render glowing through every panel's
    /// translucent fill. So while the interface shows, the window gets the
    /// interface's own ground, and the composite reaches the eye only through
    /// the preview.
    ///
    /// Hiding the interface (D) means full output.
    ///
    /// A free function rather than a method: the frame holds
    /// `&mut self.compositor` across this point, and `&self` collides with it.
    /// These arguments are disjoint fields, which borrowck accepts.
    #[allow(clippy::too_many_arguments)]
    fn present_display(
        device: &wgpu::Device,
        post_process: &PostProcessChain,
        display: &RenderTarget,
        encoder: &mut wgpu::CommandEncoder,
        surface_view: &wgpu::TextureView,
        output_fills_window: bool,
        ground: egui::Color32,
    ) {
        if output_fills_window {
            post_process.blit_target(device, encoder, display, surface_view);
            return;
        }
        let c = ground;
        // The surface is sRGB; a clear color is given in linear space.
        let lin = |v: u8| {
            let s = v as f64 / 255.0;
            if s <= 0.04045 {
                s / 12.92
            } else {
                ((s + 0.055) / 1.055).powf(2.4)
            }
        };
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("shell-ground"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: surface_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: lin(c.r()),
                        g: lin(c.g()),
                        b: lin(c.b()),
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
    }

    pub fn render(&mut self) -> Result<(), wgpu::SurfaceError> {
        // A lost device can't render anything. The caller checks `gpu.is_device_lost()` before
        // treating `Lost` as a surface loss (a resize would rebuild textures on the dead device).
        if self.gpu.is_device_lost() {
            return Err(wgpu::SurfaceError::Lost);
        }
        self.post_process.flash_budget = self
            .settings
            .flash_limit
            .flashes_per_second(self.reduce_motion);

        let output = self.gpu.surface.get_current_texture()?;
        let surface_view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("fosfora-encoder"),
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

        // Resolved before execute_and_composite: `source` borrows the compositor
        // for the rest of the frame, and the resolver reads &self.
        let alpha_mode = self.resolve_output_alpha();

        // The second output window's frame, taken here for the same reason:
        // `acquire` needs `&mut self.output_window` and the compositor borrow
        // below outlives every point where the blit is actually encoded. `None`
        // means no window is open, or its surface skipped this frame.
        let output_frame = match self.output_window {
            Some(ref mut ow) => ow.acquire(&self.gpu.device),
            None => None,
        };

        // Previews while patching in Layers mode: `execute_and_composite`
        // only runs the trama executor in Trama mode, which would leave the
        // canvas thumbnails frozen while building a patch before switching
        // over. Opt-in by the open canvas; the output target goes unused.
        // Cfg-free profiler handle for the render paths (no-op without the
        // `profiling` feature). Built from the field so `&mut self.trama`
        // below stays a disjoint borrow.
        #[cfg(feature = "profiling")]
        let profiler = crate::gpu::profiler::ProfilerHandle::some(&self.gpu_profiler.inner);
        #[cfg(not(feature = "profiling"))]
        let profiler = crate::gpu::profiler::ProfilerHandle::none();

        // Match the resident chain output targets to the chains that exist,
        // and let the executor forget any chain whose layer went away. Driven
        // off the layer stack rather than from remove_layer/move_layer,
        // because several sites mutate `layers` directly. Free when nothing
        // moved (I8). There is no preview-only execute any more: every live
        // chain runs as part of the frame.
        let master_live = self.trama.master_live();
        let (targets, trama) = (&mut self.chain_targets, &mut self.trama);
        targets.sync(&self.gpu.device, &self.layer_stack, master_live, |chain| {
            trama.drop_chain(chain);
        });

        // Layer rows draw with pictures; a hidden interface pays nothing for
        // them.
        let tap_thumbs = self.egui_overlay.visible;

        // Compute the HDR source from layer execution + compositing — shared
        // with the headless renderer.
        let postprocess = self.master_postprocess.clone();
        let source = crate::gpu::frame_graph::execute_and_composite(
            &self.layer_stack,
            &mut self.compositor,
            Some(&mut self.trama),
            &self.chain_targets,
            tap_thumbs.then_some(&self.layer_thumbs),
            &self.gpu.device,
            &self.gpu.queue,
            &mut encoder,
            profiler,
        );

        // Dissolve crossfade while a switch that dissolves is in flight: from
        // the outgoing preset still animating ("Keep moving"), or else from
        // the still captured of it. The outgoing stack composites through its
        // own compositor with no chains (it has none), no layer pictures (they
        // are keyed by live layer index) and no profiler scopes.
        let source = match (&self.active_transition, &self.transition_renderer) {
            (Some(t), Some(tr)) if t.dissolve_frame => {
                let outgoing = match (&self.retiring, self.retire_compositor.as_mut()) {
                    (Some(r), Some(compositor)) => {
                        Some(crate::gpu::frame_graph::execute_and_composite(
                            &r.stack,
                            compositor,
                            None,
                            &self.chain_targets,
                            None,
                            &self.gpu.device,
                            &self.gpu.queue,
                            &mut encoder,
                            crate::gpu::profiler::ProfilerHandle::none(),
                        ))
                    }
                    _ => None,
                };
                match outgoing {
                    Some(outgoing) => tr.blend(
                        &self.gpu.device,
                        &self.gpu.queue,
                        &mut encoder,
                        outgoing,
                        source,
                        t.progress(),
                    ),
                    None => tr.crossfade(
                        &self.gpu.device,
                        &self.gpu.queue,
                        &mut encoder,
                        source,
                        t.progress(),
                    ),
                }
                .unwrap_or(source)
            }
            _ => source,
        };
        // A staged switch captures the frame shown now, crossfade included,
        // so switching again mid-transition starts from what is on screen.
        if let (Some(staged), Some(tr)) = (self.staged_switch.as_mut(), &self.transition_renderer) {
            if staged.wants_capture() && tr.capture_snapshot(&self.gpu.device, &mut encoder, source)
            {
                staged.captured();
            }
        }

        // Post-process → display target, then blit that to the window (#3122).
        // Both are scoped so the profiler reports the indirection's own cost:
        // `display-blit` is exactly what the off-screen target added.
        {
            #[cfg(feature = "profiling")]
            let profiler = crate::gpu::profiler::ProfilerHandle::some(&self.gpu_profiler.inner);
            #[cfg(not(feature = "profiling"))]
            let profiler = crate::gpu::profiler::ProfilerHandle::none();
            let mut scope = profiler.scope("post", &mut encoder);
            self.post_process.render(
                &self.gpu.device,
                &self.gpu.queue,
                scope.encoder(),
                source,
                &self.display.view,
                self.uniforms.time,
                self.uniforms.rms,
                self.uniforms.onset,
                self.uniforms.flatness,
                &postprocess,
                alpha_mode,
            );
        }
        {
            #[cfg(feature = "profiling")]
            let profiler = crate::gpu::profiler::ProfilerHandle::some(&self.gpu_profiler.inner);
            #[cfg(not(feature = "profiling"))]
            let profiler = crate::gpu::profiler::ProfilerHandle::none();
            let mut scope = profiler.scope("display-blit", &mut encoder);
            Self::present_display(
                &self.gpu.device,
                &self.post_process,
                &self.display,
                scope.encoder(),
                &surface_view,
                !self.egui_overlay.visible,
                self.egui_overlay.palette.bg,
            );
            // Master's row picture, from the finished frame (#3123).
            if tap_thumbs {
                self.layer_thumbs.tap(
                    &self.gpu.device,
                    scope.encoder(),
                    &self.display.view,
                    crate::gpu::layer_thumbs::ThumbKind::Master,
                    0,
                );
            }
            // Second output window: the same finished frame, full-window, with
            // no interface over it (#3122). Inside the `display-blit` scope so
            // the profiler counts what the second present costs.
            if let Some((ref frame, ref view)) = output_frame {
                self.post_process.blit_target_letterboxed(
                    &self.gpu.device,
                    scope.encoder(),
                    &self.display,
                    view,
                    frame.texture.width(),
                    frame.texture.height(),
                );
            }
        }

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

        // Spout capture
        #[cfg(all(target_os = "windows", feature = "spout"))]
        if self.spout.is_running() {
            self.spout
                .capture_frame(&self.gpu.device, &mut encoder, &self.post_process, source);
        }

        // Syphon capture
        #[cfg(all(target_os = "macos", feature = "syphon"))]
        if self.syphon.is_running() {
            self.syphon
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

        // Flip ping-pong for all layers, the outgoing preset's included
        for layer in &mut self.layer_stack.layers {
            layer.flip();
        }
        if let Some(r) = self.retiring.as_mut() {
            for layer in &mut r.stack.layers {
                layer.flip();
            }
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
        // frame). The lattice request was once issued ONLY on a since-removed
        // dissolve-transition path, so on every normal frame the population map was
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
        if let Err(e) = self.gpu.device.poll(wgpu::PollType::Poll) {
            log::warn!("GPU poll failed: {e}");
        }

        // NDI: request async map on staging buffer (must be after queue.submit)
        #[cfg(feature = "ndi")]
        if self.ndi.is_running() {
            self.ndi.post_submit();
        }

        #[cfg(all(target_os = "linux", feature = "v4l2"))]
        if self.v4l2.is_running() {
            self.v4l2.post_submit();
        }

        #[cfg(all(target_os = "windows", feature = "spout"))]
        if self.spout.is_running() {
            self.spout.post_submit();
        }

        #[cfg(all(target_os = "macos", feature = "syphon"))]
        if self.syphon.is_running() {
            self.syphon.post_submit();
        }

        if self.recording.is_recording() {
            self.recording.post_submit();
        }

        output.present();
        if let Some((frame, _)) = output_frame {
            frame.present();
        }

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

/// A media file decoding for a new layer on its own thread.
pub struct MediaLoad {
    pub path: std::path::PathBuf,
    pub file_name: String,
    pub progress: std::sync::Arc<crate::media::decoder::MediaProgress>,
    rx: crossbeam_channel::Receiver<Result<crate::media::decoder::MediaSource, String>>,
    pub started: Instant,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A saved camera index that names no camera (one was unplugged, or a
    /// virtual camera quit) made every "+ Webcam" fail.
    #[cfg(feature = "webcam")]
    #[test]
    fn default_webcam_survives_renumbering() {
        let devices = vec![(0, "Virtual".to_string()), (1, "FaceTime".to_string())];
        // By name, wherever it is listed now.
        assert_eq!(
            resolve_default_webcam(&devices, Some("FaceTime"), Some(0)),
            1
        );
        // A stale index falls back to a camera that exists.
        assert_eq!(resolve_default_webcam(&devices, None, Some(2)), 0);
        assert_eq!(resolve_default_webcam(&devices, Some("Gone"), Some(1)), 1);
        assert_eq!(resolve_default_webcam(&[], Some("Gone"), Some(2)), 0);
    }
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

/// `*current = new`, with the old value kept in `previous`. Replacing a value
/// with itself keeps the older `previous`: a reset that changed nothing must
/// not lose the way back.
fn replace_keeping_previous<T: PartialEq>(current: &mut T, previous: &mut Option<T>, new: T) {
    if *current != new {
        *previous = Some(std::mem::replace(current, new));
    }
}

#[cfg(test)]
mod postprocess_reset_tests {
    use super::replace_keeping_previous;

    #[test]
    fn a_replacement_keeps_the_way_back_and_going_back_swaps() {
        let (mut cur, mut prev) = ("mine", None);
        replace_keeping_previous(&mut cur, &mut prev, "sumi");
        assert_eq!((cur, prev), ("sumi", Some("mine")));
        // "Previous settings" is a replacement with the previous value.
        let back = prev.unwrap();
        replace_keeping_previous(&mut cur, &mut prev, back);
        assert_eq!((cur, prev), ("mine", Some("sumi")));
    }

    #[test]
    fn a_replacement_that_changes_nothing_keeps_the_older_way_back() {
        let (mut cur, mut prev) = ("sumi", Some("mine"));
        replace_keeping_previous(&mut cur, &mut prev, "sumi");
        assert_eq!((cur, prev), ("sumi", Some("mine")));
    }
}
