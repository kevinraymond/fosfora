//! Room surfaces as particle emitters (board #3317). Plain numbers in, so it
//! builds and tests on the desktop as well.
//!
//! Every obstacle box carries a surface kind (from the scene anchor's
//! semantic label) and an emitter weight in two lanes of the aux block that
//! were written 0 before: `aux[67 + k].w` is the kind as a float,
//! `aux[131 + k].w` the weight in 0..1 (`flux_xr_sim.wgsl`, "Surface
//! lanes"). A world sim in surface mode picks a box by weight times the
//! kind's audio gate and spawns on its top face; the other sims and the
//! depth occluders read only `xyz` and ignore both lanes.
//!
//! The top face is found the same way here and in the shader: the box's
//! local axis whose world direction is closest to vertical, signed to point
//! up (tables and scene floors are local +Z up, the synthetic stage floor
//! +Y, the ceiling picks -Z).
//!
//! Each box also runs a behavior (board #3326), chosen per surface instead
//! of by its kind: a lane of its own in the aux block, one row per box
//! after the pour row ([`lane_row`]). The app writes every box's row from
//! the room file (`lanes::lane_rows`), its kind's default
//! ([`SurfaceBehavior::default_for`]) when the file has no entry for it,
//! so the sim never sees an unset row for a box. A zero row is unset and
//! the sim runs its own kind default (`xr_kind_behavior`), the rule from
//! before D2 (a table sheds embers, a floor the rings): an upload without
//! the lanes (the core's tests, which use tables) changes nothing.

use glam::{Quat, Vec3};

/// Surface kinds, as the sim reads them from `aux[67 + k].w`.
pub const KIND_NONE: u32 = 0;
pub const KIND_TABLE: u32 = 1;
pub const KIND_FLOOR: u32 = 2;
pub const KIND_WALL: u32 = 3;
pub const KIND_CEILING: u32 = 4;
pub const KIND_FRAME: u32 = 5;
pub const KIND_OTHER: u32 = 6;

/// The kinds a room file and the `surface` knob name, by name.
pub const KIND_NAMES: [(u32, &str); 6] = [
    (KIND_TABLE, "table"),
    (KIND_FLOOR, "floor"),
    (KIND_WALL, "wall"),
    (KIND_CEILING, "ceiling"),
    (KIND_FRAME, "frame"),
    (KIND_OTHER, "other"),
];

/// A kind's name in [`KIND_NAMES`], `none` for [`KIND_NONE`] (an anchor
/// without labels) and anything unknown.
pub fn kind_name(kind: u32) -> &'static str {
    KIND_NAMES
        .iter()
        .find(|(k, _)| *k == kind)
        .map_or("none", |(_, n)| n)
}

/// The kind [`KIND_NAMES`] calls `name` (any case).
pub fn kind_from_name(name: &str) -> Option<u32> {
    let name = name.trim();
    KIND_NAMES
        .iter()
        .find(|(_, n)| n.eq_ignore_ascii_case(name))
        .map(|(k, _)| *k)
}

/// What a surface does (board #3326): the value of its lane. Ids 0 to 3
/// are particle rules and the wall spectrum; 4 to 7 are the surface
/// shaders of the surfaces pass (`surface_fx.rs`, board #3472): the rings
/// (the floor ripple, renamed, any face), the streamlines (D1), the curls
/// and the pulse (D2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SurfaceBehavior {
    /// Nothing: the surface is an obstacle only.
    #[default]
    None,
    /// Embers shed off the top face on the beat (Embers; a table's
    /// default).
    Embers,
    /// Sparks up off the top face with the bass (Embers; a floor's
    /// default until step 2d).
    Sparks,
    /// The wall spectrum (`canvas.rs`; a wall's default).
    Spectrum,
    /// Rings of light spreading on the beat (`ripple.rs`, drawn by the
    /// surfaces pass; a floor's default): the floor ripple until D1, now
    /// on any face. The room file and the knob still read `ripple`.
    Rings,
    /// Lines of light flowing across the face along a curl-noise field
    /// (`surface_fx.rs`, the mockup's tabletop).
    Streamlines,
    /// The streamlines' field tighter and faster, read as swirls: closed
    /// curls with a faint fill (`surface_fx.rs`, the mockup's chairs).
    Curls,
    /// A whole-face glow that jumps on the downbeat and breathes with the
    /// bar (`surface_fx.rs`, for frames, lamps and anything small).
    Pulse,
}

/// Behavior names the catalogue reserves for a later step: the knob and
/// the room file reject them instead of reading them as unknown. Empty
/// since D2 made curls and pulse real.
pub const RESERVED_BEHAVIORS: [&str; 0] = [];

/// The old name of [`SurfaceBehavior::Rings`], still read (rooms saved
/// before D1, the knob's habit); a save writes `rings`.
pub const RINGS_ALIAS: &str = "ripple";

impl SurfaceBehavior {
    /// Every behavior, in id order.
    pub const ALL: [Self; 8] = [
        Self::None,
        Self::Embers,
        Self::Sparks,
        Self::Spectrum,
        Self::Rings,
        Self::Streamlines,
        Self::Curls,
        Self::Pulse,
    ];

    /// The catalogue id (`XR_BEHAVIOR_*` in `flux_xr_sim.wgsl`, `BEHAVIOR_*`
    /// in `surface_fx::SURFACE_FX_WGSL`).
    pub fn id(self) -> u32 {
        self as u32
    }

    /// The behavior of catalogue id `id`; `None` for a reserved or unknown
    /// id.
    pub fn from_id(id: u32) -> Option<Self> {
        Self::ALL.get(id as usize).copied()
    }

