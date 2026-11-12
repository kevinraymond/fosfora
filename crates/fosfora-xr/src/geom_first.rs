//! Geometry first (board #5416): where the room's boxes explain what the
//! live depth map sees, their exact edges occlude and the map does not;
//! the map keeps everything no box explains.
//!
//! Two occluders write depth in the eye pass: the live depth map
//! (`env_depth.rs`, 320 x 320 stretched about five times over an eye and
//! blended across silhouettes so it does not look blocky) and the room's
//! boxes (`particles3d.rs`, drawn depth-only with exact edges). Where both
//! cover a surface, the map's blended silhouette spills past the box's
//! edge and decides what the wearer sees, so even a scanned table has a
//! soft, wandering edge. With `debug.fosfora.geomfirst 1` the map stops
//! occluding where a box explains its samples.
//!
//! **Explained** ([`explains`]). A map sample's world point `p`, seen from
//! its depth camera at `o`, is explained by a box when the ray from `o`
//! through `p` meets the box, or the box grown sideways by a margin, and
//! `p` lies no more than a tolerance in front of where the ray first meets
//! it: on the surface, behind it, or in the blended fringe just outside its
//! silhouette, whose depth lies between the box and what is behind. The
//! margin is [`MARGIN_TEXELS`] texel widths at `p`'s depth
//! ([`texel_angle`]); the tolerance is [`TOL_M`] plus the sensor's noise at
//! `p`'s range ([`front_tolerance`], the noise model of `live_planes.rs`:
//! 0.5 cm out to 1.6 m, 1.8 cm at 3 m). Where the ray first meets a box is
//! where it enters it, or leaves it from inside, as the box occluder draws
//! it (culling off). A ray that meets the box itself is held to the box's
//! own surface; only a ray that misses it is held to the grown box's, so
//! the margin never widens the tolerance in front of a face.
//!
//! **Which boxes.** Exactly those the eye pass draws as occluders, read
//! from the same uniform the occluder draws from (`Particles3d::
//! box_occluders`): the scanned room's boxes, hidden walls included, the
//! live planes and closed boxes that join them (`live_wire.rs`) and the
//! stage floor; none while `debug.fosfora.occluders` is 0. The hands are
//! never tested: the runtime removes them from the map (`envdepthhands`)
//! and the hand mesh, drawn after, occludes with its own edge.
//!
//! **Where the test runs.** Per vertex of the occluder mesh
//! (`envdepthmesh`), in a compute pass over the eye mesh's vertices of both
//! layers ([`EXPLAIN_WGSL`]), once a frame, recorded in the frame's one
//! submission before the eye passes. Each vertex takes the world point the
//! mesh's vertex stage places it at ([`Explain::value`]). In the vertex
//! stage itself the box loop would run in the binning pass and again for
//! every bin a triangle touches, and per pixel it would run for 3 million
//! pixels an eye; the depth atlas (`depthcollide`) keeps the nearest of a
//! block, not the vertex's own texel. The full-screen pass
//! (`envdepthmesh 0`, kept for the filter A/B) has no vertices to test, so
//! the feature needs the mesh.
//!
//! **What is dropped** ([`KEEP_WGSL`], [`Keep::drops`]). A second small
//! pass marks, per vertex of each target's mesh (the eye pass, and the
//! faces pass at twice the step, whose vertices are every other one of the
//! eye's), the explained vertices to drop, and the vertex stage
//! (`vs_mesh_geom`, [`dropped_clip`]) pushes those past the far plane, so
//! the clipper removes every triangle with a dropped vertex before it is
//! binned (what survives is a sliver along the kept edge, under 0.02 % of
//! it: [`kept_fraction`]). One exception, the keep rule (mode 1): an
//! explained vertex that shares a triangle with an unexplained vertex
//! nearer to the camera is kept where it is. That triangle is the soft edge
//! of something no box explains (a person, a chair, a laptop) against a
//! box behind it, which the map keeps as it is today; without the rule
//! (mode 2) that edge would be cut back to the mesh's grid, the blocky
//! edge the bilinear default exists to avoid. An explained vertex next to
//! an unexplained one behind it (a table's edge against an unscanned
//! background) is dropped either way: that is the spill this removes.
//!
//! **At each `envdepthmesh` step.** The test runs on the eye mesh's
//! vertices: 320, 161, 81 or 41 per side at steps 1, 2, 4 and 8 (102K,
//! 26K, 6.6K and 1.7K per layer). A box's own edge is exact whatever the
//! step. Past it, an unexplained background (something behind a table that
//! no box explains) loses its occlusion for up to one mesh cell beside the
//! box, 1, 2, 4 or 8 texels, plus the margin: what that hides is only what
//! lies behind that background. The keep rule keeps a strip one cell wide
//! of the map's own surface around something unexplained in front of a
//! box.
//!
//! **It only ever shows more.** The boxes draw as before; the map draws
//! less. So nothing is hidden that was not hidden before. Where a box is
//! wrong, what shows is wrong: a box a few centimeters off its object
//! shows a crisp edge in the wrong place, and something real within the
//! tolerance in front of a box or within the margin beside its silhouette
//! (a phone on a table, a cable along a wall) loses its occlusion.
//!
//! **Knobs**, read at launch and polled once a second
//! ([`GeomFirstOptions::from_knobs`]): `debug.fosfora.geomfirst 0|1|2` (off,
//! the default; on; on without the keep rule), `geomfirstmargin` (texel
//! widths, [`MARGIN_TEXELS`]), `geomfirsttol` (m, [`TOL_M`]) and
//! `geomfirstshow 1` (a tint where the map still occludes, [`TINT`]). Off,
//! the eye pass records exactly what it did before: no compute pass, the
//! same pipeline, bind groups and uniform bytes.
//!
//! Plain numbers in, so the rule and its shaders build and test on the
//! desktop: [`explains`] and [`Explain::value`] mirror [`EXPLAIN_WGSL`],
//! [`Keep::drops`] mirrors [`KEEP_WGSL`], and `tests/geom_first_gpu.rs`
//! runs both shaders on a desktop GPU against them. The GPU side is
//! Android-only (`runtime` below).

use glam::{Mat4, Quat, Vec3, Vec4};

use crate::env_depth::{
    DepthView, ENV_DEPTH_WGSL, Obb, UNIFORM_ROWS, decode_distance, is_infinite, mesh_side,
    mesh_texel,
};
use crate::live_planes::{NOISE_K, NOISE_MIN_M};

/// The sideways margin around a box's silhouette, in depth texel widths at
/// the sample's depth (`debug.fosfora.geomfirstmargin`): the runtime's map
/// blends across about a texel at a silhouette, and the mesh vertex can sit
/// half a texel off it.
pub const MARGIN_TEXELS: f32 = 1.5;
/// The tolerance in front of a box's surface (m,
/// `debug.fosfora.geomfirsttol`), before the sensor's noise is added
/// ([`front_tolerance`]): a scanned box sits within a few centimeters of
/// its object.
pub const TOL_M: f32 = 0.04;
/// The margin knob's range (texel widths).
pub const MARGIN_MAX_TEXELS: f32 = 8.0;
/// The tolerance knob's range (m).
pub const TOL_MAX_M: f32 = 0.5;
/// A dropped vertex's clip z, in units of its clip w: far past the far
/// plane (z = w), so the clipper removes its triangles ([`kept_fraction`]).
pub const DROP_Z: f32 = 1024.0;
/// The tint of `debug.fosfora.geomfirstshow`: linear RGB and alpha, drawn
/// premultiplied over where the depth map still occludes.
pub const TINT: [f32; 4] = [1.0, 0.0, 0.85, 0.45];
/// The obstacle block's box capacity (`particles3d::MAX_BOXES`).
pub const MAX_BOXES: usize = 32;
/// Bytes of the obstacle block (`particles3d::ObstacleSet`, `struct
/// Obstacles` in [`EXPLAIN_WGSL`]): two header rows, 64 spheres and 32
/// boxes of three rows each.
pub const OBSTACLES_BYTES: usize = 16 * (2 + 64 + 3 * MAX_BOXES);
/// Rows of [`Explain::rows`], `struct GeomParams` in [`EXPLAIN_WGSL`].
pub const PARAM_ROWS: usize = 14;
/// Rows of [`Keep::rows`], `struct KeepParams` in [`KEEP_WGSL`].
pub const KEEP_ROWS: usize = 2;
/// A ray direction component smaller than this is taken as this in the
/// slab test ([`ray_surface`]), so the slabs stay finite on the GPU.
pub const PARALLEL: f32 = 1e-6;
/// The vertices that share a triangle with vertex (x, y) of the occluder
/// mesh (`env_depth::mesh_indices`: cells split along the (x + 1, y) to
/// (x, y + 1) diagonal), as (dx, dy).
pub const NEIGHBORS: [[i32; 2]; 6] = [[1, 0], [-1, 0], [0, 1], [0, -1], [1, -1], [-1, 1]];

/// The knobs' names.
pub const KNOB: &str = "debug.fosfora.geomfirst";
pub const KNOB_MARGIN: &str = "debug.fosfora.geomfirstmargin";
pub const KNOB_TOL: &str = "debug.fosfora.geomfirsttol";
pub const KNOB_SHOW: &str = "debug.fosfora.geomfirstshow";

/// `debug.fosfora.geomfirst`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// 0 or unset: the map occludes everywhere, as before.
    #[default]
    Off,
    /// 1: explained vertices dropped, but those an unexplained nearer
    /// neighbor needs (the keep rule).
    On,
    /// 2: every explained vertex dropped, no keep rule (the A/B for it).
    DropAll,
}

impl Mode {
    pub fn is_on(self) -> bool {
        self != Self::Off
    }
}

/// What the geometry-first knobs ask.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GeomFirstOptions {
    pub mode: Mode,
    /// [`MARGIN_TEXELS`], `debug.fosfora.geomfirstmargin`.
    pub margin_texels: f32,
    /// [`TOL_M`], `debug.fosfora.geomfirsttol`.
    pub tol_m: f32,
    /// Tint what the map still occludes (`debug.fosfora.geomfirstshow`).
    pub show: bool,
}

impl Default for GeomFirstOptions {
    fn default() -> Self {
        Self {
            mode: Mode::Off,
            margin_texels: MARGIN_TEXELS,
            tol_m: TOL_M,
            show: false,
        }
    }
}

