//! The in-headset debug panel: an egui UI rendered into a texture that
//! `Gfx` draws on a quad above the left palm (`palm_panel.rs` places it and
//! turns the right index fingertip into a pointer). It shows the frame
//! timing, the effect, hands, anchor and audio, and has buttons and sliders
//! for what can change without a restart. Nothing on it is told apart by
//! hue alone.

use std::collections::VecDeque;

use egui::{Color32, Pos2, Rect, RichText, Stroke, Vec2};
use glam::{Quat, Vec3};

use crate::gfx::{Gfx, PanelPose};
use crate::palm_panel::{PalmPanel, Touch};
use crate::perf::PerfSample;

/// Texture size (pixels) and egui scale: 3200 px per meter on the 20 x 30 cm
/// panel, about the display's density at arm's length. The width keeps a
/// row at a multiple of 256 bytes, which a texture-to-buffer copy (`dump`)
/// requires.
const TEX_W: u32 = 640;
const TEX_H: u32 = 960;
const _: () = assert!((TEX_W * 4).is_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT));
const PIXELS_PER_POINT: f32 = 1.6;
/// Frame-time history for the graph: two seconds at 72 Hz.
const HISTORY: usize = 144;
/// The 72 Hz frame budget (ms), drawn on the graph.
const BUDGET_MS: f32 = 1000.0 / 72.0;
const GRAPH_MAX_MS: f32 = 20.0;

/// Values the panel's sliders change; the app applies them every frame.
#[derive(Debug, Clone, Copy)]
pub struct Controls {
    pub gravity: f32,
    pub near_fade: f32,
    pub hand_pad: f32,
    pub hand_kick: f32,
}

/// What the panel's buttons asked for this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    PrevEffect,
    NextEffect,
    /// Put the anchor back around the wearer.
    Recenter,
}

/// Frame-loop numbers from the last one-second window.
#[derive(Debug, Clone, Copy, Default)]
pub struct FrameWindow {
    pub fps: f32,
    pub display_hz: f32,
    pub max_interval_ms: f32,
    pub cpu_avg_ms: f32,
    pub long_frames: u32,
}

/// Everything the panel shows this frame.
pub struct View<'a> {
    pub mode: &'a str,
    pub effect: Option<(&'a str, usize, usize)>,
    pub alive: Option<u32>,
    pub frames: FrameWindow,
    pub perf: PerfSample,
    pub tracked: [bool; 2],
    pub tip_mm: [Option<f32>; 2],
    pub pinching: [bool; 2],
    pub gesture: String,
    pub anchor: [f32; 3],
    pub room_boxes: usize,
    pub rms: f32,
    pub bass: f32,
    pub beat: f32,
    pub audio: &'a str,
}

pub struct Hud {
    placement: PalmPanel,
    touch: Touch,
    was_pressed: bool,
    ctx: egui::Context,
    renderer: egui_wgpu::Renderer,
    /// Owns the image `Gfx`'s panel bind group samples.
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    started: std::time::Instant,
    gpu_history: VecDeque<f32>,
    /// Whether the panel was showing, to log changes only.
    shown_logged: bool,
}