    /// The name the room file and the knob use.
    pub fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Embers => "embers",
            Self::Sparks => "sparks",
            Self::Spectrum => "spectrum",
            Self::Rings => "rings",
            Self::Streamlines => "streamlines",
            Self::Curls => "curls",
            Self::Pulse => "pulse",
        }
    }

    /// The behavior called `name` (any case), [`RINGS_ALIAS`] included.
    pub fn from_name(name: &str) -> Option<Self> {
        let name = name.trim();
        if name.eq_ignore_ascii_case(RINGS_ALIAS) {
            return Some(Self::Rings);
        }
        Self::ALL
            .into_iter()
            .find(|b| b.name().eq_ignore_ascii_case(name))
    }

    /// The next behavior in the whole catalogue, wrapping: none, embers,
    /// sparks, spectrum, rings, streamlines, curls, pulse, none. The room editor steps
    /// through a kind's own ([`Self::next_for`]); this one is for the
    /// knob's side.
    #[must_use]
    pub fn next(self) -> Self {
        Self::ALL[(self.id() as usize + 1) % Self::ALL.len()]
    }

    /// The behaviors that render on a surface of `kind`, in the room
    /// editor's cycle order, `none` first and the kind's default
    /// ([`Self::default_for`]) next, so one tap from the default goes to
    /// the next look of that kind (board #3488, the mockup's room): a
    /// table none, streamlines, curls, pulse, embers, sparks; a floor
    /// none, rings, streamlines, curls, sparks; a wall none, spectrum,
    /// streamlines, rings, pulse; a ceiling none, rings, pulse,
    /// streamlines; a frame none, pulse, rings, streamlines; any other
    /// kind and an unlabeled anchor none, curls, streamlines, pulse,
    /// embers. Embers and sparks stay on the lists of the volumes they
    /// fall off: the cloud comes back per surface (decision #3474).
    /// Stepping through the whole catalogue instead left most steps looking
    /// alike on a given surface (a wall on embers, sparks or none emits
    /// nothing; Kevin, worn, Sep 29). The knob still takes any behavior on
    /// any kind.
    pub fn catalogue(kind: u32) -> &'static [Self] {
        match kind {
            KIND_TABLE => &[
                Self::None,
                Self::Streamlines,
                Self::Curls,
                Self::Pulse,
                Self::Embers,
                Self::Sparks,
            ],
            KIND_FLOOR => &[
                Self::None,
                Self::Rings,
                Self::Streamlines,
                Self::Curls,
                Self::Sparks,
            ],
            KIND_WALL => &[
                Self::None,
                Self::Spectrum,
                Self::Streamlines,
                Self::Rings,
                Self::Pulse,
            ],
            KIND_CEILING => &[Self::None, Self::Rings, Self::Pulse, Self::Streamlines],
            KIND_FRAME => &[Self::None, Self::Pulse, Self::Rings, Self::Streamlines],
            _ => &[
                Self::None,
                Self::Curls,
                Self::Streamlines,
                Self::Pulse,
                Self::Embers,
            ],
        }
    }

    /// The step after this one in `kind`'s [`Self::catalogue`], wrapping.
    /// Off the catalogue (the knob put the spectrum on a table), the first
    /// entry after `none`, or `none` when the catalogue is only `none`.
    #[must_use]
    pub fn next_for(self, kind: u32) -> Self {
        let order = Self::catalogue(kind);
        match order.iter().position(|&b| b == self) {
            Some(i) => order[(i + 1) % order.len()],
            None => order.get(1).copied().unwrap_or(Self::None),
        }
    }

    /// Whether the sim spawns particles on a surface running it.
    pub fn emits(self) -> bool {
        matches!(self, Self::Embers | Self::Sparks)
    }

    /// Whether the surfaces pass draws a surface running it
    /// (`surface_fx.rs`): the rings, the streamlines, the curls and the
    /// pulse. Embers and sparks are the cloud's, the spectrum is still the
    /// canvas's own draw.
    pub fn draws_on_face(self) -> bool {
        matches!(
            self,
            Self::Rings | Self::Streamlines | Self::Curls | Self::Pulse
        )
    }

    /// The audio band a surface running it follows when its lane does not
    /// name one (`surface_fx::BAND_*`): the bass for the rings and the
    /// pulse, which hit with the kick; the rms for the rest, whose flow
    /// wants the whole mix.
    pub fn default_band(self) -> u32 {
        match self {
            Self::Rings | Self::Pulse => crate::surface_fx::BAND_BASS,
            _ => crate::surface_fx::BAND_RMS,
        }
    }

    /// What a surface of `kind` runs with no assignment: the mockup's
    /// room (board #3488): tables the streamlines, floors the rings (the
    /// ripple, step 2d, decision #3459), walls the spectrum, frames the
    /// pulse, any other labeled surface (storage, a couch, a screen, a
    /// lamp, a plant) the curls; ceilings and unlabeled anchors nothing.
    /// Nothing emits by default: the cloud comes back on a surface by
    /// assigning it embers or sparks (decision #3474). Each default but
    /// none is the first entry after `none` in the kind's
    /// [`Self::catalogue`], so a fresh room is the mockup and one tap goes
    /// to the next look of that kind. Until D2 tables shed embers and
    /// frames and other surfaces ran nothing.
    pub fn default_for(kind: u32) -> Self {
        match kind {
            KIND_TABLE => Self::Streamlines,
            KIND_FLOOR => Self::Rings,
            KIND_WALL => Self::Spectrum,
            KIND_FRAME => Self::Pulse,
            KIND_OTHER => Self::Curls,
            _ => Self::None,
        }
    }
}

/// The surfaces pass's color per kind (linear RGB, board #3472), the
/// mockup's family: tables a clear blue, floors violet, walls the canvas's
/// warm white, storage and other volumes green, frames and ceilings amber.
/// In [`KIND_NAMES`]' order. The color is the surface's and the shape is
/// the behavior's: two surfaces on the same behavior differ by color, two
/// behaviors on one surface by shape, never by hue alone.
pub const PALETTE: [(u32, [f32; 3]); 6] = [
    (KIND_TABLE, [0.22, 0.52, 1.0]),
    (KIND_FLOOR, [0.58, 0.32, 1.0]),
    (KIND_WALL, [1.0, 0.86, 0.68]),
    (KIND_CEILING, [1.0, 0.62, 0.18]),
    (KIND_FRAME, [1.0, 0.62, 0.18]),
    (KIND_OTHER, [0.3, 1.0, 0.5]),
];

/// The color of a surface of `kind` in the surfaces pass ([`PALETTE`]); an
/// unlabeled anchor or an unknown kind takes other's green.
pub fn palette(kind: u32) -> [f32; 3] {
    PALETTE
        .iter()
        .find(|(k, _)| *k == kind)
        .or_else(|| PALETTE.iter().find(|(k, _)| *k == KIND_OTHER))
        .map_or([1.0; 3], |(_, c)| *c)
}

/// Rows of the surface behavior lanes, one per obstacle box
/// (`XR_AUX_SURFACE_ROWS` in `flux_xr_sim.wgsl`, `MAX_BOXES`).
pub const SURFACE_LANE_ROWS: usize = 32;

/// A box's lane: `x` the behavior's id + 1, so the zero row stays
/// "unset", `y` the strength 0..1 (the sim scales the behavior's gate by
/// it), `z` and `w` two parameters (unused in this pass).
pub fn lane_row(behavior: SurfaceBehavior, strength: f32, params: [f32; 2]) -> [f32; 4] {
    [
        (behavior.id() + 1) as f32,
        strength.clamp(0.0, 1.0),
        params[0],
        params[1],
    ]
}

/// The behavior a box of `kind` runs with lane `row`: the lane's when set,
/// else the kind's default (the sim's `xr_box_behavior` reads a set row
/// the same way; for an unset one it keeps the pre-D2 kind rule, which
/// only an upload without the lanes sees). An unknown id runs nothing.
pub fn lane_behavior(row: [f32; 4], kind: u32) -> SurfaceBehavior {
    let code = (row[0].max(0.0) + 0.5) as u32;
    if code == 0 {
        SurfaceBehavior::default_for(kind)
    } else {
        SurfaceBehavior::from_id(code - 1).unwrap_or_default()
    }
}

/// The surface kind of a scene anchor from its semantic labels (the
/// runtime's comma-separated list, `RECOGNIZED_LABELS` in `room.rs`). When
/// an anchor carries several, the most specific wins: a table before a
/// floor, a floor before the rest.
pub fn surface_kind(label: &str) -> u32 {
    let kind_of = |l: &str| match l.trim().to_ascii_uppercase().as_str() {
        "" => KIND_NONE,
        "DESK" | "TABLE" => KIND_TABLE,
        "FLOOR" => KIND_FLOOR,
        "WALL_FACE" | "INVISIBLE_WALL_FACE" => KIND_WALL,
        "CEILING" => KIND_CEILING,
        "WINDOW_FRAME" | "DOOR_FRAME" => KIND_FRAME,
        _ => KIND_OTHER,
    };
    // Lower rank = more specific.
    let rank = |k: u32| match k {
        KIND_TABLE => 0,
        KIND_FLOOR => 1,
        KIND_CEILING => 2,
        KIND_WALL => 3,
        KIND_FRAME => 4,
        KIND_OTHER => 5,
        _ => 6,
    };
    label
        .split(',')
        .map(kind_of)
        .min_by_key(|&k| rank(k))
        .unwrap_or(KIND_NONE)
}

/// Whether an anchor's labels name a wall the runtime hides
/// (`INVISIBLE_WALL_FACE`: a boundary Space Setup adds to close an open
/// room, with no real surface behind it). Its kind stays [`KIND_WALL`], so
/// the particles still collide with it, but nothing is drawn on it.
pub fn is_hidden_wall(label: &str) -> bool {
    label
        .split(',')
        .any(|l| l.trim().eq_ignore_ascii_case("INVISIBLE_WALL_FACE"))
}