impl GeomFirstOptions {
    /// The options the four knobs ask, each read through `prop` (a system
    /// property by name, `None` unset). An unreadable or out-of-range value
    /// takes the default or the nearest end of the range.
    pub fn from_knobs(prop: impl Fn(&str) -> Option<String>) -> Self {
        let d = Self::default();
        let number = |name: &str| {
            prop(name)
                .and_then(|v| v.trim().parse::<f32>().ok())
                .filter(|v| v.is_finite())
        };
        Self {
            mode: match prop(KNOB).as_deref().map(str::trim) {
                Some("1") => Mode::On,
                Some("2") => Mode::DropAll,
                _ => Mode::Off,
            },
            margin_texels: number(KNOB_MARGIN)
                .map_or(d.margin_texels, |v| v.clamp(0.0, MARGIN_MAX_TEXELS)),
            tol_m: number(KNOB_TOL).map_or(d.tol_m, |v| v.clamp(0.0, TOL_MAX_M)),
            show: prop(KNOB_SHOW).as_deref().map(str::trim) == Some("1"),
        }
    }

    /// The state in a few words, for the log.
    pub fn describe(&self) -> String {
        format!(
            "{} · margin {} texels sideways · tolerance {} m + the sensor's noise in front · tint {}",
            match self.mode {
                Mode::Off => "off (the depth map occludes everywhere)",
                Mode::On => "on (the depth map leaves to the room's boxes what they explain)",
                Mode::DropAll => {
                    "on without the keep rule (geomfirst 2: every explained vertex dropped)"
                }
            },
            self.margin_texels,
            self.tol_m,
            if self.show { "on" } else { "off" },
        )
    }

    /// The launch line. `boxes`: whether the room's boxes are drawn as
    /// occluders in this run (`debug.fosfora.occluders`).
    pub fn line(&self, boxes: bool) -> String {
        format!(
            "geometry first: {}{}",
            self.describe(),
            no_boxes(self, boxes)
        )
    }

    /// The line for a change from `before`.
    pub fn change_line(&self, before: &Self, boxes: bool) -> String {
        format!(
            "geometry first: {}{} (was: {})",
            self.describe(),
            no_boxes(self, boxes),
            before.describe()
        )
    }
}

fn no_boxes(o: &GeomFirstOptions, boxes: bool) -> &'static str {
    if o.mode.is_on() && !boxes {
        " · no box occluders in this run (debug.fosfora.occluders 0): nothing is explained"
    } else {
        ""
    }
}

/// The angle a depth texel spans, as the tangent span over the texels
/// (meters across per meter of depth along the camera's -Z), the larger of
/// its width and height. `tan` is the fov's tangents: left, right, up,
/// down.
pub fn texel_angle(tan: [f32; 4], size: [u32; 2]) -> f32 {
    let [l, r, u, d] = tan;
    ((r - l) / size[0].max(1) as f32).max((u - d) / size[1].max(1) as f32)
}

/// How far in front of a box's surface a sample at `range` (m) may lie and
/// still be explained: `tol_m` plus one standard deviation of the sensor's
/// noise at that range (`live_planes::NoiseModel`).
pub fn front_tolerance(range: f32, tol_m: f32) -> f32 {
    tol_m + (NOISE_K * range * range).max(NOISE_MIN_M)
}

/// `v` turned by unit quaternion `q`, as the shaders do it.
pub fn quat_rotate(q: Quat, v: Vec3) -> Vec3 {
    let qv = Vec3::new(q.x, q.y, q.z);
    v + 2.0 * qv.cross(qv.cross(v) + q.w * v)
}

/// Where the ray from `o` along unit `u` first crosses the boundary of the
/// box of half extents `half` about the origin (all in the box's frame):
/// where it enters, or where it leaves when `o` is inside. `None` when the
/// line misses the box or the box is behind `o`. Mirrors `ray_surface` in
/// [`EXPLAIN_WGSL`].
pub fn ray_surface(o: Vec3, u: Vec3, half: Vec3) -> Option<f32> {
    let uu = Vec3::select(
        u.abs().cmplt(Vec3::splat(PARALLEL)),
        Vec3::splat(PARALLEL),
        u,
    );
    let a = (-half - o) / uu;
    let b = (half - o) / uu;
    let (lo, hi) = (a.min(b), a.max(b));
    let enter = lo.x.max(lo.y).max(lo.z);
    let exit = hi.x.min(hi.y).min(hi.z);
    if enter > exit || exit <= 0.0 {
        return None;
    }
    Some(if enter > 0.0 { enter } else { exit })
}

/// Whether box `b` explains the depth sample at world point `p` seen from
/// `o` (see the module header): the ray from `o` through `p` meets `b`, or
/// `b` grown by `lateral_m` on every side, and `p` is no more than
/// `front_m` in front of where it first does. Mirrors `explains` in
/// [`EXPLAIN_WGSL`].
pub fn explains(b: &Obb, o: Vec3, p: Vec3, lateral_m: f32, front_m: f32) -> bool {
    let range = (p - o).length();
    if range <= 0.0 {
        return false;
    }
    let u = (p - o) / range;
    let grown = b.half + Vec3::splat(lateral_m);
    // A ray that passes the grown box's bounding sphere by is skipped.
    let oc = b.center - o;
    let along = oc.dot(u);
    if oc.dot(oc) - along * along > grown.dot(grown) {
        return false;
    }
    let inv = b.rot.conjugate();
    let lo = quat_rotate(inv, o - b.center);
    let lu = quat_rotate(inv, u);
    ray_surface(lo, lu, b.half)
        .or_else(|| ray_surface(lo, lu, grown))
        .is_some_and(|s| range >= s - front_m)
}

/// The depth map coordinates of the mesh vertex at `texel`, as the vertex
/// stage takes them (`EyeReprojection::mesh_uv`), and its depth camera ray
/// at z = -1 (`EyeReprojection::depth_ray`), in the shaders' arithmetic.
/// `tan`: left, right, up, down.
pub fn vertex_ray(tan: [f32; 4], size: [u32; 2], flip_v: bool, texel: [u32; 2]) -> Vec3 {
    let at = |t: u32, len: u32| {
        if t + 1 >= len {
            1.0
        } else if t == 0 {
            0.0
        } else {
            (t as f32 + 0.5) / len as f32
        }
    };
    let (u, v) = (at(texel[0], size[0]), at(texel[1], size[1]));
    let v_up = if flip_v { 1.0 - v } else { v };
    let mix = |a: f32, b: f32, t: f32| a * (1.0 - t) + b * t;
    let [l, r, up, down] = tan;
    Vec3::new(mix(l, r, u), mix(down, up, v_up), -1.0)
}

/// One depth layer for the explain pass: its camera's pose (depth camera
/// to world, the inverse of `DepthView::view_matrix`, exactly as the
/// occluder's uniform carries it) and the tangents of its fov.
#[derive(Debug, Clone, Copy)]
pub struct ExplainLayer {
    pub to_world: Mat4,
    /// Left, right, up, down.
    pub tan: [f32; 4],
}

impl ExplainLayer {
    pub fn of(view: &DepthView) -> Self {
        let f = view.fov;
        Self {
            to_world: view.view_matrix().inverse(),
            tan: [f.left.tan(), f.right.tan(), f.up.tan(), f.down.tan()],
        }
    }
}

/// Everything the explain pass needs for one frame, and its CPU mirror.
#[derive(Debug, Clone, Copy)]
pub struct Explain {
    pub layers: [ExplainLayer; 2],
    /// The map's projection planes, as acquired.
    pub near: f32,
    pub far: f32,
    /// `debug.fosfora.envdepthnear`.
    pub near_cut_m: f32,
    /// The map's size (texels).
    pub size: [u32; 2],
    /// `debug.fosfora.envdepthflipv`.
    pub flip_v: bool,
    /// The eye pass's mesh step (texels per vertex).
    pub step: u32,
    /// Whether the boxes are drawn as occluders: off, nothing is explained.
    pub boxes_on: bool,
    pub margin_texels: f32,
    pub tol_m: f32,
}

impl Explain {
    /// The eye mesh's vertices per row and rows: the pass's grid.
    pub fn grid(&self) -> [u32; 2] {
        [
            mesh_side(self.size[0], self.step),
            mesh_side(self.size[1], self.step),
        ]
    }

    /// The rows of `struct GeomParams` in [`EXPLAIN_WGSL`].
    pub fn rows(&self) -> [[f32; 4]; PARAM_ROWS] {
        let mut rows = [[0.0; 4]; PARAM_ROWS];
        for (l, layer) in self.layers.iter().enumerate() {
            for (c, col) in layer.to_world.to_cols_array_2d().into_iter().enumerate() {
                rows[4 * l + c] = col;
            }
            rows[8 + l] = layer.tan;
        }
        let infinite = is_infinite(self.near, self.far);
        rows[10] = [
            self.near,
            if infinite { 0.0 } else { self.far },
            if infinite { 1.0 } else { 0.0 },
            self.near_cut_m,
        ];
        rows[11] = [
            self.size[0] as f32,
            self.size[1] as f32,
            if self.flip_v { 1.0 } else { 0.0 },
            self.step as f32,
        ];
        let grid = self.grid();
        rows[12] = [
            grid[0] as f32,
            grid[1] as f32,
            if self.boxes_on { MAX_BOXES as f32 } else { 0.0 },
            0.0,
        ];
        rows[13] = [self.margin_texels, self.tol_m, NOISE_K, NOISE_MIN_M];
        rows
    }

    /// The world point the mesh puts vertex `k` of `layer` at, for a texel
    /// at distance `dist` (m) along the camera's -Z, and its camera's
    /// position.
    pub fn point(&self, layer: usize, k: [u32; 2], dist: f32) -> (Vec3, Vec3) {
        let t = [
            mesh_texel(k[0], self.size[0], self.step),
            mesh_texel(k[1], self.size[1], self.step),
        ];
        let l = &self.layers[layer];
        let ray = vertex_ray(l.tan, self.size, self.flip_v, t);
        let w = l.to_world * (ray * dist).extend(1.0);
        (l.to_world.w_axis.truncate(), w.truncate() / w.w)
    }

    /// What the explain pass writes for vertex `k` (its grid coordinates)
    /// of `layer`, whose texel holds the stored value `d`: 0 without data
    /// or under the discard distance, else the range from the camera (m),
    /// negative when `boxes` (the obstacle block's, in order) explain it.
    pub fn value(&self, layer: usize, k: [u32; 2], d: f32, boxes: &[Obb]) -> f32 {
        if d >= 1.0 {
            return 0.0;
        }
        let dist = decode_distance(d, self.near, self.far);
        if dist < self.near_cut_m {
            return 0.0;
        }
        let (o, p) = self.point(layer, k, dist);
        let range = (p - o).length();
        let lateral = self.margin_texels * texel_angle(self.layers[layer].tan, self.size) * dist;
        let front = front_tolerance(range, self.tol_m);
        let n = if self.boxes_on {
            boxes.len().min(MAX_BOXES)
        } else {
            0
        };
        if boxes[..n].iter().any(|b| explains(b, o, p, lateral, front)) {
            -range
        } else {
            range
        }
    }