impl Hud {
    /// Build the panel and hand its texture to `gfx`.
    pub fn new(gfx: &mut Gfx) -> Self {
        let texture = gfx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("xr-hud"),
            size: wgpu::Extent3d {
                width: TEX_W,
                height: TEX_H,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        gfx.set_panel(&view);
        let renderer = egui_wgpu::Renderer::new(
            &gfx.device,
            wgpu::TextureFormat::Rgba8UnormSrgb,
            egui_wgpu::RendererOptions::default(),
        );
        let ctx = egui::Context::default();
        ctx.style_mut(|s| {
            s.visuals = egui::Visuals::dark();
            s.spacing.slider_width = 150.0;
            s.spacing.button_padding = Vec2::new(10.0, 6.0);
            s.spacing.item_spacing = Vec2::new(8.0, 5.0);
        });
        Self {
            placement: PalmPanel::default(),
            touch: Touch::default(),
            was_pressed: false,
            ctx,
            renderer,
            texture,
            view,
            started: std::time::Instant::now(),
            gpu_history: VecDeque::with_capacity(HISTORY),
            shown_logged: false,
        }
    }

    /// Place the panel for this frame's left palm and head, and read the
    /// right index fingertip against it. Returns whether it is showing.
    pub fn place(
        &mut self,
        gfx: &Gfx,
        palm: Option<([f32; 3], [f32; 4])>,
        right_tip: Option<[f32; 4]>,
        head: [f32; 3],
    ) -> bool {
        let palm = palm.map(|(p, q)| (Vec3::from(p), Quat::from_array(q)));
        let placement = self.placement.place(palm, Vec3::from(head));
        self.touch = self
            .placement
            .touch(right_tip.map(|t| (Vec3::new(t[0], t[1], t[2]), t[3])));
        gfx.set_panel_pose(placement.map(|p| {
            let (right, up) = p.half_vectors();
            PanelPose {
                center: p.center.to_array(),
                right: right.to_array(),
                up: up.to_array(),
            }
        }));
        let shown = placement.is_some();
        if shown != self.shown_logged {
            self.shown_logged = shown;
            log::info!("debug panel {}", if shown { "shown" } else { "hidden" });
        }
        shown
    }

    /// Diagnostic (`debug.fosfora.hudtest`): the panel parked 45 cm ahead
    /// of the view and a little below, untracked, for a screencap.
    pub fn place_parked(&mut self, gfx: &Gfx, head: [f32; 3], head_rot: [f32; 4]) {
        let head = Vec3::from(head);
        let ahead = Quat::from_array(head_rot) * Vec3::NEG_Z;
        let p = crate::palm_panel::Placement::facing(head + ahead * 0.45 - Vec3::Y * 0.05, head);
        let (right, up) = p.half_vectors();
        gfx.set_panel_pose(Some(PanelPose {
            center: p.center.to_array(),
            right: right.to_array(),
            up: up.to_array(),
        }));
        self.touch = Touch::default();
    }

    /// Diagnostic: copy the panel texture to `path` as raw RGBA8 rows
    /// (`TEX_W` x `TEX_H`, sRGB), for checking the layout pixel-exact off
    /// the device. Blocks until the copy is read back.
    pub fn dump(&self, gfx: &Gfx, path: &std::path::Path) -> anyhow::Result<()> {
        let bytes_per_row = TEX_W * 4;
        let buffer = gfx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("xr-hud-dump"),
            size: u64::from(bytes_per_row * TEX_H),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = gfx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("xr-hud-dump"),
            });
        encoder.copy_texture_to_buffer(
            self.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(TEX_H),
                },
            },
            wgpu::Extent3d {
                width: TEX_W,
                height: TEX_H,
                depth_or_array_layers: 1,
            },
        );
        gfx.queue.submit([encoder.finish()]);
        let slice = buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        gfx.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        std::fs::write(path, &*slice.get_mapped_range())?;
        log::info!(
            "debug panel dumped to {} ({TEX_W}x{TEX_H} RGBA8)",
            path.display()
        );
        Ok(())
    }

    /// Record this frame's GPU time for the graph (every frame, shown or not).
    pub fn record(&mut self, perf: &PerfSample) {
        if let Some(ms) = perf.app_gpu_ms {
            if self.gpu_history.len() == HISTORY {
                self.gpu_history.pop_front();
            }
            self.gpu_history.push_back(ms);
        }
    }

    /// Run the UI and render it into the panel texture (only while shown).
    pub fn render(&mut self, gfx: &Gfx, view: &View<'_>, controls: &mut Controls) -> Vec<Action> {
        let size = Vec2::new(
            TEX_W as f32 / PIXELS_PER_POINT,
            TEX_H as f32 / PIXELS_PER_POINT,
        );
        let mut raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
            time: Some(self.started.elapsed().as_secs_f64()),
            focused: true,
            ..egui::RawInput::default()
        };
        raw.viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .native_pixels_per_point = Some(PIXELS_PER_POINT);
        match self.touch.pointer {
            Some([u, v]) => {
                let pos = Pos2::new(u * size.x, v * size.y);
                raw.events.push(egui::Event::PointerMoved(pos));
                if self.touch.pressed != self.was_pressed {
                    raw.events.push(egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed: self.touch.pressed,
                        modifiers: egui::Modifiers::NONE,
                    });
                }
            }
            None => {
                if self.was_pressed {
                    // Lift the press where it was before the pointer goes.
                    if let Some(pos) = self.ctx.input(|i| i.pointer.latest_pos()) {
                        raw.events.push(egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed: false,
                            modifiers: egui::Modifiers::NONE,
                        });
                    }
                }
                raw.events.push(egui::Event::PointerGone);
            }
        }
        self.was_pressed = self.touch.pressed && self.touch.pointer.is_some();

        let mut actions = Vec::new();
        let history: Vec<f32> = self.gpu_history.iter().copied().collect();
        let touch = self.touch;
        let output = self.ctx.run(raw, |ctx| {
            egui::CentralPanel::default()
                .frame(
                    egui::Frame::new()
                        .fill(Color32::from_rgba_unmultiplied(18, 18, 22, 245))
                        .inner_margin(10.0)
                        .corner_radius(10.0),
                )
                .show(ctx, |ui| {
                    panel_ui(ui, view, &history, controls, &mut actions);
                    // The fingertip as a ring (hover) or a dot (press).
                    if let Some([u, v]) = touch.pointer {
                        let at = Pos2::new(u * size.x, v * size.y);
                        let painter = ui.ctx().layer_painter(egui::LayerId::new(
                            egui::Order::Foreground,
                            egui::Id::new("poke"),
                        ));
                        if touch.pressed {
                            painter.circle_filled(at, 6.0, Color32::WHITE);
                        } else {
                            painter.circle_stroke(at, 8.0, Stroke::new(2.0_f32, Color32::WHITE));
                        }
                    }
                });
        });

        for (id, delta) in &output.textures_delta.set {
            self.renderer
                .update_texture(&gfx.device, &gfx.queue, *id, delta);
        }
        let jobs = self.ctx.tessellate(output.shapes, output.pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [TEX_W, TEX_H],
            pixels_per_point: output.pixels_per_point,
        };
        let mut encoder = gfx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("xr-hud"),
            });
        let user_buffers =
            self.renderer
                .update_buffers(&gfx.device, &gfx.queue, &mut encoder, &jobs, &screen);
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("xr-hud"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            self.renderer
                .render(&mut pass.forget_lifetime(), &jobs, &screen);
        }
        gfx.queue.submit(
            user_buffers
                .into_iter()
                .chain(std::iter::once(encoder.finish())),
        );
        for id in &output.textures_delta.free {
            self.renderer.free_texture(id);
        }
        actions
    }
}

