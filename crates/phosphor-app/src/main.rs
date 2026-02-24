mod app;
mod audio;
mod effect;
mod gpu;
mod params;
mod shader;
mod ui;

use std::sync::Arc;

use anyhow::Result;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Fullscreen, Window, WindowAttributes, WindowId};

use app::App;

struct PhosphorApp {
    app: Option<App>,
    window: Option<Arc<Window>>,
}

impl PhosphorApp {
    fn new() -> Self {
        Self {
            app: None,
            window: None,
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
                    _ => {}
                }
            }
            WindowEvent::RedrawRequested => {
                app.update();

                // Prepare egui frame
                app.egui_overlay.begin_frame(&app.window);
                {
                    let ctx = app.egui_overlay.context();
                    let particle_count = app
                        .pass_executor
                        .particle_system
                        .as_ref()
                        .map(|ps| ps.max_particles);
                    crate::ui::panels::draw_panels(
                        &ctx,
                        app.egui_overlay.visible,
                        &mut app.audio,
                        &mut app.param_store,
                        &app.shader_error,
                        &app.uniforms,
                        &app.effect_loader,
                        &mut app.post_process.enabled,
                        particle_count,
                    );
                }
                app.egui_overlay.end_frame(&app.window);

                // Handle effect loading from UI
                let pending: Option<usize> = app.egui_overlay.context().data_mut(|d| {
                    d.remove_temp(egui::Id::new("pending_effect"))
                });
                if let Some(idx) = pending.or(app.egui_overlay.pending_effect_load.take()) {
                    app.load_effect(idx);
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
