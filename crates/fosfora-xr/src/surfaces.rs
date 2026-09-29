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
//! after the pour row ([`lane_row`]). A zero row is unset and the box runs
//! its kind's default ([`SurfaceBehavior::default_for`]), which is what
//! every box ran before the lanes, so an upload without them (the core's
//! tests, a desktop run) changes nothing.

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

/// What a surface does (board #3326): the value of its lane. The first
/// pass ships the five behaviors that exist as code; ids 5 to 7 (drips,
/// dust, pool: [`RESERVED_BEHAVIORS`]) are kept for the second.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SurfaceBehavior {
    /// Nothing: the surface is an obstacle only.
    #[default]
    None,
    /// Embers shed off the top face on the beat (Flux XR Room; a table's
    /// default).
    Embers,
    /// Sparks up off the top face with the bass (Flux XR Room; a floor's
    /// default).
    Sparks,
    /// The wall spectrum (`canvas.rs`; a wall's default).
    Spectrum,
    /// The floor ripple (`ripple.rs`): assigning it to a floor pins the
    /// ripple there.
    Ripple,
}

/// Behavior names the catalogue reserves for the second pass: the knob
/// and the room file reject them instead of reading them as unknown.
pub const RESERVED_BEHAVIORS: [&str; 3] = ["drips", "dust", "pool"];

impl SurfaceBehavior {
    /// Every behavior, in id order.
    pub const ALL: [Self; 5] = [
        Self::None,
        Self::Embers,
        Self::Sparks,
        Self::Spectrum,
        Self::Ripple,
    ];

    /// The catalogue id (`XR_BEHAVIOR_*` in `flux_xr_sim.wgsl`).
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
            Self::Ripple => "ripple",
        }
    }

    /// The behavior called `name` (any case).
    pub fn from_name(name: &str) -> Option<Self> {
        let name = name.trim();
        Self::ALL
            .into_iter()
            .find(|b| b.name().eq_ignore_ascii_case(name))
    }

    /// Whether the sim spawns particles on a surface running it.
    pub fn emits(self) -> bool {
        matches!(self, Self::Embers | Self::Sparks)
    }

    /// What a surface of `kind` runs with no assignment: what the fixed
    /// rule per kind gave it before the lanes (tables shed embers, floors
    /// spark, walls carry the spectrum), so the defaults keep today's look.
    pub fn default_for(kind: u32) -> Self {
        match kind {
            KIND_TABLE => Self::Embers,
            KIND_FLOOR => Self::Sparks,
            KIND_WALL => Self::Spectrum,
            _ => Self::None,
        }
    }
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
/// else the kind's default, as the sim reads it (`xr_box_behavior`). A
/// reserved id runs nothing.
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

/// The synthetic stage floor's emitter flag: 1 when the room returned no
/// FLOOR box, else 0, so the floor never emits twice.
pub fn synthetic_floor_emit(room_kinds: impl IntoIterator<Item = u32>) -> f32 {
    if room_kinds.into_iter().any(|k| k == KIND_FLOOR) {
        0.0
    } else {
        1.0
    }
}

/// A box's upward face: its outward normal, the center of the face and its
/// two in-plane axes (unit) with their half extents.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TopFace {
    pub normal: Vec3,
    pub center: Vec3,
    pub axes: [Vec3; 2],
    pub half: [f32; 2],
}