/// What the room editor's status row calls box `k` of `boxes` (the lanes'
/// order: the room's boxes, then the stage floor): the label that decides
/// its kind ([`surface_kind`]) as a word, `desk`, `table`, `wall`,
/// `floor`, `ceiling`, `window`, `door`, `storage`, `couch`, any other
/// label lowercased with spaces for underscores, the stage floor `floor`;
/// with the box index appended (`table 3`) when another box in `boxes`
/// has the same name, so two tables are told apart by a word, not a
/// color. Empty past the boxes.
pub fn friendly_name(k: usize, boxes: &[crate::lanes::LaneBox<'_>]) -> String {
    let Some(b) = boxes.get(k) else {
        return String::new();
    };
    let name = base_name(b);
    if boxes
        .iter()
        .enumerate()
        .any(|(i, o)| i != k && base_name(o) == name)
    {
        format!("{name} {k}")
    } else {
        name
    }
}

/// [`friendly_name`] without the disambiguation.
fn base_name(b: &crate::lanes::LaneBox<'_>) -> String {
    if b.uuid == crate::room_file::STAGE_FLOOR_UUID {
        return "floor".to_owned();
    }
    let kind = surface_kind(b.label);
    let label = b
        .label
        .split(',')
        .map(str::trim)
        .find(|l| surface_kind(l) == kind)
        .unwrap_or("")
        .to_ascii_uppercase();
    match label.as_str() {
        "" => "surface".to_owned(),
        "DESK" => "desk".to_owned(),
        "TABLE" => "table".to_owned(),
        "WALL_FACE" => "wall".to_owned(),
        "FLOOR" => "floor".to_owned(),
        "CEILING" => "ceiling".to_owned(),
        "WINDOW_FRAME" => "window".to_owned(),
        "DOOR_FRAME" => "door".to_owned(),
        "STORAGE" => "storage".to_owned(),
        "COUCH" => "couch".to_owned(),
        other => other.to_ascii_lowercase().replace('_', " "),
    }
}

/// The synthetic stage floor's emitter flag: 1 when the room returned no
/// FLOOR box, else 0, so the floor never emits twice. The flag only lets
/// the stage floor emit; its behavior decides whether it does: the rings
/// by default (no weight), sparks when assigned.
pub fn synthetic_floor_emit(room_kinds: impl IntoIterator<Item = u32>) -> f32 {
    if room_kinds.into_iter().any(|k| k == KIND_FLOOR) {
        0.0
    } else {
        1.0
    }
}

/// Half thickness `room.rs` gives a 2D scene plane (a wall, a floor, the
/// ceiling, a door or window frame) so fast particles cannot tunnel
/// through it in one step (m): a box with a half extent this thin is a
/// plane, not a volume ([`is_plane`]).
pub const PLANE_HALF_THICKNESS_M: f32 = 0.02;

/// Whether a box of half extents `half` is a scene plane given
/// [`PLANE_HALF_THICKNESS_M`], not a volume (a table, storage, a couch,
/// the stage floor's 10 cm slab).
pub fn is_plane(half: Vec3) -> bool {
    half.min_element() <= PLANE_HALF_THICKNESS_M + 1e-4
}

/// The face of a box the behavior on it acts on, the room editor's
/// highlight (step 2h): a volume's top face ([`Face::of`], the face the
/// sim spawns on), a plane's face toward `head` ([`Face::facing`], a
/// wall's room side, a floor's top, the ceiling's underside). The
/// highlight tinted the face the ray entered ([`Face::across`]) before, so
/// a table's side and its top lit as two surfaces while the behavior is
/// one per box: "It's not obvious that the top and sides of table 14 are
/// connected" (Kevin, worn, Sep 30).
pub fn acting_face(center: Vec3, rot: Quat, half: Vec3, head: Vec3) -> Face {
    if is_plane(half) {
        Face::facing(center, rot, half, head)
    } else {
        Face::of(center, rot, half)
    }
}

/// Whether the top face of a box (`center` relative to the anchor, `rot`
/// box -> world, `half`) reaches into the emitter cube of half extent
/// `cube_half` around the anchor, the volume the sim respawns out of: the
/// face's point nearest the anchor is inside it. A box that does not gets
/// no emitter weight ([`emitter_weights`]).
pub fn reaches(center: Vec3, rot: Quat, half: Vec3, cube_half: f32) -> bool {
    TopFace::of(center, rot, half)
        .nearest(Vec3::ZERO)
        .abs()
        .max_element()
        <= cube_half
}

/// One face of a box: its outward normal, the center of the face and its
/// two in-plane axes (unit) with their half extents. [`Face::of`] is the
/// upward face (the emitters'), [`Face::facing`] a plane's face toward a
/// point (the wall spectrum's), [`Face::across`] the face a ray entered,
/// [`acting_face`] the one of those a behavior acts on (the room editor's
/// highlight, the surfaces pass).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Face {
    pub normal: Vec3,
    pub center: Vec3,
    pub axes: [Vec3; 2],
    pub half: [f32; 2],
}

/// The name the upward-face callers use: a [`Face`] from [`Face::of`].
pub type TopFace = Face;

impl Face {
    /// Of `center` / `rot` (box -> world) / `half`, the face whose outward
    /// normal is the box axis most aligned with `normal`, signed: the face
    /// a ray entered, from the cast's hit normal (`instruments::Hit`). A
    /// table hit from above gives its top, a wall hit from the room its
    /// room-facing side of the 4 cm slab, a storage volume hit on a side
    /// that side.
    pub fn across(center: Vec3, rot: Quat, half: Vec3, normal: Vec3) -> Self {
        let axes = [rot * Vec3::X, rot * Vec3::Y, rot * Vec3::Z];
        let h = half.to_array();
        // First of the largest |cos|.
        let mut i = 0;
        for k in 1..3 {
            if axes[k].dot(normal).abs() > axes[i].dot(normal).abs() {
                i = k;
            }
        }
        let n = if axes[i].dot(normal) >= 0.0 {
            axes[i]
        } else {
            -axes[i]
        };
        let (j, k) = ((i + 1) % 3, (i + 2) % 3);
        Self {
            normal: n,
            center: center + n * h[i],
            axes: [axes[j], axes[k]],
            half: [h[j], h[k]],
        }
    }

    /// The face as a quad `lift_m` off it along its normal, corners in
    /// order around it: `-a-b`, `+a-b`, `+a+b`, `-a+b` for the two axes
    /// times their half extents.
    pub fn corners(&self, lift_m: f32) -> [Vec3; 4] {
        let c = self.center + self.normal * lift_m;
        let (a, b) = (self.axes[0] * self.half[0], self.axes[1] * self.half[1]);
        [c - a - b, c + a - b, c + a + b, c - a + b]
    }

    /// Of `center` / `rot` (box -> world) / `half`, the face whose outward
    /// normal points most nearly up.
    pub fn of(center: Vec3, rot: Quat, half: Vec3) -> Self {
        let axes = [rot * Vec3::X, rot * Vec3::Y, rot * Vec3::Z];
        let h = half.to_array();
        // First of the largest |y|, as the shader's comparisons pick it.
        let mut i = 0;
        for k in 1..3 {
            if axes[k].y.abs() > axes[i].y.abs() {
                i = k;
            }
        }
        let normal = if axes[i].y >= 0.0 { axes[i] } else { -axes[i] };
        let (j, k) = ((i + 1) % 3, (i + 2) % 3);
        Self {
            normal,
            center: center + normal * h[i],
            axes: [axes[j], axes[k]],
            half: [h[j], h[k]],
        }
    }

    /// Of `center` / `rot` / `half`, the face across the box's thinnest
    /// local axis whose outward normal points toward `toward`: a scene
    /// plane's face on the side of `toward` (a wall's room-facing face,
    /// seen from the wearer's head). The same shape as the top face, any
    /// orientation.
    pub fn facing(center: Vec3, rot: Quat, half: Vec3, toward: Vec3) -> Self {
        let axes = [rot * Vec3::X, rot * Vec3::Y, rot * Vec3::Z];
        let h = half.to_array();
        // Last of the smallest half extent: a plane's local Z on a tie.
        let mut i = 2;
        for k in (0..2).rev() {
            if h[k] < h[i] {
                i = k;
            }
        }
        let normal = if axes[i].dot(toward - center) >= 0.0 {
            axes[i]
        } else {
            -axes[i]
        };
        let (j, k) = ((i + 1) % 3, (i + 2) % 3);
        Self {
            normal,
            center: center + normal * h[i],
            axes: [axes[j], axes[k]],
            half: [h[j], h[k]],
        }
    }

    /// Area of the face (m²).
    pub fn area(&self) -> f32 {
        4.0 * self.half[0] * self.half[1]
    }

    /// The point of the face nearest `p`.
    pub fn nearest(&self, p: Vec3) -> Vec3 {
        let d = p - self.center;
        let u = d.dot(self.axes[0]).clamp(-self.half[0], self.half[0]);
        let v = d.dot(self.axes[1]).clamp(-self.half[1], self.half[1]);
        self.center + self.axes[0] * u + self.axes[1] * v
    }
}

/// Per-kind emitter weights (knobs `debug.fosfora.tableweight` and
/// `floorweight`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceWeights {
    /// Scales every table; the largest emitting table in the volume (the
    /// desk) gets this, the others their top-face area's share of it.
    pub table: f32,
    pub floor: f32,
}

