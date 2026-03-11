mod app;
mod audio;
mod bindings;
#[cfg(feature = "depth")]
mod depth;
mod effect;
mod gpu;
mod media;
mod midi;
#[cfg(feature = "ndi")]
mod ndi;
mod osc;
mod params;
mod preset;
mod scene;
mod settings;
mod shader;
mod ui;
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

struct PhosphorApp {
    app: Option<App>,
    window: Option<Arc<Window>>,
    file_dialog_rx: Option<Receiver<PathBuf>>,
    obstacle_dialog_rx: Option<Receiver<PathBuf>>,
    /// Debounced param save: (effect_index, last_change_time)
    param_save_pending: Option<(usize, std::time::Instant)>,
}

impl PhosphorApp {
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

impl ApplicationHandler for PhosphorApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let mut attrs = WindowAttributes::default()
            .with_title("Phosphor")
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
                log::info!("Phosphor initialized");
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

        match event {
            WindowEvent::CloseRequested => {
                app.quit_requested = true;
            }
            WindowEvent::Resized(size) => {
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
                        // Close binding matrix first, then shader editor, then quit
                        if app.binding_matrix.open {
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
                    KeyCode::Space => {
                        // Scene: go to next cue (when timeline has cues)
                        if !app.timeline.cues.is_empty() {
                            app.egui_overlay.context().data_mut(|d| {
                                d.insert_temp(egui::Id::new("scene_go_next"), true);
                            });
                        }
                    }
                    KeyCode::KeyT => {
                        // Toggle timeline active (when cues loaded)
                        if !app.timeline.cues.is_empty() {
                            app.egui_overlay.context().data_mut(|d| {
                                d.insert_temp(egui::Id::new("scene_toggle_play"), true);
                            });
                        }
                    }
                    KeyCode::KeyB => {
                        if !app.shader_editor.open {
                            app.binding_matrix.open = !app.binding_matrix.open;
                        }
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
                            let (source_type, source_name) = if ps.image_source.is_video() {
                                (
                                    "video".to_string(),
                                    ps.video_path.clone().unwrap_or_default(),
                                )
                            } else if ps.image_source.is_webcam() {
                                ("webcam".to_string(), "webcam".to_string())
                            } else {
                                ("static".to_string(), ps.def.emitter.image.clone())
                            };
                            let (video_playing, video_looping, video_speed) = {
                                #[cfg(feature = "video")]
                                {
                                    if let crate::gpu::particle::ParticleImageSource::Video {
                                        playing,
                                        looping,
                                        speed,
                                        ..
                                    } = &ps.image_source
                                    {
                                        (*playing, *looping, *speed)
                                    } else {
                                        (false, true, 1.0)
                                    }
                                }
                                #[cfg(not(feature = "video"))]
                                {
                                    (false, true, 1.0)
                                }
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
                                has_trails: ps.def.trail_length >= 2,
                                trail_length: ps.def.trail_length,
                                has_interaction: ps.def.interaction,
                                has_sprite: ps.sprite.is_some(),
                                is_compute_raster: ps.is_compute_raster(),
                                max_scaled_count: ps.def.max_scaled_count,
                                has_image_source: ps.has_aux_data
                                    || ps.def.emitter.shape == "image",
                                source_type,
                                source_name,
                                video_playing,
                                video_looping,
                                video_speed,
                                video_position_secs: ps.image_source.video_position_secs(),
                                video_duration_secs: ps.image_source.video_duration_secs(),
                                is_transitioning: ps.source_transition.is_some(),
                                source_loading: false, // set below
                                source_loading_name: String::new(),
                                builtin_images: Vec::new(), // set below
                                has_morph: ps.morph_state.is_some(),
                                morph_target_count: ps.morph_state.as_ref().map_or(0, |m| m.target_count),
                                morph_source_index: ps.morph_state.as_ref().map_or(0, |m| m.source_index),
                                morph_dest_index: ps.morph_state.as_ref().map_or(0, |m| m.dest_index),
                                morph_progress: ps.morph_state.as_ref().map_or(0.0, |m| m.progress),
                                morph_transitioning: ps.morph_state.as_ref().map_or(false, |m| m.transitioning),
                                morph_transition_style: ps.morph_state.as_ref().map_or(0, |m| m.transition_style),
                                morph_auto_cycle: ps.morph_state.as_ref().map_or(0, |m| {
                                    match m.auto_cycle {
                                        crate::gpu::particle::morph::AutoCycle::Off => 0,
                                        crate::gpu::particle::morph::AutoCycle::OnBeat => 1,
                                        crate::gpu::particle::morph::AutoCycle::Timed(_) => 2,
                                    }
                                }),
                                morph_hold_duration: ps.morph_state.as_ref().map_or(2.0, |m| m.hold_duration),
                                morph_target_labels: ps.def.morph_targets.as_ref().map_or_else(Vec::new, |targets| {
                                    targets.iter().map(|t| {
                                        if t.source == "random" {
                                            "random".to_string()
                                        } else if t.source == "snapshot" {
                                            "snap".to_string()
                                        } else if let Some(shape) = t.source.strip_prefix("geometry:") {
                                            shape.to_string()
                                        } else if let Some(img) = t.source.strip_prefix("image:") {
                                            let name = img.trim_end_matches(".png");
                                            let name = name.strip_prefix("raster_").unwrap_or(name);
                                            name.to_string()
                                        } else if let Some(text) = t.source.strip_prefix("text:") {
                                            if text.len() > 8 {
                                                format!("{}...", &text[..8])
                                            } else {
                                                text.to_string()
                                            }
                                        } else if let Some(rest) = t.source.strip_prefix("video:") {
                                            // "video:clip.mp4:f42" → "f42"
                                            rest.rsplit(':').next().unwrap_or(rest).to_string()
                                        } else {
                                            t.source.clone()
                                        }
                                    }).collect()
                                }),
                            }
                        });
                    // Overlay loader state + built-in images onto particle info
                    if let Some(ref mut pi) = particle_info {
                        pi.source_loading = app.particle_source_loader.loading;
                        pi.source_loading_name = app.particle_source_loader.loading_name.clone();
                        if pi.has_image_source {
                            pi.builtin_images =
                                crate::gpu::particle::builtin_raster_images().clone();
                        }
                    }
                    let particle_count = particle_info.as_ref().map(|p| p.max_count);

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
                                    }
                                } else {
                                    crate::ui::panels::obstacle_panel::ObstacleInfo {
                                        enabled: false,
                                        mode: crate::gpu::particle::ObstacleMode::Bounce,
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
                                capture_running: app.webcam_capture.as_ref().map_or(false, |c| c.is_running()),
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
                            output_width: app.ndi.capture.as_ref().map_or(0, |c| c.width),
                            output_height: app.ndi.capture.as_ref().map_or(0, |c| c.height),
                            alpha_from_luma: app.ndi.config.alpha_from_luma,
                        };
                        ctx.data_mut(|d| {
                            d.insert_temp(egui::Id::new("ndi_info"), ndi_info);
                            d.insert_temp(egui::Id::new("ndi_running"), app.ndi.is_running());
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
                                scene_info,
                                &app.status_error,
                                app.settings.theme,
                                app.settings.particle_quality,
                                app.settings.use_ffmpeg_webcam,
                            );
                        }
                        // Sync global postprocess enabled from layer
                        app.post_process.enabled = layer.postprocess.enabled;
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
                        });
                    }

                    // Draw binding matrix modal
                    if app.binding_matrix.open {
                        let layers: Vec<crate::ui::panels::binding_helpers::LayerParamInfo> =
                            app.layer_stack.layers.iter().enumerate().map(|(i, l)| {
                                let effect_name = l.effect_index()
                                    .and_then(|idx| app.effect_loader.effects.get(idx))
                                    .map(|eff| eff.name.clone())
                                    .unwrap_or_default();
                                let param_names = l.param_store.defs
                                    .iter()
                                    .filter(|d| matches!(d, crate::params::ParamDef::Float { .. } | crate::params::ParamDef::Bool { .. }))
                                    .map(|d| d.name().to_string())
                                    .collect();
                                crate::ui::panels::binding_helpers::LayerParamInfo {
                                    index: i,
                                    effect_name,
                                    param_names,
                                }
                            }).collect();
                        let bind_info = crate::ui::panels::binding_helpers::BindingPanelInfo {
                            layers,
                            active_layer,
                            layer_count: layer_infos.len(),
                            preset_name: app.preset_store.current_name().unwrap_or("(unsaved)").to_string(),
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

                        egui::Window::new("Quit Phosphor?")
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
                                "FFmpeg not found in PATH. Install FFmpeg to use this feature.".into(),
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
                        let res = match res_u8 {
                            0 => crate::ndi::types::OutputResolution::Match,
                            1 => crate::ndi::types::OutputResolution::Res720p,
                            2 => crate::ndi::types::OutputResolution::Res1080p,
                            3 => crate::ndi::types::OutputResolution::Res4K,
                            _ => crate::ndi::types::OutputResolution::Match,
                        };
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
                                        ObstacleCommand::UseWebcam => {
                                            // Start webcam capture if not already running
                                            #[cfg(feature = "webcam")]
                                            {
                                                obstacle_start_webcam = app.webcam_capture.is_none();
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
                    let switch_obs_device: Option<u32> = app
                        .egui_overlay
                        .context()
                        .data_mut(|d| d.remove_temp(egui::Id::new("switch_obstacle_webcam_device")));
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
                                app.status_error =
                                    Some((format!("Camera failed: {e}"), std::time::Instant::now()));
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
                        if is_video {
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
                                                log::error!("Failed to decode obstacle video: {e}")
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
                                app.status_error =
                                    Some((format!("Camera failed: {e}"), std::time::Instant::now()));
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
                        app.layer_stack.remove_layer(active);
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

                    let mut particle_save_info: Option<(usize, gpu::particle::types::ParticleDef)> =
                        None;
                    if emit_rate.is_some()
                        || burst.is_some()
                        || lifetime.is_some()
                        || speed.is_some()
                        || size.is_some()
                        || drag.is_some()
                    {
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
                        if let Some(effect) = app.effect_loader.effects.get_mut(idx) {
                            if !EffectLoader::is_builtin(effect) {
                                effect.particles = Some(updated_def);
                                if let Some(ref path) = effect.source_path {
                                    if let Ok(json) = serde_json::to_string_pretty(effect) {
                                        let _ = std::fs::write(path, json);
                                    }
                                }
                            }
                        }
                    }
                }

                // Handle particle source change signals
                {
                    let ctx = app.egui_overlay.context();

                    // Select built-in raster image
                    let select_builtin: Option<String> =
                        ctx.data_mut(|d| d.remove_temp(egui::Id::new("particle_select_builtin")));
                    if let Some(name) = select_builtin {
                        if !app.particle_source_loader.loading {
                            let path = crate::gpu::particle::builtin_raster_path(&name);
                            app.particle_source_loader.load_image(path);
                            app.preset_store.mark_dirty();
                        }
                    }

                    // Load image as particle source (dialog + decode on background thread)
                    let load_image: Option<bool> =
                        ctx.data_mut(|d| d.remove_temp(egui::Id::new("particle_load_image")));
                    if load_image.is_some() && !app.particle_source_loader.loading {
                        app.particle_source_loader.open_image_dialog();
                        app.preset_store.mark_dirty();
                    }

                    // Load video as particle source (dialog + decode on background thread)
                    #[cfg(feature = "video")]
                    {
                        let load_video: Option<bool> =
                            ctx.data_mut(|d| d.remove_temp(egui::Id::new("particle_load_video")));
                        if load_video.is_some() && !app.particle_source_loader.loading {
                            app.particle_source_loader.open_video_dialog();
                            app.preset_store.mark_dirty();
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
                                        if let crate::gpu::particle::ParticleImageSource::Video {
                                            playing: ref mut p,
                                            looping: ref mut l,
                                            speed: ref mut s,
                                            ..
                                        } = ps.image_source
                                        {
                                            if let Some(v) = playing {
                                                *p = v;
                                            }
                                            if let Some(v) = looping {
                                                *l = v;
                                            }
                                            if let Some(v) = speed {
                                                *s = v;
                                            }
                                        }
                                        if let Some(v) = seek {
                                            ps.image_source.seek_to_secs(v);
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
                            || morph_snapshot.is_some()
                            || morph_clear_slot.is_some()
                            || morph_manual_blend.is_some()
                            || morph_set_source.is_some()
                        {
                            if let Some(layer) = app.layer_stack.active_mut() {
                                if let Some(effect) = layer.as_effect_mut() {
                                    if let Some(ps) =
                                        effect.pass_executor.particle_system.as_mut()
                                    {
                                        let needs_upload = if let Some(ref mut morph) = ps.morph_state {
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
                                                morph.source_index = src.min(morph.target_count.saturating_sub(1));
                                            }
                                            if let Some(progress) = morph_manual_blend {
                                                // Manual scrub: set progress directly, mark transitioning so shader interpolates
                                                morph.progress = progress;
                                                morph.transitioning = progress < 1.0;
                                                // Reset hold timer so auto-cycle doesn't immediately fire
                                                morph.hold_timer = 0.0;
                                            }
                                            // Helper: pick target slot — selected slot, or next empty (based on def count), or last
                                            let def_count = ps.def.morph_targets.as_ref().map_or(0, |t| t.len() as u32);
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
                                                if let Some(ref mut targets) = ps.def.morph_targets {
                                                    if (slot as usize) < targets.len() {
                                                        targets.remove(slot as usize);
                                                    }
                                                }
                                                needs_upload = true;
                                            }

                                            if let Some(shape) = morph_add_geo {
                                                let slot = pick_slot();
                                                let data = crate::gpu::particle::morph::generate_geometry(
                                                    &shape,
                                                    ps.max_particles,
                                                    ps.def.initial_size,
                                                );
                                                morph.load_target(slot, data);
                                                let def = crate::gpu::particle::types::MorphTargetDef {
                                                    source: format!("geometry:{}", shape),
                                                    color: None,
                                                };
                                                if let Some(ref mut targets) = ps.def.morph_targets {
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
                                                    if let Some(ref mut targets) = ps.def.morph_targets {
                                                        while targets.len() <= slot as usize {
                                                            targets.push(crate::gpu::particle::types::MorphTargetDef {
                                                                source: String::new(), color: None,
                                                            });
                                                        }
                                                        targets[slot as usize] = def;
                                                    }
                                                    log::info!("Loaded morph text target into slot {}: \"{}\"", slot, text);
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
                                            ctx.data_mut(|d| d.remove_temp::<u32>(egui::Id::new("morph_selected_slot")));
                                        }
                                        // Snapshot needs &self on ps, so do it outside the morph borrow
                                        if morph_snapshot.is_some() {
                                            let slot = if let Some(s) = morph_selected_slot {
                                                s.min(3)
                                            } else {
                                                // Use def count to stay in sync with morph_targets vec
                                                (ps.def.morph_targets.as_ref().map_or(0, |t| t.len()) as u32).min(3)
                                            };
                                            let data = ps.snapshot_particles(
                                                &app.gpu.device,
                                                &app.gpu.queue,
                                            );
                                            if !data.is_empty() {
                                                if let Some(ref mut morph) = ps.morph_state {
                                                    morph.load_target(slot, data);
                                                }
                                                let def = crate::gpu::particle::types::MorphTargetDef {
                                                    source: "snapshot".to_string(),
                                                    color: None,
                                                };
                                                if let Some(ref mut targets) = ps.def.morph_targets {
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
                                                log::info!("Loaded morph snapshot into slot {}", slot);
                                            }
                                            ctx.data_mut(|d| d.remove_temp::<u32>(egui::Id::new("morph_selected_slot")));
                                        }
                                        if morph_load_img.is_some() {
                                            // Store pending flag + target slot for when result arrives
                                            let target_slot = morph_selected_slot;
                                            ctx.data_mut(|d| {
                                                d.insert_temp(egui::Id::new("morph_image_pending"), true);
                                                if let Some(s) = target_slot {
                                                    d.insert_temp(egui::Id::new("morph_pending_slot"), s);
                                                }
                                                d.remove_temp::<u32>(egui::Id::new("morph_selected_slot"));
                                            });
                                            app.particle_source_loader.open_image_dialog();
                                        }
                                        #[cfg(feature = "video")]
                                        if morph_load_video.is_some() {
                                            ctx.data_mut(|d| {
                                                d.insert_temp(egui::Id::new("morph_video_pending"), true);
                                                d.remove_temp::<u32>(egui::Id::new("morph_selected_slot"));
                                            });
                                            app.particle_source_loader.open_video_dialog();
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // Drain background particle source loader results
                    if let Some(result) = app.particle_source_loader.try_recv() {
                        // Check if this image/video was requested for a morph slot
                        let morph_pending: Option<bool> = app
                            .egui_overlay
                            .context()
                            .data_mut(|d| d.remove_temp(egui::Id::new("morph_image_pending")));
                        let morph_video_pending: Option<bool> = app
                            .egui_overlay
                            .context()
                            .data_mut(|d| d.remove_temp(egui::Id::new("morph_video_pending")));
                        let morph_pending_slot: Option<u32> = app
                            .egui_overlay
                            .context()
                            .data_mut(|d| d.remove_temp(egui::Id::new("morph_pending_slot")));
                        if let Some(layer) = app.layer_stack.active_mut() {
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
                                            if morph_pending.is_some() {
                                                if !aux.is_empty() {
                                                    if let Some(ref mut morph) = ps.morph_state {
                                                        let slot = morph_pending_slot.unwrap_or_else(|| {
                                                            (ps.def.morph_targets.as_ref().map_or(0, |t| t.len()) as u32).min(3)
                                                        });
                                                        morph.load_target(slot, aux);
                                                        let filename = std::path::Path::new(&path)
                                                            .file_name()
                                                            .map(|f| f.to_string_lossy().to_string())
                                                            .unwrap_or_default();
                                                        let def = crate::gpu::particle::types::MorphTargetDef {
                                                            source: format!("image:{}", filename),
                                                            color: None,
                                                        };
                                                        if let Some(ref mut targets) = ps.def.morph_targets {
                                                            while targets.len() <= slot as usize {
                                                                targets.push(crate::gpu::particle::types::MorphTargetDef {
                                                                    source: String::new(), color: None,
                                                                });
                                                            }
                                                            targets[slot as usize] = def;
                                                        }
                                                        log::info!(
                                                            "Loaded morph target image into slot {}: {} ({}x{})",
                                                            slot, path, width, height
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
                                                ps.image_source = crate::gpu::particle::ParticleImageSource::Static;
                                                ps.video_path = None;
                                                ps.static_image_path = Some(path.clone());
                                                // Update emitter image name so UI selector reflects the change
                                                let filename = std::path::Path::new(&path)
                                                    .file_name()
                                                    .map(|f| f.to_string_lossy().to_string())
                                                    .unwrap_or_default();
                                                ps.def.emitter.image = filename;
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
                                            if morph_video_pending.is_some() {
                                                // Load evenly-spaced frames into morph slots
                                                if let Some(ref mut morph) = ps.morph_state {
                                                    // When full, replace all 4 slots with video frames
                                                    let def_count = ps.def.morph_targets.as_ref().map_or(0, |t| t.len()) as u32;
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
                                                    for (i, (label, data)) in targets.into_iter().enumerate() {
                                                        let slot = start_slot + i as u32;
                                                        if slot < 4 && !data.is_empty() {
                                                            morph.load_target(slot, data);
                                                            let def = crate::gpu::particle::types::MorphTargetDef {
                                                                source: label.clone(),
                                                                color: None,
                                                            };
                                                            if let Some(ref mut defs) = ps.def.morph_targets {
                                                                while defs.len() <= slot as usize {
                                                                    defs.push(crate::gpu::particle::types::MorphTargetDef {
                                                                        source: String::new(), color: None,
                                                                    });
                                                                }
                                                                defs[slot as usize] = def;
                                                            }
                                                            log::info!("Loaded morph video frame into slot {}: {}", slot, label);
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
                                                    // Without video feature, use first frame as static
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
                                                    let _ = (path, delays_ms);
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
                    app.layer_stack.remove_layer(idx);
                    app.sync_active_layer();
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

                let layer_move: Option<(usize, usize)> = app
                    .egui_overlay
                    .context()
                    .data_mut(|d| d.remove_temp(egui::Id::new("layer_move")));
                if let Some((from, to)) = layer_move {
                    app.layer_stack.move_layer(from, to);
                    app.sync_active_layer();
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
                                    let _ = std::fs::write(path, &json);
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
                                                app.shader_editor.paired_disk_content = json;
                                            }
                                        }
                                    }
                                }
                            }
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
                    Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                        let w = app.gpu.surface_config.width;
                        let h = app.gpu.surface_config.height;
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

    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(winit::event_loop::ControlFlow::Poll);

    let mut app = PhosphorApp::new();
    event_loop.run_app(&mut app)?;

    Ok(())
}
