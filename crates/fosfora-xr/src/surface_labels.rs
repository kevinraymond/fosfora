//! The surface labels (board #5378): every surface the voice path can name
//! shows that name on itself. Voice commands address surfaces by name
//! ("wall 6", "table 3", "desk"), but nothing in the room showed those
//! names. "Make the wall blue" came back `Ambiguous(["wall 1", "wall 6",
//! "wall 8", "wall 14"])`, "make wall 16 blue" came back "No surface called
//! wall 16", and the wearer could not tell which wall was which (Kevin,
//! worn, Nov 12: "what I need is some way to see the registered surfaces,
//! like labels on the walls").
//!
//! **What a label says.** Two lines. The first is the surface's name
//! exactly as the grammar takes it (`intent::surfaces`, the vocabulary's
//! `name`). The second is its effective behavior in the editor's status
//! cell's word (`label::behavior_word`: "aurora", "embers", "none").
//!
//! **Where** ([`pose`], [`placed`]). Every lane box gets one: the room's
//! planes and volumes, then the stage floor. A plane's label lies flat on
//! its face toward the head (`surfaces::Face::facing`), centered on the
//! rect and lifted `label::OUT_M` toward the room. It stands upright on a
//! wall and turns on a floor or a ceiling so it reads from the head. A
//! volume's label floats over the center of its top face and faces the
//! head, as the editor's label floats over a hit (`label::billboard`).
//! Each is `label::label_w_m` wide for its distance from the head, its
//! height by the cell's aspect. A surface behind the head or farther than
//! [`MAX_DISTANCE_M`] shows none.
//!
//! **When** ([`SurfaceLabels`]). The labels show while the room editor is
//! on, and while a voice window is open (listening, or its transcription
//! running). They stay [`HOLD_S`] after the window closes, and [`HOLD_S`]
//! after a miss on a surface's name (`intent::Reason::names_surfaces`:
//! `UnknownSurface`, `Ambiguous`). With `debug.fosfora.labels 1` they show
//! all the time. They fade in over `label::FADE_S` and out over the same,
//! a hold fading over its last `FADE_S` as `label::fade_for` ends a
//! label's life.
//!
//! **The atlas.** One texture of [`GRID`] cells of [`CELL_PX`], a cell per
//! lane box up to [`MAX_LABELS`] (the lanes' rows, `SURFACE_LANE_ROWS`).
//! On Android [`LabelAtlas`] rasterizes every label into its cell with egui,
//! as the editor's label is drawn (`label::TextureCanvas`). It redraws the
//! atlas only when a name, a behavior or the fade's stage
//! ([`SurfaceLabels::stage`]) changes. Each label shown is a posed quad of
//! the panel's pipeline reading its cell ([`cell`]), so turning the head
//! changes which quads draw, never the atlas. With no label shown nothing
//! is drawn and the atlas is left alone.

use glam::{Quat, Vec3};

use crate::label::{Billboard, FADE_S, OUT_M, billboard, facing, fade_for, label_w_m};
use crate::surfaces::{Face, is_plane};

/// How long the labels stay after a voice window closes, and after a miss
/// on a surface's name (s), their fade included.
pub const HOLD_S: f32 = 4.0;
/// The farthest a label shows from the head (m).
pub const MAX_DISTANCE_M: f32 = 6.0;
/// The most labels: a cell each, one per lane box, as the lanes have a row
/// per box up to `surfaces::SURFACE_LANE_ROWS`.
pub const MAX_LABELS: usize = crate::surfaces::SURFACE_LANE_ROWS;
/// One label's cell in the atlas (px): its name over its behavior.
pub const CELL_PX: [u32; 2] = [512, 192];
/// The atlas's cells, across and down.
pub const GRID: [u32; 2] = [4, 8];
/// The atlas's size (px).
pub const ATLAS_PX: [u32; 2] = [CELL_PX[0] * GRID[0], CELL_PX[1] * GRID[1]];
const _: () = assert!(
    (GRID[0] * GRID[1]) as usize == MAX_LABELS,
    "a cell per label"
);
/// A label's height for its width: the cell's aspect.
pub const ASPECT: f32 = CELL_PX[1] as f32 / CELL_PX[0] as f32;
/// The fade's steps: the atlas is drawn at a stage's alpha, so a fade
/// redraws it this many times, not every frame.
pub const FADE_STAGES: u8 = 8;
/// A plane whose face tilts less than 60 degrees from vertical is a wall
/// to its label: the length of the world's up laid into it.
const UPRIGHT_MIN: f32 = 0.5;

