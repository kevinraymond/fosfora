//! The hand menu and the debug panel: an egui UI rendered into a texture
//! that `Gfx` draws on a quad above the left palm (`palm_panel.rs` places
//! it and turns the right hand's ray or fingertip into a pointer).
//!
//! Turning the left palm up always shows the hand menu: for now a single
//! row, the debug panel's on/off toggle. With debug on the same quad grows
//! upward into the debug panel (frame timing, the effect, hands, reach,
//! anchor and audio, and controls for what can change without a restart),
//! the toggle still its bottom row. The quad keeps its bottom edge when it
//! resizes, so the toggle stays under the pointer either way.
//!
//! The controls are built for low precision (they must work without stereo
//! depth perception, and a ray jitters): each is a full-width
//! row about 2.5 cm tall, picked by the pointer's height alone, with a left
//! and a right half where it has two actions (Prev/Next, -/+). The target
//! under the pointer when a press starts fires at once and stays locked
//! until release, so drift during the press changes nothing; holding a -/+
//! repeats. egui only lays out and paints; it gets no pointer input. Nothing
//! is told apart by hue alone: the row under the pointer has a thick
//! outline, a pressed half turns white.

use std::collections::VecDeque;

use egui::{Color32, Pos2, Rect, RichText, Stroke, Vec2};
use glam::{Quat, Vec3};

use crate::gfx::{Beam, Gfx, PanelPose};
use crate::palm_panel::{PalmPanel, Touch};
use crate::perf::PerfSample;

/// Texture size (pixels) and egui scale: 3200 px per meter on the 20 x 40 cm
/// panel, about the display's density at arm's length. The width keeps a
/// row at a multiple of 256 bytes, which a texture-to-buffer copy (`dump`)
/// requires.
const TEX_W: u32 = 640;
const TEX_H: u32 = 1376;
const _: () = assert!((TEX_W * 4).is_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT));
const PIXELS_PER_POINT: f32 = 1.6;
/// Frame-time history for the graph: two seconds at 72 Hz.
const HISTORY: usize = 144;
/// The 72 Hz frame budget (ms), drawn on the graph.
const BUDGET_MS: f32 = 1000.0 / 72.0;
const GRAPH_MAX_MS: f32 = 20.0;

/// The hand menu's height (points): the part of the texture it shows. The
/// toggle row sits as far above the bottom edge as it does in the full
/// debug panel, so toggling leaves it exactly under the pointer.
const MENU_H_PT: f32 = 124.0;
/// Control rows: height and gap (points; 1 point = 0.5 mm on the panel).
const ROW_H: f32 = 50.0;
const ROW_GAP: f32 = 6.0;
/// The -/+ and Prev/Next boxes at the row ends.
const END_BOX_W: f32 = 64.0;
/// Holding a -/+ repeats after this delay, at this interval (seconds).
const REPEAT_DELAY_S: f64 = 0.45;
const REPEAT_EVERY_S: f64 = 0.15;
/// The -/+ rows: label, range and step of each `Controls` field.
const STEPPERS: [(&str, f32, f32, f32); 6] = [
    ("settle m/s", 0.0, 1.5, 0.1),
    ("near fade m", 0.0, 0.5, 0.05),
    ("hand pad m", 0.0, 0.25, 0.02),
    ("hand kick m/s", 0.0, 1.5, 0.1),
    ("reach 1:1 within m", 0.2, 0.8, 0.05),
    ("reach gain", 0.0, 60.0, 2.0),
];

/// Values the panel's -/+ rows change; the app applies them every frame.
#[derive(Debug, Clone, Copy)]
pub struct Controls {
    pub gravity: f32,
    pub near_fade: f32,
    pub hand_pad: f32,
    pub hand_kick: f32,
    pub reach_threshold: f32,
    pub reach_gain: f32,
}

