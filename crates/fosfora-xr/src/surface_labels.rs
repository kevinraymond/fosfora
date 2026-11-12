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
//! **Where** ([`pose`], [`placed`], board #5379). Every lane box gets one:
//! the room's planes and volumes, then the stage floor. Each label faces
//! the head and stays upright, its right edge level, so its text never
//! tilts ([`upright_axes`]). It sits on the line from its surface's center
//! (a plane's rect, a volume's box) to the head, just in front of the face
//! that line leaves by, so on the screen it covers the surface's center. It
//! stands [`CLEAR_M`] off that face at its nearest corner, so it never
//! z-fights with a wall or sinks into a table; seen at a slant it comes
//! nearer the head for that, and shrinks to keep its size on the screen.
//! Each is `label::label_w_m` wide for the face's distance from the head,
//! its height by the cell's aspect. A label behind the head or farther
//! than [`MAX_DISTANCE_M`] does not show. The first build laid a plane's
//! label flat on the plane; after wearing it Kevin asked for every label to
//! face the wearer (Nov 12).
//!
//! **Crowding** ([`spread`], [`Spread`], board #5379). Labels whose rects
//! overlap on the screen move apart along the screen's vertical, the one
//! whose surface is higher going up. A label that moved off its surface's
//! center gets a thin leader from its edge to that point, in the label's
//! ink ([`Leader`]). The spread is plain numbers in and out (rects on the
//! head's level view, offsets back), desktop-tested.
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
//! changes which quads draw, never the atlas. The leaders are strips of the
//! beams' pipeline. With no label shown nothing is drawn and the atlas is
//! left alone.

use glam::{Quat, Vec3};

use crate::label::{Billboard, FADE_S, fade_for, label_w_m};

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
/// A label's nearest corner stays this far off the face it stands in front
/// of (m): clear of a wall's or a table's occluder, and of the depth map's
/// noise on the real one.
pub const CLEAR_M: f32 = 0.04;
/// The point a label stands for, its leader's end: where the line from the
/// surface's center to the head leaves the surface, this far out (m).
pub const ANCHOR_GAP_M: f32 = 0.02;
/// A label comes at most this share of the way from its face to the head
/// to clear the face: a wall seen nearly edge-on keeps its label there and
/// lets the wall hide part of it, rather than bring it up to the eyes.
const MAX_PULL: f32 = 0.5;
/// The nearest a label's face comes to the head (m): a head inside a box
/// still sees its label ahead.
const NEAR_M: f32 = 0.3;
/// Seen from nearer straight above or below than this (the sine of the
/// angle from vertical), a label's level right shrinks toward nothing and
/// the head's own right blends in ([`upright_axes`]).
const LEVEL_MIN: f32 = 0.3;
/// The spread's passes over the pairs at most ([`spread`]); it stops early
/// once nothing moves. 32 labels packed 8 across and 4 up, each overlapping
/// its neighbors, need about 30 (16 leave them overlapping).
pub const SPREAD_PASSES: usize = 32;
/// The gap the spread keeps between two labels, of their heights.
pub const SPREAD_GAP: f32 = 0.15;
/// Two labels side by side push apart in full once their rects overlap
/// across by this share of the narrower one's width, and in part below
/// it, so the push grows from nothing as they slide together.
pub const SPREAD_RAMP: f32 = 0.25;
/// Two labels keep last frame's order (which went up) until their surfaces
/// are this share of the gap they need the other way round, so a near tie
/// does not swap them back and forth as the head moves.
pub const SPREAD_FLIP: f32 = 0.5;
/// A leader's width, of its label's height.
pub const LEADER_W: f32 = 0.05;

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

/// A surface's label before [`Spread`] moves it: its pose, and the point it
/// stands for, where the line from the surface's center to the head leaves
/// the surface ([`ANCHOR_GAP_M`] out). The two lie on that line, so on the
/// screen the label covers the point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Anchored {
    pub label: Billboard,
    pub anchor: Vec3,
}

/// The label of a box (`center`, `rot` box -> world, `half`) seen from
/// `head` turned by `head_rot`: facing the head with no roll
/// ([`upright_axes`]), on the line from the box's center to the head, its
/// nearest corner [`CLEAR_M`] in front of the face that line leaves by.
/// As wide as `label::label_w_m` gives for that face's distance, [`ASPECT`]
/// of that high. At a slant it has to come nearer the head to clear the
/// face (at most `MAX_PULL` of the way), and shrinks by as much, so its
/// size on the screen stays that of a label at the face.
pub fn pose(center: Vec3, rot: Quat, half: Vec3, head: Vec3, head_rot: Quat) -> Anchored {
    let to_head = head - center;
    let d = to_head.length();
    let front = to_head.normalize_or(head_rot * Vec3::Z);
    let (out, normal) = exit(rot.inverse() * front, half);
    let normal = rot * normal;
    let out = out.min((d - NEAR_M).max(0.0));
    let face = (d - out).max(NEAR_M);
    let (right, up) = upright_axes(front, head_rot);
    let half_w = label_w_m(face) * 0.5;
    let half_h = half_w * ASPECT;
    // Pulled `pull` toward the head and shrunk by (face - pull) / face, its
    // corners' reach across the face is that share of `reach`; it clears
    // the face once pull * cos >= that + CLEAR_M.
    let cos = front.dot(normal).max(1e-3);
    let reach = half_w * right.dot(normal).abs() + half_h * up.dot(normal).abs();
    let pull = ((reach + CLEAR_M) / (cos + reach / face)).min(face * MAX_PULL);
    let shrink = (face - pull) / face;
    Anchored {
        label: Billboard {
            center: center + front * (out + pull),
            right,
            up,
            half_w: half_w * shrink,
            half_h: half_h * shrink,
        },
        anchor: center + front * (out + ANCHOR_GAP_M.min(pull)),
    }
}