/// What one label says: the surface's name as the grammar takes it, and
/// its effective behavior (`label::behavior_word`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tag {
    pub name: String,
    pub behavior: &'static str,
}

/// Why the labels show this frame, besides a hold.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Reasons {
    /// The room editor is on.
    pub editing: bool,
    /// A voice window is open: listening, or its transcription running.
    pub listening: bool,
    /// `debug.fosfora.labels 1`.
    pub knob: bool,
}

impl Reasons {
    /// Whether any reason holds.
    pub fn any(self) -> bool {
        self.editing || self.listening || self.knob
    }

    /// The reasons for the log, "edit room, voice window"; empty for none.
    pub fn words(self) -> String {
        let words: Vec<&str> = [
            (self.editing, "edit room"),
            (self.listening, "voice window"),
            (self.knob, "debug.fosfora.labels"),
        ]
        .into_iter()
        .filter_map(|(on, word)| on.then_some(word))
        .collect();
        words.join(", ")
    }
}

/// When the labels show, across frames: the reasons last seen, the hold
/// left, and the alpha. Plain numbers, desktop-tested.
#[derive(Debug, Clone, Default)]
pub struct SurfaceLabels {
    was: Reasons,
    /// Seconds left of the hold: after a voice window, after a miss, or
    /// the fade after the last reason went.
    hold: f32,
    /// What started the last hold of [`HOLD_S`], for the log.
    held_by: &'static str,
    alpha: f32,
}

impl SurfaceLabels {
    /// A miss on a surface's name: the labels show for [`HOLD_S`] from
    /// now, or longer if a hold already runs past that.
    pub fn miss(&mut self) {
        self.hold = self.hold.max(HOLD_S);
        self.held_by = "a miss on a name";
    }

    /// This frame's `reasons`, `dt` seconds after the last. A voice window
    /// that closed holds the labels for [`HOLD_S`]; the last reason going
    /// fades them out over `FADE_S`. They fade in over `FADE_S`.
    pub fn step(&mut self, reasons: Reasons, dt: f32) {
        let dt = dt.max(0.0);
        if self.was.listening && !reasons.listening {
            self.hold = self.hold.max(HOLD_S);
            self.held_by = "a voice window closed";
        }
        if self.was.any() && !reasons.any() {
            self.hold = self.hold.max(FADE_S);
        }
        self.was = reasons;
        self.hold = (self.hold - dt).max(0.0);
        let to = if reasons.any() {
            1.0
        } else {
            fade_for(0.0, self.hold)
        };
        self.alpha = to.min(self.alpha + dt / FADE_S);
    }

    /// Why they show, for the log: the last step's reasons
    /// ([`Reasons::words`]), else what started the hold.
    pub fn why(&self) -> String {
        if self.was.any() {
            self.was.words()
        } else {
            self.held_by.to_owned()
        }
    }

    /// The labels' alpha now, 0 hidden.
    pub fn alpha(&self) -> f32 {
        self.alpha
    }

    /// The fade's stage now: 0 hidden, [`FADE_STAGES`] at full, any alpha
    /// above 0 at least 1. The atlas is drawn at `stage / FADE_STAGES`.
    pub fn stage(&self) -> u8 {
        (self.alpha.clamp(0.0, 1.0) * f32::from(FADE_STAGES)).ceil() as u8
    }
}

