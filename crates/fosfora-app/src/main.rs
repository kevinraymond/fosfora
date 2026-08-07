#[cfg(feature = "analyze")]
mod analyze;
mod app;
mod audio;
mod bindings;
#[cfg(feature = "depth")]
mod depth;
mod download;
mod effect;
mod gpu;
mod headless;
#[cfg(feature = "link")]
mod link;
mod media;
mod midi;
#[cfg(feature = "ndi")]
mod ndi;
mod osc;
#[cfg(any(
    feature = "ndi",
    all(target_os = "linux", feature = "v4l2"),
    all(target_os = "windows", feature = "spout"),
    all(target_os = "macos", feature = "syphon")
))]
mod output;
mod params;
mod paths;
mod preset;
mod recording;
mod scene;
mod settings;
mod shader;
mod signal;
#[cfg(all(target_os = "windows", feature = "spout"))]
mod spout;
#[cfg(all(target_os = "macos", feature = "syphon"))]
mod syphon;
mod trama;
mod ui;
#[cfg(all(target_os = "linux", feature = "v4l2"))]
mod v4l2;
mod web;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use crossbeam_channel::Receiver;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Fullscreen, Icon, Window, WindowAttributes, WindowId};

use app::App;
use effect::loader::EffectLoader;
use gpu::layer::BlendMode;

struct FosforaApp {
    app: Option<App>,
    window: Option<Arc<Window>>,
    file_dialog_rx: Option<Receiver<PathBuf>>,
    obstacle_dialog_rx: Option<Receiver<PathBuf>>,
    /// Debounced param save: (effect_index, last_change_time)
    param_save_pending: Option<(usize, std::time::Instant)>,
}

impl FosforaApp {
    fn new() -> Self {
        Self {
            app: None,
            window: None,
            file_dialog_rx: None,
            obstacle_dialog_rx: None,
            param_save_pending: None,
        }
    }
}

