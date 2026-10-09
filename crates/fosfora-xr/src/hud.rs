//! The hand menu and the debug panel: an egui UI rendered into a texture
//! that `Gfx` draws on a quad above the left palm (`palm_panel.rs` places
//! it and turns the right hand's ray or fingertip into a pointer).
//!
//! Turning the left palm toward the face always shows the hand menu (a palm
//! turned more to the ceiling is a hold instead, `palm_panel.rs`): five
//! rows, the world effect's `<` `>` on top (board #3336: the effect cycle
//! left the bare pinch-hold; "one effect in this mode" outside world mode),
//! the particle pitcher's on/off toggle and the debug panel's under it,
//! then the room editor's toggle beside its status ("desk: embers",
//! board #3326), under that the cloud's toggle (`room_edit::Cloud`), and
//! at the bottom the music's play/stop (`music.rs`, board #3472). With
//! Edit room on, three more rows under the editor's (board #3472, D3):
//! the color, the band and the strength of the surface under the beam,
//! each stepped with `<` and `>`, the menu growing upward to hold them.
//! With debug on the same quad grows upward into the debug
//! panel (frame timing, the effect, hands, reach, anchor and audio, and
//! controls for what can change without a restart), the menu rows still
//! at its bottom (the effect row at the top of its controls). The quad keeps its bottom edge when it resizes, so the
//! toggles stay under the pointer either way.
//!
//! The controls are built for low precision (they must work without stereo
//! depth perception, and a ray jitters): rows 1.7 cm tall at the bottom of
//! the panel, picked by the pointer's height alone, then by its side: a
//! wide row (Prev/Next) has a left and a right half, the others two cells
//! side by side, each with a left and a right half where it has two
//! actions (-/+); `panel_grid.rs` holds the layout and the hit test. The
//! target under the pointer when a press starts fires at once and stays
//! locked until release, so drift during the press changes nothing;
//! holding a -/+ repeats. egui only lays out and paints; it gets no pointer
//! input. Nothing is told apart by hue alone: the control under the
//! pointer has a thick outline, a pressed one turns white.

use std::collections::VecDeque;

use egui::{Color32, Pos2, Rect, RichText, Stroke, Vec2};
use fosfora_app::ui::theme::palette::Palette;
use glam::{Quat, Vec3};

use crate::gfx::{Beam, Gfx, PanelPose};
use crate::palm_panel::{PalmPanel, Touch};
use crate::panel_grid::{
    self as grid, FONT_BUTTON, FONT_END, FONT_GRAPH, FONT_LABEL, FONT_TITLE, FONT_VALUE,
    PIXELS_PER_POINT, TEX_H, TEX_W,
};
use crate::perf::PerfSample;

/// The texture's width keeps a row at a multiple of 256 bytes, which a
/// texture-to-buffer copy (`dump`) requires.
const _: () = assert!((TEX_W * 4).is_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT));
/// Frame-time history for the graph: two seconds at 72 Hz.
const HISTORY: usize = 144;
/// The 72 Hz frame budget (ms), drawn on the graph.
const BUDGET_MS: f32 = 1000.0 / 72.0;
const GRAPH_MAX_MS: f32 = 20.0;

/// Holding a -/+ repeats after this delay, at this interval (seconds).
const REPEAT_DELAY_S: f64 = 0.45;
const REPEAT_EVERY_S: f64 = 0.15;
/// The -/+ rows: label, range and step of each `Controls` field.
const STEPPERS: [(&str, f32, f32, f32); 10] = [
    ("settle m/s", 0.0, 1.5, 0.1),
    ("near fade m", 0.0, 0.5, 0.05),
    ("hand pad m", 0.0, 0.25, 0.02),
    ("hand kick m/s", 0.0, 1.5, 0.1),
    ("reach 1:1 within m", 0.2, 0.8, 0.05),
    ("reach gain", 0.0, 60.0, 2.0),
    ("hand scare", 0.0, 1.0, 0.25),
    (
        "pitcher /s",
        crate::instruments::PITCHER_RATE_MIN,
        crate::instruments::PITCHER_RATE_MAX,
        500.0,
    ),
    ("cloud density", 0.05, 1.0, 0.05),
    (
        "space half m",
        crate::space::SPACE_HALF_MIN,
        crate::space::SPACE_HALF_MAX,
        crate::space::SPACE_HALF_STEP,
    ),
];

