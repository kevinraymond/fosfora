use std::sync::Arc;

use egui::Context;
use winit::event::WindowEvent;
use winit::window::Window;

use super::theme::ThemeMode;
use super::theme::colors::set_theme_colors;

const INTER_REGULAR: &[u8] = include_bytes!("../../../../assets/fonts/Inter-Regular.ttf");
const INTER_BOLD: &[u8] = include_bytes!("../../../../assets/fonts/Inter-Bold.ttf");
const JETBRAINS_MONO: &[u8] = include_bytes!("../../../../assets/fonts/JetBrainsMono-Regular.ttf");

pub struct EguiOverlay {
    pub state: egui_winit::State,
    pub renderer: egui_wgpu::Renderer,
    pub visible: bool,
    pub theme: ThemeMode,
    pub pending_effect_load: Option<usize>,
    shapes: Vec<egui::ClippedPrimitive>,
    textures_delta: egui::TexturesDelta,
    screen_descriptor: egui_wgpu::ScreenDescriptor,
}

impl EguiOverlay {
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        window: &Window,
        theme: ThemeMode,
    ) -> Self {
        let ctx = Context::default();
        ctx.set_visuals(theme.visuals());
        set_theme_colors(&ctx, theme.colors());

        // Register bundled fonts (Inter proportional, JetBrains Mono monospace)
        let mut fonts = egui::FontDefinitions::default();
        fonts.font_data.insert(
            "Inter-Regular".into(),
            Arc::new(egui::FontData::from_static(INTER_REGULAR)),
        );
        fonts.font_data.insert(
            "Inter-Bold".into(),
            Arc::new(egui::FontData::from_static(INTER_BOLD)),
        );
        fonts.font_data.insert(
            "JetBrainsMono".into(),
            Arc::new(egui::FontData::from_static(JETBRAINS_MONO)),
        );
        fonts
            .families
            .get_mut(&egui::FontFamily::Proportional)
            .unwrap()
            .insert(0, "Inter-Regular".into());
        fonts
            .families
            .get_mut(&egui::FontFamily::Monospace)
            .unwrap()
            .insert(0, "JetBrainsMono".into());
        ctx.set_fonts(fonts);

        // Dense VJ typography and spacing
        let mut style = (*ctx.style()).clone();
        style.spacing.interact_size = egui::vec2(
            super::theme::tokens::MIN_INTERACT_WIDTH,
            super::theme::tokens::MIN_INTERACT_HEIGHT,
        );
        style.spacing.item_spacing = egui::vec2(
            super::theme::tokens::SPACING,
            super::theme::tokens::SPACING_Y,
        );
        style.spacing.button_padding = egui::vec2(6.0, 2.0);
        style.text_styles.insert(
            egui::TextStyle::Body,
            egui::FontId::proportional(super::theme::tokens::BODY_SIZE),
        );
        style.text_styles.insert(
            egui::TextStyle::Small,
            egui::FontId::proportional(super::theme::tokens::SMALL_SIZE),
        );
        style.text_styles.insert(
            egui::TextStyle::Heading,
            egui::FontId::proportional(super::theme::tokens::HEADING_SIZE),
        );
        style.text_styles.insert(
            egui::TextStyle::Monospace,
            egui::FontId::monospace(super::theme::tokens::MONO_SIZE),
        );
        ctx.set_style(style);

        let viewport_id = ctx.viewport_id();
        let state = egui_winit::State::new(ctx, viewport_id, window, None, None, None);

        let renderer = egui_wgpu::Renderer::new(
            device,
            format,
            egui_wgpu::RendererOptions {
                msaa_samples: 1,
                ..Default::default()
            },
        );

        let size = window.inner_size();
        let screen_descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [size.width, size.height],
            pixels_per_point: window.scale_factor() as f32,
        };

        Self {
            state,
            renderer,
            visible: false,
            theme,
            pending_effect_load: None,
            shapes: Vec::new(),
            textures_delta: egui::TexturesDelta::default(),
            screen_descriptor,
        }
    }

    pub fn set_theme(&mut self, theme: ThemeMode) {
        self.theme = theme;
        let ctx = self.state.egui_ctx();
        ctx.set_visuals(theme.visuals());
        set_theme_colors(ctx, theme.colors());
    }

    pub fn handle_event(&mut self, window: &Window, event: &WindowEvent) -> bool {
        self.state.on_window_event(window, event).consumed
    }

    pub fn wants_keyboard(&self) -> bool {
        self.state.egui_ctx().wants_keyboard_input()
    }

    pub fn wants_mouse(&self) -> bool {
        self.state.egui_ctx().wants_pointer_input()
    }

    pub fn toggle_visible(&mut self) {
        self.visible = !self.visible;
    }

    pub fn context(&self) -> Context {
        self.state.egui_ctx().clone()
    }

    pub fn resize(&mut self, width: u32, height: u32, pixels_per_point: f32) {
        self.screen_descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [width, height],
            pixels_per_point,
        };
    }

    pub fn begin_frame(&mut self, window: &Window) {
        // Refresh theme colors each frame so panels always have them
        set_theme_colors(self.state.egui_ctx(), self.theme.colors());
        let raw_input = self.state.take_egui_input(window);
        self.state.egui_ctx().begin_pass(raw_input);
    }

    pub fn end_frame(&mut self, window: &Window) {
        let output = self.state.egui_ctx().end_pass();
        self.state
            .handle_platform_output(window, output.platform_output);
        self.shapes = self
            .state
            .egui_ctx()
            .tessellate(output.shapes, output.pixels_per_point);
        self.textures_delta = output.textures_delta;
    }

    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
    ) {
        // Update textures
        for (id, delta) in &self.textures_delta.set {
            self.renderer.update_texture(device, queue, *id, delta);
        }

        self.renderer.update_buffers(
            device,
            queue,
            encoder,
            &self.shapes,
            &self.screen_descriptor,
        );

        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("egui-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load, // Don't clear — render on top
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            let mut pass = pass.forget_lifetime();
            self.renderer
                .render(&mut pass, &self.shapes, &self.screen_descriptor);
        }

        // Free old textures
        for id in &self.textures_delta.free {
            self.renderer.free_texture(id);
        }
    }
}