/// What the panel's buttons asked for this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    PrevEffect,
    NextEffect,
    /// Put the anchor back around the wearer.
    Recenter,
    /// The hand menu's debug toggle changed; the app saves it.
    SetDebug(bool),
}

impl Controls {
    fn field(&mut self, i: usize) -> &mut f32 {
        match i {
            0 => &mut self.gravity,
            1 => &mut self.near_fade,
            2 => &mut self.hand_pad,
            3 => &mut self.hand_kick,
            4 => &mut self.reach_threshold,
            _ => &mut self.reach_gain,
        }
    }
}

/// A control the pointer can land on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    Prev,
    Next,
    Recenter,
    ToggleDebug,
    /// A `STEPPERS` row: index, and up (+) or down (-).
    Step(usize, bool),
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
    /// Seated reach per hand: (real, virtual) shoulder-to-palm meters.
    pub reach: [Option<(f32, f32)>; 2],
}

pub struct Hud {
    /// The debug panel is on (else the hand menu shows only its toggle).
    debug: bool,
    placement: PalmPanel,
    touch: Touch,
    was_pressed: bool,
    /// The target a press landed on, held until release.
    locked: Option<Target>,
    repeat_at: f64,
    logged_pressed: bool,
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
    /// Build the hand menu, with the debug panel on or off, and hand its
    /// texture to `gfx`.
    pub fn new(gfx: &mut Gfx, debug: bool) -> Self {
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
            debug,
            placement: PalmPanel::default(),
            touch: Touch::default(),
            was_pressed: false,
            locked: None,
            repeat_at: 0.0,
            logged_pressed: false,
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
    /// right hand against it: its ray (shoulder through index knuckle) with
    /// its pinch, and its index fingertip for a poke. Returns what the
    /// panel does with the pointer this frame (default when hidden).
    pub fn place(
        &mut self,
        gfx: &Gfx,
        hands: &crate::input::HandsFrame,
        head: [f32; 3],
        head_rot: [f32; 4],
    ) -> Touch {
        let head = Vec3::from(head);
        let palm = hands.palm[0].map(|(p, q)| (Vec3::from(p), Quat::from_array(q)));
        self.placement.set_height(self.height_m());
        let placement = self.placement.place(palm, head);
        let ray = hands.index_knuckle[1].map(|k| {
            crate::palm_panel::hand_ray(Vec3::from(k), head, Quat::from_array(head_rot), 1.0)
        });
        self.touch = self.placement.input(
            hands.index_tip[1].map(|t| (Vec3::new(t[0], t[1], t[2]), t[3])),
            ray,
            hands.pinching[1],
        );
        let v_max = self.visible_pts() * PIXELS_PER_POINT / TEX_H as f32;
        gfx.set_panel_pose(placement.map(|p| {
            let (right, up) = p.half_vectors();
            PanelPose {
                center: p.center.to_array(),
                right: right.to_array(),
                up: up.to_array(),
                v_max,
            }
        }));
        let shown = placement.is_some();
        gfx.set_beam(
            crate::gfx::BEAM_PANEL,
            self.touch.beam.filter(|_| shown).map(|(start, end)| Beam {
                start: start.to_array(),
                end: end.to_array(),
                eye: head.to_array(),
                width_m: if self.touch.pressed { 0.006 } else { 0.003 },
                alpha: if self.touch.pressed { 0.9 } else { 0.55 },
            }),
        );
        if shown != self.shown_logged {
            self.shown_logged = shown;
            log::info!("hand menu {}", if shown { "shown" } else { "hidden" });
        }
        if self.touch.pressed != self.logged_pressed {
            self.logged_pressed = self.touch.pressed;
            log::info!(
                "hand menu {} ({})",
                if self.touch.pressed {
                    "press"
                } else {
                    "release"
                },
                if self.touch.poke_depth.is_some() {
                    "poke"
                } else {
                    "ray"
                }
            );
        }
        self.touch
    }

    /// Whether the panel is showing this frame.
    pub fn shown(&self) -> bool {
        self.shown_logged
    }

    /// Whether the debug panel is on.
    pub fn debug(&self) -> bool {
        self.debug
    }

    /// The texture height the quad shows (points): the menu strip, or all.
    fn visible_pts(&self) -> f32 {
        if self.debug {
            TEX_H as f32 / PIXELS_PER_POINT
        } else {
            MENU_H_PT
        }
    }

    /// The quad's height (meters) for what it shows, at the texture's
    /// density across the panel's width.
    fn height_m(&self) -> f32 {
        self.visible_pts() * PIXELS_PER_POINT / TEX_W as f32 * crate::palm_panel::PANEL_W_M
    }

    /// Diagnostic (`debug.fosfora.hudtest`): the panel parked 45 cm ahead
    /// of the view and a little below, untracked, for a screencap.
    pub fn place_parked(&mut self, gfx: &Gfx, head: [f32; 3], head_rot: [f32; 4]) {
        let head = Vec3::from(head);
        let ahead = Quat::from_array(head_rot) * Vec3::NEG_Z;
        let p = crate::palm_panel::Placement::facing(
            head + ahead * 0.45 - Vec3::Y * 0.05,
            head,
            self.height_m(),
        );
        let (right, up) = p.half_vectors();
        gfx.set_panel_pose(Some(PanelPose {
            center: p.center.to_array(),
            right: right.to_array(),
            up: up.to_array(),
            v_max: self.visible_pts() * PIXELS_PER_POINT / TEX_H as f32,
        }));
        self.touch = Touch::default();
        gfx.set_beam(crate::gfx::BEAM_PANEL, None);
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
        let now = self.started.elapsed().as_secs_f64();
        // The pointer is relative to the part of the texture the quad shows.
        let visible = self.visible_pts();
        let cursor = self
            .touch
            .pointer
            .map(|[u, v]| Pos2::new(u * size.x, v * visible));
        // The press is the pinch alone: a cursor that leaves the panel for a
        // frame (ray jitter past the margin, a poke sliding off) must not
        // count as a release, or the return fires the target a second time.
        let pressed = self.touch.pressed;
        let press_began = pressed && !self.was_pressed;
        self.was_pressed = pressed;
        if !pressed {
            self.locked = None;
        }

        let mut actions = Vec::new();
        let history: Vec<f32> = self.gpu_history.iter().copied().collect();
        let touch = self.touch;
        let locked = self.locked.filter(|_| pressed);
        let mut hovered = None;
        let debug = self.debug;
        let output = self.ctx.run(raw, |ctx| {
            egui::CentralPanel::default()
                .frame(
                    egui::Frame::new()
                        .fill(Color32::from_rgba_unmultiplied(18, 18, 22, 245))
                        .inner_margin(10.0)
                        .corner_radius(10.0),
                )
                .show(ctx, |ui| {
                    let rows = Rows {
                        cursor,
                        locked,
                        hovered: &mut hovered,
                    };
                    if debug {
                        panel_ui(ui, view, &history, controls, rows);
                    } else {
                        menu_ui(ui, rows);
                    }
                    // The cursor: a filled dot while pressed, else a ring.
                    // For a poke the ring shrinks as the fingertip closes
                    // in (a distance cue that needs no stereo).
                    if let Some(at) = cursor {
                        let painter = ui.ctx().layer_painter(egui::LayerId::new(
                            egui::Order::Foreground,
                            egui::Id::new("cursor"),
                        ));
                        let outline = Stroke::new(1.5_f32, Color32::BLACK);
                        if touch.pressed {
                            painter.circle(at, 7.0, Color32::WHITE, outline);
                        } else {
                            let r = touch
                                .poke_depth
                                .map_or(10.0, |d| 5.0 + (d.max(0.0) * 1000.0 * 0.5).min(20.0));
                            painter.circle_stroke(at, r + 1.0, outline);
                            painter.circle_stroke(at, r, Stroke::new(2.5_f32, Color32::WHITE));
                        }
                    }
                });
        });

        // Fire on press, on the target under the pointer then; repeat a
        // held -/+.
        let fire = if press_began {
            self.locked = hovered;
            self.repeat_at = now + REPEAT_DELAY_S;
            hovered
        } else if let Some(t @ Target::Step(..)) = self.locked
            && pressed
            && now >= self.repeat_at
        {
            // From now, not from the schedule: after a stall (an effect
            // switch, a hand-mesh upload) a scheduled repeat fired once per
            // frame until it caught up and jumped the value to its clamp.
            self.repeat_at = now + REPEAT_EVERY_S;
            Some(t)
        } else {
            None
        };
        match fire {
            Some(Target::Prev) => actions.push(Action::PrevEffect),
            Some(Target::Next) => actions.push(Action::NextEffect),
            Some(Target::Recenter) => actions.push(Action::Recenter),
            Some(Target::ToggleDebug) => {
                self.debug = !self.debug;
                actions.push(Action::SetDebug(self.debug));
            }
            Some(Target::Step(i, up)) => {
                let (name, lo, hi, step) = STEPPERS[i];
                let v = controls.field(i);
                let next = ((*v / step).round() + if up { 1.0 } else { -1.0 }) * step;
                *v = next.clamp(lo, hi);
                log::info!("debug panel: {name} {:.3}", *v);
            }
            None => {}
        }

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

/// The pointer against the control rows during one layout pass.
struct Rows<'a> {
    cursor: Option<Pos2>,
    /// The target held by a press in progress (drawn pressed).
    locked: Option<Target>,
    /// Out: the target under the pointer.
    hovered: &'a mut Option<Target>,
}

impl Rows<'_> {
    /// Allocate a full-width control row and return its rect and which
    /// half (false left, true right) the pointer is on, if it is on the
    /// row. A row owns the gap around it, so the rows tile the area.
    fn row(&mut self, ui: &mut egui::Ui) -> (Rect, Option<bool>) {
        let (rect, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), egui::Sense::hover());
        let band = rect.expand2(Vec2::new(0.0, ROW_GAP * 0.5 + 0.5));
        let half = self
            .cursor
            .filter(|c| c.y >= band.top() && c.y < band.bottom())
            .map(|c| c.x >= rect.center().x);
        (rect, half)
    }