const _: () = assert!(STEPPERS.len() == grid::STEPPERS);
/// The "cloud density" row of `STEPPERS`.
const DENSITY: usize = 8;

/// Values the panel's -/+ rows change; the app applies them every frame.
#[derive(Debug, Clone, Copy)]
pub struct Controls {
    pub gravity: f32,
    pub near_fade: f32,
    pub hand_pad: f32,
    pub hand_kick: f32,
    pub reach_threshold: f32,
    pub reach_gain: f32,
    /// Murmur's hand predator strength, 0..1.
    pub hand_scare: f32,
    /// The particle pitcher is on (the hand menu's Pitcher toggle).
    pub pitcher: bool,
    /// The pitcher's particles per second.
    pub pitcher_rate: f32,
    /// The world effect's emission against its preset's, 0.05..1: thins
    /// the cloud (not the pitcher's pour). The wearer's setting, the most
    /// the thermal governor gives back.
    pub density: f32,
    /// The thermal governor's density while it holds the cloud below
    /// `density` (board #3789; the app sets it each frame): the stepper
    /// shows it, marked "(gov)", and steps from it.
    pub density_gov: Option<f32>,
    /// The world effect's space size: the half extent of the cube its
    /// particles live in (meters; board #3325).
    pub space_half: f32,
    /// The room editor is on (the hand menu's Edit room toggle; not saved,
    /// off at launch).
    pub edit_room: bool,
    /// The world effect's cloud is on (the hand menu's Cloud toggle; not
    /// saved, on at launch): off, the effect is hidden, its sim stepping
    /// on, or with `edit_room` on only the pointed surface spawns
    /// (`room_edit::Cloud`).
    pub cloud: bool,
    /// The bundled clip plays, the analysis on its tap (the hand menu's
    /// Music row; not saved, off at launch unless the launch source or
    /// `debug.fosfora.music` plays it; `music.rs`).
    pub music: bool,
    /// The voice path listens (the hand menu's Voice toggle, saved in
    /// `hand_menu.json`; board #3751, V4): off, neither the left fist nor
    /// the thumb tap opens a window.
    pub voice: bool,
    /// The agent's provider (the hand menu's Agent control, saved in
    /// `hand_menu.json`; board #3776): the pick showing and the one a
    /// press asks for, `None` with no provider to pick ("Agent: none").
    /// The app sets it; a press only asks.
    pub agent: Option<crate::agent::Control>,
}

/// What the panel's buttons asked for this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    PrevEffect,
    NextEffect,
    /// Put the anchor back around the wearer.
    Recenter,
    /// Relaunch Space Setup and requery the room's anchors.
    RescanRoom,
    /// The hand menu's debug toggle changed; the app saves it.
    SetDebug(bool),
    /// The hand menu's pitcher toggle changed (not saved).
    SetPitcher(bool),
    /// The cloud density stepper moved: `Controls::density` is the
    /// wearer's new setting, which clears the thermal governor's debt.
    SetDensity,
    /// The hand menu's Edit room toggle changed (not saved).
    SetEditRoom(bool),
    /// The hand menu's Cloud toggle changed (not saved).
    SetCloud(bool),
    /// The hand menu's Music row changed: play or stop (not saved).
    SetMusic(bool),
    /// The hand menu's Voice toggle changed (saved; board #3751, V4).
    SetVoice(bool),
    /// The hand menu's Agent control asked for this provider (saved;
    /// board #3776).
    SetAgent(crate::agent::Pick),
    /// The "All: none" button: every room surface and every kind default
    /// to none, saved (board #3326, Kevin's debug ask).
    AllNone,
    /// A surface row's `<` (false) or `>` (true) with Edit room on: step
    /// that parameter of the surface under the beam (board #3472, D3).
    SurfaceParam(crate::lanes::Param, bool),
}

