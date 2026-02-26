mod app;
mod audio;
mod effect;
mod gpu;
mod media;
mod midi;
mod osc;
mod params;
mod preset;
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
use winit::window::{Fullscreen, Window, WindowAttributes, WindowId};

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

        let attrs = WindowAttributes::default()
            .with_title("Phosphor")
            .with_inner_size(winit::dpi::LogicalSize::new(1280, 720));

        let window = Arc::new(event_loop.create_window(attrs).expect("Failed to create window"));
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
                event_loop.exit();
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
                        event_loop.exit();
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
                                playing: m.transport.playing,
                                looping: m.transport.looping,
                                speed: m.transport.speed,
                                direction: m.transport.direction,
                                current_frame: m.current_frame,
                            }
                        })
                    });

                    // Get active layer's param_store (mutable for MIDI badges)
                    let active_params = app.layer_stack.active_mut();
                    if let Some(layer) = active_params {
                        crate::ui::panels::draw_panels(
                            &ctx,
                            app.egui_overlay.visible,
                            &mut app.audio,
                            &mut layer.param_store,
                            &shader_error,
                            &app.uniforms,
                            &app.effect_loader,
                            &mut app.post_process.enabled,
                            particle_count,
                            &mut app.midi,
                            &mut app.osc,
                            &mut app.web,
                            &app.preset_store,
                            &layer_infos,
                            active_layer,
                            media_info,
                        );
                    }
                }
                app.egui_overlay.end_frame(&app.window);

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
                            if let Some(path) = rfd::FileDialog::new()
                                .add_filter("Images", &["png", "jpg", "jpeg", "gif", "bmp", "webp"])
                                .pick_file()
                            {
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
                    let num_effects = app.effect_loader.effects.len();
                    match trigger {
                        TriggerAction::NextEffect if num_effects > 0 => {
                            let current = app
                                .layer_stack
                                .active()
                                .and_then(|l| l.effect_index())
                                .unwrap_or(0);
                            app.load_effect((current + 1) % num_effects);
                        }
                        TriggerAction::PrevEffect if num_effects > 0 => {
                            let current = app
                                .layer_stack
                                .active()
                                .and_then(|l| l.effect_index())
                                .unwrap_or(0);
                            app.load_effect(
                                if current == 0 { num_effects - 1 } else { current - 1 },
                            );
                        }
                        TriggerAction::TogglePostProcess => {
                            app.post_process.enabled = !app.post_process.enabled;
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