impl TopFace {
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
    /// Scales every table; the largest table (the desk) gets this, the
    /// others their top-face area's share of it.
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
/// top-face area against the largest such table, floors at
/// `weights.floor`, any other kind like a table no larger than the largest
/// one (`weights.table` when no table emits). A box whose top face does
/// not reach into the emitter cube (`cube_half` around the origin, the
/// volume the sim respawns out of) gets 0, so a dragged anchor never picks
/// a surface its particles would
/// respawn from at once. The test is on the face's point nearest the
/// anchor, not its center: the synthetic floor is 20 m across and centered
/// on the stage origin, not under the anchor.
pub fn emitter_weights(
    boxes: &[SurfaceBox],
    cube_half: f32,
    weights: SurfaceWeights,
    out: &mut [f32],
) {
    let faces = || {
        boxes
            .iter()
            .map(|b| (b, TopFace::of(b.center, b.rot, b.half)))
    };
    let largest_table = faces()
        .filter(|(b, _)| b.kind == KIND_TABLE && b.emit > 0.0 && b.behavior.emits())
        .map(|(_, f)| f.area())
        .fold(0.0f32, f32::max);
    for (w, (b, face)) in out.iter_mut().zip(faces()) {
        let base = match b.kind {
            _ if !b.behavior.emits() => 0.0,
            KIND_TABLE if largest_table > 0.0 => weights.table * face.area() / largest_table,
            KIND_TABLE => 0.0,
            KIND_FLOOR => weights.floor,
            _ if largest_table > 0.0 => weights.table * (face.area() / largest_table).min(1.0),
            _ => weights.table,
        };
        let reach = face.nearest(Vec3::ZERO);
        let inside = reach.abs().max_element() <= cube_half;
        *w = if inside {
            (base * b.emit).clamp(0.0, 1.0)
        } else {
            0.0
        };
    }
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
    fn the_largest_table_weighs_one_and_the_others_by_area() {
        let boxes = [
            table(Vec3::new(0.0, -0.5, -0.5), [0.8, 0.4]),
            table(Vec3::new(0.8, -0.5, 0.3), [0.4, 0.4]),
            synthetic_floor(Vec3::new(0.0, 1.0, 0.0), 1.0),
        ];
        let w = weights_of(&boxes, 1.5);
        assert!((w[0] - 1.0).abs() < 1e-6, "{w:?}");
        assert!((w[1] - 0.5).abs() < 1e-6, "{w:?}");
        assert!((w[2] - 0.5).abs() < 1e-6, "floor default {w:?}");
        let mut out = [0.0; 3];
        emitter_weights(
            &boxes,
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
        // Still weighed against the desk, which exists in the room.
        assert!((w[1] - 0.5).abs() < 1e-6, "{w:?}");
        // A floor 2 m below the anchor (the top face out of reach in y).
        let w = weights_of(&[synthetic_floor(Vec3::new(0.0, 2.0, 0.0), 1.0)], 1.5);
        assert_close!(w[0], 0.0);
        // The anchor dragged 5 m across the room: the floor under it still
        // counts, though its center (the stage origin) is out of the cube.
        let w = weights_of(&[synthetic_floor(Vec3::new(5.0, 1.0, 3.0), 1.0)], 1.5);
        assert!((w[0] - 0.5).abs() < 1e-6, "{w:?}");
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
    fn the_catalogue_names_and_numbers_the_five_behaviors() {
        for (id, b) in SurfaceBehavior::ALL.into_iter().enumerate() {
            assert_eq!(b.id() as usize, id);
            assert_eq!(SurfaceBehavior::from_id(b.id()), Some(b));
            assert_eq!(SurfaceBehavior::from_name(b.name()), Some(b));
        }
        assert_eq!(
            SurfaceBehavior::from_name(" Embers "),
            Some(SurfaceBehavior::Embers)
        );
        // 5 to 7 are reserved for the second pass.
        for id in 5..8 {
            assert_eq!(SurfaceBehavior::from_id(id), None);
        }
        for name in RESERVED_BEHAVIORS {
            assert_eq!(SurfaceBehavior::from_name(name), None);
        }
        let emitting: Vec<_> = SurfaceBehavior::ALL
            .into_iter()
            .filter(|b| b.emits())
            .collect();
        assert_eq!(
            emitting,
            vec![SurfaceBehavior::Embers, SurfaceBehavior::Sparks]
        );
    }

    #[test]
    fn the_kind_defaults_are_the_fixed_rule() {
        use SurfaceBehavior as B;
        assert_eq!(B::default_for(KIND_TABLE), B::Embers);
        assert_eq!(B::default_for(KIND_FLOOR), B::Sparks);
        assert_eq!(B::default_for(KIND_WALL), B::Spectrum);
        for kind in [KIND_NONE, KIND_CEILING, KIND_FRAME, KIND_OTHER, 99] {
            assert_eq!(B::default_for(kind), B::None, "kind {kind}");
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
        assert_eq!(lane_behavior([6.0, 1.0, 0.0, 0.0], KIND_TABLE), B::None);
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
        // The floor on ripple (or none): its sparks are off.
        for b in [B::Ripple, B::None, B::Spectrum] {
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