    /// [`Self::value`] for every vertex of both layers, laid out as the
    /// pass writes them: `(layer * rows + y) * per_row + x`. `stored`
    /// gives the stored value of (layer, texel).
    pub fn values(&self, boxes: &[Obb], stored: impl Fn(usize, [u32; 2]) -> f32) -> Vec<f32> {
        let [nx, ny] = self.grid();
        let mut out = Vec::with_capacity((2 * nx * ny) as usize);
        for layer in 0..2 {
            for y in 0..ny {
                for x in 0..nx {
                    let t = [
                        mesh_texel(x, self.size[0], self.step),
                        mesh_texel(y, self.size[1], self.step),
                    ];
                    out.push(self.value(layer, [x, y], stored(layer, t), boxes));
                }
            }
        }
        out
    }
}

/// The keep pass for one target's mesh: which of its vertices to drop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Keep {
    /// This target's mesh: vertices per row, rows.
    pub grid: [u32; 2],
    /// Its step over the explain pass's (1, or 2 for the faces pass).
    pub ratio: u32,
    /// The explain pass's grid.
    pub explain_grid: [u32; 2],
    /// The keep rule (mode 1); off, every explained vertex is dropped.
    pub keep: bool,
}

impl Keep {
    /// The keep pass for a mesh at `step` over a `size` map, against the
    /// explain pass at `explain_step`; `None` unless `step` is a whole
    /// multiple of it (the faces pass draws at 1 or 2 times the eye's).
    pub fn of(size: [u32; 2], explain_step: u32, step: u32, keep: bool) -> Option<Self> {
        (explain_step > 0 && step >= explain_step && step.is_multiple_of(explain_step)).then(|| {
            Self {
                grid: [mesh_side(size[0], step), mesh_side(size[1], step)],
                ratio: step / explain_step,
                explain_grid: [
                    mesh_side(size[0], explain_step),
                    mesh_side(size[1], explain_step),
                ],
                keep,
            }
        })
    }

    /// The rows of `struct KeepParams` in [`KEEP_WGSL`].
    pub fn rows(&self) -> [[f32; 4]; KEEP_ROWS] {
        [
            [
                self.grid[0] as f32,
                self.grid[1] as f32,
                self.ratio as f32,
                if self.keep { 1.0 } else { 0.0 },
            ],
            [
                self.explain_grid[0] as f32,
                self.explain_grid[1] as f32,
                0.0,
                0.0,
            ],
        ]
    }

    /// The explain pass's vertex for this mesh's vertex (x, y): the same
    /// texel ([`mesh_texel`] at either step).
    pub fn explain_vertex(&self, x: u32, y: u32) -> [u32; 2] {
        [
            (x * self.ratio).min(self.explain_grid[0] - 1),
            (y * self.ratio).min(self.explain_grid[1] - 1),
        ]
    }

    /// Per vertex of both layers (`(layer * rows + y) * per_row + x`),
    /// whether the vertex stage drops it, from the explain pass's `values`
    /// ([`Explain::values`]): an explained vertex, unless the keep rule is
    /// on and a vertex it shares a triangle with ([`NEIGHBORS`]) is
    /// unexplained, has data and is nearer to the camera. Mirrors
    /// [`KEEP_WGSL`].
    pub fn drops(&self, values: &[f32]) -> Vec<bool> {
        let [nx, ny] = self.grid;
        let [ex, ey] = self.explain_grid;
        let at = |layer: u32, x: i32, y: i32| -> f32 {
            if x < 0 || y < 0 || x >= nx as i32 || y >= ny as i32 {
                return 0.0;
            }
            let [vx, vy] = self.explain_vertex(x as u32, y as u32);
            values
                .get(((layer * ey + vy) * ex + vx) as usize)
                .copied()
                .unwrap_or(0.0)
        };
        let mut out = Vec::with_capacity((2 * nx * ny) as usize);
        for layer in 0..2 {
            for y in 0..ny as i32 {
                for x in 0..nx as i32 {
                    let v = at(layer, x, y);
                    let kept = self.keep
                        && NEIGHBORS.iter().any(|[dx, dy]| {
                            let n = at(layer, x + dx, y + dy);
                            n > 0.0 && n < -v
                        });
                    out.push(v < 0.0 && !kept);
                }
            }
        }
        out
    }
}

/// The clip position the vertex stage gives a dropped vertex at `clip`:
/// z moved to [`DROP_Z`] times w, past the far plane.
pub fn dropped_clip(clip: Vec4) -> Vec4 {
    Vec4::new(clip.x, clip.y, clip.w * DROP_Z, clip.w)
}

/// How much of the edge from a vertex at clip position `a` to one at `b`
/// is left once the triangle is clipped at the far plane (z <= w), from
/// `a`'s end: 1 when `b` is not past it.
pub fn kept_fraction(a: Vec4, b: Vec4) -> f32 {
    let (fa, fb) = (a.w - a.z, b.w - b.z);
    if fb >= 0.0 { 1.0 } else { fa / (fa - fb) }
}

/// The occluder's uniform rows (`EyeReprojection::uniform`) with the
/// mesh row's z (`EnvEye.mesh.z`) set to the drop buffer's vertices per
/// layer, which turns the drop on in `vs_mesh_geom`; 0, the uniform's own
/// value, leaves it off.
pub fn set_drop_vertices(rows: &mut [[f32; 4]; UNIFORM_ROWS], per_layer: u32) {
    rows[UNIFORM_ROWS - 1][2] = per_layer as f32;
}

/// The obstacle block's rows (`ObstacleSet::as_vec4s` on the device) with
/// `boxes` and no spheres, for the desktop tests of [`EXPLAIN_WGSL`]: the
/// header (sphere count 0, box count as u32 bits), 64 sphere rows, then
/// the boxes' centers, rotations and half extents, 32 rows each.
pub fn obstacle_rows(boxes: &[Obb]) -> Vec<[f32; 4]> {
    let n = boxes.len().min(MAX_BOXES);
    let mut rows = vec![[0.0; 4]; OBSTACLES_BYTES / 16];
    rows[0][1] = f32::from_bits(n as u32);
    let (centers, rots, halves) = (66, 66 + MAX_BOXES, 66 + 2 * MAX_BOXES);
    for (k, b) in boxes[..n].iter().enumerate() {
        rows[centers + k] = b.center.extend(0.0).to_array();
        rows[rots + k] = b.rot.to_array();
        rows[halves + k] = b.half.extend(0.0).to_array();
    }
    rows
}

/// The explain pass. Group 0: binding 0 the depth map, 1 the obstacle
/// block (the same uniform the box occluder draws from), 2 the frame's
/// `GeomParams` ([`Explain::rows`]), 3 the values out, one per vertex of
/// the eye mesh of both layers ([`Explain::value`] is the CPU mirror).
pub const EXPLAIN_WGSL: &str = r"
const PARALLEL: f32 = 1e-6;
const MAX_BOXES: u32 = 32u;

// particles3d.rs's ObstacleSet, as particles3d_occluder.wgsl reads it.
struct Obstacles {
    sphere_count: u32,
    box_count: u32,
    restitution: f32,
    margin: f32,
    sphere_shrink: f32,
    sphere_kick: f32,
    _pad1: f32,
    _pad2: f32,
    spheres: array<vec4<f32>, 64>,
    box_center: array<vec4<f32>, 32>,
    box_rot: array<vec4<f32>, 32>,
    box_half: array<vec4<f32>, 32>,
}

struct GeomParams {
    // Depth camera to world, per layer.
    to_world: array<mat4x4<f32>, 2>,
    // Tangents of each layer's fov: left, right, up, down.
    tan: array<vec4<f32>, 2>,
    // near, far (0 when infinite), 1 when infinite, discard distance (m)
    depth_range: vec4<f32>,
    // map width, height, 1 = texture row 0 is the top, texels per vertex
    map: vec4<f32>,
    // vertices per row, rows, boxes tested at most (0: none), 0
    grid: vec4<f32>,
    // margin (texel widths), tolerance (m), noise (1/m), noise floor (m)
    rule: vec4<f32>,
}

@group(0) @binding(0) var depth_map: texture_depth_2d_array;
@group(0) @binding(1) var<uniform> obstacles: Obstacles;
@group(0) @binding(2) var<uniform> p: GeomParams;
@group(0) @binding(3) var<storage, read_write> values: array<f32>;

fn decode(d: f32) -> f32 {
    let n = p.depth_range.x;
    if p.depth_range.z > 0.5 {
        return n / (1.0 - d);
    }
    let f = p.depth_range.y;
    return 2.0 * n * f / (f + n - (2.0 * d - 1.0) * (f - n));
}

fn quat_rotate(q: vec4<f32>, v: vec3<f32>) -> vec3<f32> {
    return v + 2.0 * cross(q.xyz, cross(q.xyz, v) + q.w * v);
}

// Where the ray from o along unit u first crosses the boundary of the box
// of half extents h about the origin: entering, or leaving from inside; -1
// when the line misses it or it is behind o (geom_first::ray_surface).
fn ray_surface(o: vec3<f32>, u: vec3<f32>, h: vec3<f32>) -> f32 {
    let uu = select(u, vec3<f32>(PARALLEL), abs(u) < vec3<f32>(PARALLEL));
    let a = (-h - o) / uu;
    let b = (h - o) / uu;
    let lo = min(a, b);
    let hi = max(a, b);
    let enter = max(max(lo.x, lo.y), lo.z);
    let exit = min(min(hi.x, hi.y), hi.z);
    if enter > exit || exit <= 0.0 {
        return -1.0;
    }
    return select(exit, enter, enter > 0.0);
}

