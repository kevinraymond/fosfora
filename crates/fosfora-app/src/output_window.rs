//! The second output window (#3122).
//!
//! A borderless window on a chosen display that shows the finished frame and
//! nothing else: the same display target the main window blits and the panels
//! preview, copied once more onto this window's swapchain. No interface is
//! drawn on it, and the only input it answers is the key that closes it.
//!
//! It exists so the laptop screen can keep the workspace while a projector or
//! a second monitor carries the output — the arrangement every VJ setup wants
//! and which a single full-screen window cannot give.

use std::sync::Arc;

use anyhow::{Result, anyhow};
use winit::event_loop::ActiveEventLoop;
use winit::monitor::MonitorHandle;
use winit::window::{Fullscreen, Window, WindowAttributes, WindowId};

use crate::gpu::context::GpuContext;

/// A display the output window can be opened on.
#[derive(Clone, PartialEq, Eq)]
pub struct DisplayInfo {
    /// What the platform calls it, or `Display 2` when it will not say.
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub refresh_hz: Option<u32>,
}

impl DisplayInfo {
    /// One line for a picker: name, size and refresh rate.
    pub fn label(&self) -> String {
        match self.refresh_hz {
            Some(hz) => format!("{} — {}×{} @ {hz} Hz", self.name, self.width, self.height),
            None => format!("{} — {}×{}", self.name, self.width, self.height),
        }
    }
}

fn display_name(monitor: &MonitorHandle, index: usize) -> String {
    monitor
        .name()
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| format!("Display {}", index + 1))
}

/// The displays the window system currently reports, in the order winit gives
/// them — the same order [`OutputWindow::open`] indexes.
pub fn displays(event_loop: &ActiveEventLoop) -> Vec<DisplayInfo> {
    event_loop
        .available_monitors()
        .enumerate()
        .map(|(i, m)| {
            let size = m.size();
            DisplayInfo {
                name: display_name(&m, i),
                width: size.width,
                height: size.height,
                refresh_hz: m
                    .refresh_rate_millihertz()
                    .map(|mhz| (mhz as f32 / 1000.0).round() as u32),
            }
        })
        .collect()
}

pub struct OutputWindow {
    pub window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    /// Where it is, for the UI to say so without re-enumerating displays.
    pub display_name: String,
}

impl OutputWindow {
    /// Open a borderless output window filling `display_index`.
    ///
    /// Fails rather than opening a window that cannot be drawn on: a surface
    /// whose formats do not include the one the blit pipeline was built for
    /// would fail validation on the first frame instead, which aborts the
    /// whole app.
    pub fn open(
        event_loop: &ActiveEventLoop,
        gpu: &GpuContext,
        display_index: usize,
    ) -> Result<Self> {
        let monitor = event_loop
            .available_monitors()
            .nth(display_index)
            .ok_or_else(|| anyhow!("display {} is gone", display_index + 1))?;
        let display_name = display_name(&monitor, display_index);

        let attrs = WindowAttributes::default()
            .with_title("Fosfora Output")
            .with_decorations(false)
            .with_fullscreen(Some(Fullscreen::Borderless(Some(monitor))));
        let window = Arc::new(event_loop.create_window(attrs)?);
        window.set_cursor_visible(false);

        let surface = gpu.instance.create_surface(window.clone())?;
        let capabilities = surface.get_capabilities(&gpu.adapter);
        if !capabilities.formats.contains(&gpu.format) {
            return Err(anyhow!(
                "display {display_name} offers no {:?} surface (has {:?})",
                gpu.format,
                capabilities.formats
            ));
        }

        // Not vsync. Both surfaces are presented from one thread in one frame,
        // so a second FIFO swapchain on a display with its own refresh clock
        // makes every frame wait for both — the main window then runs at the
        // beat frequency of the two. Mailbox presents the newest frame without
        // tearing and without blocking; IMMEDIATE is the tearing fallback, and
        // FIFO is taken only when the driver offers nothing else. Pacing comes
        // from the main window's vsync either way.
        let present_mode = [
            wgpu::PresentMode::Mailbox,
            wgpu::PresentMode::Immediate,
            wgpu::PresentMode::Fifo,
        ]
        .into_iter()
        .find(|m| capabilities.present_modes.contains(m))
        .unwrap_or(wgpu::PresentMode::Fifo);

        let size = window.inner_size();
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: gpu.format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode,
            desired_maximum_frame_latency: 2,
            // Opaque for the same reason the main window pins it: the composite
            // can carry alpha < 1 under the passthrough output mode, and a
            // non-opaque swapchain would make the output itself transparent.
            alpha_mode: if capabilities
                .alpha_modes
                .contains(&wgpu::CompositeAlphaMode::Opaque)
            {
                wgpu::CompositeAlphaMode::Opaque
            } else {
                capabilities.alpha_modes[0]
            },
            view_formats: vec![],
        };
        surface.configure(&gpu.device, &config);

        log::info!(
            "Output window open on {display_name} ({}×{}, present mode {present_mode:?})",
            config.width,
            config.height
        );

        Ok(Self {
            window,
            surface,
            config,
            display_name,
        })
    }

    pub fn id(&self) -> WindowId {
        self.window.id()
    }

    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        if width > 0 && height > 0 && (width, height) != (self.config.width, self.config.height) {
            self.config.width = width;
            self.config.height = height;
            self.surface.configure(device, &self.config);
        }
    }

    /// Take this frame's swapchain image, or `None` when the surface is not
    /// presentable right now. A lost or outdated surface is reconfigured and
    /// skipped for one frame; the main window's frame is never held up by it.
    pub fn acquire(
        &mut self,
        device: &wgpu::Device,
    ) -> Option<(wgpu::SurfaceTexture, wgpu::TextureView)> {
        match self.surface.get_current_texture() {
            Ok(frame) => {
                let view = frame
                    .texture
                    .create_view(&wgpu::TextureViewDescriptor::default());
                Some((frame, view))
            }
            Err(e @ (wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated)) => {
                log::debug!("Output window surface {e:?} — reconfiguring");
                self.surface.configure(device, &self.config);
                None
            }
            Err(e) => {
                log::warn!("Output window surface error: {e}");
                None
            }
        }
    }
}