/// Where a ray from the center of a box of half extents `half` along the
/// unit `dir` (both box-local) leaves the box: the distance, and the
/// outward normal of the face it leaves by.
fn exit(dir: Vec3, half: Vec3) -> (f32, Vec3) {
    let mut out = (f32::INFINITY, Vec3::Z);
    for (i, axis) in [Vec3::X, Vec3::Y, Vec3::Z].into_iter().enumerate() {
        if dir[i].abs() > 1e-6 {
            let t = half[i] / dir[i].abs();
            if t < out.0 {
                out = (t, axis * dir[i].signum());
            }
        }
    }
    out
}

/// The right and up of a label that faces along the unit `front` (from
/// the label to the head) with no roll: its right level, world up across
/// the line of sight, on the side of the head's right, and its up from the
/// two, so its text never tilts. Seen from nearly straight above or below,
/// where the level right shrinks toward nothing, the head's right across
/// the line of sight blends in under `LEVEL_MIN`, so the label reads the
/// right way round from the head and does not spin as the head passes over
/// it.
pub fn upright_axes(front: Vec3, head_rot: Quat) -> (Vec3, Vec3) {
    let head_right = head_rot * Vec3::X;
    let mut level = Vec3::Y.cross(front);
    if level.dot(head_right) < 0.0 {
        level = -level;
    }
    let across = head_right - front * head_right.dot(front);
    let blend = (1.0 - level.length() / LEVEL_MIN).max(0.0);
    let right = (level + across * blend).normalize_or(front.any_orthonormal_vector());
    (right, front.cross(right))
}

/// The labels this frame draws, of `poses` (a lane box's index and its
/// label): those with a cell (an index under [`MAX_LABELS`]) whose center
/// is ahead of the head (`head_rot`'s view) and within [`MAX_DISTANCE_M`]
/// of it, farthest first, so a nearer label blends over a farther one.
pub fn placed(
    poses: impl IntoIterator<Item = (usize, Anchored)>,
    head: Vec3,
    head_rot: Quat,
) -> Vec<(usize, Anchored)> {
    let ahead = head_rot * Vec3::NEG_Z;
    let mut shown: Vec<(usize, Anchored, f32)> = poses
        .into_iter()
        .filter(|(k, _)| *k < MAX_LABELS)
        .filter_map(|(k, a)| {
            let to = a.label.center - head;
            let d = to.length();
            (to.dot(ahead) > 0.0 && d <= MAX_DISTANCE_M).then_some((k, a, d))
        })
        .collect();
    shown.sort_by(|a, b| b.2.total_cmp(&a.2));
    shown.into_iter().map(|(k, a, _)| (k, a)).collect()
}

/// A label's rect on the head's level view (`View`): its center and half
/// extents on the plane one meter ahead (x right, y up, 1 is 45 degrees).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenRect {
    pub x: f32,
    pub y: f32,
    pub half_w: f32,
    pub half_h: f32,
}

/// How far each of `rects` moves up the screen (down when negative) so
/// that no two overlap, `before` being each one's offset last frame (0 for
/// one not shown then). A plain relaxation: each pass pushes every pair
/// that overlaps apart along y, half each, until they are their heights
/// and [`SPREAD_GAP`] apart, up to [`SPREAD_PASSES`] passes. Only pairs
/// whose rects overlap across push, the push growing to its full size over
/// [`SPREAD_RAMP`] of that overlap. The one whose rect is higher goes up;
/// when the two are within [`SPREAD_FLIP`] of a gap of each other, the one
/// that was higher last frame does, and on a tie the earlier of the two
/// (farther, as [`placed`] orders them).
pub fn spread(rects: &[ScreenRect], before: &[f32]) -> Vec<f32> {
    let was = |i: usize| before.get(i).copied().unwrap_or(0.0);
    let mut dy = vec![0.0f32; rects.len()];
    for _ in 0..SPREAD_PASSES {
        let mut moved = 0.0f32;
        for i in 0..rects.len() {
            for j in i + 1..rects.len() {
                let (a, b) = (rects[i], rects[j]);
                let across = a.half_w + b.half_w - (a.x - b.x).abs();
                if across <= 0.0 {
                    continue;
                }
                let full = (a.half_h + b.half_h) * (1.0 + SPREAD_GAP);
                let ramp = (across / (SPREAD_RAMP * 2.0 * a.half_w.min(b.half_w))).min(1.0);
                let apart = b.y - a.y;
                let j_up = if apart.abs() > SPREAD_FLIP * full {
                    apart > 0.0
                } else {
                    b.y + was(j) > a.y + was(i)
                };
                let (lo, hi) = if j_up { (i, j) } else { (j, i) };
                let short = full * ramp - (rects[hi].y + dy[hi] - rects[lo].y - dy[lo]);
                if short > 0.0 {
                    dy[hi] += short * 0.5;
                    dy[lo] -= short * 0.5;
                    moved = moved.max(short);
                }
            }
        }
        if moved < 1e-6 {
            break;
        }
    }
    dy
}