impl Default for SurfaceWeights {
    fn default() -> Self {
        Self {
            table: 1.0,
            floor: 0.5,
        }
    }
}

/// One obstacle box as the weights see it, center relative to the emitter
/// cube's center (the effect anchor).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceBox {
    pub kind: u32,
    /// The box's emitter flag (0 or 1): the synthetic floor's is
    /// [`synthetic_floor_emit`], every room box's 1.
    pub emit: f32,
    /// The behavior it runs ([`lane_behavior`]): only a box whose behavior
    /// emits gets a weight.
    pub behavior: SurfaceBehavior,
    pub center: Vec3,
    pub rot: Quat,
    pub half: Vec3,
}

/// The emitter weight in 0..1 of each box, in order, for the boxes whose
/// behavior emits (embers or sparks; every other box 0): tables by
/// top-face area against the largest such table inside the volume, floors
/// at `weights.floor`, any other kind like a table no larger than that one
/// (`weights.table` when no table in the volume emits). A box whose top
/// face does not reach into the emitter cube (`cube_half` around the
/// origin, the volume the sim respawns out of) gets 0, so a dragged anchor
/// never picks a surface its particles would respawn from at once. The
/// test is on the face's point nearest the anchor, not its center: the
/// synthetic floor is 20 m across and centered on the stage origin, not
/// under the anchor.
///
/// The reference is the largest table that can emit, not the room's
/// largest (step 2c): with the desk out of the volume the tables in reach
/// were slivers of it (a summed weight of 0.99 with the floor's 0.5, Sep
/// 29), and a room whose only emitting table is a side table now gives it
/// 1.0.
pub fn emitter_weights(
    boxes: &[SurfaceBox],
    cube_half: f32,
    weights: SurfaceWeights,
    out: &mut [f32],
) {
    let faces = || {
        boxes.iter().map(|b| {
            let face = TopFace::of(b.center, b.rot, b.half);
            (b, face, reaches(b.center, b.rot, b.half, cube_half))
        })
    };
    let largest_table = faces()
        .filter(|(b, _, inside)| {
            *inside && b.kind == KIND_TABLE && b.emit > 0.0 && b.behavior.emits()
        })
        .map(|(_, f, _)| f.area())
        .fold(0.0f32, f32::max);
    for (w, (b, face, inside)) in out.iter_mut().zip(faces()) {
        let base = match b.kind {
            _ if !b.behavior.emits() => 0.0,
            KIND_TABLE if largest_table > 0.0 => weights.table * face.area() / largest_table,
            KIND_TABLE => 0.0,
            KIND_FLOOR => weights.floor,
            _ if largest_table > 0.0 => weights.table * (face.area() / largest_table).min(1.0),
            _ => weights.table,
        };
        *w = if inside {
            (base * b.emit).clamp(0.0, 1.0)
        } else {
            0.0
        };
    }
}