    /// A row with a left and a right action and text between them.
    fn two_way(
        &mut self,
        ui: &mut egui::Ui,
        left: (Target, &str),
        right: (Target, &str),
        middle: &str,
    ) {
        let (rect, half) = self.row(ui);
        if let Some(h) = half {
            *self.hovered = Some(if h { right.0 } else { left.0 });
        }
        let painter = ui.painter();
        painter.rect_filled(rect, 6.0, Color32::from_gray(34));
        let (lr, rr) = rect.split_left_right_at_x(rect.center().x);
        for (target, label, half_rect, is_right) in
            [(left.0, left.1, lr, false), (right.0, right.1, rr, true)]
        {
            let on = half == Some(is_right);
            if on {
                painter.rect_filled(half_rect, 6.0, Color32::from_gray(52));
            }
            let boxed = if is_right {
                Rect::from_min_max(Pos2::new(rect.right() - END_BOX_W, rect.top()), rect.max)
            } else {
                Rect::from_min_max(rect.min, Pos2::new(rect.left() + END_BOX_W, rect.bottom()))
            };
            let pressed = self.locked == Some(target);
            let (fill, text) = if pressed {
                (Color32::WHITE, Color32::BLACK)
            } else {
                (Color32::from_gray(70), Color32::WHITE)
            };
            painter.rect_filled(boxed, 6.0, fill);
            painter.text(
                boxed.center(),
                egui::Align2::CENTER_CENTER,
                label,
                egui::FontId::proportional(24.0),
                text,
            );
            if on {
                painter.rect_stroke(
                    half_rect,
                    6.0,
                    Stroke::new(3.0_f32, Color32::WHITE),
                    egui::StrokeKind::Inside,
                );
            }
        }
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            middle,
            egui::FontId::proportional(16.0),
            Color32::WHITE,
        );
        ui.add_space(ROW_GAP);
    }

    /// A row with one action across its width.
    fn single(&mut self, ui: &mut egui::Ui, target: Target, label: &str) {
        let (rect, half) = self.row(ui);
        if half.is_some() {
            *self.hovered = Some(target);
        }
        let painter = ui.painter();
        let pressed = self.locked == Some(target);
        let (fill, text) = if pressed {
            (Color32::WHITE, Color32::BLACK)
        } else if half.is_some() {
            (Color32::from_gray(70), Color32::WHITE)
        } else {
            (Color32::from_gray(45), Color32::WHITE)
        };
        painter.rect_filled(rect, 6.0, fill);
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(20.0),
            text,
        );
        if half.is_some() {
            painter.rect_stroke(
                rect,
                6.0,
                Stroke::new(3.0_f32, Color32::WHITE),
                egui::StrokeKind::Inside,
            );
        }
        ui.add_space(ROW_GAP);
    }
}