/// The label of a box (`center`, `rot` box -> world, `half`) seen from
/// `head` turned by `head_rot`: on a plane, flat on its face toward the
/// head at the rect's center, lifted `label::OUT_M`; on a volume, over its
/// top face's center facing the head (`label::billboard`). As wide as
/// `label::label_w_m` gives for its distance, [`ASPECT`] of that high.
pub fn pose(center: Vec3, rot: Quat, half: Vec3, head: Vec3, head_rot: Quat) -> Billboard {
    if is_plane(half) {
        let face = Face::facing(center, rot, half, head);
        flat(
            face.center + face.normal * OUT_M,
            face.normal,
            head,
            head_rot,
        )
    } else {
        let top = Face::of(center, rot, half);
        let b = billboard(top.center, top.normal, head, head_rot);
        Billboard {
            half_h: b.half_w * ASPECT,
            ..b
        }
    }
}

/// A label lying on a plane: centered at `center`, its front along the
/// plane's unit `normal` (toward the head). On a wall it stands upright
/// (its up the world's laid into the plane); on a floor or a ceiling its
/// up is that of a label facing the head there laid into the plane, so it
/// reads from the head the right way round.
fn flat(center: Vec3, normal: Vec3, head: Vec3, head_rot: Quat) -> Billboard {
    let level = Vec3::Y - normal * normal.y;
    let up = if level.length() > UPRIGHT_MIN {
        level.normalize()
    } else {
        let seen = facing(center, head, head_rot).up;
        (seen - normal * seen.dot(normal)).normalize_or(normal.any_orthonormal_vector())
    };
    let half_w = label_w_m(center.distance(head)) * 0.5;
    Billboard {
        center,
        right: up.cross(normal),
        up,
        half_w,
        half_h: half_w * ASPECT,
    }
}

/// The labels this frame draws, of `poses` (a lane box's index and its
/// label's pose): those with a cell (an index under [`MAX_LABELS`]) whose
/// center is ahead of the head (`head_rot`'s view) and within
/// [`MAX_DISTANCE_M`] of it, farthest first, so a nearer label blends
/// over a farther one.
pub fn placed(
    poses: impl IntoIterator<Item = (usize, Billboard)>,
    head: Vec3,
    head_rot: Quat,
) -> Vec<(usize, Billboard)> {
    let ahead = head_rot * Vec3::NEG_Z;
    let mut shown: Vec<(usize, Billboard, f32)> = poses
        .into_iter()
        .filter(|(k, _)| *k < MAX_LABELS)
        .filter_map(|(k, b)| {
            let to = b.center - head;
            let d = to.length();
            (to.dot(ahead) > 0.0 && d <= MAX_DISTANCE_M).then_some((k, b, d))
        })
        .collect();
    shown.sort_by(|a, b| b.2.total_cmp(&a.2));
    shown.into_iter().map(|(k, b, _)| (k, b)).collect()
}

/// Cell `k`'s top-left corner in the atlas (px), row by row.
pub fn cell_origin(k: usize) -> [u32; 2] {
    let k = k as u32;
    [(k % GRID[0]) * CELL_PX[0], (k / GRID[0]) * CELL_PX[1]]
}

/// Cell `k`'s rect in texture coordinates: its top-left corner and its
/// size (left, top, width, height), as `gfx::Gfx::set_surface_label_poses`
/// takes it.
pub fn cell(k: usize) -> [f32; 4] {
    let [x, y] = cell_origin(k);
    [
        x as f32 / ATLAS_PX[0] as f32,
        y as f32 / ATLAS_PX[1] as f32,
        1.0 / GRID[0] as f32,
        1.0 / GRID[1] as f32,
    ]
}

#[cfg(target_os = "android")]
pub use render::LabelAtlas;

#[cfg(target_os = "android")]
mod render {
    use egui::{FontId, Pos2, Rect, Vec2};

    use super::{ATLAS_PX, CELL_PX, FADE_STAGES, MAX_LABELS, Tag, cell, cell_origin};
    use crate::gfx::{Gfx, PanelPose};
    use crate::label::{Billboard, GROUND_ALPHA, PALETTE, TextureCanvas};