/// The head's level view, the screen [`spread`] works on: its forward,
/// and a right and up with no roll (the right level).
#[derive(Debug, Clone, Copy)]
struct View {
    head: Vec3,
    forward: Vec3,
    right: Vec3,
    up: Vec3,
}

impl View {
    fn of(head: Vec3, head_rot: Quat) -> Self {
        let forward = head_rot * Vec3::NEG_Z;
        let level = forward.cross(Vec3::Y);
        // Looking straight up or down: the head's right, made level.
        let right = if level.length() > 0.1 {
            level.normalize()
        } else {
            (head_rot * Vec3::X).with_y(0.0).normalize_or(Vec3::X)
        };
        let up = right.cross(forward).normalize_or(Vec3::Y);
        Self {
            head,
            forward,
            right,
            up,
        }
    }

    /// `b`'s rect on this view, `None` for one not ahead of it.
    fn rect(&self, b: &Billboard) -> Option<ScreenRect> {
        let to = b.center - self.head;
        let depth = to.dot(self.forward);
        (depth > 1e-3).then(|| ScreenRect {
            x: to.dot(self.right) / depth,
            y: to.dot(self.up) / depth,
            half_w: b.half_w / depth,
            half_h: b.half_h / depth,
        })
    }
}

/// A leader: a thin line from a label that moved off its surface's center
/// to that point, `width_m` wide ([`LEADER_W`] of the label's height).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Leader {
    pub from: Vec3,
    pub to: Vec3,
    pub width_m: f32,
}

/// A label as drawn: its lane box, its pose after [`spread`], and its
/// leader when it moved off the point it stands for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shown {
    pub k: usize,
    pub label: Billboard,
    pub leader: Option<Leader>,
}

/// The spread across frames: each lane box's offset last frame, so two
/// labels near a tie keep their order ([`spread`]).
#[derive(Debug, Clone)]
pub struct Spread {
    before: [f32; MAX_LABELS],
}

impl Default for Spread {
    fn default() -> Self {
        Self {
            before: [0.0; MAX_LABELS],
        }
    }
}

impl Spread {
    /// This frame's `labels` ([`placed`]) as drawn from `head` turned by
    /// `head_rot`: their rects on the head's level view spread apart
    /// ([`spread`]), each moved by its offset along that view's up and
    /// turned to face the head again, keeping its size. A label moved off
    /// its anchor by more than half its height gets a [`Leader`] from the
    /// middle of its edge nearer the anchor (the bottom when it went up) to
    /// the anchor.
    pub fn place(
        &mut self,
        labels: &[(usize, Anchored)],
        head: Vec3,
        head_rot: Quat,
    ) -> Vec<Shown> {
        let view = View::of(head, head_rot);
        let rects: Vec<Option<ScreenRect>> =
            labels.iter().map(|(_, a)| view.rect(&a.label)).collect();
        let ahead: Vec<usize> = (0..labels.len()).filter(|&i| rects[i].is_some()).collect();
        let on_screen: Vec<ScreenRect> = rects.iter().filter_map(|r| *r).collect();
        let before: Vec<f32> = ahead
            .iter()
            .map(|&i| self.before.get(labels[i].0).copied().unwrap_or(0.0))
            .collect();
        let offsets = spread(&on_screen, &before);
        let mut dy = vec![0.0f32; labels.len()];
        self.before = [0.0; MAX_LABELS];
        for (&i, &d) in ahead.iter().zip(&offsets) {
            dy[i] = d;
            if let Some(b) = self.before.get_mut(labels[i].0) {
                *b = d;
            }
        }
        labels
            .iter()
            .zip(rects)
            .zip(dy)
            .map(|(((k, a), rect), dy)| {
                let Some(rect) = rect.filter(|_| dy != 0.0) else {
                    return Shown {
                        k: *k,
                        label: a.label,
                        leader: None,
                    };
                };
                let depth = (a.label.center - head).dot(view.forward);
                let center = a.label.center + view.up * (dy * depth);
                let front = (head - center).normalize_or(a.label.right.cross(a.label.up));
                let (right, up) = upright_axes(front, head_rot);
                let label = Billboard {
                    center,
                    right,
                    up,
                    ..a.label
                };
                let leader = (dy.abs() > rect.half_h).then(|| Leader {
                    from: center - up * (label.half_h * dy.signum()),
                    to: a.anchor,
                    width_m: 2.0 * label.half_h * LEADER_W,
                });
                Shown {
                    k: *k,
                    label,
                    leader,
                }
            })
            .collect()
    }
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
    use glam::Vec3;