fn panel_ui(
    ui: &mut egui::Ui,
    view: &View<'_>,
    gpu_history: &[f32],
    controls: &mut Controls,
    mut rows: Rows<'_>,
) {
    let dim = Color32::from_gray(150);
    let ms = |v: Option<f32>| v.map_or_else(|| "-".to_owned(), |v| format!("{v:.1}"));

    ui.horizontal(|ui| {
        ui.label(RichText::new("Fosfora XR").strong().size(17.0));
        ui.label(RichText::new(format!("{} · audio {}", view.mode, view.audio)).color(dim));
    });
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
    let reach = |h: usize| {
        view.reach[h].map_or_else(
            || "-".to_owned(),
            |(real, virt)| format!("{real:.2} -> {virt:.2} m"),
        )
    };
    ui.label(format!("reach  L {}   R {}", reach(0), reach(1)));
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("gesture: {}", view.gesture)).strong());
        ui.label(
            RichText::new(format!(
                "anchor ({:.2}, {:.2}, {:.2})",
                view.anchor[0], view.anchor[1], view.anchor[2]
            ))
            .color(dim),
        );
    });

    // Audio: three short labeled bars on one row.
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

    // Controls: big rows, picked by the pointer's height.
    if let Some((name, i, n)) = view.effect {
        rows.two_way(
            ui,
            (Target::Prev, "<"),
            (Target::Next, ">"),
            &format!("{name}  {}/{n}", i + 1),
        );
    }
    rows.single(ui, Target::Recenter, "Recenter the cloud");
    for (i, (name, ..)) in STEPPERS.iter().enumerate() {
        let value = *controls.field(i);
        rows.two_way(
            ui,
            (Target::Step(i, false), "-"),
            (Target::Step(i, true), "+"),
            &format!("{name}  {value:.2}"),
        );
    }
    toggle_row(&mut rows, ui, true);
}

/// The hand menu with the debug panel off: a title and the toggle.
fn menu_ui(ui: &mut egui::Ui, mut rows: Rows<'_>) {
    ui.label(RichText::new("Fosfora").strong().size(17.0));
    toggle_row(&mut rows, ui, false);
}

/// The debug toggle, the bottom row in both layouts. Its state is in the
/// words, not a color.
fn toggle_row(rows: &mut Rows<'_>, ui: &mut egui::Ui, on: bool) {
    rows.single(
        ui,
        Target::ToggleDebug,
        if on {
            "Debug panel: on"
        } else {
            "Debug panel: off"
        },
    );
}

/// GPU frame time over the last two seconds, with the 72 Hz budget line.
fn gpu_graph(ui: &mut egui::Ui, history: &[f32]) {
    let (rect, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 56.0), egui::Sense::hover());
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