    /// The name's and the behavior's largest sizes, and the smallest a
    /// long one shrinks to (px).
    const NAME_PX: f32 = 72.0;
    const BEHAVIOR_PX: f32 = 52.0;
    const MIN_FONT_PX: f32 = 28.0;
    /// The two lines' centers from the cell's top (px).
    const NAME_Y_PX: f32 = 72.0;
    const BEHAVIOR_Y_PX: f32 = 140.0;
    /// The margin the text keeps from the ground's ends, the ground's
    /// corner radius, and its inset from the cell's edge (px): the inset
    /// keeps a cell's samples off its neighbors'.
    const MARGIN_PX: f32 = 28.0;
    const CORNER_PX: u8 = 36;
    const INSET_PX: f32 = 4.0;

    /// The atlas and the tags and stage it holds.
    pub struct LabelAtlas {
        canvas: TextureCanvas,
        drawn: Option<(Vec<Tag>, u8)>,
    }

    impl LabelAtlas {
        /// Build the atlas and hand it to `gfx` with a posed quad per
        /// cell; nothing shows until [`Self::show`].
        pub fn new(gfx: &mut Gfx) -> Self {
            let canvas = TextureCanvas::new(gfx, "xr-surface-labels", ATLAS_PX);
            gfx.set_surface_labels(&canvas.view, MAX_LABELS);
            Self {
                canvas,
                drawn: None,
            }
        }

        /// Show the `placed` labels this frame (a lane box's index and its
        /// pose), each reading its cell of an atlas holding `tags` (by
        /// lane box) at fade `stage`; the atlas is redrawn only when the
        /// tags or the stage differ from the last drawn. Stage 0, or
        /// nothing placed, hides them.
        pub fn show(&mut self, gfx: &Gfx, tags: &[Tag], stage: u8, placed: &[(usize, Billboard)]) {
            let cells = tags.len().min(MAX_LABELS);
            let poses: Vec<(PanelPose, [f32; 4])> = placed
                .iter()
                .filter(|(k, _)| *k < cells)
                .map(|(k, b)| {
                    let pose = PanelPose {
                        center: b.center.to_array(),
                        right: (b.right * b.half_w).to_array(),
                        up: (b.up * b.half_h).to_array(),
                        v_max: 1.0,
                    };
                    (pose, cell(*k))
                })
                .collect();
            if stage == 0 || poses.is_empty() {
                gfx.set_surface_label_poses(&[]);
                return;
            }
            if self
                .drawn
                .as_ref()
                .is_none_or(|(t, s)| t.as_slice() != tags || *s != stage)
            {
                self.draw(
                    gfx,
                    &tags[..cells],
                    f32::from(stage) / f32::from(FADE_STAGES),
                );
                self.drawn = Some((tags.to_vec(), stage));
            }
            gfx.set_surface_label_poses(&poses);
        }

        /// Hide every label this frame; the atlas stays as drawn.
        pub fn hide(&self, gfx: &Gfx) {
            gfx.set_surface_label_poses(&[]);
        }