impl Controls {
    fn field(&mut self, i: usize) -> &mut f32 {
        match i {
            0 => &mut self.gravity,
            1 => &mut self.near_fade,
            2 => &mut self.hand_pad,
            3 => &mut self.hand_kick,
            4 => &mut self.reach_threshold,
            5 => &mut self.reach_gain,
            6 => &mut self.hand_scare,
            7 => &mut self.pitcher_rate,
            DENSITY => &mut self.density,
            _ => &mut self.space_half,
        }
    }
}

/// A control the pointer can land on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    Prev,
    Next,
    Recenter,
    Rescan,
    ToggleDebug,
    TogglePitcher,
    ToggleEdit,
    ToggleCloud,
    ToggleMusic,
    ToggleVoice,
    /// The Agent control: the next provider in its cycle.
    CycleAgent,
    AllNone,
    /// A `STEPPERS` row: index, and up (+) or down (-).
    Step(usize, bool),
    /// The surface rows' `<` and `>` (Edit room on, board #3472, D3): up
    /// is `>`, as for `Step`.
    Color(bool),
    Band(bool),
    Strength(bool),
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
    /// The room (scene anchors) is on, so it can be rescanned.
    pub room: bool,
    pub room_boxes: usize,
    pub rms: f32,
    pub bass: f32,
    pub beat: f32,
    pub audio: &'a str,
    /// Seated reach per hand: (real, virtual) shoulder-to-palm meters.
    pub reach: [Option<(f32, f32)>; 2],
    /// Each hand's pose and hold (Murmur), shown in place of "open" while
    /// the hand is not pinching.
    pub pose: &'a [String; 2],
    /// The room editor's status: the pointed surface and its behavior
    /// ("desk: embers"), "no surface" or "edit room off".
    pub edit_status: &'a str,
    /// The pointed surface's color index, band and strength
    /// (`lanes::RoomLanes::params_of`) for the surface rows; `None` with no
    /// surface under the beam or Edit room off.
    pub surface_params: Option<(u32, u32, f32)>,
}

/// The panel's colors (board #3523): the Blue and orange theme, the same
/// tokens the desktop draws with. Blue is the ground, the outlines and what
/// is live; orange is what is pressed or under the pointer. No state rests
/// on hue alone: the words and the outline carry it too.
const PALETTE: Palette = Palette::BLUE_ORANGE;

/// `c` at alpha `a`.
fn with_alpha(c: Color32, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)
}

pub struct Hud {
    /// The debug panel is on (else the hand menu shows only its toggle).
    debug: bool,
    /// Edit room was on at the last render: the menu holds the surface
    /// rows, and the quad is placed tall enough for them.
    editing: bool,
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
    /// The header was seen running into the controls (logged once).
    crowded_logged: bool,
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
            editing: false,
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
            crowded_logged: false,
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

    /// The texture height the quad shows (points): the menu strip (taller
    /// with Edit room's surface rows), or all.
    fn visible_pts(&self) -> f32 {
        if self.debug {
            TEX_H as f32 / PIXELS_PER_POINT
        } else {
            grid::menu_h(grid::menu_rows(self.editing))
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
        let mut crowded = false;
        let debug = self.debug;
        let output = self.ctx.run(raw, |ctx| {
            egui::CentralPanel::default()
                .frame(
                    egui::Frame::new()
                        .fill(with_alpha(PALETTE.panel, 245))
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
                        crowded = panel_ui(ui, view, &history, controls, rows);
                    } else {
                        menu_ui(ui, controls, view, rows);
                    }
                    // The cursor: a filled dot while pressed, else a ring.
                    // For a poke the ring shrinks as the fingertip closes
                    // in (a distance cue that needs no stereo).
                    if let Some(at) = cursor {
                        let painter = ui.ctx().layer_painter(egui::LayerId::new(
                            egui::Order::Foreground,
                            egui::Id::new("cursor"),
                        ));
                        let outline = Stroke::new(1.5_f32, PALETTE.bg);
                        if touch.pressed {
                            painter.circle(at, 7.0, PALETTE.accent, outline);
                        } else {
                            let r = touch
                                .poke_depth
                                .map_or(10.0, |d| 5.0 + (d.max(0.0) * 1000.0 * 0.5).min(20.0));
                            painter.circle_stroke(at, r + 1.0, outline);
                            painter.circle_stroke(at, r, Stroke::new(2.5_f32, PALETTE.accent));
                        }
                    }
                });
        });