impl ApplicationHandler for FosforaApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let mut attrs = WindowAttributes::default()
            .with_title("Fosfora")
            .with_inner_size(winit::dpi::LogicalSize::new(1920, 1080));

        // Center window on primary monitor via initial position hint.
        // On Wayland, set_outer_position is a no-op and compositors handle placement,
        // so we set position on WindowAttributes which winit can pass as a hint.
        if let Some(monitor) = event_loop
            .primary_monitor()
            .or_else(|| event_loop.available_monitors().next())
        {
            let monitor_size = monitor.size();
            let monitor_pos = monitor.position();
            let scale = monitor.scale_factor();
            let win_w = (1920.0 * scale) as u32;
            let win_h = (1080.0 * scale) as u32;
            let x = (monitor_size.width.saturating_sub(win_w)) / 2;
            let y = (monitor_size.height.saturating_sub(win_h)) / 2;
            attrs = attrs.with_position(winit::dpi::PhysicalPosition::new(
                monitor_pos.x + x as i32,
                monitor_pos.y + y as i32,
            ));
        }

        if let Some(icon) = load_window_icon() {
            attrs = attrs.with_window_icon(Some(icon));
        }

        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .expect("Failed to create window"),
        );

        self.window = Some(window.clone());

        match App::new(window) {
            Ok(app) => {
                self.app = Some(app);
                log::info!("Fosfora initialized");
            }
            Err(e) => {
                log::error!("Failed to initialize app: {e}");
                event_loop.exit();
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(app) = self.app.as_mut() else {
            return;
        };

        // Let egui handle events first
        let egui_consumed = app.egui_overlay.handle_event(&app.window, &event);

        // Diagnostic (FOSFORA_FRAME_LOG=1): what the window system actually
        // delivers. The per-frame FRAMELOG shows the clocks and the audio, but if
        // the picture reacts to a click, the cause may be an event we do not even
        // handle — and guessing at that from the outside has already been wrong
        // several times. High-frequency events are excluded so the sequence around
        // a click stays readable.
        if app.frame_log
            && !matches!(
                event,
                WindowEvent::RedrawRequested
                    | WindowEvent::CursorMoved { .. }
                    | WindowEvent::AxisMotion { .. }
            )
        {
            log::info!("EVENTLOG {event:?}");
        }

        match event {
            WindowEvent::CloseRequested => {
                app.quit_requested = true;
            }
            WindowEvent::Resized(size) => {
                log::info!(
                    "RESIZELOG {}x{} (was {}x{}) same={}",
                    size.width,
                    size.height,
                    app.gpu.surface_config.width,
                    app.gpu.surface_config.height,
                    size.width == app.gpu.surface_config.width
                        && size.height == app.gpu.surface_config.height
                );
                app.resize(size.width, size.height);
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(key),
                        state: ElementState::Pressed,
                        ..
                    },
                ..
            } if !egui_consumed || !app.egui_overlay.wants_keyboard() => {
                match key {
                    KeyCode::Escape => {
                        // Cancel a half-finished click-to-bind, then close the
                        // binding matrix, then the shader editor, then quit.
                        if app.binding_matrix.open && app.binding_matrix.armed.is_some() {
                            app.binding_matrix.armed = None;
                        } else if app.binding_matrix.open {
                            app.binding_matrix.open = false;
                        } else if !app.shader_editor.open {
                            app.quit_requested = true;
                        }
                    }
                    KeyCode::KeyF => {
                        let window = &app.window;
                        if window.fullscreen().is_some() {
                            window.set_fullscreen(None);
                        } else {
                            window.set_fullscreen(Some(Fullscreen::Borderless(None)));
                        }
                    }
                    KeyCode::KeyD => {
                        app.egui_overlay.toggle_visible();
                    }
                    KeyCode::Space
                        // Scene: go to next cue (when timeline has cues)
                        if !app.timeline.cues.is_empty() => {
                            app.egui_overlay.context().data_mut(|d| {
                                d.insert_temp(egui::Id::new("scene_go_next"), true);
                            });
                        }
                    KeyCode::KeyT
                        // Toggle timeline active (when cues loaded)
                        if !app.timeline.cues.is_empty() => {
                            app.egui_overlay.context().data_mut(|d| {
                                d.insert_temp(egui::Id::new("scene_toggle_play"), true);
                            });
                        }
                    KeyCode::KeyB
                        if !app.shader_editor.open => {
                            app.binding_matrix.open = !app.binding_matrix.open;
                        }
                    KeyCode::BracketLeft => {
                        // Previous layer
                        let num = app.layer_stack.layers.len();
                        if num > 1 {
                            let current = app.layer_stack.active_layer;
                            app.layer_stack.active_layer =
                                if current == 0 { num - 1 } else { current - 1 };
                            app.sync_active_layer();
                        }
                    }
                    KeyCode::BracketRight => {
                        // Next layer
                        let num = app.layer_stack.layers.len();
                        if num > 1 {
                            let current = app.layer_stack.active_layer;
                            app.layer_stack.active_layer = (current + 1) % num;
                            app.sync_active_layer();
                        }
                    }
                    _ => {}
                }
            }
            WindowEvent::RedrawRequested => {
                app.update();

                // Collect layer info snapshots before UI (avoids borrow conflicts)
                let layer_infos = app.layer_infos();
                let active_layer = app.layer_stack.active_layer;

                // Auto-show panels after startup delay
                app.egui_overlay.update_auto_show();

                // Prepare egui frame
                app.egui_overlay.begin_frame(&app.window);
                {
                    let ctx = app.egui_overlay.context();

                    // Get particle info from active layer
                    let mut particle_info = app
                        .layer_stack
                        .active()
                        .and_then(|l| l.as_effect())
                        .and_then(|e| e.pass_executor.particle_system.as_ref())
                        .map(|ps| {
                            // One field decides both, so the label can no longer
                            // name a source that is not the one on screen (#2011).
                            let source_kind = ps.source.kind();
                            let source_name = ps.source.display_name();
                            let (video_playing, video_looping, video_speed) =
                                match ps.source.playback() {
                                    Some(p) => (p.playing, p.looping, p.speed),
                                    None => (false, true, 1.0),
                                };
                            crate::ui::panels::particle_panel::ParticleInfo {
                                alive_count: ps.alive_count,
                                max_count: ps.max_particles,
                                emit_rate: ps.emit_rate,
                                burst_on_beat: ps.burst_on_beat,
                                lifetime: ps.def.lifetime,
                                initial_speed: ps.def.initial_speed,
                                initial_size: ps.def.initial_size,
                                size_end: ps.def.size_end,
                                drag: ps.def.drag,
                                attraction_strength: ps.def.attraction_strength,
                                blend_mode: ps.blend_mode.clone(),
                                has_flow_field: ps.def.flow_field,
                                has_trails: ps.trail_length() >= 2,
                                trail_length: ps.trail_length(),
                                has_interaction: ps.def.interaction,
                                has_sprite: ps.sprite.is_some(),
                                is_compute_raster: ps.is_compute_raster(),
                                max_scaled_count: ps.def.max_scaled_count,
                                has_image_source: ps.has_aux_data
                                    || ps.def.emitter.shape == "image",
                                source_kind,
                                source_name,
                                video_playing,
                                video_looping,
                                video_speed,
                                video_position_secs: ps.source.video_position_secs(),
                                video_duration_secs: ps.source.video_duration_secs(),
                                is_transitioning: ps.source_transition.is_some(),
                                source_loading: false, // set below
                                source_loading_name: String::new(),
                                builtin_images: Vec::new(), // set below
                                model_yaw: ps.model_sample.yaw_degrees,
                                model_pitch: ps.model_sample.pitch_degrees,
                                model_scale: ps.model_sample.scale,
                                model_ambient: ps.model_sample.ambient,
                                model_light_mix: ps.model_sample.light_mix,
                                model_light_x: ps.model_sample.light_x,
                                model_light_y: ps.model_sample.light_y,
                                model_light_z: ps.model_sample.light_z,
                                model_ray_strength: ps.model_sample.ray_strength,
                                has_splat: ps.def.splat.is_some(),
                                splat_sorted: ps.is_splat_sorted(),
                                splat_sh_degree: ps.splat_sh_degree(),
                                splat_scene_name: ps
                                    .splat_scene_path
                                    .as_deref()
                                    .and_then(|p| {
                                        std::path::Path::new(p)
                                            .file_name()
                                            .map(|n| n.to_string_lossy().to_string())
                                    })
                                    .unwrap_or_default(),
                                splat_count: ps.splat_loaded_count,
                                splat_total: ps.splat_total_count,
                                // Loader + demo state overlaid below (app-level)
                                splat_loading: false,
                                splat_loading_name: String::new(),
                                splat_progress: 0,
                                splat_error: None,
                                splat_demo_available: false,
                                splat_demo_cached: false,
                                splat_demo_size_mb: 0,
                                splat_demo_downloading: false,
                                splat_demo_progress: 0,
                                has_morph: ps.morph_state.is_some(),
                                morph_target_count: ps
                                    .morph_state
                                    .as_ref()
                                    .map_or(0, |m| m.target_count),
                                morph_source_index: ps
                                    .morph_state
                                    .as_ref()
                                    .map_or(0, |m| m.source_index),
                                morph_dest_index: ps
                                    .morph_state
                                    .as_ref()
                                    .map_or(0, |m| m.dest_index),
                                morph_progress: ps.morph_state.as_ref().map_or(0.0, |m| m.progress),
                                morph_transitioning: ps
                                    .morph_state
                                    .as_ref()
                                    .map_or(false, |m| m.transitioning),
                                morph_transition_style: ps
                                    .morph_state
                                    .as_ref()
                                    .map_or(0, |m| m.transition_style),
                                morph_auto_cycle: ps.morph_state.as_ref().map_or(0, |m| {
                                    match m.auto_cycle {
                                        crate::gpu::particle::morph::AutoCycle::Off => 0,
                                        crate::gpu::particle::morph::AutoCycle::OnBeat => 1,
                                        crate::gpu::particle::morph::AutoCycle::Timed(_) => 2,
                                    }
                                }),
                                morph_hold_duration: ps
                                    .morph_state
                                    .as_ref()
                                    .map_or(2.0, |m| m.hold_duration),
                                morph_target_labels: ps.def.morph_targets.as_ref().map_or_else(
                                    Vec::new,
                                    |targets| {
                                        targets
                                            .iter()
                                            .map(|t| {
                                                if t.source == "random" {
                                                    "random".to_string()
                                                } else if t.source == "snapshot" {
                                                    "snap".to_string()
                                                } else if let Some(shape) =
                                                    t.source.strip_prefix("geometry:")
                                                {
                                                    shape.to_string()
                                                } else if let Some(img) =
                                                    t.source.strip_prefix("image:")
                                                {
                                                    let name = img.trim_end_matches(".png");
                                                    let name = name
                                                        .strip_prefix("raster_")
                                                        .unwrap_or(name);
                                                    name.to_string()
                                                } else if let Some(text) =
                                                    t.source.strip_prefix("text:")
                                                {
                                                    if text.len() > 8 {
                                                        format!("{}...", &text[..8])
                                                    } else {
                                                        text.to_string()
                                                    }
                                                } else if let Some(rest) =
                                                    t.source.strip_prefix("video:")
                                                {
                                                    // "video:clip.mp4:f42" → "f42"
                                                    rest.rsplit(':')
                                                        .next()
                                                        .unwrap_or(rest)
                                                        .to_string()
                                                } else {
                                                    t.source.clone()
                                                }
                                            })
                                            .collect()
                                    },
                                ),
                            }
                        });
                    // Overlay loader state + built-in images onto particle info
                    if let Some(ref mut pi) = particle_info {
                        pi.source_loading = app.particle_source_loader.loading;
                        pi.source_loading_name = app.particle_source_loader.loading_name.clone();
                        if pi.has_image_source {
                            pi.builtin_images =
                                crate::gpu::particle::builtin_raster_images().to_vec();
                        }
                        if pi.has_splat {
                            use crate::gpu::particle::splat_source;
                            pi.splat_loading = app.splat_loader.loading;
                            pi.splat_loading_name = app.splat_loader.loading_name.clone();
                            pi.splat_progress = app
                                .splat_loader
                                .progress
                                .load(std::sync::atomic::Ordering::Relaxed);
                            pi.splat_error = app.splat_loader.last_error.clone();
                            if let Some(demo) = splat_source::demo_scene("default") {
                                pi.splat_demo_available = !demo.url.is_empty();
                                pi.splat_demo_cached = splat_source::demo_scene_cached("default");
                                pi.splat_demo_size_mb = demo.size_mb;
                            }
                            if let Some(ref dl) = app.splat_demo_download {
                                pi.splat_demo_downloading = dl.is_downloading();
                                pi.splat_demo_progress = dl.percent();
                            }
                        }
                    }
                    let particle_count = particle_info.as_ref().map(|p| p.max_count);

                    // Lattice panel info — Some only when the active effect is a
                    // Lattice effect (its particle system carries a LatticeSim).
                    let lattice_info = app
                        .layer_stack
                        .active()
                        .and_then(|l| l.as_effect())
                        .and_then(|e| e.pass_executor.particle_system.as_ref())
                        .filter(|ps| ps.lattice_enabled)
                        .map(|ps| crate::ui::panels::lattice_panel::LatticeInfo {
                            params: ps.lattice_params,
                            defaults: ps.lattice_defaults,
                        });

                    // Helix panel info — Some only when the active effect is a
                    // Helix effect (its particle system carries a HelixSim).
                    let helix_info = app
                        .layer_stack
                        .active()
                        .and_then(|l| l.as_effect())
                        .and_then(|e| e.pass_executor.particle_system.as_ref())
                        .filter(|ps| ps.helix_enabled)
                        .map(|ps| crate::ui::panels::helix_panel::HelixInfo {
                            params: ps.helix_params,
                            defaults: ps.helix_defaults,
                        });

                    // Get obstacle info from active layer
                    let obstacle_info =
                        app.layer_stack
                            .active()
                            .and_then(|l| l.as_effect())
                            .map(|e| {
                                let has_particles = e.pass_executor.particle_system.is_some();
                                let webcam_available = cfg!(feature = "webcam");
                                let video_available = cfg!(feature = "video") && {
                                    #[cfg(feature = "video")]
                                    {
                                        crate::media::video::ffmpeg_available()
                                    }
                                    #[cfg(not(feature = "video"))]
                                    {
                                        false
                                    }
                                };
                                let depth_available = cfg!(feature = "depth");
                                let depth_model_downloaded = {
                                    #[cfg(feature = "depth")]
                                    {
                                        crate::depth::model::depth_ready()
                                    }
                                    #[cfg(not(feature = "depth"))]
                                    {
                                        false
                                    }
                                };
                                let depth_downloading = {
                                    #[cfg(feature = "depth")]
                                    {
                                        app.depth_download
                                            .as_ref()
                                            .filter(|p| p.is_downloading())
                                            .map(|p| p.percent())
                                    }
                                    #[cfg(not(feature = "depth"))]
                                    {
                                        let _ = &app;
                                        None::<u8>
                                    }
                                };
                                let depth_download_error = {
                                    #[cfg(feature = "depth")]
                                    {
                                        app.depth_download
                                            .as_ref()
                                            .filter(|p| p.is_error())
                                            .and_then(|p| {
                                                p.error_message.lock().ok().and_then(|m| m.clone())
                                            })
                                    }
                                    #[cfg(not(feature = "depth"))]
                                    {
                                        None::<String>
                                    }
                                };
                                if let Some(ps) = &e.pass_executor.particle_system {
                                    crate::ui::panels::obstacle_panel::ObstacleInfo {
                                        enabled: ps.obstacle_enabled,
                                        mode: ps.obstacle_mode,
                                        fit: ps.obstacle_fit,
                                        threshold: ps.obstacle_threshold,
                                        elasticity: ps.obstacle_elasticity,
                                        source: ps.obstacle_source.clone(),
                                        image_path: ps.obstacle_image_path.clone(),
                                        has_particles,
                                        webcam_available,
                                        video_available,
                                        depth_available,
                                        depth_model_downloaded,
                                        depth_downloading,
                                        depth_download_error,
                                        #[cfg(feature = "webcam")]
                                        webcam_devices: app.webcam_devices.clone(),
                                        #[cfg(not(feature = "webcam"))]
                                        webcam_devices: vec![],
                                        #[cfg(feature = "webcam")]
                                        webcam_device_index: app.webcam_device_index,
                                        #[cfg(not(feature = "webcam"))]
                                        webcam_device_index: 0,
                                        water_enabled: ps.obstacle_water_enabled,
                                        water_level: ps.obstacle_water_params.level_scale,
                                        water_source: ps.obstacle_water_params.source_rate,
                                        water_drain: ps.obstacle_water_params.drain,
                                        water_flux: ps.obstacle_water_params.flux_gain,
                                        model_spin: ps.obstacle_model_spin,
                                        model_display: ps.obstacle_model_display,
                                        fluid_enabled: ps.obstacle_fluid_enabled,
                                        fluid_speed: ps.obstacle_fluid_params.flow_speed,
                                        fluid_coupling: ps.obstacle_fluid_coupling,
                                        fluid_vorticity: ps.obstacle_fluid_params.vorticity,
                                        fluid_viscosity: ps.obstacle_fluid_params.viscosity,
                                        fluid_grid: ps.obstacle_fluid_grid,
                                    }
                                } else {
                                    crate::ui::panels::obstacle_panel::ObstacleInfo {
                                        enabled: false,
                                        mode: crate::gpu::particle::ObstacleMode::Bounce,
                                        fit: crate::gpu::particle::ObstacleFit::Cover,
                                        threshold: 0.5,
                                        elasticity: 0.7,
                                        source: String::new(),
                                        image_path: None,
                                        has_particles,
                                        webcam_available,
                                        video_available,
                                        depth_available,
                                        depth_model_downloaded,
                                        depth_downloading,
                                        depth_download_error,
                                        #[cfg(feature = "webcam")]
                                        webcam_devices: app.webcam_devices.clone(),
                                        #[cfg(not(feature = "webcam"))]
                                        webcam_devices: vec![],
                                        #[cfg(feature = "webcam")]
                                        webcam_device_index: app.webcam_device_index,
                                        #[cfg(not(feature = "webcam"))]
                                        webcam_device_index: 0,
                                        water_enabled: false,
                                        water_level: 1.5,
                                        water_source: 0.01,
                                        water_drain: 0.06,
                                        water_flux: 0.18,
                                        model_spin: 1.0,
                                        model_display: 0.0,
                                        fluid_enabled: false,
                                        fluid_speed: 0.9,
                                        fluid_coupling: 0.8,
                                        fluid_vorticity: 0.18,
                                        fluid_viscosity: 0.02,
                                        fluid_grid: 256,
                                    }
                                }
                            });

                    // Get active layer's shader error
                    let shader_error = app
                        .layer_stack
                        .active()
                        .and_then(|l| l.shader_error().map(|s| s.to_string()));

                    // Collect media info if active layer is media (before mutable borrow)
                    let media_info = app.layer_stack.active().and_then(|l| {
                        l.as_media().filter(|m| !m.is_live()).map(|m| {
                            crate::ui::panels::media_panel::MediaInfo {
                                file_name: m.file_name.clone(),
                                media_width: m.media_width,
                                media_height: m.media_height,
                                frame_count: m.frame_count(),
                                is_animated: m.is_animated(),
                                is_video: m.is_video(),
                                playing: m.transport.playing,
                                looping: m.transport.looping,
                                speed: m.transport.speed,
                                direction: m.transport.direction,
                                current_frame: m.current_frame,
                                video_position_secs: m.position_secs(),
                                video_duration_secs: m.duration_secs(),
                            }
                        })
                    });

                    // Collect webcam info if active layer is a live webcam
                    let webcam_info = app.layer_stack.active().and_then(|l| {
                        l.as_media().filter(|m| m.is_live()).map(|m| {
                            crate::ui::panels::webcam_panel::WebcamInfo {
                                device_name: m.file_name.clone(),
                                width: m.media_width,
                                height: m.media_height,
                                #[cfg(feature = "webcam")]
                                mirror: m.mirror,
                                #[cfg(not(feature = "webcam"))]
                                mirror: false,
                                #[cfg(feature = "webcam")]
                                available_devices: app.webcam_devices.clone(),
                                #[cfg(not(feature = "webcam"))]
                                available_devices: vec![],
                                #[cfg(feature = "webcam")]
                                device_index: app.webcam_device_index,
                                #[cfg(not(feature = "webcam"))]
                                device_index: 0,
                                #[cfg(feature = "webcam")]
                                capture_running: app
                                    .webcam_capture
                                    .as_ref()
                                    .map_or(false, |c| c.is_running()),
                                #[cfg(not(feature = "webcam"))]
                                capture_running: false,
                            }
                        })
                    });

                    // Store NDI state in egui temp data for UI panels
                    #[cfg(feature = "ndi")]
                    {
                        let ndi_info = crate::ui::panels::ndi_panel::NdiInfo {
                            enabled: app.ndi.config.enabled,
                            running: app.ndi.is_running(),
                            ndi_available: crate::ndi::ffi::ndi_available(),
                            source_name: app.ndi.config.source_name.clone(),
                            resolution: app.ndi.config.resolution,
                            frames_sent: app.ndi.frames_sent(),
                            frames_dropped: app.ndi.pipeline.frames_dropped(),
                            output_width: app.ndi.capture_dimensions().0,
                            output_height: app.ndi.capture_dimensions().1,
                            alpha_from_luma: app.ndi.config.alpha_from_luma,
                            error: app.ndi.pipeline.last_error().map(String::from),
                        };
                        ctx.data_mut(|d| {
                            d.insert_temp(egui::Id::new("ndi_info"), ndi_info);
                            d.insert_temp(egui::Id::new("ndi_running"), app.ndi.is_running());
                        });
                    }

                    // Store Ableton Link state in egui temp data for UI panels
                    #[cfg(feature = "link")]
                    {
                        let tick = app.link.last_tick();
                        let link_info = crate::ui::panels::link_panel::LinkInfo {
                            enabled: app.link.config.enabled,
                            mode: app.link.config.mode,
                            quantum: app.link.config.quantum,
                            start_stop_sync: app.link.config.start_stop_sync,
                            peers: tick.map_or(0, |t| t.peers),
                            session_tempo: tick.map_or(0.0, |t| t.tempo),
                            quantum_phase: tick.map_or(0.0, |t| t.quantum_phase),
                            playing: tick.is_some_and(|t| t.playing),
                        };
                        ctx.data_mut(|d| {
                            d.insert_temp(egui::Id::new("link_info"), link_info);
                        });
                    }

                    // Store v4l2 state in egui temp data for UI panels
                    #[cfg(all(target_os = "linux", feature = "v4l2"))]
                    {
                        let v4l2_info = crate::ui::panels::v4l2_panel::V4l2Info {
                            enabled: app.v4l2.config.enabled,
                            running: app.v4l2.is_running(),
                            devices: app
                                .v4l2
                                .devices
                                .iter()
                                .map(|d| (d.path.clone(), d.name.clone()))
                                .collect(),
                            device_path: app.v4l2.config.device_path.clone(),
                            resolved_path: app.v4l2.resolved_path().map(String::from),
                            resolution: app.v4l2.config.resolution,
                            pixel_format: app.v4l2.config.pixel_format,
                            frames_sent: app.v4l2.frames_sent(),
                            frames_dropped: app.v4l2.pipeline.frames_dropped(),
                            output_width: app.v4l2.capture_dimensions().0,
                            output_height: app.v4l2.capture_dimensions().1,
                            error: app.v4l2.pipeline.last_error().map(String::from),
                        };
                        ctx.data_mut(|d| {
                            d.insert_temp(egui::Id::new("v4l2_info"), v4l2_info);
                            d.insert_temp(egui::Id::new("v4l2_running"), app.v4l2.is_running());
                        });
                    }

                    // Store Spout state in egui temp data for UI panels
                    #[cfg(all(target_os = "windows", feature = "spout"))]
                    {
                        let spout_info = crate::ui::panels::spout_panel::SpoutInfo {
                            enabled: app.spout.config.enabled,
                            running: app.spout.is_running(),
                            sender_name: app.spout.config.sender_name.clone(),
                            resolution: app.spout.config.resolution,
                            frames_sent: app.spout.frames_sent(),
                            frames_dropped: app.spout.pipeline.frames_dropped(),
                            output_width: app.spout.capture_dimensions().0,
                            output_height: app.spout.capture_dimensions().1,
                            error: app.spout.pipeline.last_error().map(String::from),
                        };
                        ctx.data_mut(|d| {
                            d.insert_temp(egui::Id::new("spout_info"), spout_info);
                            d.insert_temp(egui::Id::new("spout_running"), app.spout.is_running());
                        });
                    }

                    // Store Syphon state in egui temp data for UI panels
                    #[cfg(all(target_os = "macos", feature = "syphon"))]
                    {
                        let syphon_info = crate::ui::panels::syphon_panel::SyphonInfo {
                            available: crate::syphon::ffi::syphon_available(),
                            enabled: app.syphon.config.enabled,
                            running: app.syphon.is_running(),
                            server_name: app.syphon.config.server_name.clone(),
                            resolution: app.syphon.config.resolution,
                            frames_sent: app.syphon.frames_sent(),
                            frames_dropped: app.syphon.pipeline.frames_dropped(),
                            output_width: app.syphon.capture_dimensions().0,
                            output_height: app.syphon.capture_dimensions().1,
                            error: app.syphon.pipeline.last_error().map(String::from),
                        };
                        ctx.data_mut(|d| {
                            d.insert_temp(egui::Id::new("syphon_info"), syphon_info);
                            d.insert_temp(egui::Id::new("syphon_running"), app.syphon.is_running());
                        });
                    }

                    // Store recording state in egui temp data for UI panels
                    {
                        let rec_info = crate::ui::panels::recording_panel::RecordingInfo {
                            recording: app.recording.is_recording(),
                            has_audio: app.recording.has_audio(),
                            ffmpeg_found: app.recording.encoder_info.ffmpeg_found,
                            encoder_info: app.recording.encoder_info.clone(),
                            config: app.recording.config.clone(),
                            duration_secs: app
                                .recording
                                .duration()
                                .map_or(0.0, |d| d.as_secs_f64()),
                            frames_encoded: app.recording.frames_encoded(),
                            bytes_written: app.recording.total_bytes_written(),
                            output_width: app.recording.capture_dimensions().0,
                            output_height: app.recording.capture_dimensions().1,
                            encoder_name: match &app.recording.state {
                                crate::recording::RecordingState::Recording {
                                    encoder_name,
                                    ..
                                } => encoder_name.clone(),
                                _ => String::new(),
                            },
                            error: match &app.recording.state {
                                crate::recording::RecordingState::Error(e) => Some(e.clone()),
                                _ => None,
                            },
                            audio_active: app.audio.active,
                        };
                        ctx.data_mut(|d| {
                            d.insert_temp(egui::Id::new("recording_info"), rec_info);
                        });
                    }

                    // Store preset loading state in egui temp data for UI panels
                    {
                        let loading_state = app.preset_loader.state.clone();
                        ctx.data_mut(|d| {
                            d.insert_temp(egui::Id::new("preset_loading_state"), loading_state);
                        });
                    }

                    // Sync compile errors into shader editor
                    if app.shader_editor.open {
                        app.shader_editor.compile_error = app
                            .layer_stack
                            .active()
                            .and_then(|l| l.shader_error().map(|s| s.to_string()));
                    }

                    // Collect scene info before mutable borrows
                    let scene_info = Some(app.scene_info());

                    // Snapshot global volumetric state so a slider drag in the
                    // panel below (which mutates it by &mut) marks the preset
                    // dirty, like the other panels do.
                    let vol_before = (app.volumetric_enabled, app.volumetric_params);

                    // Get active layer's param_store (mutable for MIDI badges)
                    let active_params = app.layer_stack.active_mut();
                    if let Some(layer) = active_params {
                        if !app.shader_editor.open {
                            crate::ui::panels::draw_panels(
                                &ctx,
                                app.egui_overlay.visible,
                                &mut app.audio,
                                &mut layer.param_store,
                                &shader_error,
                                &app.uniforms,
                                &app.effect_loader,
                                &mut layer.postprocess,
                                &mut app.volumetric_enabled,
                                &mut app.volumetric_params,
                                particle_count,
                                &mut app.midi,
                                &mut app.osc,
                                &mut app.web,
                                &mut app.binding_bus,
                                &app.preset_store,
                                &layer_infos,
                                active_layer,
                                media_info,
                                webcam_info,
                                particle_info,
                                obstacle_info,
                                lattice_info,
                                helix_info,
                                scene_info,
                                &app.status_error,
                                &app.settings,
                            );
                        }
                        // Sync global postprocess enabled from layer
                        app.post_process.enabled = layer.postprocess.enabled;
                    }
                    if (app.volumetric_enabled, app.volumetric_params) != vol_before {
                        app.preset_store.mark_dirty();
                    }

                    // Draw shader editor overlay (on top of everything)
                    crate::ui::panels::shader_editor::draw_shader_editor(
                        &ctx,
                        &mut app.shader_editor,
                        app.settings.theme,
                    );
                    crate::ui::panels::shader_editor::draw_new_effect_prompt(
                        &ctx,
                        &mut app.shader_editor,
                    );

                    // Check if sidebar "Matrix" button was clicked
                    let matrix_open_requested = ctx.data_mut(|d| {
                        d.get_temp::<bool>(egui::Id::new("open_binding_matrix"))
                            .unwrap_or(false)
                    });
                    if matrix_open_requested {
                        app.binding_matrix.open = true;
                        ctx.data_mut(|d| {
                            d.insert_temp(egui::Id::new("open_binding_matrix"), false);
                            d.insert_temp(egui::Id::new("binding_matrix_just_opened"), true);
                        });
                    }

                    // Draw binding matrix modal
                    if app.binding_matrix.open {
                        let layers: Vec<crate::ui::panels::binding_helpers::LayerParamInfo> = app
                            .layer_stack
                            .layers
                            .iter()
                            .enumerate()
                            .map(|(i, l)| {
                                let effect_name = l
                                    .effect_index()
                                    .and_then(|idx| app.effect_loader.effects.get(idx))
                                    .map(|eff| eff.name.clone())
                                    .unwrap_or_default();
                                let param_names = l
                                    .param_store
                                    .defs
                                    .iter()
                                    .filter(|d| {
                                        matches!(
                                            d,
                                            crate::params::ParamDef::Float { .. }
                                                | crate::params::ParamDef::Bool { .. }
                                        )
                                    })
                                    .map(|d| d.name().to_string())
                                    .collect();
                                crate::ui::panels::binding_helpers::LayerParamInfo {
                                    index: i,
                                    effect_name,
                                    param_names,
                                }
                            })
                            .collect();
                        let bind_info = crate::ui::panels::binding_helpers::BindingPanelInfo {
                            layers,
                            active_layer,
                            layer_count: layer_infos.len(),
                            preset_name: app
                                .preset_store
                                .current_name()
                                .unwrap_or("(unsaved)")
                                .to_string(),
                        };
                        crate::ui::panels::binding_matrix::draw_binding_matrix(
                            &ctx,
                            &mut app.binding_matrix,
                            &mut app.binding_bus,
                            &bind_info,
                        );
                    }

                    // GPU profiler panel
                    #[cfg(feature = "profiling")]
                    if app.egui_overlay.visible {
                        egui::Window::new("GPU Profiler")
                            .default_pos([10.0, 10.0])
                            .default_size([250.0, 300.0])
                            .resizable(true)
                            .collapsible(true)
                            .show(&ctx, |ui| {
                                app.gpu_profiler.ui(ui);
                            });
                    }

                    // Draw depth download confirmation modal
                    crate::ui::panels::obstacle_panel::draw_depth_download_modal(&ctx);

                    // Draw quit confirmation dialog
                    if app.quit_requested {
                        // Track whether dialog was already showing last frame.
                        // On the first frame, the Esc that opened it is still in input state,
                        // so skip Esc-to-cancel until the next frame.
                        let dialog_id = egui::Id::new("quit_dialog_shown");
                        let was_shown: bool = ctx.data(|d| d.get_temp(dialog_id).unwrap_or(false));
                        ctx.data_mut(|d| d.insert_temp(dialog_id, true));

                        let tc = crate::ui::theme::colors::theme_colors(&ctx);

                        egui::Window::new("Quit Fosfora?")
                            .collapsible(false)
                            .resizable(false)
                            .fixed_size(egui::Vec2::new(280.0, 0.0))
                            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                            .show(&ctx, |ui| {
                                ui.label(
                                    egui::RichText::new("Are you sure you want to quit?")
                                        .size(14.0)
                                        .color(tc.text_primary),
                                );
                                ui.add_space(12.0);
                                let btn_size = egui::Vec2::new(100.0, 32.0);
                                let esc_cancel =
                                    was_shown && ui.input(|i| i.key_pressed(egui::Key::Escape));
                                ui.horizontal(|ui| {
                                    let quit_fill = egui::Color32::from_rgba_unmultiplied(
                                        tc.error.r(),
                                        tc.error.g(),
                                        tc.error.b(),
                                        60,
                                    );
                                    if ui
                                        .add(
                                            egui::Button::new(
                                                egui::RichText::new("Quit").color(tc.error),
                                            )
                                            .fill(quit_fill)
                                            .min_size(btn_size),
                                        )
                                        .clicked()
                                    {
                                        ui.ctx().data_mut(|d| {
                                            d.insert_temp(egui::Id::new("confirm_quit"), true);
                                        });
                                    }
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if ui
                                                .add(egui::Button::new("Cancel").min_size(btn_size))
                                                .clicked()
                                                || esc_cancel
                                            {
                                                app.quit_requested = false;
                                            }
                                        },
                                    );
                                });
                            });
                    } else {
                        // Clear the flag when dialog is dismissed
                        ctx.data_mut(|d| d.remove_temp::<bool>(egui::Id::new("quit_dialog_shown")));
                    }
                }
                app.egui_overlay.end_frame(&app.window);

                // Handle quit confirmation
                let confirm_quit: Option<bool> = app
                    .egui_overlay
                    .context()
                    .data_mut(|d| d.remove_temp(egui::Id::new("confirm_quit")));
                if confirm_quit.is_some() {
                    app.gpu.save_pipeline_cache();
                    // Flush any global binding edit still inside the 1s debounce
                    // window so it isn't lost on quit.
                    app.binding_bus.flush();
                    event_loop.exit();
                }

                // Handle shader editor signals
                let open_editor: Option<bool> = app
                    .egui_overlay
                    .context()
                    .data_mut(|d| d.remove_temp(egui::Id::new("open_shader_editor")));
                if open_editor.is_some() {
                    // Resolve active layer's shader path
                    if let Some(idx) = app.layer_stack.active().and_then(|l| l.effect_index()) {
                        if let Some(effect) = app.effect_loader.effects.get(idx).cloned() {
                            let passes = effect.normalized_passes();
                            if let Some(pass) = passes.first() {
                                let path = app.effect_loader.resolve_shader_path(&pass.shader);
                                if let Ok(content) = std::fs::read_to_string(&path) {
                                    app.shader_editor.open_file(&effect.name, path, content);
                                    // Load paired .pfx file for tab switching
                                    if let Some(ref pfx_path) = effect.source_path {
                                        if let Ok(pfx_content) = std::fs::read_to_string(pfx_path) {
                                            app.shader_editor
                                                .load_paired_pfx(pfx_path.clone(), pfx_content);
                                        }
                                    }
                                } else {
                                    log::error!("Could not read shader: {}", path.display());
                                }
                            }
                        }
                    }
                }

                let save_editor: Option<bool> = app
                    .egui_overlay
                    .context()
                    .data_mut(|d| d.remove_temp(egui::Id::new("shader_editor_save")));
                if save_editor.is_some() {
                    // Save the active tab
                    if let Some(ref path) = app.shader_editor.file_path {
                        match std::fs::write(path, &app.shader_editor.code) {
                            Ok(()) => {
                                app.shader_editor.disk_content = app.shader_editor.code.clone();
                                log::info!("Saved shader: {}", path.display());
                            }
                            Err(e) => {
                                log::error!("Failed to save shader: {e}");
                                app.status_error =
                                    Some((format!("Save failed: {e}"), std::time::Instant::now()));
                            }
                        }
                    }
                    // Also save the paired tab if it has unsaved changes
                    if app.shader_editor.paired_is_dirty() {
                        if let Some(ref paired_path) = app.shader_editor.paired_path {
                            match std::fs::write(paired_path, &app.shader_editor.paired_content) {
                                Ok(()) => {
                                    app.shader_editor.paired_disk_content =
                                        app.shader_editor.paired_content.clone();
                                    log::info!("Saved paired file: {}", paired_path.display());
                                }
                                Err(e) => {
                                    log::error!("Failed to save paired file: {e}");
                                    app.status_error = Some((
                                        format!("Save failed: {e}"),
                                        std::time::Instant::now(),
                                    ));
                                }
                            }
                        }
                    }
                }

                // Handle shader error dismiss from status bar
                let dismiss_error: Option<bool> = app
                    .egui_overlay
                    .context()
                    .data_mut(|d| d.remove_temp(egui::Id::new("dismiss_shader_error")));
                if dismiss_error.is_some() {
                    if let Some(layer) = app.layer_stack.active_mut() {
                        if let Some(e) = layer.as_effect_mut() {
                            e.shader_error = None;
                        }
                    }
                    app.shader_editor.compile_error = None;
                }

                let new_prompt: Option<bool> = app
                    .egui_overlay
                    .context()
                    .data_mut(|d| d.remove_temp(egui::Id::new("new_effect_prompt")));
                if new_prompt.is_some() {
                    app.shader_editor.new_effect_prompt = true;
                }

                // Handle "Copy Shader" prompt for built-in effects
                let copy_prompt: Option<bool> = app
                    .egui_overlay
                    .context()
                    .data_mut(|d| d.remove_temp(egui::Id::new("copy_builtin_prompt")));
                if copy_prompt.is_some() {
                    app.shader_editor.new_effect_prompt = true;
                    app.shader_editor.copy_builtin_mode = true;
                }

                // Handle copy built-in effect creation
                let copy_effect: Option<String> = app
                    .egui_overlay
                    .context()
                    .data_mut(|d| d.remove_temp(egui::Id::new("create_copy_effect")));
                if let Some(name) = copy_effect {
                    if let Err(e) = app.copy_builtin_effect(&name) {
                        log::error!("Failed to copy effect: {e}");
                        app.status_error =
                            Some((format!("Copy failed: {e}"), std::time::Instant::now()));
                    }
                }

                // Handle delete effect signal
                let delete_effect: Option<usize> = app
                    .egui_overlay
                    .context()
                    .data_mut(|d| d.remove_temp(egui::Id::new("delete_effect")));
                if let Some(idx) = delete_effect {
                    match app.effect_loader.delete_effect(idx) {
                        Ok(name) => {
                            log::info!("Deleted effect: {name}");
                            // A deleted effect can't stay pinned
                            if let Some(pos) = app
                                .settings
                                .favorite_effects
                                .iter()
                                .position(|f| *f == name)
                            {
                                app.settings.favorite_effects.remove(pos);
                                app.settings.save();
                            }
                            // Close shader editor if it was editing the deleted effect
                            if app.shader_editor.open {
                                app.shader_editor.open = false;
                            }
                            // Fix up current_effect after rescan
                            // The active layer's effect_index refers to the old list,
                            // so just clear it — the effect stays rendered but is gone from panel
                            app.effect_loader.current_effect = None;
                        }
                        Err(e) => {
                            log::error!("Failed to delete effect: {e}");
                            app.status_error =
                                Some((format!("Delete failed: {e}"), std::time::Instant::now()));
                        }
                    }
                }

                // Handle favorite star toggle from the effect browser
                let toggle_fav: Option<String> = app
                    .egui_overlay
                    .context()
                    .data_mut(|d| d.remove_temp(egui::Id::new("toggle_favorite_effect")));
                if let Some(name) = toggle_fav {
                    if let Some(pos) = app
                        .settings
                        .favorite_effects
                        .iter()
                        .position(|f| *f == name)
                    {
                        app.settings.favorite_effects.remove(pos);
                    } else {
                        app.settings.favorite_effects.push(name);
                    }
                    app.settings.save();
                }

                let create_effect: Option<String> = app
                    .egui_overlay
                    .context()
                    .data_mut(|d| d.remove_temp(egui::Id::new("create_new_effect")));
                if let Some(name) = create_effect {
                    if let Err(e) = app.create_new_effect(&name) {
                        log::error!("Failed to create effect: {e}");
                        app.status_error =
                            Some((format!("Create failed: {e}"), std::time::Instant::now()));
                    }
                }

                // Handle theme change from settings panel
                let set_theme: Option<crate::ui::theme::ThemeMode> = app
                    .egui_overlay
                    .context()
                    .data_mut(|d| d.remove_temp(egui::Id::new("set_theme")));
                if let Some(theme) = set_theme {
                    app.egui_overlay.set_theme(theme);
                    app.settings.theme = theme;
                    app.settings.save();
                }

                // Handle output-alpha mode change from settings panel
                let set_output_alpha: Option<crate::settings::AlphaOutputMode> = app
                    .egui_overlay
                    .context()
                    .data_mut(|d| d.remove_temp(egui::Id::new("set_output_alpha")));
                if let Some(mode) = set_output_alpha {
                    app.settings.output_alpha = mode;
                    app.settings.save();
                }

                // Handle particle quality change from settings panel
                let set_quality: Option<crate::settings::ParticleQuality> = app
                    .egui_overlay
                    .context()
                    .data_mut(|d| d.remove_temp(egui::Id::new("set_particle_quality")));
                if let Some(quality) = set_quality {
                    app.settings.particle_quality = quality;
                    app.settings.save();
                    // Reload active effect to rebuild particle system with new buffer size
                    let active = app.layer_stack.active_layer;
                    if let Some(layer) = app.layer_stack.layers.get(active) {
                        if let Some(effect_idx) = layer.effect_index() {
                            app.load_effect_on_layer(active, effect_idx);
                        }
                    }
                }

                // Handle band-scale change from settings panel (A1 #1452)
                let set_band_scale: Option<crate::settings::BandScale> = app
                    .egui_overlay
                    .context()
                    .data_mut(|d| d.remove_temp(egui::Id::new("set_band_scale")));
                if let Some(scale) = set_band_scale {
                    app.settings.band_scale = scale;
                    app.settings.save();
                    // Rebuild the audio pipeline so the analyzer picks up the new scaling.
                    app.audio.set_band_scale(scale);
                }

                // Handle auto-reconnect toggle from settings panel (A9 #1460)
                let set_auto_reconnect: Option<bool> = app
                    .egui_overlay
                    .context()
                    .data_mut(|d| d.remove_temp(egui::Id::new("set_auto_reconnect")));
                if let Some(on) = set_auto_reconnect {
                    app.settings.auto_reconnect = on;
                    app.settings.save();
                    app.audio.set_auto_reconnect(on);
                }

                // Persist A18 structure tuning after a slider release (#1510). The live value
                // already reached the audio thread via the shared Arc; here we just snapshot the
                // newest config into settings and save (no rebuild).
                let tuning_dirty: Option<bool> = app
                    .egui_overlay
                    .context()
                    .data_mut(|d| d.remove_temp(egui::Id::new("structure_tuning_dirty")));
                if tuning_dirty == Some(true) {
                    app.settings.structure_tuning =
                        *app.audio.tuning().lock().unwrap_or_else(|e| e.into_inner());
                    app.settings.save();
                }

                // Persist the A7 tempo prior after a slider release / preset pick (#1458). Same
                // deal as the A18 tuning above: the live value already reached the audio thread
                // through the shared Arc, so this only snapshots it into settings and saves.
                let tempo_dirty: Option<bool> = app
                    .egui_overlay
                    .context()
                    .data_mut(|d| d.remove_temp(egui::Id::new("tempo_config_dirty")));
                if tempo_dirty == Some(true) {
                    app.settings.tempo = app
                        .audio
                        .tempo()
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .config;
                    app.settings.save();
                }

                // Handle FFmpeg webcam toggle from settings panel
                #[cfg(feature = "webcam")]
                {
                    let set_ffmpeg: Option<bool> = app
                        .egui_overlay
                        .context()
                        .data_mut(|d| d.remove_temp(egui::Id::new("set_ffmpeg_webcam")));
                    if let Some(use_ffmpeg) = set_ffmpeg {
                        if use_ffmpeg && !crate::media::webcam_ffmpeg::ffmpeg_available() {
                            app.status_error = Some((
                                "FFmpeg not found in PATH. Install FFmpeg to use this feature."
                                    .into(),
                                std::time::Instant::now(),
                            ));
                        } else {
                            // Stop any active capture
                            app.webcam_capture = None;
                            app.use_ffmpeg_webcam = use_ffmpeg;
                            app.settings.use_ffmpeg_webcam = use_ffmpeg;
                            app.settings.save();
                            // Refresh device list with new backend
                            app.refresh_webcam_devices();
                            let backend = if use_ffmpeg { "FFmpeg" } else { "native" };
                            let device_count = app.webcam_devices.len();
                            log::info!(
                                "Webcam backend switched to {backend}, {device_count} device(s) found"
                            );
                            app.status_error = Some((
                                format!(
                                    "Webcam: {backend} backend, {device_count} device(s) found"
                                ),
                                std::time::Instant::now(),
                            ));
                        }
                    }
                }

                // Handle audio device switch from UI
                let switch_audio: Option<String> = app
                    .egui_overlay
                    .context()
                    .data_mut(|d| d.remove_temp(egui::Id::new("switch_audio_device")));
                if let Some(device_str) = switch_audio {
                    let device_name = if device_str.is_empty() {
                        None
                    } else {
                        Some(device_str.as_str())
                    };
                    app.audio.switch_device(device_name);
                    app.settings.audio_device = if device_str.is_empty() {
                        None
                    } else {
                        Some(device_str)
                    };
                    app.settings.save();
                }

                // Handle Ableton Link signals from UI
                #[cfg(feature = "link")]
                {
                    let ctx = app.egui_overlay.context();
                    let enable: Option<bool> =
                        ctx.data_mut(|d| d.remove_temp(egui::Id::new("link_set_enabled")));
                    if let Some(on) = enable {
                        app.link.set_enabled(on);
                    }
                    let mode: Option<u8> =
                        ctx.data_mut(|d| d.remove_temp(egui::Id::new("link_set_mode")));
                    if let Some(m) = mode {
                        app.link.set_mode(if m == 1 {
                            crate::link::LinkMode::Lead
                        } else {
                            crate::link::LinkMode::Follow
                        });
                    }
                    let quantum: Option<f64> =
                        ctx.data_mut(|d| d.remove_temp(egui::Id::new("link_set_quantum")));
                    if let Some(q) = quantum {
                        app.link.set_quantum(q);
                    }
                    let sss: Option<bool> =
                        ctx.data_mut(|d| d.remove_temp(egui::Id::new("link_set_start_stop")));
                    if let Some(on) = sss {
                        app.link.set_start_stop_sync(on);
                    }
                }

                // Handle NDI signals from UI
                #[cfg(feature = "ndi")]
                {
                    let ndi_enable: Option<bool> = app
                        .egui_overlay
                        .context()
                        .data_mut(|d| d.remove_temp(egui::Id::new("ndi_set_enabled")));
                    if let Some(enabled) = ndi_enable {
                        app.ndi.set_enabled(
                            enabled,
                            &app.gpu.device,
                            app.gpu.format,
                            app.gpu.surface_config.width,
                            app.gpu.surface_config.height,
                        );
                    }

                    let ndi_source: Option<String> = app
                        .egui_overlay
                        .context()
                        .data_mut(|d| d.remove_temp(egui::Id::new("ndi_source_name")));
                    if let Some(name) = ndi_source {
                        app.ndi.config.source_name = name;
                        app.ndi.config.save();
                        app.ndi.restart(
                            &app.gpu.device,
                            app.gpu.format,
                            app.gpu.surface_config.width,
                            app.gpu.surface_config.height,
                        );
                    }

                    let ndi_res: Option<u8> = app
                        .egui_overlay
                        .context()
                        .data_mut(|d| d.remove_temp(egui::Id::new("ndi_resolution_change")));
                    if let Some(res_u8) = ndi_res {
                        let res = crate::ndi::types::OutputResolution::ALL
                            .get(res_u8 as usize)
                            .copied()
                            .unwrap_or(crate::ndi::types::OutputResolution::Match);
                        app.ndi.config.resolution = res;
                        app.ndi.config.save();
                        app.ndi.restart(
                            &app.gpu.device,
                            app.gpu.format,
                            app.gpu.surface_config.width,
                            app.gpu.surface_config.height,
                        );
                    }

                    let ndi_alpha_luma: Option<bool> = app
                        .egui_overlay
                        .context()
                        .data_mut(|d| d.remove_temp(egui::Id::new("ndi_alpha_from_luma")));
                    if let Some(val) = ndi_alpha_luma {
                        app.ndi.config.alpha_from_luma = val;
                        app.ndi.config.save();
                    }

                    let ndi_restart: Option<bool> = app
                        .egui_overlay
                        .context()
                        .data_mut(|d| d.remove_temp(egui::Id::new("ndi_restart")));
                    if ndi_restart.is_some() {
                        app.ndi.restart(
                            &app.gpu.device,
                            app.gpu.format,
                            app.gpu.surface_config.width,
                            app.gpu.surface_config.height,
                        );
                    }
                }

                // Handle v4l2 signals from UI
                #[cfg(all(target_os = "linux", feature = "v4l2"))]
                {
                    let v4l2_enable: Option<bool> = app
                        .egui_overlay
                        .context()
                        .data_mut(|d| d.remove_temp(egui::Id::new("v4l2_set_enabled")));
                    if let Some(enabled) = v4l2_enable {
                        app.v4l2.set_enabled(
                            enabled,
                            &app.gpu.device,
                            app.gpu.format,
                            app.gpu.surface_config.width,
                            app.gpu.surface_config.height,
                        );
                    }

                    let v4l2_device: Option<Option<String>> = app
                        .egui_overlay
                        .context()
                        .data_mut(|d| d.remove_temp(egui::Id::new("v4l2_device_path")));
                    if let Some(path) = v4l2_device {
                        app.v4l2.config.device_path = path;
                        app.v4l2.config.save();
                        app.v4l2.restart(
                            &app.gpu.device,
                            app.gpu.format,
                            app.gpu.surface_config.width,
                            app.gpu.surface_config.height,
                        );
                    }

                    let v4l2_res: Option<u8> = app
                        .egui_overlay
                        .context()
                        .data_mut(|d| d.remove_temp(egui::Id::new("v4l2_resolution_change")));
                    if let Some(res_u8) = v4l2_res {
                        let res = crate::v4l2::types::OutputResolution::ALL
                            .get(res_u8 as usize)
                            .copied()
                            .unwrap_or(crate::v4l2::types::OutputResolution::Match);
                        app.v4l2.config.resolution = res;
                        app.v4l2.config.save();
                        app.v4l2.restart(
                            &app.gpu.device,
                            app.gpu.format,
                            app.gpu.surface_config.width,
                            app.gpu.surface_config.height,
                        );
                    }

                    let v4l2_fmt: Option<u8> = app
                        .egui_overlay
                        .context()
                        .data_mut(|d| d.remove_temp(egui::Id::new("v4l2_pixel_format")));
                    if let Some(fmt_u8) = v4l2_fmt {
                        let fmt = crate::v4l2::types::V4l2PixelFormat::ALL
                            .get(fmt_u8 as usize)
                            .copied()
                            .unwrap_or_default();
                        app.v4l2.config.pixel_format = fmt;
                        app.v4l2.config.save();
                        app.v4l2.restart(
                            &app.gpu.device,
                            app.gpu.format,
                            app.gpu.surface_config.width,
                            app.gpu.surface_config.height,
                        );
                    }

                    let v4l2_refresh: Option<bool> = app
                        .egui_overlay
                        .context()
                        .data_mut(|d| d.remove_temp(egui::Id::new("v4l2_refresh_devices")));
                    if v4l2_refresh.is_some() {
                        app.v4l2.refresh_devices();
                    }
                }

                // Handle Spout signals from UI
                #[cfg(all(target_os = "windows", feature = "spout"))]
                {
                    let spout_enable: Option<bool> = app
                        .egui_overlay
                        .context()
                        .data_mut(|d| d.remove_temp(egui::Id::new("spout_set_enabled")));
                    if let Some(enabled) = spout_enable {
                        app.spout.set_enabled(
                            enabled,
                            &app.gpu.device,
                            app.gpu.format,
                            app.gpu.surface_config.width,
                            app.gpu.surface_config.height,
                        );
                    }

                    let spout_name: Option<String> = app
                        .egui_overlay
                        .context()
                        .data_mut(|d| d.remove_temp(egui::Id::new("spout_sender_name")));
                    if let Some(name) = spout_name {
                        app.spout.config.sender_name = name;
                        app.spout.config.save();
                        app.spout.restart(
                            &app.gpu.device,
                            app.gpu.format,
                            app.gpu.surface_config.width,
                            app.gpu.surface_config.height,
                        );
                    }

                    let spout_res: Option<u8> = app
                        .egui_overlay
                        .context()
                        .data_mut(|d| d.remove_temp(egui::Id::new("spout_resolution_change")));
                    if let Some(res_u8) = spout_res {
                        let res = crate::spout::types::OutputResolution::ALL
                            .get(res_u8 as usize)
                            .copied()
                            .unwrap_or(crate::spout::types::OutputResolution::Match);
                        app.spout.config.resolution = res;
                        app.spout.config.save();
                        app.spout.restart(
                            &app.gpu.device,
                            app.gpu.format,
                            app.gpu.surface_config.width,
                            app.gpu.surface_config.height,
                        );
                    }
                }

                // Handle Syphon signals from UI
                #[cfg(all(target_os = "macos", feature = "syphon"))]
                {
                    let syphon_enable: Option<bool> = app
                        .egui_overlay
                        .context()
                        .data_mut(|d| d.remove_temp(egui::Id::new("syphon_set_enabled")));
                    if let Some(enabled) = syphon_enable {
                        app.syphon.set_enabled(
                            enabled,
                            &app.gpu.device,
                            app.gpu.format,
                            app.gpu.surface_config.width,
                            app.gpu.surface_config.height,
                        );
                    }

                    let syphon_name: Option<String> = app
                        .egui_overlay
                        .context()
                        .data_mut(|d| d.remove_temp(egui::Id::new("syphon_server_name")));
                    if let Some(name) = syphon_name {
                        app.syphon.config.server_name = name;
                        app.syphon.config.save();
                        app.syphon.restart(
                            &app.gpu.device,
                            app.gpu.format,
                            app.gpu.surface_config.width,
                            app.gpu.surface_config.height,
                        );
                    }

                    let syphon_res: Option<u8> = app
                        .egui_overlay
                        .context()
                        .data_mut(|d| d.remove_temp(egui::Id::new("syphon_resolution_change")));
                    if let Some(res_u8) = syphon_res {
                        let res = crate::syphon::types::OutputResolution::ALL
                            .get(res_u8 as usize)
                            .copied()
                            .unwrap_or(crate::syphon::types::OutputResolution::Match);
                        app.syphon.config.resolution = res;
                        app.syphon.config.save();
                        app.syphon.restart(
                            &app.gpu.device,
                            app.gpu.format,
                            app.gpu.surface_config.width,
                            app.gpu.surface_config.height,
                        );
                    }
                }

                // Handle recording signals from UI
                {
                    let rec_toggle: Option<bool> = app
                        .egui_overlay
                        .context()
                        .data_mut(|d| d.remove_temp(egui::Id::new("recording_toggle")));
                    if rec_toggle.is_some() {
                        if app.recording.is_recording() {
                            app.recording.stop();
                        } else {
                            let audio_source = if app.audio.active {
                                Some(crate::recording::encoder::AudioSource {
                                    ring: app.audio.recording_ring.clone(),
                                    sample_rate: app.audio.sample_rate,
                                })
                            } else {
                                None
                            };
                            if let Err(e) = app.recording.start(
                                &app.gpu.device,
                                app.gpu.format,
                                app.gpu.surface_config.width,
                                app.gpu.surface_config.height,
                                audio_source,
                            ) {
                                log::error!("Failed to start recording: {e}");
                                app.status_error = Some((
                                    format!("Recording failed to start: {e}"),
                                    std::time::Instant::now(),
                                ));
                            }
                        }
                    }

                    let rec_codec: Option<u8> = app
                        .egui_overlay
                        .context()
                        .data_mut(|d| d.remove_temp(egui::Id::new("rec_codec_change")));
                    if let Some(idx) = rec_codec {
                        if let Some(&codec) =
                            crate::recording::types::VideoCodec::ALL.get(idx as usize)
                        {
                            app.recording.config.codec = codec;
                            app.recording.config.save();
                        }
                    }

                    let rec_res: Option<u8> = app
                        .egui_overlay
                        .context()
                        .data_mut(|d| d.remove_temp(egui::Id::new("rec_resolution_change")));
                    if let Some(idx) = rec_res {
                        if let Some(&res) =
                            crate::gpu::types::OutputResolution::ALL.get(idx as usize)
                        {
                            app.recording.config.resolution = res;
                            app.recording.config.save();
                        }
                    }

                    let rec_fps: Option<u32> = app
                        .egui_overlay
                        .context()
                        .data_mut(|d| d.remove_temp(egui::Id::new("rec_fps_change")));
                    if let Some(fps) = rec_fps {
                        app.recording.config.fps = fps;
                        app.recording.config.save();
                    }

                    let rec_quality: Option<u32> = app
                        .egui_overlay
                        .context()
                        .data_mut(|d| d.remove_temp(egui::Id::new("rec_quality_change")));
                    if let Some(q) = rec_quality {
                        app.recording.config.quality = q;
                        app.recording.config.save();
                    }

                    let rec_container: Option<u8> = app
                        .egui_overlay
                        .context()
                        .data_mut(|d| d.remove_temp(egui::Id::new("rec_container_change")));
                    if let Some(idx) = rec_container {
                        if let Some(&cont) =
                            crate::recording::types::Container::ALL.get(idx as usize)
                        {
                            app.recording.config.container = cont;
                            app.recording.config.save();
                        }
                    }

                    let rec_hw: Option<bool> = app
                        .egui_overlay
                        .context()
                        .data_mut(|d| d.remove_temp(egui::Id::new("rec_hw_toggle")));
                    if let Some(hw) = rec_hw {
                        app.recording.config.use_hw_encoder = hw;
                        app.recording.config.save();
                    }

                    let rec_audio: Option<bool> = app
                        .egui_overlay
                        .context()
                        .data_mut(|d| d.remove_temp(egui::Id::new("rec_audio_toggle")));
                    if let Some(val) = rec_audio {
                        app.recording.config.record_audio = val;
                        app.recording.config.save();
                    }
                }

                // Handle effect loading from UI → loads on active layer
                let pending: Option<usize> = app
                    .egui_overlay
                    .context()
                    .data_mut(|d| d.remove_temp(egui::Id::new("pending_effect")));
                if let Some(idx) = pending.or(app.egui_overlay.pending_effect_load.take()) {
                    let active_locked = app.layer_stack.active().map_or(false, |l| l.locked);
                    if !active_locked {
                        app.load_effect(idx);
                        app.preset_store.mark_dirty();
                    }
                }

                // Handle preset signals from UI
                let pending_preset: Option<usize> = app
                    .egui_overlay
                    .context()
                    .data_mut(|d| d.remove_temp(egui::Id::new("pending_preset")));
                if let Some(idx) = pending_preset {
                    app.load_preset(idx);
                }
                let save_preset: Option<String> = app
                    .egui_overlay
                    .context()
                    .data_mut(|d| d.remove_temp(egui::Id::new("save_preset")));
                if let Some(name) = save_preset {
                    app.save_preset(&name);
                }
                let delete_preset: Option<usize> = app
                    .egui_overlay
                    .context()
                    .data_mut(|d| d.remove_temp(egui::Id::new("delete_preset")));
                if let Some(idx) = delete_preset {
                    if let Err(e) = app.preset_store.delete(idx) {
                        log::error!("Failed to delete preset: {e}");
                    }
                }
                let deselect_preset: Option<bool> = app
                    .egui_overlay
                    .context()
                    .data_mut(|d| d.remove_temp(egui::Id::new("deselect_preset")));
                if deselect_preset.is_some() {
                    app.preset_store.current_preset = None;
                    app.preset_store.dirty = false;
                }
                let new_preset: Option<bool> = app
                    .egui_overlay
                    .context()
                    .data_mut(|d| d.remove_temp(egui::Id::new("new_preset")));
                if new_preset.is_some() {
                    app.preset_store.current_preset = None;
                    app.preset_store.dirty = false;
                    app.clear_all_layers();
                }
                let copy_preset_index: Option<usize> = app
                    .egui_overlay
                    .context()
                    .data_mut(|d| d.remove_temp(egui::Id::new("copy_preset_index")));
                if let Some(src_idx) = copy_preset_index {
                    if let Some((src_name, _)) = app.preset_store.presets.get(src_idx) {
                        let base = format!("{} Copy", src_name);
                        // Generate unique name
                        let existing: Vec<&str> = app
                            .preset_store
                            .presets
                            .iter()
                            .map(|(n, _)| n.as_str())
                            .collect();
                        let copy_name = if !existing.contains(&base.as_str()) {
                            base.clone()
                        } else {
                            let mut n = 2;
                            loop {
                                let candidate = format!("{} {}", base, n);
                                if !existing.contains(&candidate.as_str()) {
                                    break candidate;
                                }
                                n += 1;
                            }
                        };
                        match app.preset_store.copy_preset(src_idx, &copy_name) {
                            Ok(new_idx) => {
                                app.load_preset(new_idx);
                            }
                            Err(e) => {
                                log::error!("Failed to copy preset: {e}");
                            }
                        }
                    }
                }

                // Handle scene UI signals
                let save_scene: Option<String> = app
                    .egui_overlay
                    .context()
                    .data_mut(|d| d.remove_temp(egui::Id::new("save_scene")));
                if let Some(name) = save_scene {
                    let is_new = !app.scene_store.scenes.iter().any(|(n, _)| n == &name);
                    if is_new {
                        // New scene: clear timeline so user starts with a blank cue list
                        app.timeline.cues.clear();
                        app.timeline.stop();
                        app.timeline.loop_mode = false;
                        app.timeline.advance_mode = crate::scene::types::AdvanceMode::Manual;
                    }
                    let set = crate::scene::types::SceneSet {
                        version: 1,
                        name: name.clone(),
                        cues: app.timeline.cues.clone(),
                        loop_mode: app.timeline.loop_mode,
                        advance_mode: app.timeline.advance_mode,
                    };
                    if let Err(e) = app.scene_store.save(&name, set) {
                        log::error!("Failed to save scene: {e}");
                    }
                }
                let load_scene_idx: Option<usize> = app
                    .egui_overlay
                    .context()
                    .data_mut(|d| d.remove_temp(egui::Id::new("load_scene")));
                if let Some(idx) = load_scene_idx {
                    app.load_scene(idx);
                }
                let delete_scene: Option<usize> = app
                    .egui_overlay
                    .context()
                    .data_mut(|d| d.remove_temp(egui::Id::new("delete_scene")));
                if let Some(idx) = delete_scene {
                    if let Err(e) = app.scene_store.delete(idx) {
                        log::error!("Failed to delete scene: {e}");
                    }
                }
                let scene_go_next: Option<bool> = app
                    .egui_overlay
                    .context()
                    .data_mut(|d| d.remove_temp(egui::Id::new("scene_go_next")));
                if scene_go_next.is_some() {
                    let event = app.timeline.go_next();
                    app.process_timeline_event(event);
                }
                let scene_go_prev: Option<bool> = app
                    .egui_overlay
                    .context()
                    .data_mut(|d| d.remove_temp(egui::Id::new("scene_go_prev")));
                if scene_go_prev.is_some() {
                    let event = app.timeline.go_prev();
                    app.process_timeline_event(event);
                }
                let scene_toggle_play: Option<bool> = app
                    .egui_overlay
                    .context()
                    .data_mut(|d| d.remove_temp(egui::Id::new("scene_toggle_play")));
                if scene_toggle_play.is_some() {
                    if app.timeline.active {
                        app.timeline.stop();
                    } else if !app.timeline.cues.is_empty() {
                        let event = app.timeline.start(0);
                        app.process_timeline_event(event);
                    }
                }

                // Drain scene transport triggers from binding bus
                let pending: Vec<String> = app.binding_bus.pending_triggers.drain(..).collect();
                for trigger in &pending {
                    match trigger.as_str() {
                        "scene.transport.go" => {
                            let event = app.timeline.go_next();
                            app.process_timeline_event(event);
                        }
                        "scene.transport.prev" => {
                            let event = app.timeline.go_prev();
                            app.process_timeline_event(event);
                        }
                        "scene.transport.stop" => {
                            app.timeline.stop();
                        }
                        _ => {}
                    }
                }

                let add_cue: Option<String> = app
                    .egui_overlay
                    .context()
                    .data_mut(|d| d.remove_temp(egui::Id::new("scene_add_cue")));
                let mut scene_dirty = false;
                if let Some(preset_name) = add_cue {
                    // In Timer mode, default hold_secs so the timer can advance
                    let hold_secs = if matches!(
                        app.timeline.advance_mode,
                        crate::scene::types::AdvanceMode::Timer
                    ) {
                        Some(4.0)
                    } else {
                        None
                    };
                    let cue = crate::scene::types::SceneCue {
                        preset_name,
                        transition: crate::scene::types::TransitionType::Cut,
                        transition_secs: 1.0,
                        hold_secs,
                        label: None,
                        param_overrides: Vec::new(),
                        transition_beats: None,
                    };
                    app.timeline.cues.push(cue);
                    scene_dirty = true;
                }
                let scene_jump: Option<usize> = app
                    .egui_overlay
                    .context()
                    .data_mut(|d| d.remove_temp(egui::Id::new("scene_jump_to_cue")));
                if let Some(cue_idx) = scene_jump {
                    let event = app.timeline.go_to_cue(cue_idx);
                    app.process_timeline_event(event);
                }
                let scene_loop: Option<bool> = app
                    .egui_overlay
                    .context()
                    .data_mut(|d| d.remove_temp(egui::Id::new("scene_set_loop")));
                if let Some(loop_mode) = scene_loop {
                    app.timeline.loop_mode = loop_mode;
                    scene_dirty = true;
                }
                let scene_remove_cue: Option<usize> = app
                    .egui_overlay
                    .context()
                    .data_mut(|d| d.remove_temp(egui::Id::new("scene_remove_cue")));
                if let Some(cue_idx) = scene_remove_cue {
                    if cue_idx < app.timeline.cues.len() {
                        app.timeline.cues.remove(cue_idx);
                        app.timeline.notify_cue_removed(cue_idx);
                        scene_dirty = true;
                    }
                }
                // Per-cue transition type
                let set_cue_transition: Option<(usize, crate::scene::types::TransitionType)> = app
                    .egui_overlay
                    .context()
                    .data_mut(|d| d.remove_temp(egui::Id::new("scene_set_cue_transition")));
                if let Some((idx, tt)) = set_cue_transition {
                    if let Some(cue) = app.timeline.cues.get_mut(idx) {
                        cue.transition = tt;
                        scene_dirty = true;
                    }
                }
                // Per-cue transition duration
                let set_cue_dur: Option<(usize, f32)> = app
                    .egui_overlay
                    .context()
                    .data_mut(|d| d.remove_temp(egui::Id::new("scene_set_cue_transition_secs")));
                if let Some((idx, secs)) = set_cue_dur {
                    if let Some(cue) = app.timeline.cues.get_mut(idx) {
                        cue.transition_secs = secs;
                        scene_dirty = true;
                    }
                }
                // Advance mode
                let set_advance: Option<u32> = app
                    .egui_overlay
                    .context()
                    .data_mut(|d| d.remove_temp(egui::Id::new("scene_set_advance_mode")));
                if let Some(mode_id) = set_advance {
                    app.timeline.advance_mode = match mode_id {
                        1 => {
                            // Initialize hold_secs for cues that don't have one
                            for cue in &mut app.timeline.cues {
                                if cue.hold_secs.is_none() {
                                    cue.hold_secs = Some(4.0);
                                }
                            }
                            crate::scene::types::AdvanceMode::Timer
                        }
                        2 => crate::scene::types::AdvanceMode::BeatSync { beats_per_cue: 4 },
                        _ => crate::scene::types::AdvanceMode::Manual,
                    };
                    scene_dirty = true;
                }
                // Beats per cue (BeatSync)
                let set_bpc: Option<u32> = app
                    .egui_overlay
                    .context()
                    .data_mut(|d| d.remove_temp(egui::Id::new("scene_set_beats_per_cue")));
                if let Some(bpc) = set_bpc {
                    if let crate::scene::types::AdvanceMode::BeatSync {
                        ref mut beats_per_cue,
                    } = app.timeline.advance_mode
                    {
                        *beats_per_cue = bpc;
                        scene_dirty = true;
                    }
                }
                // Per-cue hold seconds (Timer mode)
                let set_hold: Option<(usize, f32)> = app
                    .egui_overlay
                    .context()
                    .data_mut(|d| d.remove_temp(egui::Id::new("scene_set_cue_hold_secs")));
                if let Some((idx, hold)) = set_hold {
                    if let Some(cue) = app.timeline.cues.get_mut(idx) {
                        cue.hold_secs = Some(hold);
                        scene_dirty = true;
                    }
                }

                // Auto-save scene after any cue/timeline mutation
                if scene_dirty {
                    app.autosave_scene();
                }

                // Handle lattice panel signals — apply edits to the active effect's
                // LatticeSim (rebuild on grid change, reseed on request).
                {
                    use crate::ui::panels::lattice_panel::LatticeCommand;
                    let lattice_cmd: Option<LatticeCommand> = app
                        .egui_overlay
                        .context()
                        .data_mut(|d| d.remove_temp(egui::Id::new("lattice_cmd")));
                    if let Some(cmd) = lattice_cmd {
                        app.preset_store.mark_dirty();
                        let device = app.gpu.device.clone();
                        let hdr = crate::gpu::GpuContext::hdr_format();
                        if let Some(layer) = app.layer_stack.active_mut() {
                            if let Some(e) = layer.as_effect_mut() {
                                if let Some(ps) = &mut e.pass_executor.particle_system {
                                    ps.lattice_params = cmd.params;
                                    ps.init_lattice(&device, hdr);
                                    if cmd.reseed {
                                        ps.request_lattice_seed();
                                    }
                                }
                            }
                        }
                    }
                }

                // Handle helix panel signals — apply edits to the active
                // effect's HelixSim (rebuild on grid / ring-length change).
                {
                    use crate::ui::panels::helix_panel::HelixCommand;
                    let helix_cmd: Option<HelixCommand> = app
                        .egui_overlay
                        .context()
                        .data_mut(|d| d.remove_temp(egui::Id::new("helix_cmd")));
                    if let Some(cmd) = helix_cmd {
                        app.preset_store.mark_dirty();
                        let device = app.gpu.device.clone();
                        let hdr = crate::gpu::GpuContext::hdr_format();
                        if let Some(layer) = app.layer_stack.active_mut() {
                            if let Some(e) = layer.as_effect_mut() {
                                if let Some(ps) = &mut e.pass_executor.particle_system {
                                    ps.helix_params = cmd.params;
                                    ps.init_helix(&device, hdr);
                                }
                            }
                        }
                    }
                }

                // Handle obstacle panel signals
                {
                    use crate::ui::panels::obstacle_panel::ObstacleCommand;
                    #[cfg(feature = "webcam")]
                    let mut obstacle_start_webcam = false;
                    #[cfg(feature = "depth")]
                    let mut obstacle_start_depth = false;
                    let obstacle_cmd: Option<ObstacleCommand> = app
                        .egui_overlay
                        .context()
                        .data_mut(|d| d.remove_temp(egui::Id::new("obstacle_cmd")));
                    if let Some(cmd) = obstacle_cmd.filter(|c| !matches!(c, ObstacleCommand::None))
                    {
                        app.preset_store.mark_dirty();
                        if let Some(layer) = app.layer_stack.active_mut() {
                            if let Some(e) = layer.as_effect_mut() {
                                if let Some(ps) = &mut e.pass_executor.particle_system {
                                    match cmd {
                                        ObstacleCommand::SetEnabled(en) => {
                                            ps.obstacle_enabled = en;
                                        }
                                        ObstacleCommand::SetMode(mode) => {
                                            ps.obstacle_mode = mode;
                                        }
                                        ObstacleCommand::SetFit(fit) => {
                                            ps.obstacle_fit = fit;
                                        }
                                        ObstacleCommand::SetThreshold(t) => {
                                            ps.obstacle_threshold = t;
                                        }
                                        ObstacleCommand::SetElasticity(e_val) => {
                                            ps.obstacle_elasticity = e_val;
                                        }
                                        ObstacleCommand::LoadImage => {
                                            // Open file dialog for obstacle image
                                            if self.obstacle_dialog_rx.is_none() {
                                                let (tx, rx) = crossbeam_channel::bounded(1);
                                                self.obstacle_dialog_rx = Some(rx);
                                                std::thread::Builder::new()
                                                    .name("obstacle-dialog".into())
                                                    .spawn(move || {
                                                        let dialog = rfd::FileDialog::new()
                                                            .add_filter(
                                                                "Images",
                                                                &[
                                                                    "png", "jpg", "jpeg", "webp",
                                                                    "bmp",
                                                                ],
                                                            );
                                                        if let Some(path) = dialog.pick_file() {
                                                            let _ = tx.send(path);
                                                        }
                                                    })
                                                    .ok();
                                            }
                                        }
                                        ObstacleCommand::LoadVideo => {
                                            #[cfg(feature = "video")]
                                            if self.obstacle_dialog_rx.is_none() {
                                                let (tx, rx) = crossbeam_channel::bounded(1);
                                                self.obstacle_dialog_rx = Some(rx);
                                                std::thread::Builder::new()
                                                    .name("obstacle-video-dialog".into())
                                                    .spawn(move || {
                                                        let video_exts =
                                                            crate::media::decoder::VIDEO_EXTENSIONS;
                                                        let dialog = rfd::FileDialog::new()
                                                            .add_filter("Video", video_exts);
                                                        if let Some(path) = dialog.pick_file() {
                                                            let _ = tx.send(path);
                                                        }
                                                    })
                                                    .ok();
                                            }
                                        }
                                        ObstacleCommand::LoadModel => {
                                            // File dialog for a 3D-model obstacle
                                            // (.glb/.gltf mesh or .ply/.splat cloud, #1851).
                                            if self.obstacle_dialog_rx.is_none() {
                                                let (tx, rx) = crossbeam_channel::bounded(1);
                                                self.obstacle_dialog_rx = Some(rx);
                                                std::thread::Builder::new()
                                                    .name("obstacle-model-dialog".into())
                                                    .spawn(move || {
                                                        let dialog = rfd::FileDialog::new()
                                                            .add_filter(
                                                                "3D model",
                                                                &["glb", "gltf", "ply", "splat"],
                                                            );
                                                        if let Some(path) = dialog.pick_file() {
                                                            let _ = tx.send(path);
                                                        }
                                                    })
                                                    .ok();
                                            }
                                        }
                                        ObstacleCommand::UseWebcam => {
                                            // Start webcam capture if not already running
                                            #[cfg(feature = "webcam")]
                                            {
                                                obstacle_start_webcam =
                                                    app.webcam_capture.is_none();
                                                ps.obstacle_enabled = true;
                                                ps.obstacle_source = "webcam".to_string();
                                                ps.obstacle_image_path = None;
                                            }
                                        }
                                        ObstacleCommand::UseDepth => {
                                            #[cfg(feature = "depth")]
                                            {
                                                if !crate::depth::model::ort_available() {
                                                    log::error!(
                                                        "ONNX Runtime not available for depth estimation"
                                                    );
                                                } else {
                                                    obstacle_start_depth = true;
                                                    ps.obstacle_enabled = true;
                                                    ps.obstacle_source = "depth".to_string();
                                                    ps.obstacle_image_path = None;
                                                }
                                            }
                                        }
                                        ObstacleCommand::DownloadDepthModel => {
                                            #[cfg(feature = "depth")]
                                            {
                                                if app.depth_download.is_none()
                                                    || app
                                                        .depth_download
                                                        .as_ref()
                                                        .map_or(false, |p| !p.is_downloading())
                                                {
                                                    app.depth_download =
                                                        Some(crate::depth::model::download_model());
                                                    log::info!("Starting depth model download");
                                                }
                                            }
                                        }
                                        ObstacleCommand::Clear => {
                                            ps.clear_obstacle(&app.gpu.device, &app.gpu.queue);
                                            // Stop depth thread when obstacle cleared
                                            #[cfg(feature = "depth")]
                                            {
                                                app.depth_thread = None;
                                            }
                                            #[cfg(feature = "webcam")]
                                            app.cleanup_webcam_if_unused();
                                        }
                                        ObstacleCommand::SetWaterEnabled(en) => {
                                            ps.obstacle_water_enabled = en;
                                        }
                                        ObstacleCommand::SetWaterLevel(v) => {
                                            ps.obstacle_water_params.level_scale = v;
                                        }
                                        ObstacleCommand::SetWaterSource(v) => {
                                            ps.obstacle_water_params.source_rate = v;
                                        }
                                        ObstacleCommand::SetWaterDrain(v) => {
                                            ps.obstacle_water_params.drain = v;
                                        }
                                        ObstacleCommand::SetWaterFlux(v) => {
                                            ps.obstacle_water_params.flux_gain = v;
                                        }
                                        ObstacleCommand::SetModelSpin(v) => {
                                            ps.obstacle_model_spin = v;
                                        }
                                        ObstacleCommand::SetModelDisplay(v) => {
                                            ps.obstacle_model_display = v;
                                        }
                                        ObstacleCommand::SetFluidEnabled(en) => {
                                            ps.obstacle_fluid_enabled = en;
                                        }
                                        ObstacleCommand::SetFluidSpeed(v) => {
                                            ps.obstacle_fluid_params.flow_speed = v;
                                        }
                                        ObstacleCommand::SetFluidCoupling(v) => {
                                            ps.obstacle_fluid_coupling = v;
                                        }
                                        ObstacleCommand::SetFluidVorticity(v) => {
                                            ps.obstacle_fluid_params.vorticity = v;
                                        }
                                        ObstacleCommand::SetFluidViscosity(v) => {
                                            ps.obstacle_fluid_params.viscosity = v;
                                        }
                                        ObstacleCommand::SetFluidGrid(g) => {
                                            ps.obstacle_fluid_grid = g;
                                        }
                                        ObstacleCommand::None => {}
                                    }
                                }
                            }
                        }
                    }
                    // Deferred webcam/depth starts (outside mutable layer_stack borrow)
                    #[cfg(feature = "webcam")]
                    if obstacle_start_webcam {
                        if app.webcam_capture.is_none() {
                            match app.start_webcam(app.webcam_device_index, Some((1280, 720))) {
                                Ok(capture) => {
                                    app.webcam_capture = Some(capture);
                                }
                                Err(e) => {
                                    log::error!("Failed to start webcam for obstacle: {e}");
                                }
                            }
                        }
                    }
                    #[cfg(feature = "depth")]
                    if obstacle_start_depth {
                        #[cfg(feature = "webcam")]
                        if app.webcam_capture.is_none() {
                            match app.start_webcam(app.webcam_device_index, Some((1280, 720))) {
                                Ok(capture) => {
                                    app.webcam_capture = Some(capture);
                                }
                                Err(e) => {
                                    log::error!("Failed to start webcam for depth obstacle: {e}");
                                }
                            }
                        }
                        if app.depth_thread.is_none() {
                            let model_path = crate::depth::model::model_path();
                            match crate::depth::thread::DepthThread::start(model_path) {
                                Ok(dt) => {
                                    app.depth_thread = Some(dt);
                                    log::info!("Depth estimation thread started");
                                }
                                Err(e) => {
                                    log::error!("Failed to start depth thread: {e}");
                                }
                            }
                        }
                    }
                }

                // Handle obstacle webcam device switch
                #[cfg(feature = "webcam")]
                {
                    let switch_obs_device: Option<u32> = app.egui_overlay.context().data_mut(|d| {
                        d.remove_temp(egui::Id::new("switch_obstacle_webcam_device"))
                    });
                    if let Some(new_idx) = switch_obs_device {
                        let old_idx = app.webcam_device_index;
                        app.webcam_capture = None;
                        match app.start_webcam(new_idx, Some((1280, 720))) {
                            Ok(capture) => {
                                app.webcam_capture = Some(capture);
                                app.webcam_device_index = new_idx;
                                app.settings.webcam_device = Some(new_idx);
                                app.settings.save();
                            }
                            Err(e) => {
                                log::error!("Failed to switch obstacle webcam device: {e}");
                                app.status_error = Some((
                                    format!("Camera failed: {e}"),
                                    std::time::Instant::now(),
                                ));
                                // Restore previous capture
                                match app.start_webcam(old_idx, Some((1280, 720))) {
                                    Ok(capture) => {
                                        app.webcam_capture = Some(capture);
                                    }
                                    Err(e2) => {
                                        log::error!("Failed to restore previous webcam: {e2}");
                                    }
                                }
                            }
                        }
                    }
                }

                // Drain obstacle file dialog result (non-blocking)
                if let Some(ref rx) = self.obstacle_dialog_rx {
                    if let Ok(path) = rx.try_recv() {
                        self.obstacle_dialog_rx = None;
                        let ext = path
                            .extension()
                            .map(|e| e.to_string_lossy().to_lowercase())
                            .unwrap_or_default();
                        let is_model = matches!(ext.as_str(), "glb" | "gltf" | "ply" | "splat");
                        let is_video = {
                            #[cfg(feature = "video")]
                            {
                                crate::media::decoder::VIDEO_EXTENSIONS.contains(&ext.as_str())
                            }
                            #[cfg(not(feature = "video"))]
                            {
                                let _ = &ext;
                                false
                            }
                        };
                        if is_model {
                            let path_str = path.to_string_lossy().to_string();
                            if let Some(layer) = app.layer_stack.active_mut() {
                                if let Some(e) = layer.as_effect_mut() {
                                    if let Some(ps) = &mut e.pass_executor.particle_system {
                                        if let Err(err) =
                                            ps.set_obstacle_model(&app.gpu.device, &path)
                                        {
                                            log::error!("Failed to load obstacle model: {err}");
                                            app.status_error = Some((
                                                format!("Model load failed: {err}"),
                                                std::time::Instant::now(),
                                            ));
                                        } else {
                                            log::info!("Loaded obstacle model: {path_str}");
                                        }
                                    }
                                }
                            }
                        } else if is_video {
                            #[cfg(feature = "video")]
                            {
                                match crate::media::video::probe_video(&path) {
                                    Ok(meta) => {
                                        match crate::media::video::decode_all_frames(&path, &meta) {
                                            Ok((frames, delays_ms)) => {
                                                let path_str = path.to_string_lossy().to_string();
                                                if let Some(layer) = app.layer_stack.active_mut() {
                                                    if let Some(e) = layer.as_effect_mut() {
                                                        if let Some(ps) =
                                                            &mut e.pass_executor.particle_system
                                                        {
                                                            ps.set_obstacle_video(
                                                                &app.gpu.device,
                                                                &app.gpu.queue,
                                                                frames,
                                                                delays_ms,
                                                                path_str,
                                                            );
                                                        }
                                                    }
                                                }
                                            }
                                            Err(e) => {
                                                log::error!("Failed to decode obstacle video: {e}");
                                            }
                                        }
                                    }
                                    Err(e) => log::error!("Failed to probe obstacle video: {e}"),
                                }
                            }
                        } else {
                            match image::open(&path) {
                                Ok(img) => {
                                    let rgba = img.to_rgba8();
                                    let (w, h) = rgba.dimensions();
                                    let path_str = path.to_string_lossy().to_string();
                                    if let Some(layer) = app.layer_stack.active_mut() {
                                        if let Some(e) = layer.as_effect_mut() {
                                            if let Some(ps) = &mut e.pass_executor.particle_system {
                                                ps.set_obstacle_image(
                                                    &app.gpu.device,
                                                    &app.gpu.queue,
                                                    &rgba,
                                                    w,
                                                    h,
                                                    Some(path_str),
                                                );
                                            }
                                        }
                                    }
                                }
                                Err(e) => log::error!("Failed to load obstacle image: {e}"),
                            }
                        }
                        // Stop depth thread + webcam if switching away from depth/webcam source
                        #[cfg(feature = "depth")]
                        {
                            app.depth_thread = None;
                        }
                        #[cfg(feature = "webcam")]
                        app.cleanup_webcam_if_unused();
                        app.preset_store.mark_dirty();
                    }
                }

                // Handle media layer signals
                let add_media: Option<bool> = app
                    .egui_overlay
                    .context()
                    .data_mut(|d| d.remove_temp(egui::Id::new("add_media_layer")));
                if add_media.is_some() && self.file_dialog_rx.is_none() {
                    let (tx, rx) = crossbeam_channel::bounded(1);
                    self.file_dialog_rx = Some(rx);
                    std::thread::Builder::new()
                        .name("file-dialog".into())
                        .spawn(move || {
                            #[allow(unused_mut)]
                            let mut dialog = rfd::FileDialog::new();
                            #[cfg(feature = "video")]
                            {
                                if crate::media::video::ffmpeg_available() {
                                    let image_exts: &[&str] =
                                        &["png", "jpg", "jpeg", "gif", "bmp", "webp"];
                                    let video_exts = crate::media::decoder::VIDEO_EXTENSIONS;
                                    let all: Vec<&str> = image_exts
                                        .iter()
                                        .copied()
                                        .chain(video_exts.iter().copied())
                                        .collect();
                                    dialog = dialog
                                        .add_filter("All Media", &all)
                                        .add_filter("Images", image_exts)
                                        .add_filter("Video", video_exts);
                                } else {
                                    dialog = dialog.add_filter(
                                        "Images",
                                        &["png", "jpg", "jpeg", "gif", "bmp", "webp"],
                                    );
                                }
                            }
                            #[cfg(not(feature = "video"))]
                            {
                                dialog = dialog.add_filter(
                                    "Images",
                                    &["png", "jpg", "jpeg", "gif", "bmp", "webp"],
                                );
                            }
                            if let Some(path) = dialog.pick_file() {
                                let _ = tx.send(path);
                            }
                        })
                        .ok();
                }

                // Drain file dialog result (non-blocking)
                if let Some(ref rx) = self.file_dialog_rx {
                    match rx.try_recv() {
                        Ok(path) => {
                            app.add_media_layer(path);
                            app.preset_store.mark_dirty();
                            self.file_dialog_rx = None;
                        }
                        Err(crossbeam_channel::TryRecvError::Disconnected) => {
                            // Dialog was cancelled (sender dropped without sending)
                            self.file_dialog_rx = None;
                        }
                        Err(crossbeam_channel::TryRecvError::Empty) => {
                            // Still open, keep waiting
                        }
                    }
                }

                // Handle webcam layer signals
                #[cfg(feature = "webcam")]
                {
                    // Store default device index in egui temp data for layer panel
                    app.egui_overlay.context().data_mut(|d| {
                        d.insert_temp(
                            egui::Id::new("webcam_default_device"),
                            app.webcam_device_index,
                        );
                    });

                    let add_webcam: Option<u32> = app
                        .egui_overlay
                        .context()
                        .data_mut(|d| d.remove_temp(egui::Id::new("add_webcam_layer")));
                    if let Some(device_idx) = add_webcam {
                        app.webcam_device_index = device_idx;
                        app.add_webcam_layer(device_idx);
                        app.preset_store.mark_dirty();
                    }

                    // Switch webcam device for active webcam layer
                    let switch_device: Option<u32> = app
                        .egui_overlay
                        .context()
                        .data_mut(|d| d.remove_temp(egui::Id::new("switch_webcam_device")));
                    if let Some(new_idx) = switch_device {
                        let old_idx = app.webcam_device_index;
                        // Stop old capture first (release device)
                        app.webcam_capture = None;
                        match app.start_webcam(new_idx, Some((1280, 720))) {
                            Ok(capture) => {
                                let (w, h) = capture.resolution();
                                let device_name = capture.device_name().to_string();
                                app.webcam_capture = Some(capture);
                                app.webcam_device_index = new_idx;
                                app.settings.webcam_device = Some(new_idx);
                                app.settings.save();
                                // Update active webcam layer
                                if let Some(layer) = app.layer_stack.active_mut() {
                                    if let Some(m) = layer.as_media_mut() {
                                        if m.is_live() {
                                            m.file_name = device_name;
                                            m.media_width = w;
                                            m.media_height = h;
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                log::error!("Failed to switch webcam device: {e}");
                                app.status_error = Some((
                                    format!("Camera failed: {e}"),
                                    std::time::Instant::now(),
                                ));
                                // Restore previous capture
                                match app.start_webcam(old_idx, Some((1280, 720))) {
                                    Ok(capture) => {
                                        app.webcam_capture = Some(capture);
                                    }
                                    Err(e2) => {
                                        log::error!("Failed to restore previous webcam: {e2}");
                                    }
                                }
                            }
                        }
                    }

                    let webcam_mirror: Option<bool> = app
                        .egui_overlay
                        .context()
                        .data_mut(|d| d.remove_temp(egui::Id::new("webcam_mirror")));
                    if let Some(mirror) = webcam_mirror {
                        if let Some(layer) = app.layer_stack.active_mut() {
                            if let Some(m) = layer.as_media_mut() {
                                m.set_mirror(&app.gpu.queue, mirror);
                            }
                        }
                    }

                    let webcam_disconnect: Option<bool> = app
                        .egui_overlay
                        .context()
                        .data_mut(|d| d.remove_temp(egui::Id::new("webcam_disconnect")));
                    if webcam_disconnect.is_some() {
                        // Stop capture and remove the active webcam layer
                        app.webcam_capture = None;
                        let active = app.layer_stack.active_layer;
                        app.remove_layer(active);
                        app.sync_active_layer();
                        app.preset_store.mark_dirty();
                    }
                }

                // Handle media transport signals
                let play_pause: Option<bool> = app
                    .egui_overlay
                    .context()
                    .data_mut(|d| d.remove_temp(egui::Id::new("media_play_pause")));
                if play_pause.is_some() {
                    if let Some(layer) = app.layer_stack.active_mut() {
                        if let Some(m) = layer.as_media_mut() {
                            m.transport.playing = !m.transport.playing;
                        }
                    }
                }
                let media_loop: Option<bool> = app
                    .egui_overlay
                    .context()
                    .data_mut(|d| d.remove_temp(egui::Id::new("media_loop")));
                if let Some(looping) = media_loop {
                    if let Some(layer) = app.layer_stack.active_mut() {
                        if let Some(m) = layer.as_media_mut() {
                            m.transport.looping = looping;
                        }
                    }
                }
                let media_speed: Option<f32> = app
                    .egui_overlay
                    .context()
                    .data_mut(|d| d.remove_temp(egui::Id::new("media_speed")));
                if let Some(speed) = media_speed {
                    if let Some(layer) = app.layer_stack.active_mut() {
                        if let Some(m) = layer.as_media_mut() {
                            m.transport.speed = speed;
                        }
                    }
                }
                let media_direction: Option<u8> = app
                    .egui_overlay
                    .context()
                    .data_mut(|d| d.remove_temp(egui::Id::new("media_direction")));
                if let Some(dir) = media_direction {
                    if let Some(layer) = app.layer_stack.active_mut() {
                        if let Some(m) = layer.as_media_mut() {
                            m.transport.direction = match dir {
                                0 => crate::media::types::PlayDirection::Forward,
                                1 => crate::media::types::PlayDirection::Reverse,
                                2 => crate::media::types::PlayDirection::PingPong,
                                _ => crate::media::types::PlayDirection::Forward,
                            };
                        }
                    }
                }

                // Handle media seek signal (video scrubber)
                let media_seek: Option<f64> = app
                    .egui_overlay
                    .context()
                    .data_mut(|d| d.remove_temp(egui::Id::new("media_seek")));
                if let Some(secs) = media_seek {
                    if let Some(layer) = app.layer_stack.active_mut() {
                        if let Some(m) = layer.as_media_mut() {
                            m.seek_to_secs(secs);
                        }
                    }
                }

                // Handle particle panel signals
                {
                    let ctx = app.egui_overlay.context();
                    let emit_rate: Option<f32> =
                        ctx.data_mut(|d| d.remove_temp(egui::Id::new("particle_emit_rate")));
                    let burst: Option<u32> =
                        ctx.data_mut(|d| d.remove_temp(egui::Id::new("particle_burst")));
                    let lifetime: Option<f32> =
                        ctx.data_mut(|d| d.remove_temp(egui::Id::new("particle_lifetime")));
                    let speed: Option<f32> =
                        ctx.data_mut(|d| d.remove_temp(egui::Id::new("particle_speed")));
                    let size: Option<f32> =
                        ctx.data_mut(|d| d.remove_temp(egui::Id::new("particle_size")));
                    let drag: Option<f32> =
                        ctx.data_mut(|d| d.remove_temp(egui::Id::new("particle_drag")));
                    let trail_length: Option<u32> =
                        ctx.data_mut(|d| d.remove_temp(egui::Id::new("particle_trail_length")));

                    let mut particle_save_info: Option<(usize, gpu::particle::types::ParticleDef)> =
                        None;
                    if emit_rate.is_some()
                        || burst.is_some()
                        || lifetime.is_some()
                        || speed.is_some()
                        || size.is_some()
                        || drag.is_some()
                        || trail_length.is_some()
                    {
                        // Panel edit → preset is now unsaved (particle-sim edits
                        // round-trip via LayerPreset.particle_sim).
                        app.preset_store.mark_dirty();
                        // Needed only by the trail-length branch below, which
                        // reallocates the trail buffer.
                        let device = app.gpu.device.clone();
                        let hdr = crate::gpu::GpuContext::hdr_format();
                        if let Some(layer) = app.layer_stack.active_mut() {
                            if let Some(effect) = layer.as_effect_mut() {
                                let eidx = effect.effect_index;
                                if let Some(ps) = effect.pass_executor.particle_system.as_mut() {
                                    if let Some(v) = emit_rate {
                                        ps.emit_rate = v;
                                        ps.def.emit_rate = v;
                                    }
                                    if let Some(v) = burst {
                                        ps.burst_on_beat = v;
                                        ps.def.burst_on_beat = v;
                                    }
                                    if let Some(v) = lifetime {
                                        ps.def.lifetime = v;
                                    }
                                    if let Some(v) = speed {
                                        ps.def.initial_speed = v;
                                    }
                                    if let Some(v) = size {
                                        ps.def.initial_size = v;
                                    }
                                    if let Some(v) = drag {
                                        ps.def.drag = v;
                                    }
                                    if let Some(v) = trail_length {
                                        // Keep def in sync (drives disk-save +
                                        // preset capture) and reallocate the trail
                                        // buffer; the per-frame uniform sync then
                                        // shows the new length live.
                                        ps.def.trail_length = v;
                                        ps.set_trail_length(&device, hdr, v);
                                    }
                                    if let Some(idx) = eidx {
                                        particle_save_info = Some((idx, ps.def.clone()));
                                    }
                                }
                            }
                        }
                    }

                    // Persist particle changes to disk for user effects only.
                    // Built-in effects are runtime-only; users should create a
                    // preset or copy the effect to persist changes.
                    if let Some((idx, updated_def)) = particle_save_info {
                        // Capture failures locally; can't touch app.status_error
                        // while effect_loader is mutably borrowed.
                        let mut save_err: Option<String> = None;
                        if let Some(effect) = app.effect_loader.effects.get_mut(idx) {
                            if !EffectLoader::is_builtin(effect) {
                                effect.particles = Some(updated_def);
                                if let Some(ref path) = effect.source_path {
                                    if let Ok(json) = serde_json::to_string_pretty(effect) {
                                        if let Err(e) = std::fs::write(path, json) {
                                            save_err = Some(e.to_string());
                                        }
                                    }
                                }
                            }
                        }
                        if let Some(e) = save_err {
                            log::error!("Effect save failed: {e}");
                            app.status_error = Some((
                                format!("Effect save failed: {e}"),
                                std::time::Instant::now(),
                            ));
                        }
                    }
                }

                // Handle particle source change signals
                {
                    let ctx = app.egui_overlay.context();
                    // The layer asking is the one live NOW, not whichever is active
                    // when the dialog returns (#2012).
                    let base_request = crate::gpu::particle::SourceRequest {
                        layer_idx: app.layer_stack.active_layer,
                        as_morph_target: false,
                        morph_slot: None,
                    };

                    // Select built-in raster image
                    let select_builtin: Option<String> =
                        ctx.data_mut(|d| d.remove_temp(egui::Id::new("particle_select_builtin")));
                    if let Some(name) = select_builtin {
                        if !app.particle_source_loader.loading {
                            let path = crate::gpu::particle::builtin_raster_path(&name);
                            app.particle_source_loader.load_image(path, base_request);
                            app.preset_store.mark_dirty();
                        }
                    }

                    // Load image as particle source (dialog + decode on background thread)
                    let load_image: Option<bool> =
                        ctx.data_mut(|d| d.remove_temp(egui::Id::new("particle_load_image")));
                    if load_image.is_some() && !app.particle_source_loader.loading {
                        app.particle_source_loader.open_image_dialog(base_request);
                        app.preset_store.mark_dirty();
                    }

                    // Load video as particle source (dialog + decode on background thread)
                    #[cfg(feature = "video")]
                    {
                        let load_video: Option<bool> =
                            ctx.data_mut(|d| d.remove_temp(egui::Id::new("particle_load_video")));
                        if load_video.is_some() && !app.particle_source_loader.loading {
                            app.particle_source_loader.open_video_dialog(base_request);
                            app.preset_store.mark_dirty();
                        }
                    }

                    // Load a 3D model as particle source (#1993). Dialog only on the
                    // background thread; the raster happens where the device lives.
                    let load_model: Option<bool> =
                        ctx.data_mut(|d| d.remove_temp(egui::Id::new("particle_load_model")));
                    if load_model.is_some() && !app.particle_source_loader.loading {
                        app.particle_source_loader.open_model_dialog(base_request);
                        app.preset_store.mark_dirty();
                    }

                    // Model pose changed — re-raster and re-sample the live model.
                    // The panel only emits this on slider RELEASE, so this runs once
                    // per adjustment rather than once per drag frame.
                    let model_pose: Option<[f32; 9]> =
                        ctx.data_mut(|d| d.remove_temp(egui::Id::new("particle_model_pose")));
                    if let Some(pose) = model_pose {
                        let def = crate::gpu::particle::types::ModelSampleDef {
                            yaw_degrees: pose[0],
                            pitch_degrees: pose[1],
                            scale: pose[2],
                            ambient: pose[3],
                            light_mix: pose[4],
                            light_x: pose[5],
                            light_y: pose[6],
                            light_z: pose[7],
                            ray_strength: pose[8],
                        };
                        let (device, queue) = (&app.gpu.device, &app.gpu.queue);
                        if let Some(ps) = app
                            .layer_stack
                            .active_mut()
                            .and_then(|l| l.as_effect_mut())
                            .and_then(|e| e.pass_executor.particle_system.as_mut())
                        {
                            // Only a live model source re-samples on a pose change. A
                            // stale path here is what put the model back over a picture
                            // the user had just loaded (#2011).
                            if let crate::gpu::particle::ParticleSource::Model { path } = &ps.source
                            {
                                let path = path.clone();
                                match ps.apply_model_source(
                                    device,
                                    queue,
                                    std::path::Path::new(&path),
                                    &def,
                                ) {
                                    Ok(n) => {
                                        log::info!("Re-sampled model at new pose: {n} particles");
                                    }
                                    Err(e) => log::warn!("Model re-sample failed: {e}"),
                                }
                            }
                        }
                        app.preset_store.mark_dirty();
                    }

                    // Splat scene picker + demo download (#1800)
                    {
                        let load_scene: Option<bool> =
                            ctx.data_mut(|d| d.remove_temp(egui::Id::new("splat_load_scene")));
                        if load_scene.is_some() && !app.splat_loader.loading {
                            // The runtime override lives in ParticleSystem
                            // state (the .pfx stays read-only); the load
                            // needs the active layer's splat transform.
                            let job = app
                                .layer_stack
                                .active()
                                .and_then(|l| l.as_effect())
                                .and_then(|e| e.pass_executor.particle_system.as_ref())
                                .and_then(|ps| {
                                    ps.def.splat.as_ref().map(|s| {
                                        (
                                            ps.max_particles,
                                            crate::gpu::particle::splat_source::SceneOptions::from(
                                                s,
                                            ),
                                        )
                                    })
                                });
                            if let Some((target, opts)) = job {
                                app.splat_loader.open_dialog(
                                    target,
                                    opts,
                                    app.layer_stack.active_layer,
                                );
                                app.preset_store.mark_dirty();
                            }
                        }

                        let dl_demo: Option<bool> =
                            ctx.data_mut(|d| d.remove_temp(egui::Id::new("splat_download_demo")));
                        if dl_demo.is_some() && app.splat_demo_download.is_none() {
                            app.splat_demo_download = Some(
                                crate::gpu::particle::splat_source::download_demo_scene("default"),
                            );
                        }
                    }

                    // Poll the demo download: on completion, load the cached
                    // scene onto the active splat layer.
                    if let Some(dl) = app.splat_demo_download.clone() {
                        if dl.is_complete() {
                            app.splat_demo_download = None;
                            let job = app
                                .layer_stack
                                .active()
                                .and_then(|l| l.as_effect())
                                .and_then(|e| e.pass_executor.particle_system.as_ref())
                                .and_then(|ps| {
                                    ps.def.splat.as_ref().map(|s| {
                                        (
                                            ps.max_particles,
                                            crate::gpu::particle::splat_source::SceneOptions::from(
                                                s,
                                            ),
                                        )
                                    })
                                });
                            if let (Ok(path), Some((target, opts))) = (
                                crate::gpu::particle::splat_source::resolve_source("demo:default"),
                                job,
                            ) {
                                app.splat_loader.load(
                                    path,
                                    target,
                                    opts,
                                    app.layer_stack.active_layer,
                                );
                            }
                        } else if dl.is_error() {
                            app.splat_demo_download = None;
                            let msg = dl
                                .error_message
                                .lock()
                                .ok()
                                .and_then(|m| m.clone())
                                .unwrap_or_else(|| "demo download failed".to_string());
                            log::error!("Splat demo download failed: {msg}");
                            app.splat_loader.last_error = Some(msg);
                        }
                    }

                    // Set webcam as particle source (instant — no decode needed)
                    #[cfg(feature = "webcam")]
                    {
                        let use_webcam: Option<bool> =
                            ctx.data_mut(|d| d.remove_temp(egui::Id::new("particle_webcam")));
                        if use_webcam.is_some() {
                            if app.webcam_capture.is_none() {
                                match app.start_webcam(app.webcam_device_index, Some((1280, 720))) {
                                    Ok(capture) => {
                                        app.webcam_capture = Some(capture);
                                    }
                                    Err(e) => {
                                        log::error!("Failed to start webcam: {e}");
                                    }
                                }
                            }
                            if let Some(ref capture) = app.webcam_capture {
                                let (w, h) = capture.resolution();
                                if let Some(layer) = app.layer_stack.active_mut() {
                                    if let Some(effect) = layer.as_effect_mut() {
                                        if let Some(ps) =
                                            effect.pass_executor.particle_system.as_mut()
                                        {
                                            ps.set_webcam_source(&app.gpu.queue, w, h);
                                        }
                                    }
                                }
                            }
                            app.preset_store.mark_dirty();
                        }
                    }

                    // Video transport controls
                    #[cfg(feature = "video")]
                    {
                        let playing: Option<bool> = ctx
                            .data_mut(|d| d.remove_temp(egui::Id::new("particle_video_playing")));
                        let looping: Option<bool> = ctx
                            .data_mut(|d| d.remove_temp(egui::Id::new("particle_video_looping")));
                        let speed: Option<f32> =
                            ctx.data_mut(|d| d.remove_temp(egui::Id::new("particle_video_speed")));
                        let seek: Option<f64> =
                            ctx.data_mut(|d| d.remove_temp(egui::Id::new("particle_video_seek")));
                        if playing.is_some()
                            || looping.is_some()
                            || speed.is_some()
                            || seek.is_some()
                        {
                            if let Some(layer) = app.layer_stack.active_mut() {
                                if let Some(effect) = layer.as_effect_mut() {
                                    if let Some(ps) = effect.pass_executor.particle_system.as_mut()
                                    {
                                        if let Some(playback) = ps.source.playback_mut() {
                                            if let Some(v) = playing {
                                                playback.playing = v;
                                            }
                                            if let Some(v) = looping {
                                                playback.looping = v;
                                            }
                                            if let Some(v) = speed {
                                                playback.speed = v;
                                            }
                                        }
                                        if let Some(v) = seek {
                                            ps.source.seek_to_secs(v);
                                        }
                                    }
                                }
                            }
                            if looping.is_some() || speed.is_some() {
                                app.preset_store.mark_dirty();
                            }
                        }
                    }

                    // Morph controls
                    {
                        let morph_trigger: Option<u32> =
                            ctx.data_mut(|d| d.remove_temp(egui::Id::new("morph_trigger_target")));
                        let morph_cycle: Option<u32> =
                            ctx.data_mut(|d| d.remove_temp(egui::Id::new("morph_auto_cycle")));
                        let morph_hold: Option<f32> =
                            ctx.data_mut(|d| d.remove_temp(egui::Id::new("morph_hold_duration")));
                        let morph_style: Option<u32> =
                            ctx.data_mut(|d| d.remove_temp(egui::Id::new("morph_style")));
                        let morph_load_img: Option<bool> =
                            ctx.data_mut(|d| d.remove_temp(egui::Id::new("morph_load_image")));
                        let morph_add_geo: Option<String> =
                            ctx.data_mut(|d| d.remove_temp(egui::Id::new("morph_add_geometry")));
                        let morph_add_text: Option<String> =
                            ctx.data_mut(|d| d.remove_temp(egui::Id::new("morph_add_text")));
                        let morph_load_video: Option<bool> =
                            ctx.data_mut(|d| d.remove_temp(egui::Id::new("morph_load_video")));
                        let morph_load_model: Option<bool> =
                            ctx.data_mut(|d| d.remove_temp(egui::Id::new("morph_load_model")));
                        let morph_snapshot: Option<bool> =
                            ctx.data_mut(|d| d.remove_temp(egui::Id::new("morph_snapshot")));
                        let morph_clear_slot: Option<u32> =
                            ctx.data_mut(|d| d.remove_temp(egui::Id::new("morph_clear_slot")));
                        let morph_manual_blend: Option<f32> =
                            ctx.data_mut(|d| d.remove_temp(egui::Id::new("morph_manual_blend")));
                        let morph_set_source: Option<u32> =
                            ctx.data_mut(|d| d.remove_temp(egui::Id::new("morph_set_source")));
                        // Read the selected slot for targeting (don't remove — UI manages it)
                        let morph_selected_slot: Option<u32> =
                            ctx.data(|d| d.get_temp(egui::Id::new("morph_selected_slot")));

                        if morph_trigger.is_some()
                            || morph_cycle.is_some()
                            || morph_hold.is_some()
                            || morph_style.is_some()
                            || morph_load_img.is_some()
                            || morph_add_geo.is_some()
                            || morph_add_text.is_some()
                            || morph_load_video.is_some()
                            || morph_load_model.is_some()
                            || morph_snapshot.is_some()
                            || morph_clear_slot.is_some()
                            || morph_manual_blend.is_some()
                            || morph_set_source.is_some()
                        {
                            if let Some(layer) = app.layer_stack.active_mut() {
                                if let Some(effect) = layer.as_effect_mut() {
                                    if let Some(ps) = effect.pass_executor.particle_system.as_mut()
                                    {
                                        let needs_upload = if let Some(ref mut morph) =
                                            ps.morph_state
                                        {
                                            if let Some(target) = morph_trigger {
                                                morph.trigger_morph(target);
                                            }
                                            if let Some(mode) = morph_cycle {
                                                morph.auto_cycle = match mode {
                                                    0 => crate::gpu::particle::morph::AutoCycle::Off,
                                                    1 => crate::gpu::particle::morph::AutoCycle::OnBeat,
                                                    _ => crate::gpu::particle::morph::AutoCycle::Timed(4.0),
                                                };
                                            }
                                            if let Some(hold) = morph_hold {
                                                morph.hold_duration = hold;
                                            }
                                            if let Some(style) = morph_style {
                                                morph.transition_style = style;
                                            }
                                            if let Some(src) = morph_set_source {
                                                morph.source_index =
                                                    src.min(morph.target_count.saturating_sub(1));
                                            }
                                            if let Some(progress) = morph_manual_blend {
                                                // Manual scrub: set progress directly, mark transitioning so shader interpolates
                                                morph.progress = progress;
                                                morph.transitioning = progress < 1.0;
                                                // Reset hold timer so auto-cycle doesn't immediately fire
                                                morph.hold_timer = 0.0;
                                            }
                                            // Helper: pick target slot — selected slot, or next empty (based on def count), or last
                                            let def_count = ps
                                                .def
                                                .morph_targets
                                                .as_ref()
                                                .map_or(0, |t| t.len() as u32);
                                            let pick_slot = || -> u32 {
                                                if let Some(s) = morph_selected_slot {
                                                    s.min(3)
                                                } else {
                                                    def_count.min(3)
                                                }
                                            };
                                            let mut needs_upload = false;

                                            // Clear slot — shift data + labels to fill the gap
                                            if let Some(clear) = morph_clear_slot {
                                                let slot = clear.min(3);
                                                morph.remove_target(slot);
                                                if let Some(ref mut targets) = ps.def.morph_targets
                                                {
                                                    if (slot as usize) < targets.len() {
                                                        targets.remove(slot as usize);
                                                    }
                                                }
                                                needs_upload = true;
                                            }

                                            if let Some(shape) = morph_add_geo {
                                                let slot = pick_slot();
                                                let data =
                                                    crate::gpu::particle::morph::generate_geometry(
                                                        &shape,
                                                        ps.max_particles,
                                                        ps.def.initial_size,
                                                    );
                                                morph.load_target(slot, data);
                                                let def =
                                                    crate::gpu::particle::types::MorphTargetDef {
                                                        source: format!("geometry:{}", shape),
                                                        color: None,
                                                    };
                                                if let Some(ref mut targets) = ps.def.morph_targets
                                                {
                                                    while targets.len() <= slot as usize {
                                                        targets.push(crate::gpu::particle::types::MorphTargetDef {
                                                            source: String::new(), color: None,
                                                        });
                                                    }
                                                    targets[slot as usize] = def;
                                                }
                                                needs_upload = true;
                                            }
                                            if let Some(ref text) = morph_add_text {
                                                let slot = pick_slot();
                                                let data = crate::gpu::particle::text_source::render_text_to_particles(
                                                    text,
                                                    ps.max_particles,
                                                    ps.def.initial_size,
                                                );
                                                if !data.is_empty() {
                                                    morph.load_target(slot, data);
                                                    let def = crate::gpu::particle::types::MorphTargetDef {
                                                        source: format!("text:{}", text),
                                                        color: None,
                                                    };
                                                    if let Some(ref mut targets) =
                                                        ps.def.morph_targets
                                                    {
                                                        while targets.len() <= slot as usize {
                                                            targets.push(crate::gpu::particle::types::MorphTargetDef {
                                                                source: String::new(), color: None,
                                                            });
                                                        }
                                                        targets[slot as usize] = def;
                                                    }
                                                    log::info!(
                                                        "Loaded morph text target into slot {}: \"{}\"",
                                                        slot,
                                                        text
                                                    );
                                                    needs_upload = true;
                                                }
                                            }
                                            needs_upload
                                        } else {
                                            false
                                        };
                                        if needs_upload {
                                            ps.upload_morph_targets(
                                                &app.gpu.device,
                                                &app.gpu.queue,
                                            );
                                            ctx.data_mut(|d| {
                                                d.remove_temp::<u32>(egui::Id::new(
                                                    "morph_selected_slot",
                                                ))
                                            });
                                        }
                                        // Snapshot needs &self on ps, so do it outside the morph borrow
                                        if morph_snapshot.is_some() {
                                            let slot = if let Some(s) = morph_selected_slot {
                                                s.min(3)
                                            } else {
                                                // Use def count to stay in sync with morph_targets vec
                                                (ps.def
                                                    .morph_targets
                                                    .as_ref()
                                                    .map_or(0, |t| t.len())
                                                    as u32)
                                                    .min(3)
                                            };
                                            let data = ps.snapshot_particles(
                                                &app.gpu.device,
                                                &app.gpu.queue,
                                            );
                                            if !data.is_empty() {
                                                if let Some(ref mut morph) = ps.morph_state {
                                                    morph.load_target(slot, data);
                                                }
                                                let def =
                                                    crate::gpu::particle::types::MorphTargetDef {
                                                        source: "snapshot".to_string(),
                                                        color: None,
                                                    };
                                                if let Some(ref mut targets) = ps.def.morph_targets
                                                {
                                                    while targets.len() <= slot as usize {
                                                        targets.push(crate::gpu::particle::types::MorphTargetDef {
                                                            source: String::new(), color: None,
                                                        });
                                                    }
                                                    targets[slot as usize] = def;
                                                }
                                                ps.upload_morph_targets(
                                                    &app.gpu.device,
                                                    &app.gpu.queue,
                                                );
                                                log::info!(
                                                    "Loaded morph snapshot into slot {}",
                                                    slot
                                                );
                                            }
                                            ctx.data_mut(|d| {
                                                d.remove_temp::<u32>(egui::Id::new(
                                                    "morph_selected_slot",
                                                ))
                                            });
                                        }
                                        // "Which layer, which slot, is this a morph
                                        // target" all ride with the request now, so a
                                        // layer or slot change while the dialog is open
                                        // cannot redirect the result (#2012).
                                        let morph_request = crate::gpu::particle::SourceRequest {
                                            layer_idx: app.layer_stack.active_layer,
                                            as_morph_target: true,
                                            morph_slot: morph_selected_slot,
                                        };
                                        let clear_slot = || {
                                            ctx.data_mut(|d| {
                                                d.remove_temp::<u32>(egui::Id::new(
                                                    "morph_selected_slot",
                                                ))
                                            });
                                        };
                                        if morph_load_img.is_some() {
                                            clear_slot();
                                            app.particle_source_loader
                                                .open_image_dialog(morph_request);
                                        }
                                        #[cfg(feature = "video")]
                                        if morph_load_video.is_some() {
                                            clear_slot();
                                            app.particle_source_loader
                                                .open_video_dialog(morph_request);
                                        }
                                        if morph_load_model.is_some() {
                                            clear_slot();
                                            app.particle_source_loader
                                                .open_model_dialog(morph_request);
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // Drain background splat scene loader results (#1800).
                    // Targets the layer the load was requested for (not the
                    // active layer); a stale result — layer swapped to a
                    // non-splat effect meanwhile — is dropped harmlessly.
                    if let Some(result) = app.splat_loader.try_recv() {
                        match result {
                            crate::gpu::particle::SplatLoadResult::Loaded { layer_idx, cloud } => {
                                let ps = app
                                    .layer_stack
                                    .layers
                                    .get_mut(layer_idx)
                                    .and_then(|l| l.as_effect_mut())
                                    .and_then(|e| e.pass_executor.particle_system.as_mut())
                                    .filter(|ps| ps.def.splat.is_some());
                                if let Some(ps) = ps {
                                    ps.upload_splat_cloud(&app.gpu.device, &app.gpu.queue, &cloud);
                                    // The scene path round-trips through the
                                    // preset — light the unsaved-changes bar.
                                    app.preset_store.mark_dirty();
                                } else {
                                    log::info!(
                                        "Splat scene for layer {layer_idx} arrived after the layer changed — dropped"
                                    );
                                }
                            }
                            crate::gpu::particle::SplatLoadResult::Error { layer_idx, message } => {
                                log::error!(
                                    "Splat scene load failed (layer {layer_idx}): {message}"
                                );
                            }
                        }
                    }

                    // Drain background particle source loader results
                    if let Some((request, result)) = app.particle_source_loader.try_recv() {
                        // Route by the request, not by whatever is active now — the
                        // dialog ran on another thread and the user may have moved on
                        // (#2012). A layer that is gone, or no longer an effect with a
                        // particle system, drops the result rather than misplacing it.
                        let morph_pending = request.as_morph_target;
                        let morph_pending_slot = request.morph_slot;
                        if let Some(layer) = app.layer_stack.layers.get_mut(request.layer_idx) {
                            if let Some(effect) = layer.as_effect_mut() {
                                if let Some(ps) = effect.pass_executor.particle_system.as_mut() {
                                    match result {
                                        crate::gpu::particle::ParticleSourceResult::Image {
                                            path,
                                            data,
                                            width,
                                            height,
                                        } => {
                                            let aux = crate::gpu::particle::image_source::sample_rgba_buffer(
                                                &data, width, height,
                                                &ps.sample_def,
                                                ps.max_particles,
                                            );
                                            // Load into morph slot if pending, otherwise normal source
                                            if morph_pending {
                                                if !aux.is_empty() {
                                                    if let Some(ref mut morph) = ps.morph_state {
                                                        let slot = morph_pending_slot
                                                            .unwrap_or_else(|| {
                                                                (ps.def
                                                                    .morph_targets
                                                                    .as_ref()
                                                                    .map_or(0, |t| t.len())
                                                                    as u32)
                                                                    .min(3)
                                                            });
                                                        morph.load_target(slot, aux);
                                                        let filename = std::path::Path::new(&path)
                                                            .file_name()
                                                            .map(|f| {
                                                                f.to_string_lossy().to_string()
                                                            })
                                                            .unwrap_or_default();
                                                        let def = crate::gpu::particle::types::MorphTargetDef {
                                                            source: format!("image:{}", filename),
                                                            color: None,
                                                        };
                                                        if let Some(ref mut targets) =
                                                            ps.def.morph_targets
                                                        {
                                                            while targets.len() <= slot as usize {
                                                                targets.push(crate::gpu::particle::types::MorphTargetDef {
                                                                    source: String::new(), color: None,
                                                                });
                                                            }
                                                            targets[slot as usize] = def;
                                                        }
                                                        log::info!(
                                                            "Loaded morph target image into slot {}: {} ({}x{})",
                                                            slot,
                                                            path,
                                                            width,
                                                            height
                                                        );
                                                    }
                                                    ps.upload_morph_targets(
                                                        &app.gpu.device,
                                                        &app.gpu.queue,
                                                    );
                                                }
                                            } else if !aux.is_empty() {
                                                // Start transition from current aux
                                                if !ps.current_aux.is_empty() {
                                                    ps.source_transition = Some(
                                                        crate::gpu::particle::SourceTransition {
                                                            from_aux: ps.current_aux.clone(),
                                                            to_aux: aux.clone(),
                                                            progress: 0.0,
                                                            duration_secs: 0.5,
                                                        },
                                                    );
                                                } else {
                                                    ps.update_aux_in_place(&app.gpu.queue, &aux);
                                                }
                                                ps.store_current_aux(aux);
                                                ps.has_aux_data = true;
                                                // Retires whatever was there — model
                                                // included, which used to need its own
                                                // hand-written clear here (#2011).
                                                ps.set_source(
                                                    crate::gpu::particle::ParticleSource::Image {
                                                        path: path.clone(),
                                                    },
                                                );
                                            }
                                            log::info!(
                                                "Loaded particle image source: {} ({}x{})",
                                                path,
                                                width,
                                                height
                                            );
                                        }
                                        crate::gpu::particle::ParticleSourceResult::Animated {
                                            path,
                                            frames,
                                            delays_ms,
                                        } => {
                                            if morph_pending {
                                                // Load evenly-spaced frames into morph slots
                                                if let Some(ref mut morph) = ps.morph_state {
                                                    // When full, replace all 4 slots with video frames
                                                    let def_count = ps
                                                        .def
                                                        .morph_targets
                                                        .as_ref()
                                                        .map_or(0, |t| t.len())
                                                        as u32;
                                                    let num_slots = if def_count >= 4 {
                                                        4u32
                                                    } else {
                                                        (4 - def_count).min(4)
                                                    };
                                                    let start_slot = if def_count >= 4 {
                                                        0u32
                                                    } else {
                                                        def_count
                                                    };
                                                    let targets = crate::gpu::particle::morph::load_video_morph_targets(
                                                        &frames,
                                                        num_slots,
                                                        ps.max_particles,
                                                        &path,
                                                    );
                                                    for (i, (label, data)) in
                                                        targets.into_iter().enumerate()
                                                    {
                                                        let slot = start_slot + i as u32;
                                                        if slot < 4 && !data.is_empty() {
                                                            morph.load_target(slot, data);
                                                            let def = crate::gpu::particle::types::MorphTargetDef {
                                                                source: label.clone(),
                                                                color: None,
                                                            };
                                                            if let Some(ref mut defs) =
                                                                ps.def.morph_targets
                                                            {
                                                                while defs.len() <= slot as usize {
                                                                    defs.push(crate::gpu::particle::types::MorphTargetDef {
                                                                        source: String::new(), color: None,
                                                                    });
                                                                }
                                                                defs[slot as usize] = def;
                                                            }
                                                            log::info!(
                                                                "Loaded morph video frame into slot {}: {}",
                                                                slot,
                                                                label
                                                            );
                                                        }
                                                    }
                                                    ps.upload_morph_targets(
                                                        &app.gpu.device,
                                                        &app.gpu.queue,
                                                    );
                                                }
                                            } else {
                                                #[cfg(feature = "video")]
                                                {
                                                    let path_clone = path.clone();
                                                    ps.set_video_source(
                                                        &app.gpu.queue,
                                                        frames,
                                                        delays_ms,
                                                        path_clone,
                                                    );
                                                    log::info!(
                                                        "Loaded animated particle source: {}",
                                                        path,
                                                    );
                                                }
                                                #[cfg(not(feature = "video"))]
                                                {
                                                    // Without the video feature this really
                                                    // IS a still — the first frame and
                                                    // nothing more. It used to set only
                                                    // has_aux_data, leaving every source
                                                    // field pointing at whatever was loaded
                                                    // before it (#2011).
                                                    if let Some(frame) = frames.first() {
                                                        let aux = crate::gpu::particle::image_source::sample_rgba_buffer(
                                                            &frame.data, frame.width, frame.height,
                                                            &ps.sample_def,
                                                            ps.max_particles,
                                                        );
                                                        if !aux.is_empty() {
                                                            ps.update_aux_in_place(
                                                                &app.gpu.queue,
                                                                &aux,
                                                            );
                                                            ps.store_current_aux(aux);
                                                            ps.has_aux_data = true;
                                                        }
                                                    }
                                                    ps.set_source(
                                                        crate::gpu::particle::ParticleSource::Image {
                                                            path: path.clone(),
                                                        },
                                                    );
                                                    let _ = (path, delays_ms);
                                                }
                                            }
                                        }
                                        crate::gpu::particle::ParticleSourceResult::Model {
                                            path,
                                        } => {
                                            // The dialog thread only picked the file
                                            // (#1993); rastering it needs the device,
                                            // so the work lands here.
                                            let def = ps.model_sample.clone();
                                            let p = std::path::Path::new(&path);
                                            // A morph effect has no meaningful "base"
                                            // source — its aux is four interleaved
                                            // slots — so a model always becomes a
                                            // TARGET there, whether it arrived from the
                                            // morph row or from the main picker.
                                            let outcome = if ps.morph_state.is_some() {
                                                ps.apply_model_morph_target(
                                                    &app.gpu.device,
                                                    &app.gpu.queue,
                                                    p,
                                                    &def,
                                                    morph_pending_slot,
                                                )
                                                .map(|(slot, n)| {
                                                    format!("morph slot {slot} ({n} particles)")
                                                })
                                            } else {
                                                ps.apply_model_source(
                                                    &app.gpu.device,
                                                    &app.gpu.queue,
                                                    p,
                                                    &def,
                                                )
                                                .map(|n| format!("source ({n} particles)"))
                                            };
                                            match outcome {
                                                Ok(where_) => log::info!(
                                                    "Loaded particle model {path} into {where_}"
                                                ),
                                                Err(e) => {
                                                    log::error!("Model source load failed: {e}");
                                                    app.status_error = Some((
                                                        format!("Model source: {e}"),
                                                        std::time::Instant::now(),
                                                    ));
                                                }
                                            }
                                        }
                                        crate::gpu::particle::ParticleSourceResult::Error(e) => {
                                            log::error!("Particle source load failed: {e}");
                                            app.status_error = Some((
                                                format!("Particle source: {e}"),
                                                std::time::Instant::now(),
                                            ));
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // Handle layer UI signals
                let add_layer: Option<bool> = app
                    .egui_overlay
                    .context()
                    .data_mut(|d| d.remove_temp(egui::Id::new("add_layer")));
                if add_layer.is_some() {
                    app.add_layer();
                    app.preset_store.mark_dirty();
                }

                let remove_layer: Option<usize> = app
                    .egui_overlay
                    .context()
                    .data_mut(|d| d.remove_temp(egui::Id::new("remove_layer")));
                if let Some(idx) = remove_layer {
                    // App::remove_layer, not layer_stack's — it carries the
                    // bindings with the renumbering (#2026).
                    app.remove_layer(idx);
                    app.preset_store.mark_dirty();
                    #[cfg(feature = "webcam")]
                    app.cleanup_webcam_if_unused();
                }

                // Handle clear all layers
                let clear_all: Option<bool> = app
                    .egui_overlay
                    .context()
                    .data_mut(|d| d.remove_temp(egui::Id::new("clear_all_layers")));
                if clear_all.is_some() {
                    #[cfg(feature = "webcam")]
                    {
                        app.webcam_capture = None;
                    }
                    app.clear_all_layers();
                    app.preset_store.mark_dirty();
                }

                // Handle layer rename
                let layer_rename: Option<(usize, Option<String>)> = app
                    .egui_overlay
                    .context()
                    .data_mut(|d| d.remove_temp(egui::Id::new("layer_rename")));
                if let Some((idx, new_name)) = layer_rename {
                    if let Some(layer) = app.layer_stack.layers.get_mut(idx) {
                        layer.custom_name = new_name;
                        app.preset_store.mark_dirty();
                    }
                }

                let select_layer: Option<usize> = app
                    .egui_overlay
                    .context()
                    .data_mut(|d| d.remove_temp(egui::Id::new("select_layer")));
                if let Some(idx) = select_layer {
                    if idx < app.layer_stack.layers.len() {
                        app.layer_stack.active_layer = idx;
                        app.sync_active_layer();
                    }
                }

                // Handle lock/pin toggles
                let toggle_lock: Option<(usize, bool)> = app
                    .egui_overlay
                    .context()
                    .data_mut(|d| d.remove_temp(egui::Id::new("layer_toggle_lock")));
                if let Some((idx, locked)) = toggle_lock {
                    if let Some(layer) = app.layer_stack.layers.get_mut(idx) {
                        layer.locked = locked;
                        app.preset_store.mark_dirty();
                    }
                }

                let toggle_pin: Option<(usize, bool)> = app
                    .egui_overlay
                    .context()
                    .data_mut(|d| d.remove_temp(egui::Id::new("layer_toggle_pin")));
                if let Some((idx, pinned)) = toggle_pin {
                    if let Some(layer) = app.layer_stack.layers.get_mut(idx) {
                        layer.pinned = pinned;
                        app.preset_store.mark_dirty();
                    }
                }

                let layer_blend: Option<u32> = app
                    .egui_overlay
                    .context()
                    .data_mut(|d| d.remove_temp(egui::Id::new("layer_blend")));
                if let Some(mode_u32) = layer_blend {
                    if let Some(layer) = app.layer_stack.active_mut() {
                        if !layer.locked {
                            layer.blend_mode = BlendMode::from_u32(mode_u32);
                            app.preset_store.mark_dirty();
                        }
                    }
                }

                let layer_opacity: Option<f32> = app
                    .egui_overlay
                    .context()
                    .data_mut(|d| d.remove_temp(egui::Id::new("layer_opacity")));
                if let Some(opacity) = layer_opacity {
                    if let Some(layer) = app.layer_stack.active_mut() {
                        if !layer.locked {
                            layer.opacity = opacity;
                            app.preset_store.mark_dirty();
                        }
                    }
                }

                let layer_displace: Option<f32> = app
                    .egui_overlay
                    .context()
                    .data_mut(|d| d.remove_temp(egui::Id::new("layer_displace")));
                if let Some(amount) = layer_displace {
                    if let Some(layer) = app.layer_stack.active_mut() {
                        if !layer.locked {
                            layer.displace_amount = amount;
                            app.preset_store.mark_dirty();
                        }
                    }
                }

                let layer_move: Option<(usize, usize)> = app
                    .egui_overlay
                    .context()
                    .data_mut(|d| d.remove_temp(egui::Id::new("layer_move")));
                if let Some((from, to)) = layer_move {
                    app.move_layer(from, to);
                    app.preset_store.mark_dirty();
                }

                let toggle_enable: Option<(usize, bool)> = app
                    .egui_overlay
                    .context()
                    .data_mut(|d| d.remove_temp(egui::Id::new("layer_toggle_enable")));
                if let Some((idx, enabled)) = toggle_enable {
                    if let Some(layer) = app.layer_stack.layers.get_mut(idx) {
                        if !layer.locked {
                            layer.enabled = enabled;
                            app.preset_store.mark_dirty();
                        }
                    }
                }

                // Check if active layer params changed (marks preset dirty + schedules .pfx save)
                if let Some(layer) = app.layer_stack.active_mut() {
                    if layer.param_store.changed {
                        layer.param_store.changed = false;
                        app.preset_store.mark_dirty();

                        // Schedule debounced save for user effects (avoid writing on every slider frame)
                        if let Some(eidx) = layer.effect_index() {
                            if let Some(effect) = app.effect_loader.effects.get(eidx) {
                                if !EffectLoader::is_builtin(effect) {
                                    self.param_save_pending =
                                        Some((eidx, std::time::Instant::now()));
                                }
                            }
                        }
                    }
                }

                // Flush debounced param save after 500ms of no changes
                if let Some((eidx, last_change)) = self.param_save_pending {
                    if last_change.elapsed() >= std::time::Duration::from_millis(500) {
                        self.param_save_pending = None;
                        // Gather current values from the layer using this effect
                        let values: Option<
                            std::collections::HashMap<String, crate::params::types::ParamValue>,
                        > = app.layer_stack.layers.iter().find_map(|l| {
                            if l.effect_index() == Some(eidx) {
                                Some(l.param_store.values.clone())
                            } else {
                                None
                            }
                        });
                        // Capture failures locally; can't touch app.status_error
                        // while effect_loader is mutably borrowed.
                        let mut save_err: Option<String> = None;
                        if let (Some(values), Some(effect)) =
                            (values, app.effect_loader.effects.get_mut(eidx))
                        {
                            for input in &mut effect.inputs {
                                if let Some(val) = values.get(input.name()) {
                                    input.set_default(val);
                                }
                            }
                            if let Some(ref path) = effect.source_path {
                                if let Ok(json) = serde_json::to_string_pretty(effect) {
                                    if let Err(e) = std::fs::write(path, &json) {
                                        save_err = Some(e.to_string());
                                    }
                                    // Update editor paired content if showing this .pfx
                                    if app.shader_editor.open {
                                        let pfx_canonical =
                                            path.canonicalize().unwrap_or_else(|_| path.clone());
                                        if let Some(ref paired) = app.shader_editor.paired_path {
                                            let paired_canonical = paired
                                                .canonicalize()
                                                .unwrap_or_else(|_| paired.clone());
                                            if paired_canonical == pfx_canonical {
                                                app.shader_editor.paired_content = json.clone();
                                                // Only mirror disk content on a successful
                                                // write, so the editor keeps showing unsaved
                                                // state (and Ctrl+S retries) after a failure.
                                                if save_err.is_none() {
                                                    app.shader_editor.paired_disk_content = json;
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        if let Some(e) = save_err {
                            log::error!("Effect save failed: {e}");
                            app.status_error = Some((
                                format!("Effect save failed: {e}"),
                                std::time::Instant::now(),
                            ));
                        }
                    }
                }

                // Handle MIDI + OSC triggers
                let mut triggers: Vec<_> = app.pending_midi_triggers.drain(..).collect();
                triggers.append(&mut app.pending_osc_triggers);
                triggers.append(&mut app.pending_web_triggers);
                for trigger in triggers {
                    use crate::midi::types::TriggerAction;
                    // Build visible (non-hidden) effect indices for cycling
                    let visible: Vec<usize> = app
                        .effect_loader
                        .effects
                        .iter()
                        .enumerate()
                        .filter(|(_, e)| !e.hidden)
                        .map(|(i, _)| i)
                        .collect();
                    match trigger {
                        TriggerAction::NextEffect if !visible.is_empty() => {
                            let current = app
                                .layer_stack
                                .active()
                                .and_then(|l| l.effect_index())
                                .unwrap_or(0);
                            let pos = visible.iter().position(|&i| i == current).unwrap_or(0);
                            app.load_effect(visible[(pos + 1) % visible.len()]);
                        }
                        TriggerAction::PrevEffect if !visible.is_empty() => {
                            let current = app
                                .layer_stack
                                .active()
                                .and_then(|l| l.effect_index())
                                .unwrap_or(0);
                            let pos = visible.iter().position(|&i| i == current).unwrap_or(0);
                            app.load_effect(
                                visible[if pos == 0 { visible.len() - 1 } else { pos - 1 }],
                            );
                        }
                        TriggerAction::TogglePostProcess => {
                            app.post_process.enabled = !app.post_process.enabled;
                            if let Some(layer) = app.layer_stack.active_mut() {
                                layer.postprocess.enabled = app.post_process.enabled;
                            }
                        }
                        TriggerAction::ToggleOverlay => {
                            app.egui_overlay.toggle_visible();
                        }
                        TriggerAction::NextPreset if !app.preset_store.presets.is_empty() => {
                            let num = app.preset_store.presets.len();
                            let current = app.preset_store.current_preset.unwrap_or(0);
                            app.load_preset((current + 1) % num);
                        }
                        TriggerAction::PrevPreset if !app.preset_store.presets.is_empty() => {
                            let num = app.preset_store.presets.len();
                            let current = app.preset_store.current_preset.unwrap_or(0);
                            app.load_preset(if current == 0 { num - 1 } else { current - 1 });
                        }
                        TriggerAction::NextLayer if app.layer_stack.layers.len() > 1 => {
                            let num = app.layer_stack.layers.len();
                            let current = app.layer_stack.active_layer;
                            app.layer_stack.active_layer = (current + 1) % num;
                            app.sync_active_layer();
                        }
                        TriggerAction::PrevLayer if app.layer_stack.layers.len() > 1 => {
                            let num = app.layer_stack.layers.len();
                            let current = app.layer_stack.active_layer;
                            app.layer_stack.active_layer =
                                if current == 0 { num - 1 } else { current - 1 };
                            app.sync_active_layer();
                        }
                        TriggerAction::SceneGoNext => {
                            let event = app.timeline.go_next();
                            app.process_timeline_event(event);
                        }
                        TriggerAction::SceneGoPrev => {
                            let event = app.timeline.go_prev();
                            app.process_timeline_event(event);
                        }
                        TriggerAction::TempoHalf => {
                            app.audio
                                .send_tempo_command(crate::audio::TempoCommand::ShiftOctave(-1));
                        }
                        TriggerAction::TempoDouble => {
                            app.audio
                                .send_tempo_command(crate::audio::TempoCommand::ShiftOctave(1));
                        }
                        TriggerAction::TempoTap => {
                            app.audio.tap_tempo();
                        }
                        TriggerAction::ToggleTimeline => {
                            if app.timeline.active {
                                app.timeline.stop();
                            } else if !app.timeline.cues.is_empty() {
                                let event = app.timeline.start(0);
                                app.process_timeline_event(event);
                            }
                        }
                        _ => {}
                    }
                }

                match app.render() {
                    Ok(()) => {}
                    Err(e @ (wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated)) => {
                        let w = app.gpu.surface_config.width;
                        let h = app.gpu.surface_config.height;
                        log::info!("SURFACELOG {e:?} -> full resize {w}x{h}");
                        app.resize(w, h);
                    }
                    Err(wgpu::SurfaceError::OutOfMemory) => {
                        log::error!("Out of GPU memory");
                        event_loop.exit();
                    }
                    Err(e) => {
                        log::warn!("Surface error: {e}");
                    }
                }

                app.window.request_redraw();
            }
            _ => {}
        }
    }
}

/// Shared flag parsing for `--signal` / `--signal-dump` (house pattern: positional
/// scans, exit 2 on usage errors at the call site).
fn parse_signal_flags(args: &[String]) -> Result<signal::SignalCliArgs, String> {
    let mut cli = signal::SignalCliArgs::default();
    if let Some(i) = args.iter().position(|a| a == "--host") {
        cli.host = Some(
            args.get(i + 1)
                .filter(|a| !a.starts_with("--"))
                .ok_or("--host needs a value")?
                .clone(),
        );
    }
    if let Some(i) = args.iter().position(|a| a == "--port") {
        cli.port = Some(
            args.get(i + 1)
                .and_then(|a| a.parse().ok())
                .ok_or("--port needs a number (1-65535)")?,
        );
    }
    if let Some(i) = args.iter().position(|a| a == "--rate") {
        cli.rate = Some(
            args.get(i + 1)
                .and_then(|a| a.parse().ok())
                .ok_or("--rate needs a number (Hz)")?,
        );
    }
    cli.feat_bus = args.iter().any(|a| a == "--feat-bus");
    cli.no_stems = args.iter().any(|a| a == "--no-stems");
    if let Some(i) = args.iter().position(|a| a == "--device") {
        cli.device = Some(
            args.get(i + 1)
                .filter(|a| !a.starts_with("--"))
                .ok_or("--device needs a device name")?
                .clone(),
        );
    }
    Ok(cli)
}

fn load_window_icon() -> Option<Icon> {
    let png_bytes = include_bytes!("../../../assets/icon/icon_256x256.png");
    let img = image::load_from_memory(png_bytes).ok()?.into_rgba8();
    let (w, h) = img.dimensions();
    Icon::from_rgba(img.into_raw(), w, h).ok()
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();

    // Before any config file is read: move a pre-rename ~/.config/phosphor/ to fosfora.
    crate::paths::migrate_legacy_config_dir();

    // Suppress noisy ALSA/JACK C library messages on Linux (missing JACK server, OSS, dsnoop)
    crate::audio::capture::suppress_audio_library_noise();

    // --audio-test: run standalone audio diagnostic (no GPU, no window)
    if std::env::args().any(|a| a == "--audio-test") {
        #[cfg(target_os = "linux")]
        {
            crate::audio::pulse_capture::PulseCapture::run_diagnostic(3);
            return Ok(());
        }
        #[cfg(not(target_os = "linux"))]
        {
            eprintln!("--audio-test is only supported on Linux (PulseAudio backend)");
            std::process::exit(1);
        }
    }

    // --signal: headless analysis broadcast over /fosfora/v1 (no window, no GPU).
    // Ships in release builds — the audio engine and OSC are unconditional deps.
    // See docs/SIGNAL.md for the wire contract.
    {
        let args: Vec<String> = std::env::args().collect();
        if args.iter().any(|a| a == "--signal") {
            let cli = match parse_signal_flags(&args) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!(
                        "--signal: {e}\nusage: --signal [--host H] [--port N] [--rate HZ] \
                         [--feat-bus] [--no-stems] [--device NAME]"
                    );
                    std::process::exit(2);
                }
            };
            match signal::run(&cli) {
                Ok(()) => return Ok(()),
                Err(e) => {
                    eprintln!("--signal failed: {e:#}");
                    std::process::exit(1);
                }
            }
        }
    }

    // --signal-dump <audio>: the same Signal emitter driven offline, JSONL out.
    #[cfg(feature = "analyze")]
    {
        let args: Vec<String> = std::env::args().collect();
        if let Some(i) = args.iter().position(|a| a == "--signal-dump") {
            let usage = "--signal-dump <audio> [--out <file.jsonl>|-] [--rate HZ] \
                         [--feat-bus] [--no-stems]";
            let Some(input) = args.get(i + 1).filter(|a| !a.starts_with("--")) else {
                eprintln!("--signal-dump needs a path: {usage}");
                std::process::exit(2);
            };
            let cli = match parse_signal_flags(&args) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("--signal-dump: {e}\nusage: {usage}");
                    std::process::exit(2);
                }
            };
            let out = args
                .iter()
                .position(|a| a == "--out")
                .and_then(|j| args.get(j + 1))
                .map(PathBuf::from);
            match signal::run_dump(std::path::Path::new(input), out.as_deref(), &cli) {
                Ok(()) => return Ok(()),
                Err(e) => {
                    eprintln!("--signal-dump failed: {e:#}");
                    std::process::exit(1);
                }
            }
        }
    }

    // --render-loop <spec.loop.json>: seamless beat-locked loop export (#2063).
    // Ships in release builds (decision #2066) — no cargo feature involved.
    {
        let args: Vec<String> = std::env::args().collect();
        if let Some(i) = args.iter().position(|a| a == "--render-loop") {
            let Some(spec_path) = args.get(i + 1).filter(|a| !a.starts_with("--")) else {
                eprintln!(
                    "--render-loop needs a path: --render-loop <spec.loop.json> [--out <file>]"
                );
                std::process::exit(2);
            };
            let json = match std::fs::read_to_string(spec_path) {
                Ok(j) => j,
                Err(e) => {
                    eprintln!("cannot read {spec_path}: {e}");
                    std::process::exit(2);
                }
            };
            let spec = match headless::loop_spec::LoopSpec::from_json(&json) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("{e}");
                    std::process::exit(2);
                }
            };
            // Best-effort modes (P2.7): explicit flags only, second-class by design.
            let crossfade_bars = args
                .iter()
                .position(|a| a == "--crossfade-bars")
                .and_then(|j| args.get(j + 1))
                .and_then(|v| v.parse::<u32>().ok());
            let warmup_bars = args
                .iter()
                .position(|a| a == "--warmup-bars")
                .and_then(|j| args.get(j + 1))
                .and_then(|v| v.parse::<u32>().ok())
                .unwrap_or(2);
            let mode = if let Some(tail_bars) = crossfade_bars {
                headless::loop_spec::BestEffort::Crossfade {
                    tail_bars,
                    warmup_bars,
                }
            } else if args.iter().any(|a| a == "--allow-non-loop") {
                headless::loop_spec::BestEffort::TimeWrapped
            } else {
                headless::loop_spec::BestEffort::None
            };
            let xfade_tag = if crossfade_bars.is_some() {
                "~xfade"
            } else {
                ""
            };
            let out = args
                .iter()
                .position(|a| a == "--out")
                .and_then(|j| args.get(j + 1))
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| {
                    std::path::PathBuf::from(format!(
                        "{}_{}bpm_{}bar{}.{}",
                        spec.effect.to_lowercase().replace(' ', "_"),
                        spec.bpm.round() as u32,
                        spec.bars,
                        xfade_tag,
                        spec.codec.extension()
                    ))
                });
            match spec.snap() {
                Ok(t) => eprintln!(
                    "requested {:.2} -> effective {:.2} BPM ({} frames @ {}fps, {:.3}s)",
                    spec.bpm, t.effective_bpm, t.frames, spec.fps, t.duration_secs
                ),
                Err(e) => {
                    eprintln!("{e}");
                    std::process::exit(2);
                }
            }
            match headless::loop_encode::render_and_encode(&spec, mode, &out) {
                Ok(_) => {
                    eprintln!("wrote {}", out.display());
                    std::process::exit(0);
                }
                Err(e) => {
                    eprintln!("loop render failed: {e}");
                    std::process::exit(1);
                }
            }
        }
    }

    // --analyze <file>: offline song analysis (#2027). No GPU, no window, no audio device —
    // decodes the file and runs the production per-hop chain over it faster than realtime.
    #[cfg(feature = "analyze")]
    {
        let args: Vec<String> = std::env::args().collect();
        if let Some(i) = args.iter().position(|a| a == "--analyze") {
            let Some(input) = args.get(i + 1).filter(|a| !a.starts_with("--")) else {
                eprintln!("--analyze needs a path: --analyze <file> [--out <file>] [--dense]");
                std::process::exit(2);
            };
            let out = args
                .iter()
                .position(|a| a == "--out")
                .and_then(|j| args.get(j + 1))
                .map(std::path::PathBuf::from);
            let dense = args.iter().any(|a| a == "--dense");
            let written = crate::analyze::run(std::path::Path::new(input), out.as_deref(), dense)?;
            println!("{}", written.display());
            return Ok(());
        }
    }

    // --dump-schema: what this build can be told to do, as JSON (#2027). Same
    // no-GPU early exit as --analyze. The scene generator reads this instead of
    // carrying its own copy of the effect, source and target vocabulary.
    #[cfg(feature = "analyze")]
    {
        let args: Vec<String> = std::env::args().collect();
        if args.iter().any(|a| a == "--dump-schema") {
            let out = args
                .iter()
                .position(|a| a == "--out")
                .and_then(|j| args.get(j + 1))
                .map(std::path::PathBuf::from);
            let written = crate::analyze::schema_dump::run(out.as_deref())?;
            println!("{}", written.display());
            return Ok(());
        }
    }

    // --render-scene <dir> --song <file>: render a generated scene headless
    // against the song's own feature stream, writing stills + audio-muxed clips
    // (#2027). Needs a GPU but no window, no audio device, no wall clock.
    #[cfg(feature = "analyze")]
    {
        let args: Vec<String> = std::env::args().collect();
        if let Some(i) = args.iter().position(|a| a == "--render-scene") {
            let Some(dir) = args.get(i + 1).filter(|a| !a.starts_with("--")) else {
                eprintln!(
                    "--render-scene needs a directory: --render-scene <scene_dir> --song <audio>                      [--out <dir>] [--res WxH] [--quality low|medium|high] [--window-secs N]"
                );
                std::process::exit(2);
            };
            let flag = |name: &str| {
                args.iter()
                    .position(|a| a == name)
                    .and_then(|j| args.get(j + 1))
                    .cloned()
            };
            let Some(song) = flag("--song") else {
                eprintln!("--render-scene needs --song <audio file>");
                std::process::exit(2);
            };
            let out = flag("--out")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| std::path::PathBuf::from(dir).join("_render"));
            let (width, height) = flag("--res")
                .and_then(|r| {
                    let (w, h) = r.split_once('x')?;
                    Some((w.parse().ok()?, h.parse().ok()?))
                })
                .unwrap_or((640, 360));
            let quality = match flag("--quality").as_deref() {
                Some("low") => crate::settings::ParticleQuality::Low,
                Some("high") => crate::settings::ParticleQuality::High,
                _ => crate::settings::ParticleQuality::Medium,
            };
            let window_secs = flag("--window-secs")
                .and_then(|w| w.parse().ok())
                .unwrap_or(6.0);
            let render_args = crate::headless::driver::RenderSceneArgs {
                scene_dir: std::path::PathBuf::from(dir),
                song: std::path::PathBuf::from(song),
                out,
                width,
                height,
                quality,
                window_secs,
            };
            crate::headless::driver::run(&render_args)?;
            return Ok(());
        }
    }

    // --validate <dir>: reject a generated scene before the app loads it (#2027).
    // Exits 1 on problems so a generator's repair loop can branch on it.
    #[cfg(feature = "analyze")]
    {
        let args: Vec<String> = std::env::args().collect();
        if let Some(i) = args.iter().position(|a| a == "--validate") {
            let Some(dir) = args.get(i + 1).filter(|a| !a.starts_with("--")) else {
                eprintln!("--validate needs a directory: --validate <dir>");
                std::process::exit(2);
            };
            let report = crate::analyze::validate::run(std::path::Path::new(dir))?;
            for problem in &report.problems {
                println!("{problem}");
            }
            for note in &report.notes {
                println!("note: {note}");
            }
            println!(
                "\n{} preset(s), {} binding(s), {} cue(s) checked — {}",
                report.presets_checked,
                report.bindings_checked,
                report.cues_checked,
                if report.is_clean() {
                    "clean".to_string()
                } else {
                    format!("{} problem(s)", report.problems.len())
                }
            );
            std::process::exit(i32::from(!report.is_clean()));
        }
    }

    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(winit::event_loop::ControlFlow::Poll);

    let mut app = FosforaApp::new();
    event_loop.run_app(&mut app)?;

    Ok(())
}