fn panel_ui(
    ui: &mut egui::Ui,
    view: &View<'_>,
    gpu_history: &[f32],
    controls: &mut Controls,
    actions: &mut Vec<Action>,
) {
    let dim = Color32::from_gray(150);
    let ms = |v: Option<f32>| v.map_or_else(|| "-".to_owned(), |v| format!("{v:.1}"));

    ui.horizontal(|ui| {
        ui.label(RichText::new("Fosfora XR").strong().size(17.0));
        ui.label(RichText::new(view.mode).color(dim));
    });
    if let Some((name, i, n)) = view.effect {
        ui.horizontal(|ui| {
            if ui.button("< Prev").clicked() {
                actions.push(Action::PrevEffect);
            }
            if ui.button("Next >").clicked() {
                actions.push(Action::NextEffect);
            }
            ui.label(RichText::new(format!("{name}  {}/{n}", i + 1)).strong());
        });
    }
    ui.separator();

    // Frame timing.
    let f = view.frames;
    ui.label(
        RichText::new(format!(
            "{:.0} fps / {:.0} Hz   long {}   max {:.0} ms",
            f.fps, f.display_hz, f.long_frames, f.max_interval_ms
        ))
        .strong(),
    );
    let pct = |v: Option<f32>| v.map_or_else(|| "-".to_owned(), |v| format!("{v:.0}%"));
    ui.label(
        RichText::new(format!(
            "GPU {} ms ({})   loop CPU {:.1} ms",
            ms(view.perf.app_gpu_ms),
            pct(view.perf.gpu_util_pct),
            f.cpu_avg_ms
        ))
        .strong(),
    );
    ui.label(
        RichText::new(format!(
            "CPU {}  dropped {}  latency {} ms",
            pct(view.perf.cpu_util_pct),
            view.perf
                .dropped_frames
                .map_or("-".into(), |v| v.to_string()),
            ms(view.perf.latency_ms),
        ))
        .color(dim),
    );
    gpu_graph(ui, gpu_history);
    ui.label(
        RichText::new(format!(
            "particles {}   room boxes {}",
            view.alive.map_or("-".into(), |a| a.to_string()),
            view.room_boxes
        ))
        .color(dim),
    );
    ui.separator();

    // Hands and gesture.
    let hand = |h: usize| {
        let side = if h == 0 { "L" } else { "R" };
        if !view.tracked[h] {
            return format!("{side} lost");
        }
        let tip = view.tip_mm[h].map_or("-".into(), |d| format!("{d:.0} mm"));
        let pinch = if view.pinching[h] { "PINCH" } else { "open" };
        format!("{side} {pinch} {tip}")
    };
    ui.label(format!("{}    {}", hand(0), hand(1)));
    ui.label(RichText::new(format!("gesture: {}", view.gesture)).strong());
    ui.horizontal(|ui| {
        ui.label(format!(
            "anchor ({:.2}, {:.2}, {:.2})",
            view.anchor[0], view.anchor[1], view.anchor[2]
        ));
        if ui.button("Recenter").clicked() {
            actions.push(Action::Recenter);
        }
    });
    ui.separator();

    // Audio: three short labeled bars on one row.
    ui.label(RichText::new(format!("audio {}", view.audio)).color(dim));
    ui.horizontal(|ui| {
        let w = (ui.available_width() - 3.0 * 44.0) / 3.0;
        for (name, v) in [("rms", view.rms), ("bass", view.bass), ("beat", view.beat)] {
            ui.label(RichText::new(name).monospace());
            ui.add(
                egui::ProgressBar::new(v.clamp(0.0, 1.0))
                    .desired_width(w)
                    .desired_height(10.0),
            );
        }
    });
    ui.separator();

    // Live controls.
    ui.add(egui::Slider::new(&mut controls.gravity, 0.0..=1.5).text("settle m/s"));
    ui.add(egui::Slider::new(&mut controls.near_fade, 0.0..=0.5).text("near fade m"));
    ui.add(egui::Slider::new(&mut controls.hand_pad, 0.0..=0.25).text("hand pad m"));
    ui.add(egui::Slider::new(&mut controls.hand_kick, 0.0..=1.5).text("hand kick m/s"));
}