        if crowded && !self.crowded_logged {
            log::warn!("debug panel: the header runs into the controls");
        }
        self.crowded_logged |= crowded;

        // Fire on press, on the target under the pointer then; repeat a
        // held -/+.
        let fire = if press_began {
            self.locked = hovered;
            self.repeat_at = now + REPEAT_DELAY_S;
            hovered
        } else if let Some(t @ (Target::Step(..) | Target::Strength(_))) = self.locked
            && pressed
            && now >= self.repeat_at
        {
            // From now, not from the schedule: after a stall (an effect
            // switch, a hand-mesh upload) a scheduled repeat fired once per
            // frame until it caught up and jumped the value to its clamp.
            // A held Strength `>` ramps too; the color and the band do not
            // repeat (a held press would spin through them).
            self.repeat_at = now + REPEAT_EVERY_S;
            Some(t)
        } else {
            None
        };
        match fire {
            Some(Target::Prev) => actions.push(Action::PrevEffect),
            Some(Target::Next) => actions.push(Action::NextEffect),
            Some(Target::Recenter) => actions.push(Action::Recenter),
            Some(Target::Rescan) => actions.push(Action::RescanRoom),
            Some(Target::ToggleDebug) => {
                self.debug = !self.debug;
                actions.push(Action::SetDebug(self.debug));
            }
            Some(Target::TogglePitcher) => {
                controls.pitcher = !controls.pitcher;
                actions.push(Action::SetPitcher(controls.pitcher));
            }
            Some(Target::ToggleEdit) => {
                controls.edit_room = !controls.edit_room;
                actions.push(Action::SetEditRoom(controls.edit_room));
            }
            Some(Target::ToggleCloud) => {
                controls.cloud = !controls.cloud;
                actions.push(Action::SetCloud(controls.cloud));
            }
            Some(Target::ToggleMusic) => {
                controls.music = !controls.music;
                actions.push(Action::SetMusic(controls.music));
            }
            Some(Target::ToggleVoice) => {
                controls.voice = !controls.voice;
                actions.push(Action::SetVoice(controls.voice));
            }
            // The app switches and sets `controls.agent` (the cycle
            // depends on what the headset has); with nothing to pick the
            // press does nothing.
            Some(Target::CycleAgent) => {
                if let Some(agent) = controls.agent {
                    actions.push(Action::SetAgent(agent.next));
                }
            }
            Some(Target::AllNone) => actions.push(Action::AllNone),
            Some(Target::Color(up)) => {
                actions.push(Action::SurfaceParam(crate::lanes::Param::Color, up));
            }
            Some(Target::Band(up)) => {
                actions.push(Action::SurfaceParam(crate::lanes::Param::Band, up));
            }
            Some(Target::Strength(up)) => {
                actions.push(Action::SurfaceParam(crate::lanes::Param::Strength, up));
            }
            Some(Target::Step(i, up)) => {
                let (name, lo, hi, step) = STEPPERS[i];
                // A governed density steps from the value it shows.
                let governed = if i == DENSITY {
                    controls.density_gov.take()
                } else {
                    None
                };
                let v = controls.field(i);
                let from = governed.unwrap_or(*v);
                let next = ((from / step).round() + if up { 1.0 } else { -1.0 }) * step;
                *v = next.clamp(lo, hi);
                log::info!("debug panel: {name} {:.3}", *v);
                if i == DENSITY {
                    actions.push(Action::SetDensity);
                }
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
        // The next frame places the quad for the rows this state lays out
        // (a toggle this frame was laid out before it fired).
        self.editing = controls.edit_room;
        actions
    }
}

/// The pointer against the controls during one layout pass.
struct Rows<'a> {
    cursor: Option<Pos2>,
    /// The target held by a press in progress (drawn pressed).
    locked: Option<Target>,
    /// Out: the target under the pointer.
    hovered: &'a mut Option<Target>,
}