    use super::{ATLAS_PX, CELL_PX, FADE_STAGES, MAX_LABELS, Shown, Tag, cell, cell_origin};
    use crate::gfx::{Beam, Gfx, PanelPose};
    use crate::label::{GROUND_ALPHA, PALETTE, TextureCanvas};

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

        /// Show this frame's labels (`Spread::place`), each reading its
        /// cell of an atlas holding `tags` (by lane box) at fade `stage`,
        /// and their leaders in the labels' ink at the same alpha, turned
        /// to face `head`. The atlas is redrawn only when the tags or the
        /// stage differ from the last drawn. Stage 0, or nothing shown,
        /// hides them.
        pub fn show(&mut self, gfx: &Gfx, tags: &[Tag], stage: u8, shown: &[Shown], head: Vec3) {
            let cells = tags.len().min(MAX_LABELS);
            let shown: Vec<&Shown> = shown.iter().filter(|s| s.k < cells).collect();
            if stage == 0 || shown.is_empty() {
                self.hide(gfx);
                return;
            }
            let alpha = f32::from(stage) / f32::from(FADE_STAGES);
            if self
                .drawn
                .as_ref()
                .is_none_or(|(t, s)| t.as_slice() != tags || *s != stage)
            {
                self.draw(gfx, &tags[..cells], alpha);
                self.drawn = Some((tags.to_vec(), stage));
            }
            let poses: Vec<(PanelPose, [f32; 4])> = shown
                .iter()
                .map(|s| {
                    let b = &s.label;
                    let pose = PanelPose {
                        center: b.center.to_array(),
                        right: (b.right * b.half_w).to_array(),
                        up: (b.up * b.half_h).to_array(),
                        v_max: 1.0,
                    };
                    (pose, cell(s.k))
                })
                .collect();
            let ink = egui::Rgba::from(PALETTE.text);
            let leaders: Vec<(Beam, [f32; 3])> = shown
                .iter()
                .filter_map(|s| s.leader)
                .map(|l| {
                    let beam = Beam {
                        start: l.from.to_array(),
                        end: l.to.to_array(),
                        eye: head.to_array(),
                        width_m: l.width_m,
                        alpha,
                    };
                    (beam, [ink.r(), ink.g(), ink.b()])
                })
                .collect();
            gfx.set_surface_label_poses(&poses);
            gfx.set_surface_label_leaders(&leaders);
        }