/// [`emitter_weights`] with box `k` running the behavior row `k` of
/// `lanes` gives it ([`lane_behavior`]): the rows the sim reads, so the
/// weights and the sim's gates agree. The behaviors in `boxes` are
/// overwritten. `ObstacleSet::set_emitter_weights` (Android only) weighs
/// the obstacle block through it, so the desktop tests take the same path.
pub fn lane_emitter_weights(
    boxes: &mut [SurfaceBox],
    lanes: &[[f32; 4]],
    cube_half: f32,
    weights: SurfaceWeights,
    out: &mut [f32],
) {
    for (b, row) in boxes.iter_mut().zip(lanes) {
        b.behavior = lane_behavior(*row, b.kind);
    }
    emitter_weights(boxes, cube_half, weights, out);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A table as the runtime reports one: local +Z up (a -90° turn about X).
    fn table(center: Vec3, half_xy: [f32; 2]) -> SurfaceBox {
        SurfaceBox {
            kind: KIND_TABLE,
            emit: 1.0,
            behavior: SurfaceBehavior::Embers,
            center,
            rot: Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2),
            half: Vec3::new(half_xy[0], half_xy[1], 0.37),
        }
    }

    /// The stage floor, assigned sparks (its default, the rings, weighs
    /// nothing).
    fn synthetic_floor(anchor: Vec3, emit: f32) -> SurfaceBox {
        SurfaceBox {
            kind: KIND_FLOOR,
            emit,
            behavior: SurfaceBehavior::Sparks,
            center: Vec3::new(0.0, -0.05, 0.0) - anchor,
            rot: Quat::IDENTITY,
            half: Vec3::new(10.0, 0.05, 10.0),
        }
    }

    fn weights_of(boxes: &[SurfaceBox], cube_half: f32) -> Vec<f32> {
        let mut out = vec![-1.0; boxes.len()];
        emitter_weights(boxes, cube_half, SurfaceWeights::default(), &mut out);
        out
    }

    #[test]
    fn labels_map_to_kinds() {
        assert_eq!(surface_kind("DESK"), KIND_TABLE);
        assert_eq!(surface_kind("TABLE"), KIND_TABLE);
        assert_eq!(surface_kind("FLOOR"), KIND_FLOOR);
        assert_eq!(surface_kind("WALL_FACE"), KIND_WALL);
        assert_eq!(surface_kind("INVISIBLE_WALL_FACE"), KIND_WALL);
        assert_eq!(surface_kind("CEILING"), KIND_CEILING);
        assert_eq!(surface_kind("WINDOW_FRAME"), KIND_FRAME);
        assert_eq!(surface_kind("DOOR_FRAME"), KIND_FRAME);
        assert_eq!(surface_kind("STORAGE"), KIND_OTHER);
        assert_eq!(surface_kind("COUCH"), KIND_OTHER);
        assert_eq!(surface_kind(""), KIND_NONE);
        // Several labels: the most specific wins, in any order.
        assert_eq!(surface_kind("OTHER,TABLE"), KIND_TABLE);
        assert_eq!(surface_kind("TABLE, DESK"), KIND_TABLE);
        assert_eq!(surface_kind("STORAGE,FLOOR"), KIND_FLOOR);
        assert_eq!(surface_kind("desk"), KIND_TABLE);
    }

    #[test]
    fn the_top_face_points_up_for_every_measured_orientation() {
        // Table (+Z up): top face 0.37 above the center.
        let t = table(Vec3::new(0.0, 0.4, -0.5), [0.6, 0.4]);
        let f = TopFace::of(t.center, t.rot, t.half);
        assert!(f.normal.abs_diff_eq(Vec3::Y, 1e-5), "{:?}", f.normal);
        assert!((f.center.y - 0.77).abs() < 1e-5);
        assert!((f.area() - 0.96).abs() < 1e-5);
        // Synthetic floor (+Y up): top at y = 0.
        let s = synthetic_floor(Vec3::ZERO, 1.0);
        let f = TopFace::of(s.center, s.rot, s.half);
        assert!(f.normal.abs_diff_eq(Vec3::Y, 1e-5));
        assert!(f.center.y.abs() < 1e-5);
        // Ceiling (+Z down): the -Z face is the upward one.
        let rot = Quat::from_rotation_x(std::f32::consts::FRAC_PI_2);
        assert!((rot * Vec3::Z).abs_diff_eq(Vec3::NEG_Y, 1e-5));
        let f = TopFace::of(Vec3::new(0.0, 2.5, 0.0), rot, Vec3::new(2.0, 2.0, 0.02));
        assert!(f.normal.abs_diff_eq(Vec3::Y, 1e-5));
        assert!((f.center.y - 2.52).abs() < 1e-5);
    }

    #[test]
    fn hidden_walls_are_told_apart() {
        assert!(is_hidden_wall("INVISIBLE_WALL_FACE"));
        assert!(is_hidden_wall("OTHER, invisible_wall_face"));
        assert!(!is_hidden_wall("WALL_FACE"));
        assert!(!is_hidden_wall("WALL_ART"));
        assert!(!is_hidden_wall(""));
    }

    #[test]
    fn the_facing_face_is_the_thin_side_toward_the_viewer() {
        // A wall plane 4 m wide, 2.5 m tall, 4 cm thick, its local +Z
        // pointing +X (into the room from a wall at x = -2).
        let rot = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        let center = Vec3::new(-2.0, 1.25, 0.0);
        let half = Vec3::new(2.0, 1.25, 0.02);
        let f = TopFace::facing(center, rot, half, Vec3::new(0.0, 1.2, 0.5));
        assert!(f.normal.abs_diff_eq(Vec3::X, 1e-5), "{:?}", f.normal);
        assert!(f.center.abs_diff_eq(Vec3::new(-1.98, 1.25, 0.0), 1e-5));
        assert!((f.area() - 10.0).abs() < 1e-4);
        // Seen from behind, the other face.
        let b = TopFace::facing(center, rot, half, Vec3::new(-3.0, 1.2, 0.0));
        assert!(b.normal.abs_diff_eq(Vec3::NEG_X, 1e-5));
        assert!(b.center.abs_diff_eq(Vec3::new(-2.02, 1.25, 0.0), 1e-5));
    }

    #[test]
    fn the_face_across_a_hit_normal_is_the_face_the_ray_entered() {
        use crate::instruments::{RayBox, cast};
        let cast_one = |b: RayBox, origin: Vec3, to: Vec3| {
            let hit = cast(origin, (to - origin).normalize(), &[b], 8.0).expect("a hit");
            Face::across(b.center, b.rot, b.half, hit.normal)
        };
        // A table (local +Z up) seen from a seated head above and in front.
        let t = table(Vec3::new(0.0, 0.4, -0.8), [0.6, 0.4]);
        let rb = RayBox {
            center: t.center,
            rot: t.rot,
            half: t.half,
            kind: KIND_TABLE,
        };
        let f = cast_one(rb, Vec3::new(0.0, 1.2, 0.0), Vec3::new(0.1, 0.77, -0.7));
        assert_eq!(f, TopFace::of(t.center, t.rot, t.half));
        assert!((f.center.y - 0.77).abs() < 1e-5);
        // A wall plane at x = -2 (local +Z into the room), 4 cm thick,
        // hit from the room at an angle: its room-facing side.
        let wall = RayBox {
            center: Vec3::new(-2.0, 1.25, 0.0),
            rot: Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
            half: Vec3::new(2.0, 1.25, 0.02),
            kind: KIND_WALL,
        };
        let f = cast_one(wall, Vec3::new(0.0, 1.2, 0.0), Vec3::new(-2.0, 1.5, -1.2));
        assert!(f.normal.abs_diff_eq(Vec3::X, 1e-5), "{:?}", f.normal);
        assert!(f.center.abs_diff_eq(Vec3::new(-1.98, 1.25, 0.0), 1e-5));
        assert!((f.area() - 10.0).abs() < 1e-4);
        assert_eq!(
            f,
            TopFace::facing(wall.center, wall.rot, wall.half, Vec3::ZERO)
        );
        // A storage volume (0.4 x 0.9 x 0.3 half, +Z up) hit on its +X
        // side from the right.
        let storage = RayBox {
            center: Vec3::new(1.0, 0.45, -1.0),
            rot: Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2),
            half: Vec3::new(0.4, 0.3, 0.45),
            kind: KIND_OTHER,
        };
        let f = cast_one(storage, Vec3::new(2.5, 0.5, -1.0), storage.center);
        assert!(f.normal.abs_diff_eq(Vec3::X, 1e-5), "{:?}", f.normal);
        assert!(f.center.abs_diff_eq(Vec3::new(1.4, 0.45, -1.0), 1e-5));
        let mut half = f.half;
        half.sort_by(f32::total_cmp);
        assert_close!(half, [0.3, 0.45]);
        // A normal off the axes still picks the nearest axis, signed.
        let f = Face::across(
            storage.center,
            storage.rot,
            storage.half,
            Vec3::new(-0.2, 0.9, 0.1),
        );
        assert!(f.normal.abs_diff_eq(Vec3::Y, 1e-5));
    }

    #[test]
    fn the_acting_face_is_a_volumes_top_and_a_planes_side_toward_the_head() {
        use crate::instruments::{RayBox, cast};
        // Step 2h: whichever face the ray entered, the tinted one is the
        // face the behavior acts on.
        let head = Vec3::new(0.0, 1.2, 0.0);
        let acting = |b: RayBox, to: Vec3| {
            let hit = cast(head, (to - head).normalize(), &[b], 8.0).expect("a hit");
            (
                Face::across(b.center, b.rot, b.half, hit.normal),
                acting_face(b.center, b.rot, b.half, head),
            )
        };
        // A table (local +Z up) hit on its near side from the chair: the
        // ray entered the side, the top is tinted.
        let t = table(Vec3::new(0.0, 0.4, -0.8), [0.6, 0.4]);
        let rb = RayBox {
            center: t.center,
            rot: t.rot,
            half: t.half,
            kind: KIND_TABLE,
        };
        assert!(!is_plane(rb.half));
        let (entered, f) = acting(rb, Vec3::new(0.0, 0.3, -0.4));
        assert!(entered.normal.abs_diff_eq(Vec3::Z, 1e-5), "{entered:?}");
        assert_eq!(f, TopFace::of(t.center, t.rot, t.half));
        assert!(f.normal.abs_diff_eq(Vec3::Y, 1e-5));
        assert!((f.center.y - 0.77).abs() < 1e-5);
        // A wall plane at x = -2 (local +Z into the room): the room-facing
        // side of the slab, not its top edge or its back.
        let wall = RayBox {
            center: Vec3::new(-2.0, 1.25, 0.0),
            rot: Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
            half: Vec3::new(2.0, 1.25, PLANE_HALF_THICKNESS_M),
            kind: KIND_WALL,
        };
        assert!(is_plane(wall.half));
        let (_, f) = acting(wall, Vec3::new(-2.0, 1.5, -1.2));
        assert!(f.normal.abs_diff_eq(Vec3::X, 1e-5), "{:?}", f.normal);
        assert!(f.center.abs_diff_eq(Vec3::new(-1.98, 1.25, 0.0), 1e-5));
        // A scene floor plane (local +Z up) and the stage floor's 10 cm
        // slab (a volume, +Y up): their tops.
        let floor = RayBox {
            center: Vec3::ZERO,
            rot: Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2),
            half: Vec3::new(2.0, 1.5, PLANE_HALF_THICKNESS_M),
            kind: KIND_FLOOR,
        };
        let (_, f) = acting(floor, Vec3::new(0.5, 0.0, -1.0));
        assert!(f.normal.abs_diff_eq(Vec3::Y, 1e-5), "{:?}", f.normal);
        assert!((f.center.y - PLANE_HALF_THICKNESS_M).abs() < 1e-5);
        let s = synthetic_floor(Vec3::ZERO, 1.0);
        assert!(!is_plane(s.half));
        let f = acting_face(s.center, s.rot, s.half, head);
        assert!(f.normal.abs_diff_eq(Vec3::Y, 1e-5));
        assert!(f.center.y.abs() < 1e-5);
        // The ceiling plane (local +Z down) from below: its underside,
        // the side the wearer sees.
        let rot = Quat::from_rotation_x(std::f32::consts::FRAC_PI_2);
        let half = Vec3::new(2.0, 2.0, PLANE_HALF_THICKNESS_M);
        let f = acting_face(Vec3::new(0.0, 2.5, 0.0), rot, half, head);
        assert!(f.normal.abs_diff_eq(Vec3::NEG_Y, 1e-5), "{:?}", f.normal);
    }

    #[test]
    fn a_top_face_reaches_the_cube_by_its_point_nearest_the_anchor() {
        let t = table(Vec3::new(1.9, -0.5, 0.0), [0.4, 0.4]);
        // The top spans x 1.5..2.3: in at 1.5, out below.
        assert!(reaches(t.center, t.rot, t.half, 1.5));
        assert!(!reaches(t.center, t.rot, t.half, 1.49));
        // The stage floor under an anchor 5 m from its center.
        let s = synthetic_floor(Vec3::new(5.0, 1.0, 3.0), 1.0);
        assert!(reaches(s.center, s.rot, s.half, 1.0));
        assert!(!reaches(s.center, s.rot, s.half, 0.9));
    }

    #[test]
    fn the_corners_go_round_the_face_lifted_off_it() {
        let f = Face {
            normal: Vec3::Y,
            center: Vec3::new(1.0, 0.75, -1.0),
            axes: [Vec3::X, Vec3::NEG_Z],
            half: [0.6, 0.4],
        };
        let c = f.corners(0.01);
        assert!(c.iter().all(|p| (p.y - 0.76).abs() < 1e-6), "{c:?}");
        assert!(c[0].abs_diff_eq(Vec3::new(0.4, 0.76, -0.6), 1e-6));
        assert!(c[2].abs_diff_eq(Vec3::new(1.6, 0.76, -1.4), 1e-6));
        // Around, not across: each side is an axis.
        assert!(((c[1] - c[0]).length() - 1.2).abs() < 1e-5);
        assert!(((c[2] - c[1]).length() - 0.8).abs() < 1e-5);
        assert!(((c[3] - c[2]).length() - 1.2).abs() < 1e-5);
    }

    #[test]
    fn the_editor_names_surfaces_in_words_and_tells_twins_apart() {
        use crate::lanes::LaneBox;
        let b = |n: u8, label: &'static str| LaneBox {
            uuid: [n; 16],
            kind: surface_kind(label),
            label,
        };
        let stage = LaneBox {
            uuid: crate::room_file::STAGE_FLOOR_UUID,
            kind: KIND_FLOOR,
            label: "",
        };
        let singles = [
            ("DESK", "desk"),
            ("TABLE", "table"),
            ("WALL_FACE", "wall"),
            ("CEILING", "ceiling"),
            ("WINDOW_FRAME", "window"),
            ("DOOR_FRAME", "door"),
            ("STORAGE", "storage"),
            ("COUCH", "couch"),
            ("WALL_ART", "wall art"),
            ("Lamp", "lamp"),
        ];
        let boxes: Vec<_> = singles
            .iter()
            .enumerate()
            .map(|(i, (l, _))| b(i as u8 + 1, l))
            .chain([stage])
            .collect();
        for (k, (_, name)) in singles.iter().enumerate() {
            assert_eq!(friendly_name(k, &boxes), *name, "{}", singles[k].0);
        }
        // The stage floor alone is "floor".
        assert_eq!(friendly_name(singles.len(), &boxes), "floor");
        assert_eq!(friendly_name(99, &boxes), "");
        // Two tables, four walls and a scene floor next to the stage
        // floor: each carries its index.
        let room = [
            b(1, "TABLE"),
            b(2, "WALL_FACE"),
            b(3, "TABLE"),
            b(4, "DESK"),
            b(5, "WALL_FACE"),
            b(6, "FLOOR"),
            b(7, "OTHER,TABLE"),
            stage,
        ];
        let names: Vec<_> = (0..room.len()).map(|k| friendly_name(k, &room)).collect();
        assert_eq!(
            names,
            [
                "table 0", "wall 1", "table 2", "desk", "wall 4", "floor 5", "table 6", "floor 7"
            ]
        );
    }

    #[test]
    fn the_largest_table_in_the_volume_weighs_one_and_the_others_by_area() {
        let boxes = [
            table(Vec3::new(0.0, -0.5, -0.5), [0.8, 0.4]),
            table(Vec3::new(0.8, -0.5, 0.3), [0.4, 0.4]),
            synthetic_floor(Vec3::new(0.0, 1.0, 0.0), 1.0),
            // A dining table four times the desk's top, beyond the cube
            // (x 3.0..5.0): no weight, and not the reference.
            table(Vec3::new(4.0, -0.5, 0.0), [1.0, 1.28]),
        ];
        let w = weights_of(&boxes, 1.5);
        assert!((w[0] - 1.0).abs() < 1e-6, "{w:?}");
        assert!((w[1] - 0.5).abs() < 1e-6, "{w:?}");
        assert!((w[2] - 0.5).abs() < 1e-6, "the floor on sparks {w:?}");
        assert_close!(w[3], 0.0);
        let boxes = &boxes[..3];
        let mut out = [0.0; 3];
        emitter_weights(
            boxes,
            1.5,
            SurfaceWeights {
                table: 0.5,
                floor: 1.0,
            },
            &mut out,
        );
        assert!((out[0] - 0.5).abs() < 1e-6 && (out[1] - 0.25).abs() < 1e-6);
        assert!((out[2] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn a_surface_outside_the_emitter_cube_weighs_nothing() {
        // The desk entirely beyond a 1.5 m cube (x 1.7..3.3), a table
        // inside it.
        let boxes = [
            table(Vec3::new(2.5, -0.5, 0.0), [0.8, 0.4]),
            table(Vec3::new(0.3, -0.5, 0.0), [0.4, 0.4]),
        ];
        let w = weights_of(&boxes, 1.5);
        assert_close!(w[0], 0.0);
        // Weighed against the tables that can emit, not the desk out of
        // reach (step 2c): the largest in the volume weighs 1.
        assert!((w[1] - 1.0).abs() < 1e-6, "{w:?}");
        // A floor 2 m below the anchor (the top face out of reach in y).
        let w = weights_of(&[synthetic_floor(Vec3::new(0.0, 2.0, 0.0), 1.0)], 1.5);
        assert_close!(w[0], 0.0);
        // The anchor dragged 5 m across the room: the floor under it still
        // counts, though its center (the stage origin) is out of the cube.
        let w = weights_of(&[synthetic_floor(Vec3::new(5.0, 1.0, 3.0), 1.0)], 1.5);
        assert!((w[0] - 0.5).abs() < 1e-6, "{w:?}");
    }

    #[test]
    fn with_the_largest_table_out_of_the_volume_a_small_table_inside_weighs_one() {
        use SurfaceBehavior as B;
        // Step 2c, the unworn case of Sep 29: the desk beyond a 1.5 m cube
        // (x 1.7..3.3), a side table half its top inside, a shelf half the
        // side table's, the floor.
        let desk = table(Vec3::new(2.5, -0.5, 0.0), [0.8, 0.4]);
        let side = table(Vec3::new(0.3, -0.5, 0.0), [0.4, 0.4]);
        let shelf = SurfaceBox {
            kind: KIND_OTHER,
            ..table(Vec3::new(-0.6, -0.2, 0.3), [0.4, 0.2])
        };
        let floor = synthetic_floor(Vec3::new(0.0, 1.0, 0.0), 1.0);
        let w = weights_of(&[desk, side, shelf, floor], 1.5);
        // The side table is the reference; the shelf its share of it; the
        // floor its own weight.
        assert_close!(w, [0.0, 1.0, 0.5, 0.5]);
        // The room's only emitting table: 1, at any size.
        let tiny = table(Vec3::new(0.3, -0.5, 0.0), [0.1, 0.1]);
        assert_close!(weights_of(&[desk, tiny], 1.5), [0.0, 1.0]);
        // A larger table inside that does not emit is not the reference.
        let dark = SurfaceBox {
            behavior: B::None,
            ..table(Vec3::new(-0.3, -0.5, -0.5), [0.8, 0.4])
        };
        assert_close!(weights_of(&[dark, side], 1.5), [0.0, 1.0]);
        // The space grown to take the desk in: the desk is the reference
        // again and the side table half of it.
        assert_close!(weights_of(&[desk, side], 2.5), [1.0, 0.5]);
    }

    #[test]
    fn the_space_size_decides_which_surfaces_emit() {
        // Board #3325: a table whose top face starts 2 m from the anchor
        // (x 2.0..2.8), and a desk inside any size.
        let boxes = [
            table(Vec3::new(2.4, -0.5, 0.0), [0.4, 0.4]),
            table(Vec3::new(0.3, -0.5, 0.0), [0.8, 0.4]),
        ];
        let small = weights_of(&boxes, 1.5);
        assert_close!(small[0], 0.0);
        let large = weights_of(&boxes, 2.5);
        assert!(large[0] > 0.0, "{large:?}");
        assert!(
            (large[0] - 0.5).abs() < 1e-6,
            "by area against the desk {large:?}"
        );
        // The desk emits at both, and at the smallest step.
        for half in [0.75, 1.5, 2.5] {
            assert!((weights_of(&boxes, half)[1] - 1.0).abs() < 1e-6, "{half}");
        }
    }

    #[test]
    fn the_synthetic_floor_emits_only_without_a_scene_floor() {
        assert_close!(synthetic_floor_emit([KIND_TABLE, KIND_WALL]), 1.0);
        assert_close!(synthetic_floor_emit([]), 1.0);
        assert_close!(synthetic_floor_emit([KIND_TABLE, KIND_FLOOR]), 0.0);
        let w = weights_of(&[synthetic_floor(Vec3::new(0.0, 1.0, 0.0), 0.0)], 1.5);
        assert_close!(
            w[0],
            0.0,
            "a flagged-off floor must not double the scene floor"
        );
    }

    #[test]
    fn other_kinds_and_unflagged_boxes_weigh_nothing() {
        let mut wall = table(Vec3::ZERO, [1.0, 1.0]);
        wall.kind = KIND_WALL;
        wall.behavior = SurfaceBehavior::default_for(KIND_WALL);
        let mut off = table(Vec3::ZERO, [1.0, 1.0]);
        off.emit = 0.0;
        let w = weights_of(&[wall, off], 1.5);
        assert_eq!(w, vec![0.0, 0.0]);
    }

    #[test]
    fn the_catalogue_names_and_numbers_the_eight_behaviors() {
        use SurfaceBehavior as B;
        for (id, b) in B::ALL.into_iter().enumerate() {
            assert_eq!(b.id() as usize, id);
            assert_eq!(B::from_id(b.id()), Some(b));
            assert_eq!(B::from_name(b.name()), Some(b));
        }
        let names: Vec<_> = B::ALL.into_iter().map(B::name).collect();
        assert_eq!(
            names,
            [
                "none",
                "embers",
                "sparks",
                "spectrum",
                "rings",
                "streamlines",
                "curls",
                "pulse"
            ]
        );
        assert_eq!((B::Rings.id(), B::Streamlines.id()), (4, 5));
        assert_eq!((B::Curls.id(), B::Pulse.id()), (6, 7));
        assert_eq!(B::from_name(" Embers "), Some(B::Embers));
        // The ripple's old name reads as the rings; the name is the new one.
        assert_eq!(B::from_name("ripple"), Some(B::Rings));
        assert_eq!(B::from_name(" RIPPLE "), Some(B::Rings));
        assert_eq!(B::Rings.name(), "rings");
        // D2 made 6 and 7 real: nothing is reserved, 8 on is unknown.
        assert!(RESERVED_BEHAVIORS.is_empty());
        assert_eq!(B::from_name("Curls"), Some(B::Curls));
        assert_eq!(B::from_name(" PULSE "), Some(B::Pulse));
        for id in 8..16 {
            assert_eq!(B::from_id(id), None);
        }
        let emitting: Vec<_> = B::ALL.into_iter().filter(|b| b.emits()).collect();
        assert_eq!(emitting, vec![B::Embers, B::Sparks]);
        let drawn: Vec<_> = B::ALL.into_iter().filter(|b| b.draws_on_face()).collect();
        assert_eq!(drawn, vec![B::Rings, B::Streamlines, B::Curls, B::Pulse]);
        // The rings and the pulse hit with the bass; the flows follow the
        // whole mix.
        let bass: Vec<_> = B::ALL
            .into_iter()
            .filter(|b| b.default_band() == crate::surface_fx::BAND_BASS)
            .collect();
        assert_eq!(bass, vec![B::Rings, B::Pulse]);
        for b in B::ALL {
            assert!(b.default_band() < crate::surface_fx::BANDS);
        }
    }

    #[test]
    fn the_palette_has_a_color_per_kind() {
        assert_eq!(PALETTE.len(), 6);
        // One entry per named kind, in their order.
        let kinds: Vec<u32> = PALETTE.iter().map(|(k, _)| *k).collect();
        let named: Vec<u32> = KIND_NAMES.iter().map(|(k, _)| *k).collect();
        assert_eq!(kinds, named);
        for (kind, c) in PALETTE {
            assert_close!(palette(kind), c);
            assert!(c.iter().all(|v| (0.0..=1.0).contains(v)), "{c:?}");
            assert!(
                c.iter().copied().fold(0.0, f32::max) > 0.99,
                "bright: {c:?}"
            );
        }
        // Tables blue, floors violet, walls the canvas's warm white, other
        // green, frames and ceilings amber.
        let [r, g, b] = palette(KIND_TABLE);
        assert!(b > g && g > r, "blue");
        let [r, g, b] = palette(KIND_FLOOR);
        assert!(b > r && r > g, "violet");
        assert_close!(palette(KIND_WALL), crate::canvas::COLOR);
        let [r, g, b] = palette(KIND_OTHER);
        assert!(g > r && g > b, "green");
        assert_close!(palette(KIND_CEILING), palette(KIND_FRAME));
        let [r, g, b] = palette(KIND_FRAME);
        assert!(r > g && g > b, "amber");
        // Unlabeled or unknown: other's.
        assert_close!(palette(KIND_NONE), palette(KIND_OTHER));
        assert_close!(palette(99), palette(KIND_OTHER));
    }

    #[test]
    fn each_kind_cycles_through_what_renders_on_it_and_wraps() {
        use SurfaceBehavior as B;
        // Every step of each kind's order, from none round to none.
        let other: &[B] = &[B::None, B::Curls, B::Streamlines, B::Pulse, B::Embers];
        let orders: [(u32, &[B]); 7] = [
            (
                KIND_TABLE,
                &[
                    B::None,
                    B::Streamlines,
                    B::Curls,
                    B::Pulse,
                    B::Embers,
                    B::Sparks,
                ],
            ),
            (KIND_OTHER, other),
            (KIND_NONE, other),
            (
                KIND_FLOOR,
                &[B::None, B::Rings, B::Streamlines, B::Curls, B::Sparks],
            ),
            (
                KIND_WALL,
                &[B::None, B::Spectrum, B::Streamlines, B::Rings, B::Pulse],
            ),
            (KIND_CEILING, &[B::None, B::Rings, B::Pulse, B::Streamlines]),
            (KIND_FRAME, &[B::None, B::Pulse, B::Rings, B::Streamlines]),
        ];
        for (kind, order) in orders {
            assert_eq!(B::catalogue(kind), order, "kind {kind}");
            let mut b = B::None;
            let mut seen = Vec::new();
            for _ in 0..order.len() {
                b = b.next_for(kind);
                seen.push(b);
            }
            let mut expected = order[1..].to_vec();
            expected.push(B::None);
            assert_eq!(seen, expected, "kind {kind}");
            // Each behavior once.
            for (i, x) in order.iter().enumerate() {
                assert!(!order[i + 1..].contains(x), "kind {kind}: {x:?} twice");
            }
        }
        // One tap from each default goes to the next look of its kind.
        assert_eq!(B::Streamlines.next_for(KIND_TABLE), B::Curls);
        assert_eq!(B::Spectrum.next_for(KIND_WALL), B::Streamlines);
        assert_eq!(B::Pulse.next_for(KIND_FRAME), B::Rings);
        assert_eq!(B::Curls.next_for(KIND_OTHER), B::Streamlines);
        let mut floor = vec![B::default_for(KIND_FLOOR)];
        for _ in 0..5 {
            floor.push(floor.last().unwrap().next_for(KIND_FLOOR));
        }
        assert_eq!(
            floor,
            [
                B::Rings,
                B::Streamlines,
                B::Curls,
                B::Sparks,
                B::None,
                B::Rings
            ]
        );
    }

    #[test]
    fn every_default_is_the_first_entry_after_none_in_its_kinds_cycle() {
        use SurfaceBehavior as B;
        // A fresh room is the mockup and one tap goes to the next look.
        // Ceilings and unlabeled anchors default to none (the brief's
        // defaults), so theirs is the cycle's start: a tap lights them.
        for kind in [
            KIND_TABLE,
            KIND_FLOOR,
            KIND_WALL,
            KIND_FRAME,
            KIND_OTHER,
            KIND_CEILING,
            KIND_NONE,
        ] {
            let order = B::catalogue(kind);
            assert_eq!(order[0], B::None, "kind {kind}");
            match B::default_for(kind) {
                B::None => {
                    assert!(matches!(kind, KIND_CEILING | KIND_NONE), "kind {kind}");
                    assert_eq!(B::None.next_for(kind), order[1]);
                }
                d => assert_eq!(order[1], d, "kind {kind}"),
            }
        }
    }

    #[test]
    fn off_the_catalogue_a_cycle_starts_after_none() {
        use SurfaceBehavior as B;
        // The knob put the spectrum on a table, the rings on a couch,
        // embers on a wall or a floor, sparks on a ceiling.
        assert_eq!(B::Spectrum.next_for(KIND_TABLE), B::Streamlines);
        assert_eq!(B::Rings.next_for(KIND_OTHER), B::Curls);
        assert_eq!(B::Embers.next_for(KIND_WALL), B::Spectrum);
        assert_eq!(B::Embers.next_for(KIND_FLOOR), B::Rings);
        assert_eq!(B::Sparks.next_for(KIND_CEILING), B::Rings);
        // A ceiling on none goes to the rings, a frame to the pulse.
        assert_eq!(B::None.next_for(KIND_CEILING), B::Rings);
        assert_eq!(B::None.next_for(KIND_FRAME), B::Pulse);
    }

    #[test]
    fn the_kind_defaults_are_the_mockups_room() {
        use SurfaceBehavior as B;
        // Board #3488: streamlines on the table, rings on the floor (step
        // 2d, decision #3459), the spectrum on the walls, the pulse on the
        // frames, curls on everything else labeled.
        assert_eq!(B::default_for(KIND_TABLE), B::Streamlines);
        assert_eq!(B::default_for(KIND_FLOOR), B::Rings);
        assert_eq!(B::default_for(KIND_WALL), B::Spectrum);
        assert_eq!(B::default_for(KIND_FRAME), B::Pulse);
        assert_eq!(B::default_for(KIND_OTHER), B::Curls);
        for kind in [KIND_NONE, KIND_CEILING, 99] {
            assert_eq!(B::default_for(kind), B::None, "kind {kind}");
        }
        // Nothing emits by default: the cloud comes back per surface.
        for kind in 0..8 {
            assert!(!B::default_for(kind).emits(), "kind {kind}");
        }
        for (kind, name) in KIND_NAMES {
            assert_eq!(kind_name(kind), name);
            assert_eq!(kind_from_name(name), Some(kind));
        }
        assert_eq!(kind_from_name("WALL"), Some(KIND_WALL));
        assert_eq!(kind_name(KIND_NONE), "none");
        assert_eq!(kind_from_name("none"), None);
    }

    #[test]
    fn a_lane_carries_its_behavior_and_a_zero_lane_the_kind_default() {
        use SurfaceBehavior as B;
        // Unset: the kind's default, whatever the kind.
        for kind in [KIND_TABLE, KIND_FLOOR, KIND_WALL, KIND_OTHER] {
            assert_eq!(lane_behavior([0.0; 4], kind), B::default_for(kind));
        }
        for b in B::ALL {
            let row = lane_row(b, 0.5, [0.0, 0.0]);
            assert_close!(row, [(b.id() + 1) as f32, 0.5, 0.0, 0.0]);
            for kind in [KIND_TABLE, KIND_FLOOR, KIND_WALL] {
                assert_eq!(lane_behavior(row, kind), b);
            }
        }
        // `none` set on a table is not unset.
        assert_eq!(
            lane_behavior(lane_row(B::None, 1.0, [0.0; 2]), KIND_TABLE),
            B::None
        );
        // The strength is clamped; a reserved id runs nothing.
        assert_close!(lane_row(B::Embers, 3.0, [0.0; 2])[1], 1.0);
        assert_eq!(lane_behavior([7.0, 1.0, 0.0, 0.0], KIND_TABLE), B::Curls);
        assert_eq!(lane_behavior([8.0, 1.0, 0.0, 0.0], KIND_TABLE), B::Pulse);
        assert_eq!(lane_behavior([9.0, 1.0, 0.0, 0.0], KIND_TABLE), B::None);
    }

    #[test]
    fn only_surfaces_whose_behavior_emits_weigh_anything() {
        use SurfaceBehavior as B;
        // The desk 1.6 x 0.8, a side table 0.8 x 0.8, a floor.
        let desk = table(Vec3::new(0.0, -0.5, -0.5), [0.8, 0.4]);
        let side = table(Vec3::new(0.8, -0.5, 0.3), [0.4, 0.4]);
        let floor = synthetic_floor(Vec3::new(0.0, 1.0, 0.0), 1.0);
        // The desk off: the side table is the largest emitting table now.
        let w = weights_of(
            &[
                SurfaceBox {
                    behavior: B::None,
                    ..desk
                },
                side,
                floor,
            ],
            1.5,
        );
        assert_close!(w, [0.0, 1.0, 0.5]);
        // The floor on the rings, the streamlines or none: its sparks are
        // off.
        for b in [B::Rings, B::Streamlines, B::None, B::Spectrum] {
            let w = weights_of(
                &[
                    desk,
                    side,
                    SurfaceBox {
                        behavior: b,
                        ..floor
                    },
                ],
                1.5,
            );
            assert_close!(w, [1.0, 0.5, 0.0]);
        }
        // A table on sparks keeps the table rule; a floor on embers the
        // floor's.
        let w = weights_of(
            &[
                SurfaceBox {
                    behavior: B::Sparks,
                    ..desk
                },
                SurfaceBox {
                    behavior: B::Embers,
                    ..floor
                },
            ],
            1.5,
        );
        assert_close!(w, [1.0, 0.5]);
    }

    #[test]
    fn an_unset_floor_weighs_nothing_and_one_on_sparks_its_weight() {
        // Step 2d: a floor with no assignment runs the rings, which spawn
        // nothing; assigned sparks it weighs `weights.floor`, the stage
        // floor as a scene floor.
        let desk = table(Vec3::new(0.0, -0.5, -0.5), [0.8, 0.4]);
        let stage = synthetic_floor(Vec3::new(0.0, 1.0, 0.0), 1.0);
        let scene = SurfaceBox {
            rot: Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2),
            half: Vec3::new(2.0, 1.5, 0.02),
            center: Vec3::new(0.0, -1.02, 0.0),
            ..stage
        };
        for floor in [stage, scene] {
            let unset = SurfaceBox {
                behavior: lane_behavior([0.0; 4], KIND_FLOOR),
                ..floor
            };
            assert_eq!(unset.behavior, SurfaceBehavior::Rings);
            assert_close!(weights_of(&[desk, unset], 1.5), [1.0, 0.0]);
            let sparks = SurfaceBox {
                behavior: lane_behavior(
                    lane_row(SurfaceBehavior::Sparks, 1.0, [0.0; 2]),
                    KIND_FLOOR,
                ),
                ..floor
            };
            let mut out = [0.0; 2];
            let weights = SurfaceWeights {
                table: 1.0,
                floor: 0.35,
            };
            emitter_weights(&[desk, sparks], 1.5, weights, &mut out);
            assert_close!(out, [1.0, weights.floor]);
        }
    }

    #[test]
    fn another_kind_that_emits_weighs_like_a_table_no_larger_than_the_desk() {
        use SurfaceBehavior as B;
        let desk = table(Vec3::new(0.0, -0.5, -0.5), [0.8, 0.4]);
        let shelf = |half: [f32; 2], behavior| SurfaceBox {
            kind: KIND_OTHER,
            behavior,
            ..table(Vec3::new(-0.8, -0.2, 0.3), half)
        };
        // A shelf a quarter of the desk's top, on embers.
        let w = weights_of(&[desk, shelf([0.4, 0.2], B::Embers)], 1.5);
        assert_close!(w, [1.0, 0.25]);
        // Larger than the desk: capped at the desk's.
        let w = weights_of(&[desk, shelf([1.0, 1.0], B::Sparks)], 1.5);
        assert_close!(w, [1.0, 1.0]);
        // No table emits: the table weight.
        let mut out = [0.0; 1];
        emitter_weights(
            &[shelf([0.4, 0.2], B::Embers)],
            1.5,
            SurfaceWeights {
                table: 0.7,
                floor: 0.5,
            },
            &mut out,
        );
        assert_close!(out[0], 0.7);
        // A wall on embers, out of the volume: nothing.
        let wall = SurfaceBox {
            kind: KIND_WALL,
            behavior: B::Embers,
            ..table(Vec3::new(3.0, 0.0, 0.0), [0.4, 0.4])
        };
        assert_close!(weights_of(&[desk, wall], 1.5)[1], 0.0);
        // Its default (the spectrum): nothing either.
        let wall = SurfaceBox {
            kind: KIND_WALL,
            behavior: B::default_for(KIND_WALL),
            ..table(Vec3::new(0.3, 0.0, 0.0), [0.4, 0.4])
        };
        assert_close!(weights_of(&[desk, wall], 1.5)[1], 0.0);
    }
}