/// A row of controls.
enum Control<'a> {
    /// A left and a right action across the row, text between them.
    Wide {
        left: (Target, &'a str),
        right: (Target, &'a str),
        middle: String,
    },
    /// One action across the row.
    Button(Target, &'a str),
    /// Two cells side by side (`None`: an empty one).
    Pair([Option<Cell<'a>>; 2]),
    /// Text across the row, no target: a status.
    Text(String),
}

/// One cell of a pair row.
enum Cell<'a> {
    /// A `STEPPERS` entry: -/+ at the ends, its label over its value.
    Stepper {
        index: usize,
        label: &'a str,
        value: String,
    },
    /// One action.
    Button(Target, &'a str),
    /// Text only, no target: a status.
    Label(&'a str),
}

impl Control<'_> {
    /// The target at `hit` on this row.
    fn target(&self, hit: grid::Hit) -> Option<Target> {
        match self {
            Self::Wide { left, right, .. } => Some(if hit.col == 1 { right.0 } else { left.0 }),
            Self::Button(t, _) => Some(*t),
            Self::Text(_) => None,
            Self::Pair(cells) => match cells[hit.col].as_ref()? {
                Cell::Stepper { index, .. } => Some(Target::Step(*index, hit.right)),
                Cell::Button(t, _) => Some(*t),
                Cell::Label(_) => None,
            },
        }
    }
}

fn rect(r: grid::Rect) -> Rect {
    Rect::from_min_max(Pos2::from(r.min), Pos2::from(r.max))
}

impl Rows<'_> {
    /// Paint `controls` (top to bottom) as the block at the bottom of the
    /// visible panel, which ends at `bottom` (points), and find the target
    /// under the pointer.
    fn block(&mut self, ui: &egui::Ui, bottom: f32, controls: &[Control<'_>]) {
        let n = controls.len();
        let hit = self.cursor.and_then(|c| grid::hit(bottom, n, [c.x, c.y]));
        for (i, control) in controls.iter().enumerate() {
            let k = n - 1 - i;
            let row = grid::row(bottom, k);
            let on = hit.filter(|h| h.row == k);
            if let Some(h) = on
                && let Some(t) = control.target(h)
            {
                *self.hovered = Some(t);
            }
            let painter = ui.painter();
            match control {
                Control::Wide {
                    left,
                    right,
                    middle,
                } => {
                    self.two_way(painter, row, *left, *right, on.map(|h| h.col == 1));
                    painter.text(
                        rect(row).center(),
                        egui::Align2::CENTER_CENTER,
                        middle,
                        egui::FontId::proportional(FONT_VALUE),
                        PALETTE.text,
                    );
                }
                Control::Button(t, label) => self.button(painter, row, *t, label, on.is_some()),
                Control::Text(text) => label(painter, row, text),
                Control::Pair(cells) => {
                    for (col, cell) in cells.iter().enumerate() {
                        let area = grid::cell(row, col);
                        let on = on.filter(|h| h.col == col);
                        match cell {
                            Some(Cell::Stepper {
                                index,
                                label,
                                value,
                            }) => {
                                self.two_way(
                                    painter,
                                    area,
                                    (Target::Step(*index, false), "-"),
                                    (Target::Step(*index, true), "+"),
                                    on.map(|h| h.right),
                                );
                                let mid = rect(area.middle());
                                painter.text(
                                    Pos2::new(mid.center().x, mid.top() + 2.0),
                                    egui::Align2::CENTER_TOP,
                                    label,
                                    egui::FontId::proportional(FONT_LABEL),
                                    PALETTE.sub,
                                );
                                painter.text(
                                    Pos2::new(mid.center().x, mid.bottom() - 1.0),
                                    egui::Align2::CENTER_BOTTOM,
                                    value,
                                    egui::FontId::proportional(FONT_VALUE),
                                    PALETTE.text,
                                );
                            }
                            Some(Cell::Button(t, label)) => {
                                self.button(painter, area, *t, label, on.is_some());
                            }
                            Some(Cell::Label(text)) => label(painter, area, text),
                            None => {}
                        }
                    }
                }
            }
        }
    }

    /// A left and a right action at the ends of `area`, the half under the
    /// pointer (`Some(true)`: the right) outlined.
    fn two_way(
        &self,
        painter: &egui::Painter,
        area: grid::Rect,
        left: (Target, &str),
        right: (Target, &str),
        on: Option<bool>,
    ) {
        painter.rect_filled(rect(area), 6.0, PALETTE.well);
        let (lh, rh) = area.halves();
        let (lb, rb) = area.end_boxes();
        for (target, label, half, boxed, is_right) in [
            (left.0, left.1, lh, lb, false),
            (right.0, right.1, rh, rb, true),
        ] {
            let hovered = on == Some(is_right);
            if hovered {
                painter.rect_filled(rect(half), 6.0, PALETTE.rule);
            }
            let (fill, text) = if self.locked == Some(target) {
                (PALETTE.sel_bg, PALETTE.sel_fg)
            } else {
                (PALETTE.rule, PALETTE.text)
            };
            painter.rect_filled(rect(boxed), 6.0, fill);
            painter.text(
                rect(boxed).center(),
                egui::Align2::CENTER_CENTER,
                label,
                egui::FontId::proportional(FONT_END),
                text,
            );
            if hovered {
                painter.rect_stroke(
                    rect(half),
                    6.0,
                    Stroke::new(3.0_f32, PALETTE.accent),
                    egui::StrokeKind::Inside,
                );
            }
        }
    }

    /// One action filling `area`.
    fn button(
        &self,
        painter: &egui::Painter,
        area: grid::Rect,
        target: Target,
        label: &str,
        on: bool,
    ) {
        // Pressed inverts to orange on ink; under the pointer the outline
        // says so; idle is the blue block every pressable thing is.
        let (fill, text) = if self.locked == Some(target) {
            (PALETTE.sel_bg, PALETTE.sel_fg)
        } else {
            (PALETTE.rule, PALETTE.text)
        };
        let r = rect(area);
        painter.rect_filled(r, 6.0, fill);
        painter.text(
            r.center(),
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(FONT_BUTTON),
            text,
        );
        if on {
            painter.rect_stroke(
                r,
                6.0,
                Stroke::new(3.0_f32, PALETTE.accent),
                egui::StrokeKind::Inside,
            );
        }
    }
}

/// A status cell: `text` on a dark ground with no outline (nothing to
/// press), in the value font, or the label font when that does not fit.
fn label(painter: &egui::Painter, area: grid::Rect, text: &str) {
    let r = rect(area);
    painter.rect_filled(r, 6.0, PALETTE.well);
    let fits = |size: f32| {
        painter
            .layout_no_wrap(
                text.to_owned(),
                egui::FontId::proportional(size),
                PALETTE.text,
            )
            .size()
            .x
            <= r.width() - 8.0
    };
    let size = if fits(FONT_VALUE) {
        FONT_VALUE
    } else {
        FONT_LABEL
    };
    painter.text(
        r.center(),
        egui::Align2::CENTER_CENTER,
        text,
        egui::FontId::proportional(size),
        PALETTE.text,
    );
}

/// The debug panel: the header top down, the controls at the bottom.
/// Returns whether the header ran into the controls (for a warning).
fn panel_ui(
    ui: &mut egui::Ui,
    view: &View<'_>,
    gpu_history: &[f32],
    controls: &mut Controls,
    mut rows: Rows<'_>,
) -> bool {
    let dim = PALETTE.sub;
    let ms = |v: Option<f32>| v.map_or_else(|| "-".to_owned(), |v| format!("{v:.1}"));

    ui.horizontal(|ui| {
        ui.label(RichText::new("Fosfora XR").strong().size(FONT_TITLE));
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
        let pinch = if view.pinching[h] {
            "PINCH"
        } else {
            view.pose[h].as_str()
        };
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
    let header_end = ui.cursor().top();

    // Controls: rows at the bottom, picked by the pointer's height, then
    // its side (`panel_grid.rs`). The effect row on top, as in the menu;
    // outside world mode the panel leaves it out.
    let mut block = Vec::new();
    if view.effect.is_some() {
        block.push(effect_row(view.effect));
    }
    let steppers: Vec<Cell<'_>> = STEPPERS
        .iter()
        .enumerate()
        .map(|(i, &(label, _, _, step))| {
            // Whole steps without decimals.
            let places = if step >= 1.0 { 0 } else { 2 };
            let value = match controls.density_gov {
                Some(gov) if i == DENSITY => format!("{gov:.2} (gov)"),
                _ => format!("{:.places$}", *controls.field(i)),
            };
            Cell::Stepper {
                index: i,
                label,
                value,
            }
        })
        .collect();
    let mut steppers = steppers.into_iter();
    while let Some(left) = steppers.next() {
        block.push(Control::Pair([Some(left), steppers.next()]));
    }
    block.push(if view.room {
        Control::Pair([
            Some(Cell::Button(Target::Recenter, "Recenter the cloud")),
            Some(Cell::Button(Target::Rescan, "Rescan the room")),
        ])
    } else {
        Control::Button(Target::Recenter, "Recenter the cloud")
    });
    block.push(agent_row(controls.agent));
    block.push(menu_row(true, controls.pitcher));
    block.push(edit_row(controls.edit_room, view.edit_status));
    if controls.edit_room {
        block.extend(surface_rows(view.surface_params));
    }
    block.push(cloud_row(controls.cloud, controls.edit_room));
    block.push(music_row(controls.music, controls.voice));
    let bottom = grid::PANEL_H;
    rows.block(ui, bottom, &block);
    header_end > grid::block_top(bottom, block.len())
}

/// The hand menu with the debug panel off: a title over its rows, the
/// surface rows among them while Edit room is on.
fn menu_ui(ui: &mut egui::Ui, controls: &Controls, view: &View<'_>, mut rows: Rows<'_>) {
    ui.label(RichText::new("Fosfora").strong().size(FONT_TITLE));
    let editing = controls.edit_room;
    let mut block = Vec::with_capacity(grid::menu_rows(editing));
    block.push(effect_row(view.effect));
    block.push(agent_row(controls.agent));
    block.push(menu_row(false, controls.pitcher));
    block.push(edit_row(editing, view.edit_status));
    if editing {
        block.extend(surface_rows(view.surface_params));
    }
    block.push(cloud_row(controls.cloud, editing));
    block.push(music_row(controls.music, controls.voice));
    debug_assert_eq!(block.len(), grid::menu_rows(editing));
    rows.block(ui, grid::menu_h(block.len()), &block);
}

/// The surface rows, under the room editor's while Edit room is on (board
/// #3472, D3): the color, the band and the strength of the surface under
/// the beam (`params`, as the face runs them), each a `<` and a `>` with
/// the parameter's name and value between, "Color  amber". The editor
/// holds its hit while the panel is up, so the wearer points at a surface,
/// turns the palm up and steps it. With no surface, one line says so over
/// two empty rows (the menu keeps its height), and nothing presses.
fn surface_rows(params: Option<(u32, u32, f32)>) -> [Control<'static>; grid::SURFACE_ROWS] {
    let Some((color, band, strength)) = params else {
        return [
            Control::Text("Point at a surface".to_owned()),
            Control::Text(String::new()),
            Control::Text(String::new()),
        ];
    };
    let row = |target: fn(bool) -> Target, middle: String| Control::Wide {
        left: (target(false), "<"),
        right: (target(true), ">"),
        middle,
    };
    [
        row(
            Target::Color,
            format!("Color  {}", crate::surfaces::color_name(color)),
        ),
        row(
            Target::Band,
            format!("Band  {}", crate::surface_fx::band_name(band)),
        ),
        row(Target::Strength, format!("Strength  {strength:.1}")),
    ]
}

/// The world effect's row, the top of the menu and of the debug panel's
/// block (board #3336): `<` and `>` around the effect's name and place,
/// "Embers  1/3", the hand menu's home for the effect cycle since the bare
/// pinch-hold stopped cycling it (`Action::PrevEffect` / `NextEffect`).
/// With no world effect (`effect` `None`, not world mode) a status in its
/// place, so the menu keeps its height and nothing presses.
fn effect_row(effect: Option<(&str, usize, usize)>) -> Control<'static> {
    match effect {
        Some((name, i, n)) => Control::Wide {
            left: (Target::Prev, "<"),
            right: (Target::Next, ">"),
            middle: format!("{name}  {}/{n}", i + 1),
        },
        None => Control::Text("One effect in this mode".to_owned()),
    }
}

/// The music's row, the bottom row in both layouts: play or stop (board
/// #3472), and the voice path's toggle in the right cell (board #3751, V4),
/// so no row count changes. The state is in the words, not a color.
fn music_row(playing: bool, voice: bool) -> Control<'static> {
    Control::Pair([
        Some(Cell::Button(
            Target::ToggleMusic,
            crate::music::label(playing),
        )),
        Some(Cell::Button(
            Target::ToggleVoice,
            if voice { "Voice: on" } else { "Voice: off" },
        )),
    ])
}

/// The agent's provider (board #3776), one action across the row, over
/// the pitcher's and the debug panel's toggles in both layouts: each press
/// steps the cycle (local, the network provider `voice.json` configures,
/// off; what the headset lacks is skipped). It is the menu's top row under
/// the effect's, so adding it moved no row the wearer already reaches for
/// (the quad keeps its bottom edge and grows upward). The state is in the
/// words, not a color; "Agent: none" with nothing to pick, and a press then
/// does nothing.
fn agent_row(agent: Option<crate::agent::Control>) -> Control<'static> {
    Control::Button(Target::CycleAgent, crate::agent::control_label(agent))
}

/// The cloud's row, over the music's in both layouts: its toggle, and
/// every surface to none (step 2c). The state is in the words, not a
/// color.
fn cloud_row(on: bool, editing: bool) -> Control<'static> {
    Control::Pair([
        Some(Cell::Button(
            Target::ToggleCloud,
            // "Particles", not "cloud": the toggle hides every particle
            // of the effect, the embers painted on a surface included, and
            // the word cloud read as the ambient mass alone (Kevin, Sep 30).
            match (on, editing) {
                (true, _) => "Particles: on",
                // Off, but Edit room shows the effect: the painted embers
                // are its particles (`room_edit::Cloud::of`).
                (false, true) => "Particles: off (shown: editing)",
                (false, false) => "Particles: off",
            },
        )),
        // Every surface to none in one press, for telling what a single
        // assignment does afterwards (board #3326).
        Some(Cell::Button(Target::AllNone, "All: none")),
    ])
}

/// The room editor's row, over the cloud's in both layouts: its toggle and
/// its status (board #3326). The state is in the words, not a color.
fn edit_row(on: bool, status: &str) -> Control<'_> {
    Control::Pair([
        Some(Cell::Button(
            Target::ToggleEdit,
            if on {
                "Edit room: on"
            } else {
                "Edit room: off"
            },
        )),
        Some(Cell::Label(status)),
    ])
}

/// The row over it in both layouts: the pitcher's toggle and the debug
/// panel's. Their state is in the words, not a color.
fn menu_row(debug: bool, pitcher: bool) -> Control<'static> {
    Control::Pair([
        Some(Cell::Button(
            Target::TogglePitcher,
            if pitcher {
                "Pitcher: on"
            } else {
                "Pitcher: off"
            },
        )),
        Some(Cell::Button(
            Target::ToggleDebug,
            if debug {
                "Debug panel: on"
            } else {
                "Debug panel: off"
            },
        )),
    ])
}