/// GPU frame time over the last two seconds, with the 72 Hz budget line.
fn gpu_graph(ui: &mut egui::Ui, history: &[f32]) {
    let (rect, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 80.0), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, Color32::from_gray(30));
    let y = |ms: f32| rect.bottom() - (ms / GRAPH_MAX_MS).clamp(0.0, 1.0) * rect.height();
    // Budget: a dashed line with its label (not a color).
    painter.add(egui::Shape::dashed_line(
        &[
            Pos2::new(rect.left(), y(BUDGET_MS)),
            Pos2::new(rect.right(), y(BUDGET_MS)),
        ],
        Stroke::new(1.0_f32, Color32::from_gray(160)),
        5.0,
        4.0,
    ));
    painter.text(
        Pos2::new(rect.right() - 4.0, y(BUDGET_MS) - 2.0),
        egui::Align2::RIGHT_BOTTOM,
        "13.9 ms",
        egui::FontId::proportional(11.0),
        Color32::from_gray(160),
    );
    if history.len() > 1 {
        let step = rect.width() / (HISTORY - 1) as f32;
        let start = rect.right() - step * (history.len() - 1) as f32;
        let points: Vec<Pos2> = history
            .iter()
            .enumerate()
            .map(|(i, &ms)| Pos2::new(start + step * i as f32, y(ms)))
            .collect();
        painter.add(egui::Shape::line(
            points,
            Stroke::new(2.0_f32, Color32::WHITE),
        ));
    }
    painter.text(
        rect.left_top() + Vec2::new(4.0, 2.0),
        egui::Align2::LEFT_TOP,
        "GPU ms, 2 s",
        egui::FontId::proportional(11.0),
        Color32::from_gray(160),
    );
}