// Whether box k explains the sample range m along unit u from o
// (geom_first::explains).
fn explains(k: u32, o: vec3<f32>, u: vec3<f32>, range: f32, lateral: f32, front: f32) -> bool {
    let c = obstacles.box_center[k].xyz;
    let h = obstacles.box_half[k].xyz;
    let grown = h + vec3<f32>(lateral);
    let oc = c - o;
    let along = dot(oc, u);
    if dot(oc, oc) - along * along > dot(grown, grown) {
        return false;
    }
    let q = obstacles.box_rot[k];
    let inv = vec4<f32>(-q.xyz, q.w);
    let lo = quat_rotate(inv, o - c);
    let lu = quat_rotate(inv, u);
    var s = ray_surface(lo, lu, h);
    if s < 0.0 {
        s = ray_surface(lo, lu, grown);
    }
    return s >= 0.0 && range >= s - front;
}

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let nx = u32(p.grid.x);
    let ny = u32(p.grid.y);
    if id.x >= nx || id.y >= ny || id.z >= 2u {
        return;
    }
    // The vertex's texel and ray, as vs_mesh in env_depth.rs takes them.
    let size = p.map.xy;
    let last = vec2<u32>(size) - vec2<u32>(1u);
    let t = min(id.xy * u32(p.map.w), last);
    var uv = (vec2<f32>(t) + 0.5) / size;
    uv = select(uv, vec2<f32>(0.0), t == vec2<u32>(0u));
    uv = select(uv, vec2<f32>(1.0), t == last);
    var v_up = uv.y;
    if p.map.z > 0.5 {
        v_up = 1.0 - uv.y;
    }
    let tn = p.tan[id.z];
    let ray = vec3<f32>(mix(tn.x, tn.y, uv.x), mix(tn.w, tn.z, v_up), -1.0);
    // 0: no data, or under the discard distance; else the range from the
    // camera, negative when a box explains the sample.
    var value = 0.0;
    let d = textureLoad(depth_map, vec2<i32>(t), i32(id.z), 0);
    if d < 1.0 {
        let dist = decode(d);
        if dist >= p.depth_range.w {
            let m = p.to_world[id.z];
            let o = m[3].xyz;
            let w = m * vec4<f32>(ray * dist, 1.0);
            let pw = w.xyz / w.w;
            let range = length(pw - o);
            let u = (pw - o) / range;
            let angle = max((tn.y - tn.x) / size.x, (tn.z - tn.w) / size.y);
            let lateral = p.rule.x * angle * dist;
            let front = p.rule.y + max(p.rule.z * range * range, p.rule.w);
            let n = min(min(obstacles.box_count, u32(p.grid.z)), MAX_BOXES);
            var hit = false;
            for (var k = 0u; k < n; k++) {
                if explains(k, o, u, range, lateral, front) {
                    hit = true;
                    break;
                }
            }
            value = select(range, -range, hit);
        }
    }
    values[(id.z * ny + id.y) * nx + id.x] = value;
}
";

/// The keep pass, once per target. Group 0: binding 0 the explain pass's
/// values, 1 the target's drops out (1 dropped, 0 kept, per vertex of its
/// mesh of both layers), 2 its `KeepParams` ([`Keep::rows`]). [`Keep::drops`]
/// is the CPU mirror.
pub const KEEP_WGSL: &str = r"
struct KeepParams {
    // this mesh's vertices per row, rows, its step over the explain
    // pass's, 1 = the keep rule
    grid: vec4<f32>,
    // the explain pass's vertices per row, rows
    explain: vec4<f32>,
}

@group(0) @binding(0) var<storage, read> values: array<f32>;
@group(0) @binding(1) var<storage, read_write> drops: array<u32>;
@group(0) @binding(2) var<uniform> k: KeepParams;

// The explain pass's value for this mesh's vertex (x, y) of layer, 0
// outside the mesh.
fn value_at(layer: u32, x: i32, y: i32) -> f32 {
    if x < 0 || y < 0 || x >= i32(k.grid.x) || y >= i32(k.grid.y) {
        return 0.0;
    }
    let ex = u32(k.explain.x);
    let ey = u32(k.explain.y);
    let e = min(vec2<u32>(u32(x), u32(y)) * u32(k.grid.z), vec2<u32>(ex - 1u, ey - 1u));
    return values[(layer * ey + e.y) * ex + e.x];
}

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let nx = u32(k.grid.x);
    let ny = u32(k.grid.y);
    if id.x >= nx || id.y >= ny || id.z >= 2u {
        return;
    }
    let x = i32(id.x);
    let y = i32(id.y);
    let v = value_at(id.z, x, y);
    var dropped = 0u;
    if v < 0.0 {
        dropped = 1u;
        if k.grid.w > 0.5 {
            // The vertices that share a triangle with this one
            // (geom_first::NEIGHBORS).
            var around = array<vec2<i32>, 6>(
                vec2<i32>(1, 0), vec2<i32>(-1, 0), vec2<i32>(0, 1),
                vec2<i32>(0, -1), vec2<i32>(1, -1), vec2<i32>(-1, 1),
            );
            for (var j = 0; j < 6; j++) {
                let n = value_at(id.z, x + around[j].x, y + around[j].y);
                if n > 0.0 && n < -v {
                    dropped = 0u;
                }
            }
        }
    }
    drops[(id.z * ny + id.y) * nx + id.x] = dropped;
}
";

/// `vs_mesh`'s first line in [`ENV_DEPTH_WGSL`], which
/// [`mesh_geom_wgsl`] turns into a plain function.
const VS_MESH_HEAD: &str = "@vertex\nfn vs_mesh(@builtin(vertex_index) i: u32) -> MeshOut {";
const MESH_VERTEX_HEAD: &str = "fn mesh_vertex(i: u32) -> MeshOut {";

/// The occluder mesh's shader with the drop: [`ENV_DEPTH_WGSL`] verbatim,
/// but `vs_mesh` becomes the function `mesh_vertex` (the same body), and
/// two entry points follow. `vs_mesh_geom` is `vs_mesh` plus the drop:
/// group 0 binding 2, the target's drops ([`KEEP_WGSL`]), read while
/// `EnvEye.mesh.z` ([`set_drop_vertices`]) is not 0. `fs_mesh_tint`
/// writes [`TINT`] where the mesh has data.
pub fn mesh_geom_wgsl() -> String {
    let base = ENV_DEPTH_WGSL.replacen(VS_MESH_HEAD, MESH_VERTEX_HEAD, 1);
    let [r, g, b, a] = TINT;
    format!(
        r"{base}
// ---- Geometry first (geom_first.rs) ----
@group(0) @binding(2) var<storage, read> drops: array<u32>;

const DROP_Z: f32 = {DROP_Z:?};
const TINT: vec4<f32> = vec4<f32>({r:?}, {g:?}, {b:?}, {a:?});

// vs_mesh, and a dropped vertex past the far plane: the clipper removes
// every triangle it is part of.
@vertex
fn vs_mesh_geom(@builtin(vertex_index) i: u32) -> MeshOut {{
    var out = mesh_vertex(i);
    let per_layer = u32(env.mesh.z);
    if per_layer > 0u && drops[u32(env.misc.x) * per_layer + i] != 0u {{
        out.pos.z = out.pos.w * DROP_Z;
        out.show.y = 0.0;
    }}
    return out;
}}

// The tint where the map occludes from data (premultiplied, blended over).
@fragment
fn fs_mesh_tint(@location(0) show: vec2<f32>) -> @location(0) vec4<f32> {{
    let a = select(0.0, TINT.a, show.y >= 0.5);
    return vec4<f32>(TINT.rgb * a, a);
}}
"
    )
}

#[cfg(target_os = "android")]
pub use runtime::{Geom, GeomPass};

/// The GPU side: the two compute pipelines and their buffers, the mesh
/// pipelines with the drop and the tint, and their bind groups.
#[cfg(target_os = "android")]
mod runtime {
    use log::info;

    use super::{
        EXPLAIN_WGSL, Explain, ExplainLayer, GeomFirstOptions, KEEP_ROWS, KEEP_WGSL, Keep, Mode,
        OBSTACLES_BYTES, PARAM_ROWS, mesh_geom_wgsl,
    };
    use crate::env_depth::{EnvDepthFrame, UNIFORM_ROWS};
    use crate::gfx::SWAPCHAIN_FORMAT;
    use crate::particles3d::DEPTH_FORMAT;

    /// Geometry first's GPU side, built with the occluder mesh
    /// (`EnvDepth::create`) whatever the knob says, so turning it on later
    /// builds nothing in the frame loop but the explain pass's bind groups.
    pub struct Geom {
        opts: GeomFirstOptions,
        size: [u32; 2],
        /// Per target (`OccluderTarget`): the mesh step.
        steps: [u32; 2],
        /// The explain pass's grid (the eye mesh's) and each target's.
        explain_grid: [u32; 2],
        grids: [[u32; 2]; 2],
        geom_pipeline: wgpu::RenderPipeline,
        tint_pipeline: wgpu::RenderPipeline,
        explain_pipeline: wgpu::ComputePipeline,
        explain_layout: wgpu::BindGroupLayout,
        keep_pipeline: wgpu::ComputePipeline,
        params: wgpu::Buffer,
        values: wgpu::Buffer,
        /// Per target: its `KeepParams`, and the keep rule they were last
        /// written with.
        keep_params: [wgpu::Buffer; 2],
        keep_written: Option<bool>,
        /// Per target: its drops (kept alive by the bind groups too).
        _drops: [wgpu::Buffer; 2],
        keep_groups: [wgpu::BindGroup; 2],
        /// Per swapchain image, per target, per eye: the occluder's
        /// uniform, the image and the target's drops.
        draw_groups: Vec<[[wgpu::BindGroup; 2]; 2]>,
        /// The explain pass's bind groups per swapchain image, and the
        /// obstacle block they bind, made the first frame it runs.
        explain_groups: Option<(wgpu::Buffer, Vec<wgpu::BindGroup>)>,
        /// This frame's swapchain image and whether the faces pass's keep
        /// runs, from `prepare` until the next `begin_frame`; `None` when
        /// the test does not run this frame.
        frame: Option<(u32, bool)>,
    }

