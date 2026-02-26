mod app;
mod audio;
mod effect;
mod gpu;
mod media;
mod midi;
#[cfg(feature = "ndi")]
mod ndi;
mod osc;
mod params;
mod preset;
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
use gpu::layer::BlendMode;

struct PhosphorApp {
    app: Option<App>,
    window: Option<Arc<Window>>,
    file_dialog_rx: Option<Receiver<PathBuf>>,
}

impl PhosphorApp {
    fn new() -> Self {
        Self {
            app: None,
            window: None,
            file_dialog_rx: None,
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
            .with_inner_size(winit::dpi::LogicalSize::new(1280, 720));
        if let Some(icon) = load_window_icon() {
            attrs = attrs.with_window_icon(Some(icon));
        }

        let window = Arc::new(event_loop.create_window(attrs).expect("Failed to create window"));

        // Center window on primary monitor
        if let Some(monitor) = event_loop
            .primary_monitor()
            .or_else(|| event_loop.available_monitors().next())
        {
            let monitor_size = monitor.size();
            let window_size = window.outer_size();
            let monitor_pos = monitor.position();
            let x = (monitor_size.width.saturating_sub(window_size.width)) / 2;
            let y = (monitor_size.height.saturating_sub(window_size.height)) / 2;
            window.set_outer_position(winit::dpi::PhysicalPosition::new(
                monitor_pos.x + x as i32,
                monitor_pos.y + y as i32,
            ));
        }

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
                        // If shader editor is open, it handles Esc internally
                        if !app.shader_editor.open {
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

                // Prepare egui frame
                app.egui_overlay.begin_frame(&app.window);
                {
                    let ctx = app.egui_overlay.context();

                    // Get particle count from active layer
                    let particle_count = app
                        .layer_stack
                        .active()
                        .and_then(|l| l.as_effect())
                        .and_then(|e| e.pass_executor.particle_system.as_ref())
                        .map(|ps| ps.max_particles);

                    // Get active layer's shader error
                    let shader_error = app
                        .layer_stack
                        .active()
                        .and_then(|l| l.shader_error().map(|s| s.to_string()));

                    // Collect media info if active layer is media (before mutable borrow)
                    let media_info = app.layer_stack.active().and_then(|l| {
                        l.as_media().map(|m| {
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
                        };
                        ctx.data_mut(|d| {
                            d.insert_temp(egui::Id::new("ndi_info"), ndi_info);
                            d.insert_temp(egui::Id::new("ndi_running"), app.ndi.is_running());
                        });
                    }

                    // Sync compile errors into shader editor
                    if app.shader_editor.open {
                        app.shader_editor.compile_error = app
                            .layer_stack
                            .active()
                            .and_then(|l| l.shader_error().map(|s| s.to_string()));
                    }

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
                                &app.preset_store,
                                &layer_infos,
                                active_layer,
                                media_info,
                                &app.status_error,
                                app.settings.theme,
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

                    // Draw quit confirmation dialog
                    if app.quit_requested {
                        // Track whether dialog was already showing last frame.
                        // On the first frame, the Esc that opened it is still in input state,
                        // so skip Esc-to-cancel until the next frame.
                        let dialog_id = egui::Id::new("quit_dialog_shown");
                        let was_shown: bool = ctx.data(|d| d.get_temp(dialog_id).unwrap_or(false));
                        ctx.data_mut(|d| d.insert_temp(dialog_id, true));

                        egui::Window::new("Quit Phosphor?")
                            .collapsible(false)
                            .resizable(false)
                            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                            .show(&ctx, |ui| {
                                ui.label("Are you sure you want to quit?");
                                ui.add_space(8.0);
                                ui.horizontal(|ui| {
                                    if ui.button("Quit").clicked() {
                                        ui.ctx().data_mut(|d| {
                                            d.insert_temp(egui::Id::new("confirm_quit"), true);
                                        });
                                    }
                                    let esc_cancel = was_shown
                                        && ui.input(|i| i.key_pressed(egui::Key::Escape));
                                    if ui.button("Cancel").clicked() || esc_cancel {
                                        app.quit_requested = false;
                                    }
                                });
                            });
                    } else {
                        // Clear the flag when dialog is dismissed
                        ctx.data_mut(|d| d.remove_temp::<bool>(egui::Id::new("quit_dialog_shown")));
                    }
                }
                app.egui_overlay.end_frame(&app.window);

                // Handle quit confirmation
                let confirm_quit: Option<bool> = app.egui_overlay.context().data_mut(|d| {
                    d.remove_temp(egui::Id::new("confirm_quit"))
                });
                if confirm_quit.is_some() {
                    event_loop.exit();
                }

                // Handle shader editor signals
                let open_editor: Option<bool> = app.egui_overlay.context().data_mut(|d| {
                    d.remove_temp(egui::Id::new("open_shader_editor"))
                });
                if open_editor.is_some() {
                    // Resolve active layer's shader path
                    if let Some(idx) = app
                        .layer_stack
                        .active()
                        .and_then(|l| l.effect_index())
                    {
                        if let Some(effect) = app.effect_loader.effects.get(idx).cloned() {
                            let passes = effect.normalized_passes();
                            if let Some(pass) = passes.first() {
                                let path = app.effect_loader.resolve_shader_path(&pass.shader);
                                if let Ok(content) = std::fs::read_to_string(&path) {
                                    app.shader_editor.open_file(&effect.name, path, content);
                                } else {
                                    log::error!("Could not read shader: {}", path.display());
                                }
                            }
                        }
                    }
                }

                let save_editor: Option<bool> = app.egui_overlay.context().data_mut(|d| {
                    d.remove_temp(egui::Id::new("shader_editor_save"))
                });
                if save_editor.is_some() {
                    if let Some(ref path) = app.shader_editor.file_path {
                        match std::fs::write(path, &app.shader_editor.code) {
                            Ok(()) => {
                                app.shader_editor.disk_content = app.shader_editor.code.clone();
                                log::info!("Saved shader: {}", path.display());
                            }
                            Err(e) => {
                                log::error!("Failed to save shader: {e}");
                                app.status_error = Some((format!("Save failed: {e}"), std::time::Instant::now()));
                            }
                        }
                    }
                }

                let new_prompt: Option<bool> = app.egui_overlay.context().data_mut(|d| {
                    d.remove_temp(egui::Id::new("new_effect_prompt"))
                });
                if new_prompt.is_some() {
                    app.shader_editor.new_effect_prompt = true;
                }

                // Handle "Copy Shader" prompt for built-in effects
                let copy_prompt: Option<bool> = app.egui_overlay.context().data_mut(|d| {
                    d.remove_temp(egui::Id::new("copy_builtin_prompt"))
                });
                if copy_prompt.is_some() {
                    app.shader_editor.new_effect_prompt = true;
                    app.shader_editor.copy_builtin_mode = true;
                }

                // Handle copy built-in effect creation
                let copy_effect: Option<String> = app.egui_overlay.context().data_mut(|d| {
                    d.remove_temp(egui::Id::new("create_copy_effect"))
                });
                if let Some(name) = copy_effect {
                    if let Err(e) = app.copy_builtin_effect(&name) {
                        log::error!("Failed to copy effect: {e}");
                        app.status_error = Some((format!("Copy failed: {e}"), std::time::Instant::now()));
                    }
                }

                // Handle delete effect signal
                let delete_effect: Option<usize> = app.egui_overlay.context().data_mut(|d| {
                    d.remove_temp(egui::Id::new("delete_effect"))
                });
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
                            app.status_error = Some((format!("Delete failed: {e}"), std::time::Instant::now()));
                        }
                    }
                }

                let create_effect: Option<String> = app.egui_overlay.context().data_mut(|d| {
                    d.remove_temp(egui::Id::new("create_new_effect"))
                });
                if let Some(name) = create_effect {
                    if let Err(e) = app.create_new_effect(&name) {
                        log::error!("Failed to create effect: {e}");
                        app.status_error = Some((format!("Create failed: {e}"), std::time::Instant::now()));
                    }
                }

                // Handle theme change from settings panel
                let set_theme: Option<crate::ui::theme::ThemeMode> =
                    app.egui_overlay.context().data_mut(|d| {
                        d.remove_temp(egui::Id::new("set_theme"))
                    });
                if let Some(theme) = set_theme {
                    app.egui_overlay.set_theme(theme);
                    app.settings.theme = theme;
                    app.settings.save();
                }

                // Handle audio device switch from UI
                let switch_audio: Option<String> = app.egui_overlay.context().data_mut(|d| {
                    d.remove_temp(egui::Id::new("switch_audio_device"))
                });
                if let Some(device_str) = switch_audio {
                    let device_name = if device_str.is_empty() { None } else { Some(device_str.as_str()) };
                    app.audio.switch_device(device_name);
                    app.settings.audio_device = if device_str.is_empty() { None } else { Some(device_str) };
                    app.settings.save();
                }

                // Handle NDI signals from UI
                #[cfg(feature = "ndi")]
                {
                    let ndi_enable: Option<bool> = app.egui_overlay.context().data_mut(|d| {
                        d.remove_temp(egui::Id::new("ndi_set_enabled"))
                    });
                    if let Some(enabled) = ndi_enable {
                        app.ndi.set_enabled(
                            enabled,
                            &app.gpu.device,
                            app.gpu.format,
                            app.gpu.surface_config.width,
                            app.gpu.surface_config.height,
                        );
                    }

                    let ndi_source: Option<String> = app.egui_overlay.context().data_mut(|d| {
                        d.remove_temp(egui::Id::new("ndi_source_name"))
                    });
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

                    let ndi_res: Option<u8> = app.egui_overlay.context().data_mut(|d| {
                        d.remove_temp(egui::Id::new("ndi_resolution_change"))
                    });
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

                    let ndi_restart: Option<bool> = app.egui_overlay.context().data_mut(|d| {
                        d.remove_temp(egui::Id::new("ndi_restart"))
                    });
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
                let pending: Option<usize> = app.egui_overlay.context().data_mut(|d| {
                    d.remove_temp(egui::Id::new("pending_effect"))
                });
                if let Some(idx) = pending.or(app.egui_overlay.pending_effect_load.take()) {
                    let active_locked = app.layer_stack.active().map_or(false, |l| l.locked);
                    if !active_locked {
                        app.load_effect(idx);
                        app.preset_store.mark_dirty();
                    }
                }

                // Handle preset signals from UI
                let pending_preset: Option<usize> = app.egui_overlay.context().data_mut(|d| {
                    d.remove_temp(egui::Id::new("pending_preset"))
                });
                if let Some(idx) = pending_preset {
                    app.load_preset(idx);
                }
                let save_preset: Option<String> = app.egui_overlay.context().data_mut(|d| {
                    d.remove_temp(egui::Id::new("save_preset"))
                });
                if let Some(name) = save_preset {
                    app.save_preset(&name);
                }
                let delete_preset: Option<usize> = app.egui_overlay.context().data_mut(|d| {
                    d.remove_temp(egui::Id::new("delete_preset"))
                });
                if let Some(idx) = delete_preset {
                    if let Err(e) = app.preset_store.delete(idx) {
                        log::error!("Failed to delete preset: {e}");
                    }
                }
                let deselect_preset: Option<bool> = app.egui_overlay.context().data_mut(|d| {
                    d.remove_temp(egui::Id::new("deselect_preset"))
                });
                if deselect_preset.is_some() {
                    app.preset_store.current_preset = None;
                    app.preset_store.dirty = false;
                }

                // Handle media layer signals
                let add_media: Option<bool> = app.egui_overlay.context().data_mut(|d| {
                    d.remove_temp(egui::Id::new("add_media_layer"))
                });
                if add_media.is_some() && self.file_dialog_rx.is_none() {
                    let (tx, rx) = crossbeam_channel::bounded(1);
                    self.file_dialog_rx = Some(rx);
                    std::thread::Builder::new()
                        .name("file-dialog".into())
                        .spawn(move || {
                            let mut dialog = rfd::FileDialog::new()
                                .add_filter("Images", &["png", "jpg", "jpeg", "gif", "bmp", "webp"]);
                            #[cfg(feature = "video")]
                            {
                                if crate::media::video::ffmpeg_available() {
                                    dialog = dialog.add_filter(
                                        "Video",
                                        crate::media::decoder::VIDEO_EXTENSIONS,
                                    );
                                }
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

                // Handle media transport signals
                let play_pause: Option<bool> = app.egui_overlay.context().data_mut(|d| {
                    d.remove_temp(egui::Id::new("media_play_pause"))
                });
                if play_pause.is_some() {
                    if let Some(layer) = app.layer_stack.active_mut() {
                        if let Some(m) = layer.as_media_mut() {
                            m.transport.playing = !m.transport.playing;
                        }
                    }
                }
                let media_loop: Option<bool> = app.egui_overlay.context().data_mut(|d| {
                    d.remove_temp(egui::Id::new("media_loop"))
                });
                if let Some(looping) = media_loop {
                    if let Some(layer) = app.layer_stack.active_mut() {
                        if let Some(m) = layer.as_media_mut() {
                            m.transport.looping = looping;
                        }
                    }
                }
                let media_speed: Option<f32> = app.egui_overlay.context().data_mut(|d| {
                    d.remove_temp(egui::Id::new("media_speed"))
                });
                if let Some(speed) = media_speed {
                    if let Some(layer) = app.layer_stack.active_mut() {
                        if let Some(m) = layer.as_media_mut() {
                            m.transport.speed = speed;
                        }
                    }
                }
                let media_direction: Option<u8> = app.egui_overlay.context().data_mut(|d| {
                    d.remove_temp(egui::Id::new("media_direction"))
                });
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
                let media_seek: Option<f64> = app.egui_overlay.context().data_mut(|d| {
                    d.remove_temp(egui::Id::new("media_seek"))
                });
                if let Some(secs) = media_seek {
                    if let Some(layer) = app.layer_stack.active_mut() {
                        if let Some(m) = layer.as_media_mut() {
                            m.seek_to_secs(secs);
                        }
                    }
                }

                // Handle layer UI signals
                let add_layer: Option<bool> = app.egui_overlay.context().data_mut(|d| {
                    d.remove_temp(egui::Id::new("add_layer"))
                });
                if add_layer.is_some() {
                    app.add_layer();
                    app.preset_store.mark_dirty();
                }

                let remove_layer: Option<usize> = app.egui_overlay.context().data_mut(|d| {
                    d.remove_temp(egui::Id::new("remove_layer"))
                });
                if let Some(idx) = remove_layer {
                    app.layer_stack.remove_layer(idx);
                    app.sync_active_layer();
                    app.preset_store.mark_dirty();
                }

                // Handle clear all layers
                let clear_all: Option<bool> = app.egui_overlay.context().data_mut(|d| {
                    d.remove_temp(egui::Id::new("clear_all_layers"))
                });
                if clear_all.is_some() {
                    app.clear_all_layers();
                    app.preset_store.mark_dirty();
                }

                // Handle layer rename
                let layer_rename: Option<(usize, Option<String>)> = app.egui_overlay.context().data_mut(|d| {
                    d.remove_temp(egui::Id::new("layer_rename"))
                });
                if let Some((idx, new_name)) = layer_rename {
                    if let Some(layer) = app.layer_stack.layers.get_mut(idx) {
                        layer.custom_name = new_name;
                        app.preset_store.mark_dirty();
                    }
                }

                let select_layer: Option<usize> = app.egui_overlay.context().data_mut(|d| {
                    d.remove_temp(egui::Id::new("select_layer"))
                });
                if let Some(idx) = select_layer {
                    if idx < app.layer_stack.layers.len() {
                        app.layer_stack.active_layer = idx;
                        app.sync_active_layer();
                    }
                }

                // Handle lock/pin toggles
                let toggle_lock: Option<(usize, bool)> = app.egui_overlay.context().data_mut(|d| {
                    d.remove_temp(egui::Id::new("layer_toggle_lock"))
                });
                if let Some((idx, locked)) = toggle_lock {
                    if let Some(layer) = app.layer_stack.layers.get_mut(idx) {
                        layer.locked = locked;
                        app.preset_store.mark_dirty();
                    }
                }

                let toggle_pin: Option<(usize, bool)> = app.egui_overlay.context().data_mut(|d| {
                    d.remove_temp(egui::Id::new("layer_toggle_pin"))
                });
                if let Some((idx, pinned)) = toggle_pin {
                    if let Some(layer) = app.layer_stack.layers.get_mut(idx) {
                        layer.pinned = pinned;
                        app.preset_store.mark_dirty();
                    }
                }

                let layer_blend: Option<u32> = app.egui_overlay.context().data_mut(|d| {
                    d.remove_temp(egui::Id::new("layer_blend"))
                });
                if let Some(mode_u32) = layer_blend {
                    if let Some(layer) = app.layer_stack.active_mut() {
                        if !layer.locked {
                            layer.blend_mode = match mode_u32 {
                                0 => BlendMode::Normal,
                                1 => BlendMode::Add,
                                2 => BlendMode::Multiply,
                                3 => BlendMode::Screen,
                                4 => BlendMode::Overlay,
                                5 => BlendMode::SoftLight,
                                6 => BlendMode::Difference,
                                _ => BlendMode::Normal,
                            };
                            app.preset_store.mark_dirty();
                        }
                    }
                }

                let layer_opacity: Option<f32> = app.egui_overlay.context().data_mut(|d| {
                    d.remove_temp(egui::Id::new("layer_opacity"))
                });
                if let Some(opacity) = layer_opacity {
                    if let Some(layer) = app.layer_stack.active_mut() {
                        if !layer.locked {
                            layer.opacity = opacity;
                            app.preset_store.mark_dirty();
                        }
                    }
                }

                let layer_move: Option<(usize, usize)> = app.egui_overlay.context().data_mut(|d| {
                    d.remove_temp(egui::Id::new("layer_move"))
                });
                if let Some((from, to)) = layer_move {
                    app.layer_stack.move_layer(from, to);
                    app.sync_active_layer();
                    app.preset_store.mark_dirty();
                }

                let toggle_enable: Option<(usize, bool)> = app.egui_overlay.context().data_mut(|d| {
                    d.remove_temp(egui::Id::new("layer_toggle_enable"))
                });
                if let Some((idx, enabled)) = toggle_enable {
                    if let Some(layer) = app.layer_stack.layers.get_mut(idx) {
                        if !layer.locked {
                            layer.enabled = enabled;
                            app.preset_store.mark_dirty();
                        }
                    }
                }

                // Check if active layer params changed (marks preset dirty)
                if let Some(layer) = app.layer_stack.active_mut() {
                    if layer.param_store.changed {
                        layer.param_store.changed = false;
                        app.preset_store.mark_dirty();
                    }
                }

                // Handle MIDI + OSC triggers
                let mut triggers: Vec<_> = app.pending_midi_triggers.drain(..).collect();
                triggers.extend(app.pending_osc_triggers.drain(..));
                triggers.extend(app.pending_web_triggers.drain(..));
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

    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(winit::event_loop::ControlFlow::Poll);

    let mut app = PhosphorApp::new();
    event_loop.run_app(&mut app)?;

    Ok(())
}