        /// Render each tag into its cell at `alpha`: the editor's label's
        /// rounded dark ground, the name over the behavior, each line
        /// centered and shrunk to fit the ground's width.
        fn draw(&mut self, gfx: &Gfx, tags: &[Tag], alpha: f32) {
            self.canvas.paint(gfx, |painter| {
                let ink = PALETTE.text.gamma_multiply(alpha);
                for (k, tag) in tags.iter().enumerate() {
                    let [x, y] = cell_origin(k);
                    let cell = Rect::from_min_size(
                        Pos2::new(x as f32, y as f32),
                        Vec2::new(CELL_PX[0] as f32, CELL_PX[1] as f32),
                    );
                    let ground = cell.shrink(INSET_PX);
                    painter.rect_filled(
                        ground,
                        CORNER_PX,
                        PALETTE.panel.gamma_multiply(GROUND_ALPHA * alpha),
                    );
                    let room = ground.width() - 2.0 * MARGIN_PX;
                    for (text, px, line_y) in [
                        (tag.name.as_str(), NAME_PX, NAME_Y_PX),
                        (tag.behavior, BEHAVIOR_PX, BEHAVIOR_Y_PX),
                    ] {
                        let layout = |px: f32| {
                            painter.layout_no_wrap(text.to_owned(), FontId::proportional(px), ink)
                        };
                        let mut galley = layout(px);
                        if galley.size().x > room {
                            galley = layout((px * room / galley.size().x).max(MIN_FONT_PX));
                        }
                        let center = Pos2::new(ground.center().x, cell.top() + line_y);
                        painter.galley(center - galley.size() * 0.5, galley, ink);
                    }
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::FRAC_PI_2;

    use super::*;
    use crate::label::{LABEL_W_MIN_M, UP_M};
    use crate::surfaces::PLANE_HALF_THICKNESS_M;

    const DT: f32 = 1.0 / 72.0;

    /// Step `l` for `seconds` at 72 Hz with `reasons`.
    fn run(l: &mut SurfaceLabels, reasons: Reasons, seconds: f32) {
        for _ in 0..(seconds / DT).round() as usize {
            l.step(reasons, DT);
        }
    }

    const EDITING: Reasons = Reasons {
        editing: true,
        listening: false,
        knob: false,
    };
    const LISTENING: Reasons = Reasons {
        editing: false,
        listening: true,
        knob: false,
    };
    const KNOB: Reasons = Reasons {
        editing: false,
        listening: false,
        knob: true,
    };
    const NONE: Reasons = Reasons {
        editing: false,
        listening: false,
        knob: false,
    };

    #[test]
    fn nothing_shows_without_a_reason() {
        let mut l = SurfaceLabels::default();
        run(&mut l, NONE, 10.0);
        assert_close!(l.alpha(), 0.0);
        assert_eq!(l.stage(), 0);
    }

    #[test]
    fn the_editor_shows_them_fading_in_and_out() {
        let mut l = SurfaceLabels::default();
        l.step(EDITING, DT);
        assert!(l.alpha() > 0.0 && l.alpha() < 0.1, "{}", l.alpha());
        assert_eq!(l.stage(), 1);
        // Halfway through the fade in.
        run(&mut l, EDITING, FADE_S * 0.5 - DT);
        assert!((l.alpha() - 0.5).abs() < 0.03, "{}", l.alpha());
        run(&mut l, EDITING, 60.0);
        assert_close!(l.alpha(), 1.0);
        assert_eq!(l.stage(), FADE_STAGES);
        // Off: gone over the fade, not at once.
        l.step(NONE, DT);
        assert!(l.alpha() > 0.9, "{}", l.alpha());
        run(&mut l, NONE, FADE_S * 0.5 - DT);
        assert!((l.alpha() - 0.5).abs() < 0.03, "{}", l.alpha());
        run(&mut l, NONE, FADE_S * 0.5 + 2.0 * DT);
        assert_close!(l.alpha(), 0.0);
        assert_eq!(l.stage(), 0);
    }

    #[test]
    fn a_voice_window_shows_them_and_holds_them_after_it_closes() {
        let mut l = SurfaceLabels::default();
        run(&mut l, LISTENING, 3.0);
        assert_close!(l.alpha(), 1.0);
        assert_eq!(l.why(), "voice window");
        // Closed: full until the hold's last fade, gone at its end.
        run(&mut l, NONE, HOLD_S - FADE_S - 0.05);
        assert_close!(l.alpha(), 1.0);
        assert_eq!(l.why(), "a voice window closed");
        run(&mut l, NONE, FADE_S * 0.5 + 0.05);
        assert!((l.alpha() - 0.5).abs() < 0.03, "{}", l.alpha());
        run(&mut l, NONE, FADE_S * 0.5 + 2.0 * DT);
        assert_eq!(l.stage(), 0);
    }

    #[test]
    fn a_miss_on_a_name_shows_them_for_the_hold() {
        let mut l = SurfaceLabels::default();
        l.miss();
        run(&mut l, NONE, FADE_S + DT);
        assert_close!(l.alpha(), 1.0);
        assert_eq!(l.why(), "a miss on a name");
        run(&mut l, NONE, HOLD_S - 2.0 * FADE_S - 0.1);
        assert_close!(l.alpha(), 1.0);
        run(&mut l, NONE, FADE_S + 0.1 + 2.0 * DT);
        assert_eq!(l.stage(), 0);
        // A miss while a window's hold runs keeps the later end.
        run(&mut l, LISTENING, 1.0);
        l.step(NONE, DT);
        run(&mut l, NONE, 3.0);
        l.miss();
        run(&mut l, NONE, HOLD_S - FADE_S - 0.1);
        assert_close!(l.alpha(), 1.0);
        run(&mut l, NONE, FADE_S + 0.1 + 2.0 * DT);
        assert_eq!(l.stage(), 0);
    }

    #[test]
    fn the_knob_keeps_them_up() {
        let mut l = SurfaceLabels::default();
        run(&mut l, KNOB, 120.0);
        assert_close!(l.alpha(), 1.0);
        // A window opening and closing under the knob changes nothing.
        let both = Reasons {
            listening: true,
            ..KNOB
        };
        run(&mut l, both, 2.0);
        run(&mut l, KNOB, 10.0);
        assert_close!(l.alpha(), 1.0);
        assert_eq!(KNOB.words(), "debug.fosfora.labels");
        assert_eq!(l.why(), "debug.fosfora.labels");
        assert_eq!(
            Reasons {
                editing: true,
                ..both
            }
            .words(),
            "edit room, voice window, debug.fosfora.labels"
        );
        assert_eq!(NONE.words(), "");
    }

    #[test]
    fn the_stage_steps_the_alpha() {
        let at = |alpha: f32| {
            SurfaceLabels {
                alpha,
                ..SurfaceLabels::default()
            }
            .stage()
        };
        assert_eq!(at(0.0), 0);
        assert_eq!(at(0.001), 1);
        assert_eq!(at(1.0 / f32::from(FADE_STAGES)), 1);
        assert_eq!(at(0.5), FADE_STAGES / 2);
        assert_eq!(at(0.99), FADE_STAGES);
        assert_eq!(at(1.0), FADE_STAGES);
        // A fade redraws the atlas once per stage, not once per frame.
        let mut l = SurfaceLabels::default();
        let mut stages = vec![l.stage()];
        for _ in 0..(FADE_S / DT) as usize + 2 {
            l.step(EDITING, DT);
            if stages.last() != Some(&l.stage()) {
                stages.push(l.stage());
            }
        }
        assert_eq!(stages, (0..=FADE_STAGES).collect::<Vec<_>>());
    }

    fn assert_orthonormal_and_facing(b: &Billboard, head: Vec3) {
        assert!((b.right.length() - 1.0).abs() < 1e-5, "{b:?}");
        assert!((b.up.length() - 1.0).abs() < 1e-5, "{b:?}");
        assert!(b.right.dot(b.up).abs() < 1e-5, "{b:?}");
        // The front (right x up) faces the head's side.
        assert!(b.right.cross(b.up).dot(head - b.center) > 0.0, "{b:?}");
    }

    fn plane_half(w: f32, h: f32) -> Vec3 {
        Vec3::new(w, h, PLANE_HALF_THICKNESS_M)
    }

    #[test]
    fn a_wall_label_lies_flat_upright_at_its_center() {
        let head = Vec3::new(0.0, 1.2, 0.0);
        // A wall 3 m ahead, its local z toward the room.
        let center = Vec3::new(0.5, 1.25, -3.0);
        let b = pose(
            center,
            Quat::IDENTITY,
            plane_half(2.0, 1.25),
            head,
            Quat::IDENTITY,
        );
        let lift = PLANE_HALF_THICKNESS_M + OUT_M;
        assert!(b.center.abs_diff_eq(center + Vec3::Z * lift, 1e-5), "{b:?}");
        assert_orthonormal_and_facing(&b, head);
        // Flat on the wall: its front is the wall's normal, not the view.
        assert!(b.right.cross(b.up).abs_diff_eq(Vec3::Z, 1e-5), "{b:?}");
        assert!(b.up.abs_diff_eq(Vec3::Y, 1e-5), "{b:?}");
        assert!(b.right.abs_diff_eq(Vec3::X, 1e-5), "{b:?}");
        // The head rolled: a wall's label stays upright.
        let rolled = pose(
            center,
            Quat::IDENTITY,
            plane_half(2.0, 1.25),
            head,
            Quat::from_rotation_z(0.4),
        );
        assert!(rolled.up.abs_diff_eq(Vec3::Y, 1e-5), "{rolled:?}");
        // Seen from behind the wall's other side: the face toward the
        // head, still upright and readable.
        let behind = Vec3::new(0.0, 1.2, -6.0);
        let back = pose(
            center,
            Quat::IDENTITY,
            plane_half(2.0, 1.25),
            behind,
            Quat::IDENTITY,
        );
        assert!(back.center.z < center.z, "{back:?}");
        assert_orthonormal_and_facing(&back, behind);
        assert!(back.up.abs_diff_eq(Vec3::Y, 1e-5), "{back:?}");
        // Sized as the editor's label for its distance, the cell's aspect.
        let w = label_w_m(b.center.distance(head));
        assert_close!(b.half_w, w * 0.5);
        assert_close!(b.half_h, w * 0.5 * 192.0 / 512.0);
    }

    #[test]
    fn a_floor_or_ceiling_label_reads_from_the_head() {
        let head = Vec3::new(0.0, 1.2, 0.0);
        // A floor plane ahead, its local z up.
        let floor_rot = Quat::from_rotation_x(-FRAC_PI_2);
        let floor = Vec3::new(0.0, 0.0, -1.5);
        let b = pose(floor, floor_rot, plane_half(3.0, 2.0), head, Quat::IDENTITY);
        assert!(
            b.center
                .abs_diff_eq(floor + Vec3::Y * (PLANE_HALF_THICKNESS_M + OUT_M), 1e-5),
            "{b:?}"
        );
        assert_orthonormal_and_facing(&b, head);
        assert!(b.right.cross(b.up).abs_diff_eq(Vec3::Y, 1e-5), "{b:?}");
        // Its top away from the head, its right the head's right.
        assert!(b.up.abs_diff_eq(Vec3::NEG_Z, 1e-5), "{b:?}");
        assert!(b.right.abs_diff_eq(Vec3::X, 1e-5), "{b:?}");
        // A ceiling ahead, its local z down: its top toward the head (the
        // view's up as the head tips back to it).
        let ceiling_rot = Quat::from_rotation_x(FRAC_PI_2);
        let ceiling = Vec3::new(0.0, 2.6, -1.5);
        let c = pose(
            ceiling,
            ceiling_rot,
            plane_half(3.0, 2.0),
            head,
            Quat::IDENTITY,
        );
        assert_orthonormal_and_facing(&c, head);
        assert!(c.right.cross(c.up).abs_diff_eq(Vec3::NEG_Y, 1e-5), "{c:?}");
        assert!(c.up.abs_diff_eq(Vec3::Z, 1e-5), "{c:?}");
        assert!(c.right.abs_diff_eq(Vec3::X, 1e-5), "{c:?}");
        // The head turned to the side: the floor label turns with it.
        let turned = Quat::from_rotation_y(FRAC_PI_2);
        let side = Vec3::new(-1.5, 0.0, 0.0);
        let s = pose(side, floor_rot, plane_half(1.0, 1.0), head, turned);
        assert_orthonormal_and_facing(&s, head);
        assert!(s.up.abs_diff_eq(Vec3::NEG_X, 1e-5), "{s:?}");
        // Right under the head: the head's forward is the top.
        let under = pose(
            Vec3::new(0.0, 0.0, 0.0),
            floor_rot,
            plane_half(1.0, 1.0),
            head,
            Quat::IDENTITY,
        );
        assert_orthonormal_and_facing(&under, head);
        assert!(under.up.abs_diff_eq(Vec3::NEG_Z, 1e-5), "{under:?}");
    }

    #[test]
    fn a_volume_label_floats_over_its_top_facing_the_head() {
        let head = Vec3::new(0.0, 1.2, 0.0);
        // A table 0.75 m high, 1.2 by 0.8 m, ahead and to the right.
        let center = Vec3::new(0.8, 0.375, -1.0);
        let half = Vec3::new(0.6, 0.375, 0.4);
        let b = pose(center, Quat::IDENTITY, half, head, Quat::IDENTITY);
        assert!(
            b.center
                .abs_diff_eq(Vec3::new(0.8, 0.75 + OUT_M + UP_M, -1.0), 1e-5),
            "{b:?}"
        );
        // A billboard: its front straight at the head.
        let front = b.right.cross(b.up);
        assert!(
            front.dot((head - b.center).normalize()) > 1.0 - 1e-5,
            "{b:?}"
        );
        assert_close!(b.half_w, LABEL_W_MIN_M * 0.5);
        assert_close!(b.half_h, LABEL_W_MIN_M * 0.5 * ASPECT);
        // A box on its side is still a volume: the top is its top.
        let tipped = pose(
            center,
            Quat::from_rotation_z(FRAC_PI_2),
            Vec3::new(0.375, 0.6, 0.4),
            head,
            Quat::IDENTITY,
        );
        assert!(
            (tipped.center.y - (0.75 + OUT_M + UP_M)).abs() < 1e-5,
            "{tipped:?}"
        );
    }

    /// A pose at `center` for `placed`.
    fn at(center: Vec3) -> Billboard {
        Billboard {
            center,
            right: Vec3::X,
            up: Vec3::Y,
            half_w: 0.2,
            half_h: 0.075,
        }
    }

    #[test]
    fn only_labels_ahead_and_near_show_farthest_first() {
        let head = Vec3::new(0.0, 1.2, 0.0);
        let poses = [
            (0, at(Vec3::new(0.0, 1.2, -2.0))),
            // Behind the head.
            (1, at(Vec3::new(0.0, 1.2, 2.0))),
            // 7 m ahead: too far.
            (2, at(Vec3::new(0.0, 1.2, -7.0))),
            (3, at(Vec3::new(1.0, 0.5, -5.0))),
            // Beside the head, a little behind it: not ahead.
            (4, at(Vec3::new(3.0, 1.2, 0.2))),
            (5, at(Vec3::new(-1.0, 2.5, -0.5))),
            // Past the atlas's cells.
            (MAX_LABELS, at(Vec3::new(0.0, 1.2, -1.0))),
        ];
        let shown: Vec<usize> = placed(poses, head, Quat::IDENTITY)
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        assert_eq!(shown, [3, 0, 5]);
        // Turned around: the ones behind show instead.
        let turned = Quat::from_rotation_y(std::f32::consts::PI);
        let shown: Vec<usize> = placed(poses, head, turned)
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        assert_eq!(shown, [4, 1]);
        // At most a label per cell.
        let many = (0..2 * MAX_LABELS).map(|k| (k, at(Vec3::new(0.0, 1.2, -1.0 - 0.1 * k as f32))));
        let shown = placed(many, head, Quat::IDENTITY);
        assert_eq!(shown.len(), MAX_LABELS);
        assert!(shown.iter().all(|(k, _)| *k < MAX_LABELS));
        assert_eq!(shown[0].0, MAX_LABELS - 1);
    }

    #[test]
    fn each_label_has_its_own_cell_of_the_atlas() {
        assert_eq!(ATLAS_PX, [2048, 1536]);
        assert_eq!(cell_origin(0), [0, 0]);
        assert_eq!(cell_origin(3), [1536, 0]);
        assert_eq!(cell_origin(4), [0, 192]);
        assert_eq!(cell_origin(MAX_LABELS - 1), [1536, 1344]);
        assert_close!(cell(0), [0.0, 0.0, 0.25, 0.125]);
        assert_close!(cell(5), [0.25, 0.125, 0.25, 0.125]);
        assert_close!(cell(MAX_LABELS - 1), [0.75, 0.875, 0.25, 0.125]);
        // No two cells overlap, and every one lies in the atlas.
        for k in 0..MAX_LABELS {
            let [u, v, du, dv] = cell(k);
            assert!(u + du <= 1.0 + 1e-6 && v + dv <= 1.0 + 1e-6, "{k}");
            for j in 0..k {
                assert_ne!(cell_origin(j), cell_origin(k), "{j} {k}");
            }
        }
    }
}