    /// This frame's explain and keep passes, recorded by `Gfx::render`
    /// into the frame's command encoder before the eye passes.
    pub struct GeomPass<'a> {
        explain_pipeline: &'a wgpu::ComputePipeline,
        explain_group: &'a wgpu::BindGroup,
        explain_grid: [u32; 2],
        keep_pipeline: &'a wgpu::ComputePipeline,
        /// Per target, when its keep runs: its bind group and grid.
        keeps: [Option<(&'a wgpu::BindGroup, [u32; 2])>; 2],
    }

    impl GeomPass<'_> {
        pub fn record(&self, encoder: &mut wgpu::CommandEncoder) {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("xr-env-depth-geom"),
                timestamp_writes: None,
            });
            pass.set_pipeline(self.explain_pipeline);
            pass.set_bind_group(0, self.explain_group, &[]);
            let [ex, ey] = self.explain_grid;
            pass.dispatch_workgroups(ex.div_ceil(8), ey.div_ceil(8), 2);
            // The values go from read-write to read-only: wgpu puts a
            // barrier between the dispatches.
            pass.set_pipeline(self.keep_pipeline);
            for (group, [nx, ny]) in self.keeps.iter().flatten() {
                pass.set_bind_group(0, *group, &[]);
                pass.dispatch_workgroups(nx.div_ceil(8), ny.div_ceil(8), 2);
            }
        }
    }

    fn uniform_entry(binding: u32, bytes: usize) -> wgpu::BindGroupLayoutEntry {
        wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: wgpu::BufferSize::new(bytes as u64),
            },
            count: None,
        }
    }

    fn storage_entry(
        binding: u32,
        visibility: wgpu::ShaderStages,
        read_only: bool,
    ) -> wgpu::BindGroupLayoutEntry {
        wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }
    }

    fn depth_entry(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
        wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Depth,
                view_dimension: wgpu::TextureViewDimension::D2Array,
                multisampled: false,
            },
            count: None,
        }
    }

    fn compute_pipeline(
        device: &wgpu::Device,
        label: &str,
        source: &str,
        layout: &wgpu::BindGroupLayout,
    ) -> wgpu::ComputePipeline {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(label),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some(label),
            bind_group_layouts: &[layout],
            push_constant_ranges: &[],
        });
        device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(label),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("cs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        })
    }

    impl Geom {
        /// Build it for an occluder mesh at `steps` (per target, the eye's
        /// not 0) over a `size` map whose swapchain `images` and per-target,
        /// per-eye `uniforms` the occluder already has; `show` is
        /// `envdepthshow` (the gray diagnostic's fragment stage). `None`
        /// when the faces step is not a whole multiple of the eye's.
        pub fn build(
            device: &wgpu::Device,
            images: &[(wgpu::Texture, wgpu::TextureView)],
            uniforms: &[[wgpu::Buffer; 2]; 2],
            size: [u32; 2],
            steps: [u32; 2],
            show: bool,
            opts: GeomFirstOptions,
        ) -> Option<Self> {
            let keeps = steps.map(|s| Keep::of(size, steps[0], s, true));
            let [Some(eye_keep), Some(faces_keep)] = keeps else {
                return None;
            };
            let explain_grid = eye_keep.explain_grid;
            let grids = [eye_keep.grid, faces_keep.grid];
            let vertices = |g: [u32; 2]| u64::from(2 * g[0] * g[1]) * 4;

            // The mesh pipelines with the drop: the occluder's state, the
            // geom module's vertex stage.
            let draw_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("xr-env-depth-geom-draw"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: wgpu::BufferSize::new((UNIFORM_ROWS * 16) as u64),
                        },
                        count: None,
                    },
                    depth_entry(1, wgpu::ShaderStages::VERTEX_FRAGMENT),
                    storage_entry(2, wgpu::ShaderStages::VERTEX, true),
                ],
            });
            let source = mesh_geom_wgsl();
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("xr-env-depth-geom"),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            });
            let draw_pipeline_layout =
                device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("xr-env-depth-geom-draw"),
                    bind_group_layouts: &[&draw_layout],
                    push_constant_ranges: &[],
                });
            let mesh_pipeline = |label: &str, fragment: &str, target: wgpu::ColorTargetState| {
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some(label),
                    layout: Some(&draw_pipeline_layout),
                    vertex: wgpu::VertexState {
                        module: &shader,
                        entry_point: Some("vs_mesh_geom"),
                        compilation_options: wgpu::PipelineCompilationOptions::default(),
                        buffers: &[],
                    },
                    primitive: wgpu::PrimitiveState {
                        topology: wgpu::PrimitiveTopology::TriangleList,
                        cull_mode: None,
                        ..wgpu::PrimitiveState::default()
                    },
                    // As the mesh occluder: depth clipping on (a dropped
                    // vertex's triangles are clipped away), LessEqual.
                    depth_stencil: Some(wgpu::DepthStencilState {
                        format: DEPTH_FORMAT,
                        depth_write_enabled: true,
                        depth_compare: wgpu::CompareFunction::LessEqual,
                        stencil: wgpu::StencilState::default(),
                        bias: wgpu::DepthBiasState::default(),
                    }),
                    multisample: wgpu::MultisampleState::default(),
                    fragment: Some(wgpu::FragmentState {
                        module: &shader,
                        entry_point: Some(fragment),
                        compilation_options: wgpu::PipelineCompilationOptions::default(),
                        targets: &[Some(target)],
                    }),
                    multiview: None,
                    cache: None,
                })
            };
            let geom_pipeline = mesh_pipeline(
                if show {
                    "xr-env-depth-mesh-show-geom"
                } else {
                    "xr-env-depth-mesh-geom"
                },
                if show { "fs_mesh_show" } else { "fs_mesh" },
                wgpu::ColorTargetState {
                    format: SWAPCHAIN_FORMAT,
                    blend: None,
                    write_mask: if show {
                        wgpu::ColorWrites::ALL
                    } else {
                        wgpu::ColorWrites::empty()
                    },
                },
            );
            let tint_pipeline = mesh_pipeline(
                "xr-env-depth-mesh-tint",
                "fs_mesh_tint",
                wgpu::ColorTargetState {
                    format: SWAPCHAIN_FORMAT,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                },
            );

            // The explain pass.
            let explain_layout =
                device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("xr-env-depth-geom-explain"),
                    entries: &[
                        depth_entry(0, wgpu::ShaderStages::COMPUTE),
                        uniform_entry(1, OBSTACLES_BYTES),
                        uniform_entry(2, PARAM_ROWS * 16),
                        storage_entry(3, wgpu::ShaderStages::COMPUTE, false),
                    ],
                });
            let explain_pipeline = compute_pipeline(
                device,
                "xr-env-depth-geom-explain",
                EXPLAIN_WGSL,
                &explain_layout,
            );
            let params = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("xr-env-depth-geom-params"),
                size: (PARAM_ROWS * 16) as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let values = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("xr-env-depth-geom-values"),
                size: vertices(explain_grid),
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            });

            // The keep pass, and each target's drops.
            let keep_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("xr-env-depth-geom-keep"),
                entries: &[
                    storage_entry(0, wgpu::ShaderStages::COMPUTE, true),
                    storage_entry(1, wgpu::ShaderStages::COMPUTE, false),
                    uniform_entry(2, KEEP_ROWS * 16),
                ],
            });
            let keep_pipeline =
                compute_pipeline(device, "xr-env-depth-geom-keep", KEEP_WGSL, &keep_layout);
            let drops = grids.map(|g| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("xr-env-depth-geom-drops"),
                    size: vertices(g),
                    usage: wgpu::BufferUsages::STORAGE,
                    mapped_at_creation: false,
                })
            });
            let keep_params = grids.map(|_| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("xr-env-depth-geom-keep"),
                    size: (KEEP_ROWS * 16) as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            });
            let keep_groups = [0, 1].map(|t| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("xr-env-depth-geom-keep"),
                    layout: &keep_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: values.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: drops[t].as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: keep_params[t].as_entire_binding(),
                        },
                    ],
                })
            });
            let draw_groups = images
                .iter()
                .map(|(_, view)| {
                    [0, 1].map(|t| {
                        [0, 1].map(|eye| {
                            device.create_bind_group(&wgpu::BindGroupDescriptor {
                                label: Some("xr-env-depth-geom-draw"),
                                layout: &draw_layout,
                                entries: &[
                                    wgpu::BindGroupEntry {
                                        binding: 0,
                                        resource: uniforms[t][eye].as_entire_binding(),
                                    },
                                    wgpu::BindGroupEntry {
                                        binding: 1,
                                        resource: wgpu::BindingResource::TextureView(view),
                                    },
                                    wgpu::BindGroupEntry {
                                        binding: 2,
                                        resource: drops[t].as_entire_binding(),
                                    },
                                ],
                            })
                        })
                    })
                })
                .collect();
            info!(
                "geometry first: ready · explain pass over the eye mesh, {}x{} vertices x 2 layers · keep pass {}x{} (eye pass) and {}x{} (faces pass) · {}",
                explain_grid[0],
                explain_grid[1],
                grids[0][0],
                grids[0][1],
                grids[1][0],
                grids[1][1],
                opts.describe()
            );
            Some(Self {
                opts,
                size,
                steps,
                explain_grid,
                grids,
                geom_pipeline,
                tint_pipeline,
                explain_pipeline,
                explain_layout,
                keep_pipeline,
                params,
                values,
                keep_params,
                keep_written: None,
                _drops: drops,
                keep_groups,
                draw_groups,
                explain_groups: None,
                frame: None,
            })
        }

        pub fn set_options(&mut self, opts: GeomFirstOptions) {
            self.opts = opts;
        }

        /// At each acquire: nothing runs until `prepare` says so.
        pub fn begin_frame(&mut self) {
            self.frame = None;
        }

        /// Before the frame is recorded, after the obstacle block is
        /// written: with the knob on and the boxes drawn as occluders
        /// (`obstacles`, the block's uniform, `None` without), write the
        /// explain pass's params for `frame` and mark the passes to run.
        /// `faces`: whether a faces layer draws this frame.
        pub fn prepare(
            &mut self,
            device: &wgpu::Device,
            queue: &wgpu::Queue,
            images: &[(wgpu::Texture, wgpu::TextureView)],
            frame: &EnvDepthFrame,
            near_cut_m: f32,
            flip_v: bool,
            obstacles: Option<&wgpu::Buffer>,
            faces: bool,
        ) {
            self.frame = None;
            if !self.opts.mode.is_on() {
                return;
            }
            let Some(obstacles) = obstacles else {
                return;
            };
            if frame.index as usize >= images.len() {
                return;
            }
            if self
                .explain_groups
                .as_ref()
                .is_none_or(|(bound, _)| bound != obstacles)
            {
                let groups = images
                    .iter()
                    .map(|(_, view)| {
                        device.create_bind_group(&wgpu::BindGroupDescriptor {
                            label: Some("xr-env-depth-geom-explain"),
                            layout: &self.explain_layout,
                            entries: &[
                                wgpu::BindGroupEntry {
                                    binding: 0,
                                    resource: wgpu::BindingResource::TextureView(view),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 1,
                                    resource: obstacles.as_entire_binding(),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 2,
                                    resource: self.params.as_entire_binding(),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 3,
                                    resource: self.values.as_entire_binding(),
                                },
                            ],
                        })
                    })
                    .collect();
                info!("geometry first: the explain pass reads the occluders' obstacle block");
                self.explain_groups = Some((obstacles.clone(), groups));
            }
            let explain = Explain {
                layers: frame.views.map(|v| ExplainLayer::of(&v)),
                near: frame.near,
                far: frame.far,
                near_cut_m,
                size: self.size,
                flip_v,
                step: self.steps[0],
                boxes_on: true,
                margin_texels: self.opts.margin_texels,
                tol_m: self.opts.tol_m,
            };
            queue.write_buffer(&self.params, 0, bytemuck::cast_slice(&explain.rows()));
            let keep = self.opts.mode == Mode::On;
            if self.keep_written != Some(keep) {
                for (t, buffer) in self.keep_params.iter().enumerate() {
                    if let Some(k) = Keep::of(self.size, self.steps[0], self.steps[t], keep) {
                        queue.write_buffer(buffer, 0, bytemuck::cast_slice(&k.rows()));
                    }
                }
                self.keep_written = Some(keep);
            }
            self.frame = Some((frame.index, faces));
        }

        /// This frame's passes, `None` when the test does not run.
        pub fn pass(&self) -> Option<GeomPass<'_>> {
            let (index, faces) = self.frame?;
            let (_, groups) = self.explain_groups.as_ref()?;
            Some(GeomPass {
                explain_pipeline: &self.explain_pipeline,
                explain_group: groups.get(index as usize)?,
                explain_grid: self.explain_grid,
                keep_pipeline: &self.keep_pipeline,
                keeps: [
                    Some((&self.keep_groups[0], self.grids[0])),
                    faces.then_some((&self.keep_groups[1], self.grids[1])),
                ],
            })
        }

        /// The drop's vertices per layer for `target`'s uniform
        /// (`set_drop_vertices`), 0 when nothing is dropped this frame.
        pub fn drop_vertices(&self, target: usize) -> u32 {
            match self.frame {
                Some((_, faces)) if target == 0 || faces => {
                    self.grids[target][0] * self.grids[target][1]
                }
                _ => 0,
            }
        }

        /// The pipeline and per-eye bind groups for each target's draw
        /// from swapchain image `index`, `None` where the occluder's own
        /// stay: the eye pass's tint with `geomfirstshow` (the drop in it
        /// on with the test), the drop alone with the test running.
        pub fn draws(
            &self,
            index: u32,
        ) -> [Option<(&wgpu::RenderPipeline, &[wgpu::BindGroup; 2])>; 2] {
            let Some(groups) = self.draw_groups.get(index as usize) else {
                return [None, None];
            };
            let eye = if self.opts.show {
                Some((&self.tint_pipeline, &groups[0]))
            } else if self.drop_vertices(0) > 0 {
                Some((&self.geom_pipeline, &groups[0]))
            } else {
                None
            };
            let faces = (self.drop_vertices(1) > 0).then_some((&self.geom_pipeline, &groups[1]));
            [eye, faces]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env_depth::{EyeReprojection, faces_mesh_step, mesh_indices};
    use crate::math::Fov;

    const FOV: Fov = Fov {
        left: -0.9,
        right: 0.8,
        up: 0.85,
        down: -0.9,
    };

    fn tan(f: Fov) -> [f32; 4] {
        [f.left.tan(), f.right.tan(), f.up.tan(), f.down.tan()]
    }

    /// The default rule's margins for a sample at `p` seen from `o` at
    /// depth `z` along the camera's -Z: (sideways, in front).
    fn margins(o: Vec3, p: Vec3, z: f32) -> (f32, f32) {
        let lateral = MARGIN_TEXELS * texel_angle(tan(FOV), [320, 320]) * z;
        (lateral, front_tolerance((p - o).length(), TOL_M))
    }

    /// Whether `b` explains `p` from `o`, the camera looking down -Z (so
    /// the depth is `o.z - p.z`).
    fn explained_by(b: &Obb, o: Vec3, p: Vec3) -> bool {
        let (lateral, front) = margins(o, p, o.z - p.z);
        explains(b, o, p, lateral, front)
    }

    /// A table-sized box 2 m ahead of a camera at the origin: its front face
    /// at z = -1.8, its right edge at x = 0.5.
    fn table() -> Obb {
        Obb {
            center: Vec3::new(0.0, 0.0, -2.1),
            rot: Quat::IDENTITY,
            half: Vec3::new(0.5, 0.4, 0.3),
        }
    }

    #[test]
    fn a_texel_is_its_tangent_span_over_the_texels() {
        let t = tan(FOV);
        let wide = (t[1] - t[0]) / 320.0;
        let tall = (t[2] - t[3]) / 320.0;
        assert_close!(texel_angle(t, [320, 320]), wide.max(tall));
        // About 7.4 mm a texel at 1 m on the Quest 3's map: 1.5 texels are
        // 2.2 cm at 2 m.
        assert!((0.007..0.008).contains(&texel_angle(t, [320, 320])));
        let lateral = MARGIN_TEXELS * texel_angle(t, [320, 320]) * 2.0;
        assert!((0.021..0.023).contains(&lateral), "{lateral}");
        // The tolerance in front grows with the sensor's noise: 4.5 cm out
        // to 1.6 m, 4.8 cm at 2 m, 5.8 cm at 3 m.
        assert_close!(front_tolerance(1.0, TOL_M), 0.045);
        assert_close!(front_tolerance(2.0, TOL_M), 0.048);
        assert_close!(front_tolerance(3.0, TOL_M), 0.058);
    }

    #[test]
    fn a_point_on_a_face_is_explained_and_one_well_in_front_is_not() {
        let (b, o) = (table(), Vec3::ZERO);
        // On the front face, straight ahead and off to the side.
        for p in [Vec3::new(0.0, 0.0, -1.8), Vec3::new(0.35, -0.2, -1.8)] {
            assert!(explained_by(&b, o, p), "{p}");
        }
        // Just in front: 3 cm is within the 4.6 cm in front at that range,
        // 6 cm is not (something on the table's face).
        assert!(explained_by(&b, o, Vec3::new(0.0, 0.0, -1.77)));
        assert!(!explained_by(&b, o, Vec3::new(0.0, 0.0, -1.74)));
        // A person half a meter in front of it.
        assert!(!explained_by(&b, o, Vec3::new(0.1, 0.0, -1.3)));
        // The tolerance knob moves the line.
        let p = Vec3::new(0.0, 0.0, -1.74);
        let (lateral, _) = margins(o, p, 1.74);
        assert!(explains(&b, o, p, lateral, front_tolerance(1.74, 0.08)));
    }

    #[test]
    fn behind_a_face_is_explained() {
        let (b, o) = (table(), Vec3::ZERO);
        // Inside the box, at its back face, and a wall behind it in its
        // shadow: the box hides all of them.
        for p in [
            Vec3::new(0.0, 0.0, -2.0),
            Vec3::new(0.2, 0.1, -2.4),
            Vec3::new(0.3, 0.0, -3.5),
        ] {
            assert!(explained_by(&b, o, p), "{p}");
        }
    }

    #[test]
    fn the_silhouette_fringe_is_explained_and_past_it_is_not() {
        let (b, o) = (table(), Vec3::ZERO);
        // Beside the right edge (x = 0.5 at z -1.8), the map blends the box
        // into the wall behind: a sample there at a depth between the two.
        // 1.5 texels at that depth is about 2.4 cm sideways.
        let z = 2.3;
        let lateral = margins(o, Vec3::ZERO, z).0;
        assert!((0.022..0.027).contains(&lateral), "{lateral}");
        // The ray toward the sample passes the edge 1 cm out at the box.
        let fringe = |out_m: f32| {
            let dir = Vec3::new(0.5 + out_m, 0.0, -1.8) / 1.8;
            dir * z
        };
        assert!(explained_by(&b, o, fringe(0.01)));
        // Its depth anywhere from the box's front to far behind.
        let wall = Vec3::new(0.51, 0.0, -1.8) / 1.8 * 4.0;
        assert!(explained_by(&b, o, wall));
        // 4 cm out, past the margin: the map's own.
        assert!(!explained_by(&b, o, fringe(0.04)));
        // In the margin but well in front of the box: a hand reaching past
        // the table's edge, not the fringe.
        let hand = Vec3::new(0.51, 0.0, -1.8) / 1.8 * 1.4;
        assert!(!explained_by(&b, o, hand));
        // The margin knob widens it.
        let p = fringe(0.04);
        let (_, front) = margins(o, p, z);
        assert!(explains(&b, o, p, 0.05, front));
    }

    #[test]
    fn beside_a_thin_plane() {
        // A scanned wall: a plane given a 4 cm thickness (2 cm each side),
        // 3 m ahead, 2 m wide, turned 20 degrees.
        let wall = Obb {
            center: Vec3::new(0.0, 0.0, -3.0),
            rot: Quat::from_rotation_y(20f32.to_radians()),
            half: Vec3::new(1.0, 1.2, 0.02),
        };
        let o = Vec3::new(0.0, 0.2, 0.0);
        let on = |x: f32| {
            let q = wall.center + wall.rot * Vec3::new(x, 0.3, 0.0);
            (q, o.z - q.z)
        };
        // On the real wall (the plane's middle), and 1 cm off it.
        for x in [-0.8, 0.0, 0.9] {
            let (p, _) = on(x);
            assert!(explained_by(&wall, o, p), "{x}");
            let off = p + wall.rot * Vec3::Z * 0.01;
            assert!(explained_by(&wall, o, off), "{x} + 1 cm");
        }
        // 10 cm in front of it: a picture frame, a shelf, a person.
        let (p, _) = on(0.2);
        let front = p + wall.rot * Vec3::Z * 0.1;
        assert!(!explained_by(&wall, o, front));
        // Past the plane's edge (x = 1 in its frame): 1 cm out, the
        // fringe; 10 cm out, the map's (the wall going on beyond the scan,
        // or the room behind a doorway).
        let (edge, _) = on(1.01);
        assert!(explained_by(&wall, o, edge));
        let (beyond, _) = on(1.1);
        assert!(!explained_by(&wall, o, beyond));
    }

    #[test]
    fn inside_the_stage_floor() {
        // The stage floor as the app makes it: 20 m square, 10 cm thick,
        // its top at y = 0; the head 1.6 m above it, looking down -Z.
        let floor = Obb {
            center: Vec3::new(0.0, -0.05, 0.0),
            rot: Quat::IDENTITY,
            half: Vec3::new(10.0, 0.05, 10.0),
        };
        let o = Vec3::new(0.0, 1.6, 0.0);
        // On the floor 1, 2 and 4 m ahead; 2 cm under it (the map's noise);
        // inside the slab.
        for p in [
            Vec3::new(0.0, 0.0, -1.0),
            Vec3::new(0.5, 0.0, -2.0),
            Vec3::new(-0.3, 0.0, -4.0),
            Vec3::new(0.0, -0.02, -2.0),
            Vec3::new(0.0, -0.08, -1.5),
        ] {
            assert!(explained_by(&floor, o, p), "{p}");
        }
        // A shoe, 10 cm tall, 1.5 m ahead: not the floor.
        assert!(!explained_by(&floor, o, Vec3::new(0.0, 0.1, -1.5)));
    }

    #[test]
    fn from_inside_a_box_its_far_side_is_the_surface() {
        // A camera inside a (wrong) box sees the box occluder's far faces:
        // what is past them is explained, what is in front is not.
        let b = Obb {
            center: Vec3::ZERO,
            rot: Quat::IDENTITY,
            half: Vec3::splat(1.0),
        };
        let o = Vec3::new(0.0, 0.0, 0.5);
        assert!(explained_by(&b, o, Vec3::new(0.0, 0.0, -1.2)));
        assert!(!explained_by(&b, o, Vec3::new(0.0, 0.0, -0.2)));
        assert_close!(
            ray_surface(o, Vec3::NEG_Z, b.half).expect("leaves the box"),
            1.5
        );
        // The slab test: parallel rays, a box behind, a miss.
        assert!(ray_surface(Vec3::new(0.0, 2.0, 5.0), Vec3::NEG_Z, b.half).is_none());
        assert!(ray_surface(Vec3::new(0.0, 0.0, 5.0), Vec3::Z, b.half).is_none());
        assert_close!(
            ray_surface(Vec3::new(0.0, 0.0, 5.0), Vec3::NEG_Z, b.half).expect("enters"),
            4.0
        );
    }

    #[test]
    fn the_bounding_sphere_skip_changes_nothing() {
        // For rays all around a turned box, the test with the skip agrees
        // with the slab tests alone.
        let b = Obb {
            center: Vec3::new(0.3, -0.2, -2.0),
            rot: Quat::from_euler(glam::EulerRot::YXZ, 0.4, 0.2, 0.1),
            half: Vec3::new(0.6, 0.1, 0.3),
        };
        let o = Vec3::new(0.0, 0.1, 0.0);
        let (lateral, front) = (0.03, 0.05);
        for i in 0..40 {
            for j in 0..40 {
                let dir = Vec3::new(-0.6 + i as f32 * 0.03, -0.6 + j as f32 * 0.03, -1.0);
                for z in [1.5, 1.9, 2.5] {
                    let p = o + dir * z;
                    let range = (p - o).length();
                    let u = (p - o) / range;
                    let inv = b.rot.conjugate();
                    let (lo, lu) = (quat_rotate(inv, o - b.center), quat_rotate(inv, u));
                    let slabs = ray_surface(lo, lu, b.half)
                        .or_else(|| ray_surface(lo, lu, b.half + Vec3::splat(lateral)))
                        .is_some_and(|s| range >= s - front);
                    assert_eq!(explains(&b, o, p, lateral, front), slabs, "{p}");
                }
            }
        }
    }

    fn view(position: [f32; 3], orientation: Quat) -> DepthView {
        DepthView {
            orientation: orientation.to_array(),
            position,
            fov: FOV,
        }
    }

    fn explain(views: [DepthView; 2], step: u32) -> Explain {
        Explain {
            layers: views.map(|v| ExplainLayer::of(&v)),
            near: 0.1,
            far: f32::INFINITY,
            near_cut_m: crate::env_depth::NEAR_CUT_M,
            size: [320, 320],
            flip_v: false,
            step,
            boxes_on: true,
            margin_texels: MARGIN_TEXELS,
            tol_m: TOL_M,
        }
    }

    /// The stored value for `dist` under an infinite projection at near
    /// 0.1, as the runtime writes it.
    fn stored(dist: f32) -> f32 {
        1.0 - 0.1 / dist
    }

    #[test]
    fn the_vertex_is_where_the_occluder_mesh_puts_it() {
        // The explain pass tests the point the mesh's vertex stage places
        // (`EyeReprojection::vertex_clip` is its CPU mirror).
        let v = view([0.03, 1.5, 0.2], Quat::from_rotation_y(0.3));
        for flip_v in [false, true] {
            let mut e = explain([v; 2], 2);
            e.flip_v = flip_v;
            let r = EyeReprojection {
                view_proj: crate::math::view_projection(
                    [0.0, 0.0, 0.0, 1.0],
                    [0.0, 1.5, 0.0],
                    FOV,
                    0.05,
                    100.0,
                ),
                eye_pos: Vec3::new(0.0, 1.5, 0.0),
                depth_view: v.view_matrix(),
                depth_fov: FOV,
                near: 0.1,
                far: f32::INFINITY,
                depth_size: [320, 320],
                target_size: [1680, 1760],
                layer: 0,
                near_cut_m: 0.2,
                flip_v,
                filter_edge_m: crate::env_depth::FILTER_SOFT_M,
                mesh_step: 2,
            };
            for k in [[0, 0], [1, 80], [80, 80], [160, 3], [160, 160]] {
                let t = [mesh_texel(k[0], 320, 2), mesh_texel(k[1], 320, 2)];
                let theirs = r.depth_ray(r.mesh_uv(t));
                let ray = vertex_ray(tan(FOV), [320, 320], flip_v, t);
                assert!(ray.abs_diff_eq(theirs, 1e-6), "{k:?}");
                let (o, p) = e.point(0, k, 1.7);
                assert!(o.abs_diff_eq(Vec3::new(0.03, 1.5, 0.2), 1e-5));
                let want = r.depth_view.inverse().transform_point3(theirs * 1.7);
                assert!(p.abs_diff_eq(want, 1e-5), "{k:?}: {p} vs {want}");
            }
        }
    }

    #[test]
    fn values_carry_the_range_and_the_verdict() {
        // The camera at the origin looking down -Z at a wall 3 m off: the
        // vertex straight ahead has range 3, explained by the wall's box,
        // not by a box beside it, and nothing without data or too near.
        let e = explain([view([0.0; 3], Quat::IDENTITY); 2], 2);
        let wall = Obb {
            center: Vec3::new(0.0, 0.0, -3.02),
            rot: Quat::IDENTITY,
            half: Vec3::new(3.0, 3.0, 0.02),
        };
        let aside = Obb {
            center: Vec3::new(5.0, 0.0, -3.0),
            ..wall
        };
        let k = [80, 80];
        let dist = decode_distance(stored(3.0), 0.1, f32::INFINITY);
        let (o, p) = e.point(0, k, dist);
        let range = (p - o).length();
        assert_close!(e.value(0, k, stored(3.0), &[wall]), -range);
        assert_close!(e.value(0, k, stored(3.0), &[aside]), range);
        assert_close!(e.value(1, k, stored(3.0), &[aside, wall]), -range);
        assert_close!(e.value(0, k, 1.0, &[wall]), 0.0);
        assert_close!(e.value(0, k, stored(0.15), &[wall]), 0.0);
        // Boxes not drawn as occluders explain nothing.
        let off = Explain {
            boxes_on: false,
            ..e
        };
        assert_close!(off.value(0, k, stored(3.0), &[wall]), range);
        // The rows the shader reads.
        let rows = e.rows();
        assert_close!(rows[10], [0.1, 0.0, 1.0, crate::env_depth::NEAR_CUT_M]);
        assert_close!(rows[11], [320.0, 320.0, 0.0, 2.0]);
        assert_close!(rows[12], [161.0, 161.0, 32.0, 0.0]);
        assert_close!(rows[13], [MARGIN_TEXELS, TOL_M, NOISE_K, NOISE_MIN_M]);
        assert_close!(rows[8], tan(FOV));
        assert_close!(off.rows()[12][2], 0.0);
        let to_world = Mat4::from_cols_array_2d(&[rows[4], rows[5], rows[6], rows[7]]);
        assert!(to_world.abs_diff_eq(Mat4::IDENTITY, 1e-6));
    }

    #[test]
    fn neighbors_are_the_vertices_a_vertex_shares_triangles_with() {
        // Every pair of vertices of a triangle of the occluder mesh is in
        // NEIGHBORS, and every NEIGHBORS offset is used.
        let side = mesh_side(9, 1);
        let mut seen = std::collections::HashSet::new();
        for tri in mesh_indices([9, 9], 1).chunks(3) {
            for (a, b) in [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
                let d = [
                    (b % side) as i32 - (a % side) as i32,
                    (b / side) as i32 - (a / side) as i32,
                ];
                assert!(NEIGHBORS.contains(&d), "{d:?}");
                seen.insert(d);
            }
        }
        assert_eq!(seen.len(), NEIGHBORS.len());
    }

    #[test]
    fn the_faces_mesh_reads_the_explain_vertex_of_its_own_texel() {
        for eye in [1, 2, 4, 8] {
            let faces = faces_mesh_step(eye);
            for keep in [true, false] {
                let k = Keep::of([320, 320], eye, faces, keep).expect("a multiple");
                assert_eq!(k.ratio, faces / eye);
                assert_eq!(k.grid, [mesh_side(320, faces); 2]);
                assert_eq!(k.explain_grid, [mesh_side(320, eye); 2]);
                for x in 0..k.grid[0] {
                    let [vx, _] = k.explain_vertex(x, 0);
                    assert_eq!(mesh_texel(vx, 320, eye), mesh_texel(x, 320, faces));
                }
            }
        }
        assert!(Keep::of([320, 320], 2, 3, true).is_none());
        assert!(Keep::of([320, 320], 0, 4, true).is_none());
        assert!(Keep::of([320, 320], 4, 2, true).is_none());
        assert_close!(
            Keep::of([320, 320], 2, 4, true).unwrap().rows(),
            [[81.0, 81.0, 2.0, 1.0], [161.0, 161.0, 0.0, 0.0]]
        );
    }

    /// A 5 x 5 grid of values for layer 0 (layer 1 all without data),
    /// rows top first as written, through [`Keep::drops`] at step 1.
    fn drops_of(rows: [[f32; 5]; 5], keep: bool) -> Vec<bool> {
        let mut values: Vec<f32> = rows.iter().flatten().copied().collect();
        values.extend([0.0; 25]);
        let k = Keep {
            grid: [5, 5],
            ratio: 1,
            explain_grid: [5, 5],
            keep,
        };
        let d = k.drops(&values);
        assert!(d[25..].iter().all(|&x| !x));
        d[..25].to_vec()
    }

    #[test]
    fn the_keep_rule_keeps_an_unexplained_edge_and_drops_a_box_edge() {
        // A person (unexplained, 1 m) in front of a scanned wall
        // (explained, -3 m) on the left; the wall's vertices that share a
        // triangle with the person's stay, so the person's edge blends
        // into the wall as before; the rest of the wall goes.
        let w = -3.0;
        let person = [
            [w, w, 1.0, 1.0, 1.0],
            [w, w, 1.0, 1.0, 1.0],
            [w, w, w, 1.0, 1.0],
            [w, w, w, w, 1.0],
            [w, w, w, w, w],
        ];
        let d = drops_of(person, true);
        let at = |x: usize, y: usize| d[y * 5 + x];
        // Unexplained vertices are never dropped.
        assert!(!at(2, 0) && !at(4, 3));
        // Wall vertices that share a triangle with a person vertex stay:
        // (1, 0) with (2, 0), (1, 2) with (2, 1), (2, 3) with (3, 2),
        // (4, 4) with (4, 3).
        for (x, y) in [
            (1, 0),
            (1, 1),
            (1, 2),
            (2, 2),
            (2, 3),
            (3, 3),
            (3, 4),
            (4, 4),
        ] {
            assert!(!at(x, y), "({x}, {y}) kept");
        }
        // Those that share none go.
        for (x, y) in [
            (0, 0),
            (0, 1),
            (0, 2),
            (0, 3),
            (0, 4),
            (1, 3),
            (1, 4),
            (2, 4),
        ] {
            assert!(at(x, y), "({x}, {y}) dropped");
        }
        // Without the rule every wall vertex goes.
        let all = drops_of(person, false);
        for (i, v) in person.iter().flatten().enumerate() {
            assert_eq!(all[i], *v < 0.0, "{i}");
        }
        // A scanned table (explained, -1 m) in front of an unscanned
        // background (unexplained, 3 m): every table vertex goes, the
        // edge too, so the box's own edge is the one that shows.
        let t = -1.0;
        let table = [
            [3.0, 3.0, 3.0, 3.0, 3.0],
            [3.0, t, t, t, 3.0],
            [3.0, t, t, t, 3.0],
            [3.0, t, t, t, 3.0],
            [3.0, 3.0, 3.0, 3.0, 3.0],
        ];
        let d = drops_of(table, true);
        for (i, v) in table.iter().flatten().enumerate() {
            assert_eq!(d[i], *v < 0.0, "{i}");
        }
        // No data next to an explained vertex keeps nothing.
        let hole = [[w, 0.0, w, w, w]; 5];
        assert!(drops_of(hole, true).iter().step_by(5).all(|&x| x));
    }

    #[test]
    fn a_dropped_vertex_takes_its_triangles_with_it() {
        let proj = crate::math::projection(FOV, 0.05, 100.0);
        let clip = |p: Vec3| proj * p.extend(1.0);
        // A kept vertex 0.3 to 4 m out, a dropped one 0.3 to 4 m out: what
        // is left of the edge between them, from the kept end, is under
        // 0.02 %.
        for near in [0.3, 1.0, 4.0] {
            for far in [0.3, 1.0, 4.0] {
                let a = clip(Vec3::new(0.1, 0.0, -near));
                let c = clip(Vec3::new(0.12, 0.01, -far));
                let b = dropped_clip(c);
                assert!(b.z > b.w, "past the far plane");
                // Where it is on the screen does not move.
                assert_close!([b.x, b.y, b.w], [c.x, c.y, c.w]);
                let kept = kept_fraction(a, b);
                assert!(kept < 2e-4, "{near} {far}: {kept}");
            }
        }
        // Two kept vertices keep their edge.
        let (a, b) = (
            clip(Vec3::new(0.0, 0.0, -1.0)),
            clip(Vec3::new(0.1, 0.0, -2.0)),
        );
        assert_close!(kept_fraction(a, b), 1.0);
    }

    #[test]
    fn off_leaves_the_occluder_uniform_alone() {
        let v = view([0.0, 1.5, 0.0], Quat::IDENTITY);
        let r = EyeReprojection {
            view_proj: Mat4::IDENTITY,
            eye_pos: Vec3::ZERO,
            depth_view: v.view_matrix(),
            depth_fov: FOV,
            near: 0.1,
            far: f32::INFINITY,
            depth_size: [320, 320],
            target_size: [1680, 1760],
            layer: 1,
            near_cut_m: 0.2,
            flip_v: false,
            filter_edge_m: crate::env_depth::FILTER_SOFT_M,
            mesh_step: 2,
        };
        let rows = r.uniform();
        // The mesh row's z is 0 from the occluder: the drop is off.
        assert_eq!(rows[UNIFORM_ROWS - 1][2].to_bits(), 0);
        let mut on = rows;
        set_drop_vertices(&mut on, 161 * 161);
        assert_close!(on[UNIFORM_ROWS - 1], [2.0, 161.0, 25921.0, 0.0]);
        assert_eq!(on[..UNIFORM_ROWS - 1], rows[..UNIFORM_ROWS - 1]);
        let mut off = rows;
        set_drop_vertices(&mut off, 0);
        assert_eq!(off, rows);
        // And the knobs default to off.
        assert_eq!(GeomFirstOptions::default().mode, Mode::Off);
        assert_eq!(
            GeomFirstOptions::from_knobs(|_| None),
            GeomFirstOptions::default()
        );
    }

    #[test]
    fn the_knobs_parse_clamp_and_log() {
        let knobs = |pairs: &[(&str, &str)]| {
            let map: std::collections::HashMap<String, String> = pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect();
            GeomFirstOptions::from_knobs(move |name| map.get(name).cloned())
        };
        let on = knobs(&[(KNOB, "1")]);
        assert_eq!(on.mode, Mode::On);
        assert_eq!(knobs(&[(KNOB, "2")]).mode, Mode::DropAll);
        assert_eq!(knobs(&[(KNOB, "0")]).mode, Mode::Off);
        assert_eq!(knobs(&[(KNOB, "yes")]).mode, Mode::Off);
        let tuned = knobs(&[
            (KNOB, " 1 "),
            (KNOB_MARGIN, "2.5"),
            (KNOB_TOL, "0.06"),
            (KNOB_SHOW, "1"),
        ]);
        assert_close!(tuned.margin_texels, 2.5);
        assert_close!(tuned.tol_m, 0.06);
        assert!(tuned.show);
        let wild = knobs(&[(KNOB_MARGIN, "50"), (KNOB_TOL, "-1"), (KNOB_SHOW, "0")]);
        assert_close!(wild.margin_texels, MARGIN_MAX_TEXELS);
        assert_close!(wild.tol_m, 0.0);
        assert!(!wild.show);
        let bad = knobs(&[(KNOB_MARGIN, "nan"), (KNOB_TOL, "x")]);
        assert_close!(bad.margin_texels, MARGIN_TEXELS);
        assert_close!(bad.tol_m, TOL_M);
        // The lines name the state, the change and a run without boxes.
        let off = GeomFirstOptions::default();
        assert!(off.line(true).starts_with("geometry first: off"));
        assert!(on.line(true).contains("margin 1.5 texels"));
        assert!(on.line(false).contains("debug.fosfora.occluders 0"));
        assert!(!off.line(false).contains("occluders"));
        let change = on.change_line(&off, true);
        assert!(change.contains("geometry first: on") && change.contains("(was: off"));
    }

    fn validate(source: &str) -> naga::Module {
        let module = naga::front::wgsl::parse_str(source).expect("WGSL parses");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::default(),
        )
        .validate(&module)
        .expect("WGSL validates");
        module
    }

    fn struct_span(module: &naga::Module, name: &str) -> usize {
        let (_, ty) = module
            .types
            .iter()
            .find(|(_, t)| t.name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("struct {name}"));
        let naga::TypeInner::Struct { span, .. } = ty.inner else {
            panic!("{name} is not a struct");
        };
        span as usize
    }

    #[test]
    fn the_shaders_validate_and_match_the_rows() {
        let explain = validate(EXPLAIN_WGSL);
        assert_eq!(struct_span(&explain, "GeomParams"), PARAM_ROWS * 16);
        assert_eq!(struct_span(&explain, "Obstacles"), OBSTACLES_BYTES);
        assert_eq!(obstacle_rows(&[]).len() * 16, OBSTACLES_BYTES);
        let keep = validate(KEEP_WGSL);
        assert_eq!(struct_span(&keep, "KeepParams"), KEEP_ROWS * 16);
        let source = mesh_geom_wgsl();
        let mesh = validate(&source);
        assert_eq!(struct_span(&mesh, "EnvEye"), UNIFORM_ROWS * 16);
        for name in [
            "vs_main",
            "fs_main",
            "fs_mesh",
            "fs_mesh_show",
            "vs_mesh_geom",
            "fs_mesh_tint",
        ] {
            assert!(
                mesh.entry_points.iter().any(|e| e.name == name),
                "entry point {name}"
            );
        }
        // vs_mesh's body is mesh_vertex's, word for word: the drop is the
        // only difference from the occluder's vertex stage.
        assert_eq!(ENV_DEPTH_WGSL.matches(VS_MESH_HEAD).count(), 1);
        assert!(!source.contains("fn vs_mesh("));
        let body = |s: &str, head: &str| {
            let start = s.find(head).expect("head") + head.len();
            let end = start + s[start..].find("\n}\n").expect("end");
            s[start..end].to_owned()
        };
        assert_eq!(
            body(ENV_DEPTH_WGSL, VS_MESH_HEAD),
            body(&source, MESH_VERTEX_HEAD)
        );
        assert!(source.contains(&format!("const DROP_Z: f32 = {DROP_Z:?};")));
        for (name, module) in [("cs_main", &explain), ("cs_main", &keep)] {
            assert!(module.entry_points.iter().any(|e| e.name == name));
        }
        // The obstacle rows' layout: the box count in the header as bits,
        // a box's center, rotation and half extents in its three blocks.
        let b = table();
        let rows = obstacle_rows(&[b, b]);
        assert_eq!(rows[0][1].to_bits(), 2);
        assert_close!(rows[66], [0.0, 0.0, -2.1, 0.0]);
        assert_close!(rows[66 + 32 + 1], [0.0, 0.0, 0.0, 1.0]);
        assert_close!(rows[66 + 64], [0.5, 0.4, 0.3, 0.0]);
    }
}