/// GPU frame time over the last two seconds, with the 72 Hz budget line.
fn gpu_graph(ui: &mut egui::Ui, history: &[f32]) {
    let (rect, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 56.0), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, PALETTE.well);
    let y = |ms: f32| rect.bottom() - (ms / GRAPH_MAX_MS).clamp(0.0, 1.0) * rect.height();
    // Budget: a dashed line with its label (not a color).
    painter.add(egui::Shape::dashed_line(
        &[
            Pos2::new(rect.left(), y(BUDGET_MS)),
            Pos2::new(rect.right(), y(BUDGET_MS)),
        ],
        Stroke::new(1.0_f32, PALETTE.dim),
        5.0,
        4.0,
    ));
    painter.text(
        Pos2::new(rect.right() - 4.0, y(BUDGET_MS) - 2.0),
        egui::Align2::RIGHT_BOTTOM,
        "13.9 ms",
        egui::FontId::proportional(FONT_GRAPH),
        PALETTE.sub,
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
            Stroke::new(2.0_f32, PALETTE.line),
        ));
    }
    painter.text(
        rect.left_top() + Vec2::new(4.0, 2.0),
        egui::Align2::LEFT_TOP,
        "GPU ms, 2 s",
        egui::FontId::proportional(FONT_GRAPH),
        PALETTE.sub,
    );
}