        /// Hide every label this frame; the atlas stays as drawn.
        pub fn hide(&self, gfx: &Gfx) {
            gfx.set_surface_label_poses(&[]);
            gfx.set_surface_label_leaders(&[]);
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

    /// `b` is a label's pose that faces `head` square on, its axes
    /// orthonormal.
    fn assert_faces(b: &Billboard, head: Vec3) {
        assert!((b.right.length() - 1.0).abs() < 1e-5, "{b:?}");
        assert!((b.up.length() - 1.0).abs() < 1e-5, "{b:?}");
        assert!(b.right.dot(b.up).abs() < 1e-5, "{b:?}");
        let front = b.right.cross(b.up);
        assert!(
            front.dot((head - b.center).normalize()) > 1.0 - 1e-5,
            "{b:?}"
        );
    }

    /// [`assert_faces`], and upright: its right level, its top up.
    fn assert_upright(b: &Billboard, head: Vec3) {
        assert_faces(b, head);
        assert!(b.right.y.abs() < 1e-5, "{b:?}");
        assert!(b.up.y > 0.0, "{b:?}");
    }

    /// The label's four corners.
    fn corners(b: &Billboard) -> [Vec3; 4] {
        let (r, u) = (b.right * b.half_w, b.up * b.half_h);
        [
            b.center - r - u,
            b.center + r - u,
            b.center + r + u,
            b.center - r + u,
        ]
    }

    /// `p` on the line from `from` to `to`, between them.
    fn assert_between(p: Vec3, from: Vec3, to: Vec3) {
        let (line, at) = ((to - from).normalize(), p - from);
        assert!(at.cross(line).length() < 1e-4, "{p} off {from} -> {to}");
        assert!(at.dot(line) > 0.0 && at.length() < from.distance(to), "{p}");
    }

    /// The label's size on the screen is that of a label at `face_m` from
    /// the head, `label::label_w_m` wide for that distance.
    fn assert_sized_for(b: &Billboard, head: Vec3, face_m: f32) {
        let d = b.center.distance(head);
        let want = label_w_m(face_m) * 0.5 / face_m;
        assert!((b.half_w / d - want).abs() < 1e-5, "{b:?} {face_m}");
        assert!((b.half_h / b.half_w - ASPECT).abs() < 1e-5, "{b:?}");
    }

    fn plane_half(w: f32, h: f32) -> Vec3 {
        Vec3::new(w, h, PLANE_HALF_THICKNESS_M)
    }

    const HEAD: Vec3 = Vec3::new(0.0, 1.2, 0.0);

    #[test]
    fn a_wall_label_sits_at_its_center_facing_the_head() {
        // A wall 3 m ahead, its local z toward the room.
        let center = Vec3::new(0.5, 1.25, -3.0);
        let half = plane_half(2.0, 1.25);
        let a = pose(center, Quat::IDENTITY, half, HEAD, Quat::IDENTITY);
        let b = a.label;
        assert_upright(&b, HEAD);
        // On the line from the wall's center to the head, so it covers the
        // center on the screen, and the anchor too, nearer the wall.
        assert_between(b.center, center, HEAD);
        assert_between(a.anchor, center, b.center);
        let face_z = center.z + PLANE_HALF_THICKNESS_M;
        assert!((a.anchor.z - face_z).abs() < ANCHOR_GAP_M + 1e-3, "{a:?}");
        // Square on, a few centimeters out: its corners clear the wall.
        let nearest = corners(&b)
            .map(|c| c.z)
            .into_iter()
            .fold(f32::MAX, f32::min);
        assert!(nearest >= face_z + CLEAR_M - 1e-4, "{nearest}");
        assert!(b.center.z - face_z < 0.08, "{b:?}");
        assert_sized_for(&b, HEAD, (center + Vec3::Z * 0.02).distance(HEAD));
        // The head rolled: the same label, its text level.
        let rolled = pose(
            center,
            Quat::IDENTITY,
            half,
            HEAD,
            Quat::from_rotation_z(0.4),
        );
        assert!(rolled.label.up.abs_diff_eq(b.up, 1e-5), "{rolled:?}");
        assert!(
            rolled.label.center.abs_diff_eq(b.center, 1e-5),
            "{rolled:?}"
        );
        // Seen from behind the wall, turned to it: on that side, facing
        // that head.
        let behind = Vec3::new(0.0, 1.2, -6.0);
        let turned = Quat::from_rotation_y(std::f32::consts::PI);
        let back = pose(center, Quat::IDENTITY, half, behind, turned);
        assert!(back.label.center.z < center.z, "{back:?}");
        assert_upright(&back.label, behind);
    }

    #[test]
    fn a_slanted_wall_label_comes_nearer_to_clear_the_wall() {
        // A wall on the left, along the view, its local z (the room side)
        // turned to +x; its center 2.5 m ahead.
        let center = Vec3::new(-1.5, 1.25, -2.5);
        let rot = Quat::from_rotation_y(FRAC_PI_2);
        let half = plane_half(2.0, 1.25);
        let a = pose(center, rot, half, HEAD, Quat::IDENTITY);
        let b = a.label;
        assert_upright(&b, HEAD);
        assert_between(b.center, center, HEAD);
        // Facing the head across the wall, its corners still clear it.
        let face_x = center.x + PLANE_HALF_THICKNESS_M;
        let nearest = corners(&b)
            .map(|c| c.x)
            .into_iter()
            .fold(f32::MAX, f32::min);
        assert!(nearest >= face_x + CLEAR_M - 1e-4, "{nearest} {b:?}");
        // It came well off the wall for that, and shrank by as much: the
        // same size on the screen as at the wall.
        let face = center.distance(HEAD) - PLANE_HALF_THICKNESS_M / ((HEAD - center).normalize().x);
        assert!(b.center.distance(HEAD) < face - 0.2, "{b:?}");
        assert_sized_for(&b, HEAD, face);
        // Nearly edge-on it stops halfway from the wall to the head.
        let far = Vec3::new(-0.3, 1.25, -5.5);
        let edge = pose(far, rot, half, HEAD, Quat::IDENTITY);
        let wall_m = edge.anchor.distance(HEAD) + ANCHOR_GAP_M;
        assert!(
            (edge.label.center.distance(HEAD) - wall_m * MAX_PULL).abs() < 1e-3,
            "{edge:?}"
        );
    }

    #[test]
    fn a_floor_or_ceiling_label_faces_the_head_upright() {
        // A floor plane ahead, its local z up.
        let floor_rot = Quat::from_rotation_x(-FRAC_PI_2);
        let floor = Vec3::new(0.0, 0.0, -1.5);
        let a = pose(floor, floor_rot, plane_half(3.0, 2.0), HEAD, Quat::IDENTITY);
        let b = a.label;
        assert_upright(&b, HEAD);
        assert_between(b.center, floor, HEAD);
        // Its top away from the head, its right the head's right, and its
        // bottom edge clear of the floor.
        assert!(b.up.z < 0.0, "{b:?}");
        assert!(b.right.abs_diff_eq(Vec3::X, 1e-5), "{b:?}");
        let lowest = corners(&b)
            .map(|c| c.y)
            .into_iter()
            .fold(f32::MAX, f32::min);
        assert!(
            lowest >= PLANE_HALF_THICKNESS_M + CLEAR_M - 1e-4,
            "{lowest}"
        );
        // A ceiling ahead, its local z down: its top up, toward the far
        // side, its top edge clear of the ceiling.
        let ceiling_rot = Quat::from_rotation_x(FRAC_PI_2);
        let ceiling = Vec3::new(0.0, 2.6, -1.5);
        let c = pose(
            ceiling,
            ceiling_rot,
            plane_half(3.0, 2.0),
            HEAD,
            Quat::IDENTITY,
        )
        .label;
        assert_upright(&c, HEAD);
        assert!(c.right.abs_diff_eq(Vec3::X, 1e-5), "{c:?}");
        let highest = corners(&c)
            .map(|p| p.y)
            .into_iter()
            .fold(f32::MIN, f32::max);
        assert!(
            highest <= 2.6 - PLANE_HALF_THICKNESS_M - CLEAR_M + 1e-4,
            "{highest}"
        );
        // Right under the head: the head's forward is the top, its right
        // the right.
        let looking_down = Quat::from_rotation_x(-1.4);
        let under = pose(
            Vec3::ZERO,
            floor_rot,
            plane_half(1.0, 1.0),
            HEAD,
            looking_down,
        )
        .label;
        assert_faces(&under, HEAD);
        assert!(under.up.abs_diff_eq(Vec3::NEG_Z, 1e-4), "{under:?}");
        assert!(under.right.abs_diff_eq(Vec3::X, 1e-4), "{under:?}");
        // Turned to the side, the same.
        let turned = Quat::from_rotation_y(FRAC_PI_2) * looking_down;
        let side = pose(Vec3::ZERO, floor_rot, plane_half(1.0, 1.0), HEAD, turned).label;
        assert_faces(&side, HEAD);
        assert!(side.up.abs_diff_eq(Vec3::NEG_X, 1e-4), "{side:?}");
        // Just behind the feet, looked at past them: its top still away
        // from the head, not upside down.
        let feet = pose(
            Vec3::new(0.0, 0.0, 0.25),
            floor_rot,
            plane_half(0.3, 0.3),
            HEAD,
            looking_down,
        )
        .label;
        assert_faces(&feet, HEAD);
        assert!(feet.up.z < 0.0 && feet.right.x > 0.0, "{feet:?}");
    }

    #[test]
    fn a_volume_label_sits_at_its_center_clear_of_the_box() {
        // A table 0.75 m high, 1.2 by 0.8 m, ahead and to the right.
        let center = Vec3::new(0.8, 0.375, -1.0);
        let half = Vec3::new(0.6, 0.375, 0.4);
        let a = pose(center, Quat::IDENTITY, half, HEAD, Quat::IDENTITY);
        let b = a.label;
        assert_upright(&b, HEAD);
        assert_between(b.center, center, HEAD);
        assert_between(a.anchor, center, b.center);
        // The line to the head leaves by the side toward the head (z): the
        // label's corners are in front of it.
        let nearest = corners(&b)
            .map(|c| c.z)
            .into_iter()
            .fold(f32::MAX, f32::min);
        assert!(nearest >= center.z + half.z + CLEAR_M - 1e-4, "{nearest}");
        // A box on its side: still on the line, still clear of the face.
        let tipped = pose(
            center,
            Quat::from_rotation_z(FRAC_PI_2),
            Vec3::new(0.375, 0.6, 0.4),
            HEAD,
            Quat::IDENTITY,
        );
        assert_between(tipped.label.center, center, HEAD);
        assert!(
            tipped.label.center.abs_diff_eq(b.center, 1e-4),
            "{tipped:?}"
        );
        // A head inside a box sees the label between the center and it.
        let big = pose(
            HEAD + Vec3::new(0.0, 0.0, -0.5),
            Quat::IDENTITY,
            Vec3::splat(2.0),
            HEAD,
            Quat::IDENTITY,
        );
        assert_faces(&big.label, HEAD);
        assert!(
            big.label.center.distance(HEAD) >= NEAR_M * (1.0 - MAX_PULL) - 1e-4,
            "{big:?}"
        );
    }

    /// A label at `center` for `placed`.
    fn at(center: Vec3) -> Anchored {
        Anchored {
            label: Billboard {
                center,
                right: Vec3::X,
                up: Vec3::Y,
                half_w: 0.2,
                half_h: 0.075,
            },
            anchor: center,
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

    fn rect(x: f32, y: f32) -> ScreenRect {
        ScreenRect {
            x,
            y,
            half_w: 0.1,
            half_h: 0.0375,
        }
    }

    /// The most two of `rects` moved by `dy` overlap on the screen.
    fn worst_overlap(rects: &[ScreenRect], dy: &[f32]) -> f32 {
        let mut worst = f32::NEG_INFINITY;
        for i in 0..rects.len() {
            for j in i + 1..rects.len() {
                let (a, b) = (rects[i], rects[j]);
                let across = a.half_w + b.half_w - (a.x - b.x).abs();
                let up = a.half_h + b.half_h - (a.y + dy[i] - b.y - dy[j]).abs();
                worst = worst.max(across.min(up));
            }
        }
        worst
    }

    #[test]
    fn labels_apart_on_the_screen_stay_put() {
        let rects = [
            rect(0.0, 0.0),
            rect(0.25, 0.0),
            rect(0.0, 0.1),
            rect(-0.3, -0.3),
        ];
        let dy = spread(&rects, &[]);
        assert_close!(dy, [0.0; 4]);
        assert!(spread(&[], &[]).is_empty());
        assert_close!(spread(&[rect(0.0, 0.0)], &[]), [0.0]);
    }

    #[test]
    fn two_labels_on_top_of_each_other_part_along_the_vertical() {
        let rects = [rect(0.0, 0.0), rect(0.02, 0.01)];
        let dy = spread(&rects, &[]);
        // The higher one up, the other down, as much each, to their
        // heights and the gap apart.
        assert!(dy[1] > 0.0 && dy[0] < 0.0, "{dy:?}");
        assert!((dy[0] + dy[1]).abs() < 1e-6, "{dy:?}");
        let full = 2.0 * 0.0375 * (1.0 + SPREAD_GAP);
        assert!(
            (rects[1].y + dy[1] - rects[0].y - dy[0] - full).abs() < 1e-5,
            "{dy:?}"
        );
        assert!(worst_overlap(&rects, &dy) <= 0.0);
        // Either way round.
        let swapped = spread(&[rects[1], rects[0]], &[]);
        assert_close!(swapped, [dy[1], dy[0]]);
    }

    #[test]
    fn side_by_side_labels_push_only_as_they_overlap_across() {
        let full = 2.0 * 0.0375 * (1.0 + SPREAD_GAP);
        let gap = |x: f32| {
            let rects = [rect(0.0, 0.0), rect(x, 0.001)];
            let dy = spread(&rects, &[]);
            rects[1].y + dy[1] - dy[0]
        };
        // Apart across: no push.
        assert!((gap(0.2) - 0.001).abs() < 1e-6);
        assert!((gap(0.25) - 0.001).abs() < 1e-6);
        // A sliver across: a small push, growing with the overlap.
        let ramp = SPREAD_RAMP * 0.2;
        let (a, b) = (gap(0.2 - 0.25 * ramp), gap(0.2 - 0.5 * ramp));
        assert!(a > 0.001 && a < b && b < full, "{a} {b}");
        assert!((b - 0.5 * full).abs() < 1e-5, "{b}");
        // Past the ramp: the full gap.
        assert!((gap(0.2 - ramp) - full).abs() < 1e-5);
        assert!((gap(0.0) - full).abs() < 1e-5);
    }

    #[test]
    fn a_stack_of_labels_ends_in_order_with_no_overlap() {
        // Six labels at almost one point, their surfaces a hair apart.
        let rects: Vec<ScreenRect> = (0..6)
            .map(|i| rect(0.005 * i as f32, 0.001 * i as f32))
            .collect();
        let dy = spread(&rects, &[]);
        let full = 2.0 * 0.0375 * (1.0 + SPREAD_GAP);
        // In their surfaces' order, a gap apart, no overlap, centered.
        for i in 1..6 {
            let step = rects[i].y + dy[i] - rects[i - 1].y - dy[i - 1];
            assert!(step > full - 1e-3, "{i}: {step} {dy:?}");
        }
        assert!(worst_overlap(&rects, &dy) <= 0.0, "{dy:?}");
        assert!(dy.iter().sum::<f32>().abs() < 1e-5, "{dy:?}");
    }

    #[test]
    fn a_crowd_of_32_labels_spreads_within_the_passes() {
        // Every cell's label in one band of the view: 8 columns of 4, each
        // label overlapping its neighbors across and up.
        let rects: Vec<ScreenRect> = (0..MAX_LABELS)
            .map(|i| {
                let (col, row) = ((i % 8) as f32, (i / 8) as f32);
                rect(-0.5 + 0.13 * col, -0.05 + 0.03 * row + 0.004 * col)
            })
            .collect();
        assert!(worst_overlap(&rects, &[0.0; MAX_LABELS]) > 0.03);
        let dy = spread(&rects, &[]);
        assert!(
            worst_overlap(&rects, &dy) <= 0.0,
            "{}",
            worst_overlap(&rects, &dy)
        );
    }

    #[test]
    fn a_near_tie_keeps_last_frames_order() {
        let full = 2.0 * 0.0375 * (1.0 + SPREAD_GAP);
        // The second's surface a hair higher: it goes up...
        let rects = [rect(0.0, 0.0), rect(0.0, 0.002)];
        let dy = spread(&rects, &[]);
        assert!(dy[1] > 0.0, "{dy:?}");
        // ...unless the first was up last frame: then the first stays up.
        let kept = spread(&rects, &[0.5 * full, -0.5 * full]);
        assert!(kept[0] > 0.0 && kept[1] < 0.0, "{kept:?}");
        // Once the second is higher by more than the flip, it goes up.
        let clear = [rect(0.0, 0.0), rect(0.0, SPREAD_FLIP * full + 0.002)];
        let flipped = spread(&clear, &[0.5 * full, -0.5 * full]);
        assert!(flipped[1] > 0.0 && flipped[0] < 0.0, "{flipped:?}");
        // A dead tie: the first (farther) goes up.
        let tie = spread(&[rect(0.0, 0.0), rect(0.0, 0.0)], &[]);
        assert!(tie[0] > 0.0, "{tie:?}");
    }

    /// A wall `center` facing +z, 1 by 0.5 m.
    fn wall(center: Vec3) -> Anchored {
        pose(
            center,
            Quat::IDENTITY,
            plane_half(0.5, 0.25),
            HEAD,
            Quat::IDENTITY,
        )
    }

    #[test]
    fn crowded_labels_move_apart_and_point_at_their_surfaces() {
        // Two walls one behind the other, their centers nearly in line
        // from the head (the far one a hair higher on the screen), and a
        // third well to the side.
        let near = Vec3::new(0.0, 1.25, -3.0);
        let far = Vec3::new(0.05, 1.29, -5.0);
        let side = Vec3::new(-2.5, 1.2, -3.0);
        let labels = placed(
            [(0, wall(near)), (1, wall(far)), (2, wall(side))],
            HEAD,
            Quat::IDENTITY,
        );
        assert_eq!(
            labels.iter().map(|(k, _)| *k).collect::<Vec<_>>(),
            [1, 2, 0]
        );
        let mut spread = Spread::default();
        let shown = spread.place(&labels, HEAD, Quat::IDENTITY);
        let by = |shown: &[Shown], k: usize| *shown.iter().find(|s| s.k == k).expect("shown");
        let anchored = |k: usize| labels.iter().find(|(j, _)| *j == k).expect("placed").1;
        // The side one is alone: as placed, no leader.
        assert_eq!(by(&shown, 2).label, anchored(2).label);
        assert_eq!(by(&shown, 2).leader, None);
        // The far wall's label went up, the near one's down, each straight
        // along the vertical, still facing the head, upright, its size
        // kept.
        for (k, up) in [(1, true), (0, false)] {
            let (s, a) = (by(&shown, k), anchored(k));
            let moved = s.label.center - a.label.center;
            assert!(moved.x.abs() < 1e-6 && moved.z.abs() < 1e-6, "{k}: {moved}");
            assert_eq!(moved.y > 0.0, up, "{k}: {moved}");
            assert_upright(&s.label, HEAD);
            assert_close!(
                [s.label.half_w, s.label.half_h],
                [a.label.half_w, a.label.half_h]
            );
            // Off its surface's center: a leader from the middle of its
            // edge nearer the center to the center's anchor.
            let leader = s.leader.expect("a leader");
            assert_eq!(leader.to, a.anchor);
            let edge = if up { -s.label.half_h } else { s.label.half_h };
            assert!(
                leader
                    .from
                    .abs_diff_eq(s.label.center + s.label.up * edge, 1e-6)
            );
            assert_close!(leader.width_m, 2.0 * s.label.half_h * LEADER_W);
        }
        // On the screen they no longer overlap.
        let screen_y = |head: Vec3, s: &Shown| {
            View::of(head, Quat::IDENTITY)
                .rect(&s.label)
                .expect("ahead")
        };
        let rects = [
            screen_y(HEAD, &by(&shown, 0)),
            screen_y(HEAD, &by(&shown, 1)),
        ];
        assert!(worst_overlap(&rects, &[0.0, 0.0]) <= 0.0);
        // The next frame the head is 12 cm lower, so the near center is
        // now the higher on the screen, but by less than the flip: the
        // order holds.
        let lower = HEAD - Vec3::Y * 0.12;
        let poses = [(0, near), (1, far), (2, side)].map(|(k, c)| {
            let a = pose(
                c,
                Quat::IDENTITY,
                plane_half(0.5, 0.25),
                lower,
                Quat::IDENTITY,
            );
            (k, a)
        });
        let anchors = |k: usize| {
            screen_y(
                lower,
                &Shown {
                    k,
                    label: poses[k].1.label,
                    leader: None,
                },
            )
            .y
        };
        assert!(anchors(0) > anchors(1));
        let again = spread.place(&placed(poses, lower, Quat::IDENTITY), lower, Quat::IDENTITY);
        assert!(screen_y(lower, &by(&again, 1)).y > screen_y(lower, &by(&again, 0)).y);
        // A fresh spread puts the near one up.
        let fresh =
            Spread::default().place(&placed(poses, lower, Quat::IDENTITY), lower, Quat::IDENTITY);
        assert!(screen_y(lower, &by(&fresh, 0)).y > screen_y(lower, &by(&fresh, 1)).y);
    }

    #[test]
    fn a_head_that_rolls_moves_nothing() {
        let labels = placed(
            [
                (0, wall(Vec3::new(0.0, 1.25, -3.0))),
                (1, wall(Vec3::new(0.05, 1.29, -5.0))),
            ],
            HEAD,
            Quat::IDENTITY,
        );
        let level = Spread::default().place(&labels, HEAD, Quat::IDENTITY);
        let rolled = Spread::default().place(&labels, HEAD, Quat::from_rotation_z(0.3));
        for (a, b) in level.iter().zip(&rolled) {
            assert!(
                a.label.center.abs_diff_eq(b.label.center, 1e-5),
                "{a:?} {b:?}"
            );
            assert!(a.label.up.abs_diff_eq(b.label.up, 1e-5), "{a:?} {b:?}");
        }
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
