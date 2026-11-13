//! The depth light (board #5439): what the live depth map still occludes is
//! drawn as a cloud of points of light fixed in the room, and with the room
//! dark the surfaces' light behind it shows between them.
//!
//! **What is lit.** The depth occluder's mesh (`env_depth.rs`,
//! `envdepthmesh`), where it still occludes. With geometry first on
//! (`geom_first.rs`) that is exactly what no room box explains: a chair, a
//! vacuum, a pile, a person, the wearer's own legs. The eye pass's mesh
//! writes the same depth as before, so the particles are hidden behind real
//! things as they were. The hands are not part of it, since the runtime
//! removes them from the map and the hand mesh occludes them.
//!
//! **Where the points draw** ([`mesh_draws`]). In the faces layer
//! (`faces_layer.rs`) when there is one, the default with passthrough: at
//! half the eye's resolution a quarter of the eye's fragments shade them,
//! and the runtime's upsample of the layer keeps them soft. That pass then
//! draws its own mesh (twice the eye's step) with the light, after its
//! occluders. The eye pass draws its mesh for the depth and the veil of its
//! own light (the edge bars) alone, or depth alone where the veil has
//! nothing to do. Without a faces layer the eye pass draws the points.
//!
//! **The veil** ([`veil`]). The map no longer hides the surfaces' light
//! (the sky, the lit faces, the edge bars) with its coarse cutout. Those
//! draw first, tested against the room's boxes and the hands alone; the
//! mesh then erases the veil's share of them where it lies in front, and
//! adds its points. The veil is `1 - level` of the immersion: with the room
//! visible (level 0) the map hides the surfaces' light behind a real thing
//! as before and the passthrough shows the thing; at level 0.5 half of it
//! shows through, over the passthrough at half; with the room black
//! (level 1) all of it shows, and the thing is only its points. The faces
//! layer gets the same erase from its own mesh. At level 0 the veil erases
//! all, which drawing the mesh first gives without shading the surfaces
//! behind real things, so both passes do that there ([`veil_passes`]); at
//! level 1 it erases nothing, so the eye pass's mesh writes depth alone.
//! The blend erases the
//! share in color and alpha alike, so at level 0 the eye layer stays
//! transparent over the passthrough where the veil erased it. The points
//! add their light: the layer's alpha carries the erase and not the light,
//! which an "over" of the light itself would need a second output or a
//! second draw for. `depthlightveil 0` keeps the hard cutout at every
//! level: the mesh draws before the surfaces, as in the first version.
//!
//! **The points** ([`fragment`], [`plane_points`], [`points_tile`]). A
//! tile of soft points made once and uploaded as a small texture
//! ([`TILE`] texels, [`TILE_CELLS`] cells a side, a point anywhere in each
//! cell): per texel, the distance to the nearest point's center as a cone
//! that falls to 0 at [`REACH`], and that point's size, twinkle phase and
//! rank. The fragment reads it once per plane and octave at the world
//! position and makes the point there: a soft disk of the fill's radius
//! ([`point_layout`]) times its size ([`SIZE`]), swelling with the music
//! ([`SWELL`]) and shrinking with what the vertex keeps, present where its
//! rank is under the fill's share of cells, twinkling at 1, 2 or 3 times
//! the clock by its size from its phase with a triangle wave ([`wave`]),
//! no sine. Each plane and octave reads the tile from its own place
//! ([`tile_offset`]), and it repeats every 16 cells (64 cm 1 m from the
//! eye). The planes are weighted by the fourth power of the normal's
//! components, so a surface uses the plane it faces; a second plane only
//! between about 35 and 55 degrees off an axis ([`PLANE_MIN`]). The share
//! covered is `depthlightfill`, a quarter by default. A
//! cell's side is [`CELL_M`] at 1 m from the eye and doubles with each
//! octave of the distance, the next octave fading in over the last part of
//! each ([`OCTAVE_BLEND`], the last quarter), so a thing keeps its look on
//! screen near or far and the map's noise, larger far away, stays small
//! against the cells. Through the points run the bands from the first
//! version as a slow wave of light, one every [`BAND_SPACING_M`] of height,
//! drifting up, its crests tinted toward the top's color, its profile the
//! same triangle wave squared. A surface that faces up takes the
//! top's color. The colors are the theme's ([`colors`]): the color the
//! theme gives other surfaces, and the key's tint where the theme uses it
//! anywhere. Aurora draws teal and the key's tint; a room with no theme
//! other's own green.
//!
//! **Stable in time.** A point is a function of the world position alone:
//! its cell and its place in the cell come from the position projected on
//! an axis plane, so it stays put while the head moves and while the mesh's
//! vertices slide over the surface. The map's noise moves the surface along
//! the view ray, and the projection on the plane a surface faces turns that
//! into a much smaller shift: [`JITTER_TEST_M`] of noise moves a facing
//! surface's point by a few hundredths of a cell. A head move changes only
//! the octaves' blend, slowly. The normal is taken toward the neighbor
//! nearer in depth, so a step does not swing it. Where cells shrink to a
//! few pixels (a surface seen edge-on, the fringe that leans back at a
//! step) the points fade out ([`AA_CELLS_PER_PX`]) instead of sparkling.
//! Fading them into their mean instead drew a faint line of glow along
//! every step of the mesh in renders of the recorded office.
//!
//! **Masses and edges** ([`vertex_light`]). Inside a mass nothing changes
//! at a step between things: no line, no thinning. A step to a neighbor
//! larger than [`JUMP_MIN_M`] or [`JUMP_REL`] of the distance, whichever
//! is larger, parts two masses: the far side keeps no points at its edge,
//! the near side [`NEAR_KEEP`] of them, and the triangles between them
//! (the fringe that leans back) carry the near side's points shrinking to
//! nothing. So a mass ends in thinning points and the one behind it starts
//! again a cell away, with the sky between. A point's area follows what its
//! vertex keeps. Something thinner than a mesh step is all near side, so a
//! vacuum's handle carries points at [`NEAR_KEEP`] of their area.
//!
//! **Far things keep their color.** The light is all there to 1.5 m and
//! falls to [`FAR_FLOOR`] by 3 m ([`FADE_M`]), and stays there as far as
//! the map has data.
//!
//! **How bright** ([`peak_alpha`]). A point's center rises with the
//! immersion level from [`ALPHA_LOW`] with the room plain to [`ALPHA_HIGH`]
//! with it black, times `depthlightgain`, the band's wave and the twinkle.
//! At a quarter covered the light averages well under a lit surface's. The
//! crests follow the band the theme gives other surfaces ([`band`]; the rms
//! for Aurora), smoothed ([`MUSIC_ATTACK_S`], [`MUSIC_RELEASE_S`]).
//!
//! **Draw order.** With the veil letting some light through, the eye pass
//! draws the box and hand occluders, the quad, the surfaces when they are
//! in the eye pass, the highlight, the live outlines and the edge bars,
//! then the map's mesh, then the particles and everything after them; the
//! faces pass draws the occluders, its surfaces, then the mesh. Otherwise
//! a pass drawing the points draws them right after the box and hand
//! occluders and before its surfaces, and a pass drawing the mesh for its
//! depth alone draws it first, as before the light. Every occluder keeps
//! the nearest depth (compare `Less` or `LessEqual`), so the depth buffer
//! ends the same whatever the order.
//!
//! **Cost.** The first points (board #5439, measured on the Quest 3)
//! evaluated four hashed cells per plane and octave per fragment, a sine,
//! a square root and a length each, in the eye pass: about 4 ms for a pile
//! covering 15 % of the view, and the veil about 1.5 ms more. Now, per
//! vertex of the mesh that draws the points (the faces layer's, 6.6K per
//! eye at step 4; the eye's, 26K, without a faces layer), in the vertex
//! stage where the map is already read: four more texel loads, five rays,
//! two one-sided differences, a cross product, a rotation, two square roots
//! and three smoothsteps. Per fragment of that mesh: one `fwidth`, a
//! normalize, the planes' weights, a `log2` and a square root, then one
//! read of the tile (two in the last quarter of an octave or between two
//! axes, at most six) with about forty arithmetic operations each, no
//! transcendental; then the wave (a triangle, no cosine) and the blend.
//! No discard and no `frag_depth`, so early depth testing works as before.
//! The eye pass's mesh adds a constant fragment where the veil erases (0 <
//! level < 1) and nothing over the depth-only occluder otherwise. Between
//! the two levels the surfaces behind a real thing shade, where its depth
//! rejected them before.
//!
//! **Knobs**, read at launch and polled once a second
//! ([`DepthLightOptions::from_knobs`]): `debug.fosfora.depthlight 0|1`
//! (default 1; 0 is the frame as it was, with depth only, no color write
//! and the old order), `depthlightgain` (0 to [`GAIN_MAX`], default 1),
//! `depthlightfill` (the share covered, [`FILL_RANGE`], default [`FILL`]),
//! `depthlightveil 0|1` (default 1; 0 keeps the hard cutout at every
//! level) and `depthlightpoints 0|1` (default 1; 0 draws no points, the
//! mesh still drawn in the light's passes and order, to cost the points
//! apart). It needs the occluder's mesh. With the full-screen pass
//! (`envdepthmesh 0`, or `envdepthfilter 0` or 1) or the gray diagnostic
//! (`envdepthshow`) it is off, and the log says so ([`blocked`]).
//! `geomfirstshow`'s tint replaces it while on.
//!
//! **What it does not do.** It does not change what occludes the
//! particles: the depth written, geometry first's rule and the hand
//! removal are as before. It does not sharpen the map's edge; at levels
//! between 0 and 1 the veil still follows the map's coarse silhouette, in
//! the faces layer the coarser one of its own mesh. Where no box explains a
//! wall (no scan, roam, geometry first off) the wall is points too. In the
//! faces layer the points' masses and edges come from its mesh at twice the
//! step: a thing narrower than about four texels can fall between its
//! vertices and carry no points.
//!
//! Plain numbers in, so it builds and tests on the desktop:
//! [`vertex_light`] mirrors the WGSL's `mesh_light`, [`fragment`] its
//! `fs_mesh_light` and [`veil`] the blend, and `tests/depth_light_gpu.rs`
//! runs the stages on a desktop GPU against them.

use std::f32::consts::PI;

use glam::{Vec2, Vec3, Vec4};

use crate::env_depth::EyeReprojection;
use crate::surface_fx::{Audio, BAND_BASS, BAND_HIGH, BAND_MID};
use crate::surfaces::{COLOR_KEY, KIND_OTHER, SurfaceBehavior, color_name, surface_color};
use crate::themes::Theme;

/// The knobs' names.
pub const KNOB: &str = "debug.fosfora.depthlight";
pub const KNOB_GAIN: &str = "debug.fosfora.depthlightgain";
pub const KNOB_FILL: &str = "debug.fosfora.depthlightfill";
pub const KNOB_VEIL: &str = "debug.fosfora.depthlightveil";
pub const KNOB_POINTS: &str = "debug.fosfora.depthlightpoints";

/// Rows of [`rows`], the end of `struct EnvEye` from `light` on
/// (`env_depth::LIGHT_ROW`).
pub const ROWS: usize = 9;

/// The height of one band of the wave (m).
pub const BAND_SPACING_M: f32 = 0.2;
/// The light between the wave's crests, as a share of a crest's.
pub const STEADY: f32 = 0.55;
/// How far a crest tints a side toward the top's color.
pub const BAND_TINT: f32 = 0.35;
/// The mean of the crest's profile, [`wave`] squared: what the wave fades
/// into where a band spans few pixels (13/35).
pub const CREST_MEAN: f32 = 13.0 / 35.0;
/// Bands per pixel where the wave starts to fade into its mean and where
/// it is gone: six pixels a band and two.
pub const AA_BANDS_PER_PX: [f32; 2] = [1.0 / 6.0, 0.5];
/// The distance from the eye (m) where the light starts to fall and where
/// it reaches its floor.
pub const FADE_M: [f32; 2] = [1.5, 3.0];
/// The light past the fade, as a share of a near thing's: a far shelf is a
/// dim constellation, never black.
pub const FAR_FLOOR: f32 = 0.35;
/// The smallest step in depth to a neighbor (m) that parts two masses, and
/// the share of the vertex's distance that does when that is larger: half
/// a meter to 2 m, a quarter of the distance past it.
pub const JUMP_MIN_M: f32 = 0.5;
pub const JUMP_REL: f32 = 0.25;
/// Where the thinning at a mass's edge starts, as a share of that step.
pub const JUMP_SOFT: f32 = 0.6;
/// What the near side of a mass's edge keeps of its points' area.
pub const NEAR_KEEP: f32 = 0.5;
/// A cell's side 1 m from the eye (m): about 30 pixels of the Quest 3's
/// eye buffer.
pub const CELL_M: f32 = 0.04;
/// The share of an octave of the distance where the next octave's cells
/// start to fade in: the last quarter of each reads the tile twice.
pub const OCTAVE_BLEND: f32 = 0.75;
/// The octaves' range around 1 m, as in the WGSL: cells from 1/16 to 16
/// times [`CELL_M`].
pub const OCTAVE_RANGE: [f32; 2] = [-4.0, 3.999];
/// Cells per pixel where the points start to fade out and where they are
/// gone: under about 7 and 2.5 pixels a cell. They fade to nothing rather
/// than to their mean, so the fringe at a step reads as sky, not a line.
pub const AA_CELLS_PER_PX: [f32; 2] = [0.15, 0.4];
/// Taken off each plane's share of the points, so a plane the surface
/// barely faces fades out rather than drops; one left with none is skipped
/// and the others shared out again, as in the WGSL. A surface reads a
/// second plane only between about 35 and 55 degrees off an axis.
pub const PLANE_MIN: f32 = 0.12;
/// A point's radius as factors of the base, uniformly between them.
pub const SIZE: [f32; 2] = [0.7, 1.3];
/// Where a point's soft edge starts, as a share of its radius.
pub const SOFT_CORE: f32 = 0.35;
/// The music's swell: the radius grows by up to this share.
pub const SWELL: f32 = 0.2;
/// The points' tile ([`points_tile`]): texels a side, and cells a side in
/// it, one point in each anywhere within it, the tile repeating every
/// [`TILE_CELLS`] cells (64 cm 1 m from the eye).
pub const TILE: u32 = 256;
pub const TILE_CELLS: u32 = 16;
/// The farthest a point's light reaches from its center (cells), the
/// tile's R at 0: a radius stays within it.
pub const REACH: f32 = 0.5;
/// The share covered by points (`depthlightfill`), and the knob's range:
/// the largest point must stay within half a cell, which caps it.
pub const FILL: f32 = 0.25;
pub const FILL_RANGE: [f32; 2] = [0.05, 0.33];
/// The smallest base radius (cells): a lower fill puts a point in fewer
/// cells rather than shrinking them.
pub const RADIUS_MIN: f32 = 0.16;
/// The twinkle's depth (the share of a point's light it swings) and its
/// rate in silence (cycles a second, times 1, 2 or 3 by point).
pub const TWINKLE_DEPTH: f32 = 0.5;
pub const TWINKLE_HZ: f32 = 0.25;
/// A point's peak alpha with the room plain (immersion level 0) and with
/// it black.
pub const ALPHA_LOW: f32 = 0.06;
pub const ALPHA_HIGH: f32 = 0.8;
/// `depthlightgain`'s range.
pub const GAIN_MAX: f32 = 4.0;
/// The crests' light in silence, as a share of a loud moment's.
pub const MUSIC_FLOOR: f32 = 0.4;
/// The music's smoothing: the time constants (s) while the band's level
/// rises and while it falls.
pub const MUSIC_ATTACK_S: f32 = 0.05;
pub const MUSIC_RELEASE_S: f32 = 0.3;
/// The wave's drift (bands a second, up) in silence and at a full band;
/// the beat envelope adds up to as much again, as it speeds the surfaces'
/// clock.
pub const DRIFT_BANDS_S: [f32; 2] = [0.03, 0.15];
/// The map noise the stability test applies (m), the recordings' near
/// noise.
pub const JITTER_TEST_M: f32 = 0.004;

fn unit(x: f32) -> f32 {
    if x.is_finite() {
        x.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// WGSL's `smoothstep`.
pub fn smoothstep(low: f32, high: f32, x: f32) -> f32 {
    let t = ((x - low) / (high - low)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// What the knobs ask.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DepthLightOptions {
    /// `debug.fosfora.depthlight`.
    pub on: bool,
    /// `debug.fosfora.depthlightgain`: multiplies the peak alpha.
    pub gain: f32,
    /// `debug.fosfora.depthlightfill`: the share covered by points.
    pub fill: f32,
    /// `debug.fosfora.depthlightveil`: the surfaces' light shows through
    /// with the room dark (on), or the hard cutout at every level (off).
    pub veil: bool,
    /// `debug.fosfora.depthlightpoints`: the points drawn (on), or none,
    /// the mesh still drawn with the veil and in its order (off): the
    /// points' cost apart, for a cost run.
    pub points: bool,
}

impl Default for DepthLightOptions {
    fn default() -> Self {
        Self {
            on: true,
            gain: 1.0,
            fill: FILL,
            veil: true,
            points: true,
        }
    }
}

impl DepthLightOptions {
    /// The options the five knobs ask, each read through `prop` (a system
    /// property by name, `None` unset): `depthlight` and `depthlightveil`
    /// off only at `0`; an unreadable number is the default, one out of
    /// range the nearest end. `depthlightpoints` off only at `0` too.
    pub fn from_knobs(prop: impl Fn(&str) -> Option<String>) -> Self {
        let d = Self::default();
        let number = |name: &str| {
            prop(name)
                .and_then(|v| v.trim().parse::<f32>().ok())
                .filter(|v| v.is_finite())
        };
        let off = |name: &str| prop(name).as_deref().map(str::trim) == Some("0");
        Self {
            on: !off(KNOB),
            gain: number(KNOB_GAIN).map_or(d.gain, |v| v.clamp(0.0, GAIN_MAX)),
            fill: number(KNOB_FILL).map_or(d.fill, |v| v.clamp(FILL_RANGE[0], FILL_RANGE[1])),
            veil: !off(KNOB_VEIL),
            points: !off(KNOB_POINTS),
        }
    }

    /// Whether the light draws: asked for and not `blocked`.
    pub fn lit(&self, blocked: Option<&str>) -> bool {
        self.on && blocked.is_none()
    }

    /// The state in a few words, for the log.
    pub fn describe(&self) -> String {
        if !self.on {
            return "off (debug.fosfora.depthlight 0): the depth map writes depth only".to_owned();
        }
        format!(
            "on, points · gain {} · fill {} · {} · a point's peak alpha {ALPHA_LOW} with the room plain to {ALPHA_HIGH} black · cells {CELL_M} m at 1 m, doubling each octave · the light falls to {FAR_FLOOR} from {} to {} m · two masses part at {JUMP_MIN_M} m or {JUMP_REL} of the distance",
            self.gain,
            self.fill,
            match (self.veil, self.points) {
                (true, true) => "veil on: the surfaces' light shows through by the immersion level",
                (false, true) =>
                    "veil off (debug.fosfora.depthlightveil 0): the hard cutout at every level",
                (true, false) => "no points (debug.fosfora.depthlightpoints 0), the veil alone",
                (false, false) =>
                    "no points (debug.fosfora.depthlightpoints 0) and the hard cutout (debug.fosfora.depthlightveil 0)",
            },
            FADE_M[0],
            FADE_M[1],
        )
    }

    /// The launch line; `blocked` says why the light cannot draw in this
    /// run ([`blocked`]).
    pub fn line(&self, blocked: Option<&str>) -> String {
        match blocked {
            Some(why) if self.on => format!("depth light: off, {why} (asked: {})", self.describe()),
            _ => format!("depth light: {}", self.describe()),
        }
    }

    /// The line for a change from `before`.
    pub fn change_line(&self, before: &Self, blocked: Option<&str>) -> String {
        format!("{} (was: {})", self.line(blocked), before.describe())
    }
}

/// Why the light cannot draw in a run whose environment depth occludes
/// (`occlude`), shows the gray diagnostic (`show`) and draws the eye pass's
/// mesh at `mesh_step` (`EnvDepthOptions::drawn_mesh_step`, 0 the
/// full-screen pass); `None` when it can.
pub fn blocked(occlude: bool, show: bool, mesh_step: u32) -> Option<&'static str> {
    if show {
        Some("the diagnostic gray draws the depth map (debug.fosfora.envdepthshow 1)")
    } else if !occlude {
        Some("the depth map does not occlude in this run (debug.fosfora.envdepth)")
    } else if mesh_step == 0 {
        Some(
            "it needs the occluder mesh, and the full-screen pass draws (envdepthmesh 0, or envdepthfilter 0 or 1)",
        )
    } else {
        None
    }
}

/// A point's peak alpha at immersion level `level` (0..1) and gain `gain`:
/// from [`ALPHA_LOW`] at level 0 to [`ALPHA_HIGH`] at 1, times the gain.
pub fn peak_alpha(level: f32, gain: f32) -> f32 {
    let gain = if gain.is_finite() {
        gain.clamp(0.0, GAIN_MAX)
    } else {
        0.0
    };
    (ALPHA_LOW + (ALPHA_HIGH - ALPHA_LOW) * unit(level)) * gain
}

/// The veil at immersion level `level`: the share of the surfaces' light
/// behind the map that the map erases, `1 - level`; 1 at every level with
/// the veil off (`on` false), which with the mesh drawn first changes
/// nothing but its own folds.
pub fn veil_share(level: f32, on: bool) -> f32 {
    if on { 1.0 - unit(level) } else { 1.0 }
}

/// The veil's share within which it counts as none or all (`veil_passes`).
pub const VEIL_EPS: f32 = 1e-3;

/// How the passes draw the mesh this frame with the veil asked for (`on`)
/// at share `v` ([`veil_share`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VeilPasses {
    /// The surfaces draw before the mesh, which erases `v` of them. Not at
    /// `v` 1 (the room plain): the mesh first, as without the veil, gives
    /// the same frame without shading the surfaces behind real things.
    pub surfaces_first: bool,
    /// The faces pass draws the mesh: first and depth only without the
    /// veil's order, after its surfaces with the veil alone in it, but not
    /// at `v` 0 (the room black), where there is nothing to erase.
    pub faces_mesh: bool,
}

/// [`VeilPasses`] for the veil asked for (`on`) at share `v`.
pub fn veil_passes(on: bool, v: f32) -> VeilPasses {
    let surfaces_first = on && v < 1.0 - VEIL_EPS;
    VeilPasses {
        surfaces_first,
        faces_mesh: !surfaces_first || v > VEIL_EPS,
    }
}

/// What a pass draws the mesh with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeshDraw {
    /// Depth alone, color masked off: the occluder as before the light.
    Depth,
    /// Depth and the veil alone (`fs_mesh_veil`): the veil's share of the
    /// light behind erased, nothing added.
    Veil,
    /// Depth, the veil and the points (`fs_mesh_light`).
    Light,
}

/// How the eye pass and the faces pass draw the mesh in a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeshDraws {
    pub eye: MeshDraw,
    /// `None`: the faces pass draws no mesh.
    pub faces: Option<MeshDraw>,
    /// Both passes draw their surfaces before the mesh.
    pub surfaces_first: bool,
}

/// How the passes draw the mesh with the light on or not (`lit`), the veil
/// asked for (`veil_on`) at share `v`, and with a faces layer this frame
/// (`faces_layer`). With a faces layer the points go there: at half the
/// eye's resolution a quarter of the eye's fragments shade them, and the
/// runtime's upsample of the layer keeps them soft. The eye pass then
/// draws the mesh for its depth (the particles) and the veil of its own
/// light (the edge bars): with the veil alone while it lets some light
/// through and erases some, depth alone otherwise. Without a faces layer
/// the eye pass draws the points. Without the light, as before: depth
/// alone, first.
pub fn mesh_draws(lit: bool, veil_on: bool, v: f32, faces_layer: bool) -> MeshDraws {
    if !lit {
        return MeshDraws {
            eye: MeshDraw::Depth,
            faces: Some(MeshDraw::Depth),
            surfaces_first: false,
        };
    }
    let passes = veil_passes(veil_on, v);
    if faces_layer {
        MeshDraws {
            eye: if passes.surfaces_first && v > VEIL_EPS {
                MeshDraw::Veil
            } else {
                MeshDraw::Depth
            },
            faces: Some(MeshDraw::Light),
            surfaces_first: passes.surfaces_first,
        }
    } else {
        MeshDraws {
            eye: MeshDraw::Light,
            faces: None,
            surfaces_first: passes.surfaces_first,
        }
    }
}

/// What the blend leaves in a pixel holding `dst` (premultiplied RGBA)
/// when the mesh's fragment writes `src`, whose alpha is the veil: the
/// veil's share of `dst` erased in color and alpha, `src`'s light added
/// (rgb One, OneMinusSrcAlpha; alpha Zero, OneMinusSrcAlpha). The faces
/// pass's veil is the same with no light.
pub fn veil(dst: [f32; 4], src: [f32; 4]) -> [f32; 4] {
    let keep = 1.0 - src[3];
    [
        src[0] + dst[0] * keep,
        src[1] + dst[1] * keep,
        src[2] + dst[2] * keep,
        dst[3] * keep,
    ]
}

/// Whether `theme` gives any kind of surface the key's tint.
pub fn uses_key(theme: Option<Theme>) -> bool {
    theme.is_some_and(|t| t.looks().any(|(_, l)| l.color == COLOR_KEY))
}

/// The color index the light's sides take under `theme`: the one it gives
/// other surfaces; 0 (other's own) with no theme.
pub fn side_index(theme: Option<Theme>) -> u32 {
    theme
        .and_then(|t| t.look(KIND_OTHER))
        .map_or(0, |l| l.color)
}

/// The sides' and the tops' colors (linear RGB) under `theme` while the
/// key's tint is `key`: the sides the color the theme gives other
/// surfaces, the tops the key's tint where the theme uses it anywhere,
/// else the sides' color. With no theme, other's own color on both.
pub fn colors(theme: Option<Theme>, key: [f32; 3]) -> [[f32; 3]; 2] {
    let side = surface_color(side_index(theme), KIND_OTHER, key);
    let top = if uses_key(theme) { key } else { side };
    [side, top]
}

/// The colors in words, for the log: "teal sides, the key's tint on top
/// (theme aurora)".
pub fn colors_line(theme: Option<Theme>) -> String {
    let side = color_name(side_index(theme));
    let side = if side == "kind" { "other's own" } else { side };
    let top = if uses_key(theme) {
        "the key's tint on top".to_owned()
    } else {
        "the same on top".to_owned()
    };
    let name = theme.map_or_else(
        || "no theme saved".to_owned(),
        |t| format!("theme {}", t.name()),
    );
    format!("depth light: {side} sides, {top} ({name})")
}

/// The audio band (`surface_fx::BAND_*`) the crests and the swell follow
/// under `theme`: the one it gives other surfaces; with no theme, other's
/// default behavior's.
pub fn band(theme: Option<Theme>) -> u32 {
    theme.and_then(|t| t.look(KIND_OTHER)).map_or_else(
        || SurfaceBehavior::default_for(KIND_OTHER).default_band(),
        |l| l.band,
    )
}

/// `band`'s level in `audio`, 0..1, as the surfaces pass reads it.
pub fn band_level(audio: &Audio, band: u32) -> f32 {
    unit(match band {
        BAND_BASS => audio.bass,
        BAND_MID => audio.mid,
        BAND_HIGH => audio.high,
        _ => audio.rms,
    })
}

/// The music the light follows, the wave's phase and the twinkle's clock,
/// stepped once a frame.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Drive {
    music: f32,
    phase: f32,
    twinkle: f32,
}

impl Drive {
    /// Advance by `dt` seconds with the band's level `level` and the beat
    /// envelope `beat` (both 0..1): the music one-pole toward the level
    /// ([`MUSIC_ATTACK_S`] rising, [`MUSIC_RELEASE_S`] falling); the wave's
    /// phase on by [`DRIFT_BANDS_S`] at that music and the twinkle's clock
    /// by [`TWINKLE_HZ`] times one plus the music, both times one plus the
    /// beat and wrapped to a whole cycle.
    pub fn step(&mut self, dt: f32, level: f32, beat: f32) {
        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
        let level = unit(level);
        let tau = if level > self.music {
            MUSIC_ATTACK_S
        } else {
            MUSIC_RELEASE_S
        };
        self.music += (level - self.music) * (1.0 - (-dt / tau).exp());
        let beat = 1.0 + unit(beat);
        let speed = DRIFT_BANDS_S[0] + (DRIFT_BANDS_S[1] - DRIFT_BANDS_S[0]) * self.music;
        self.phase = (self.phase + dt * speed * beat).fract();
        self.twinkle = (self.twinkle + dt * TWINKLE_HZ * (1.0 + self.music) * beat).fract();
    }

    /// The smoothed music, 0..1.
    pub fn music(&self) -> f32 {
        self.music
    }

    /// The wave's phase, 0..1 of a band.
    pub fn phase(&self) -> f32 {
        self.phase
    }

    /// The twinkle's clock, 0..1 of a cycle.
    pub fn twinkle(&self) -> f32 {
        self.twinkle
    }
}

/// The base radius (cells) and the share of cells with a point for a fill
/// of `fill` (the share covered): the radius that covers it with a point in
/// every cell, but at least [`RADIUS_MIN`] (then fewer cells hold one) and
/// at most what keeps the largest swollen point within half a cell.
pub fn point_layout(fill: f32) -> (f32, f32) {
    let fill = if fill.is_finite() {
        fill.clamp(FILL_RANGE[0], FILL_RANGE[1])
    } else {
        FILL
    };
    let area = PI * size_square_mean();
    let largest = 0.5 / (SIZE[1] * (1.0 + SWELL));
    let radius = (fill / area).sqrt().clamp(RADIUS_MIN, largest);
    (radius, (fill / (area * radius * radius)).min(1.0))
}

/// The mean of a point's size factor squared ([`SIZE`], uniform).
pub fn size_square_mean() -> f32 {
    let [a, b] = SIZE;
    (b * b * b - a * a * a) / (3.0 * (b - a))
}

/// A soft point's light over its disk, as a share of the disk's area: 1
/// inside `soft` of the radius, a smoothstep down to 0 at the radius.
pub fn profile_share(soft: f32) -> f32 {
    soft + 0.3 * (1.0 - soft) * (1.0 - soft)
}

/// This frame's rows and whether the veil is on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    pub rows: [[f32; 4]; ROWS],
    /// The surfaces draw before the mesh and show through it by the level.
    pub veil: bool,
}

/// The light's rows of `struct EnvEye` (from `light` on, written at
/// `env_depth::LIGHT_ROW` of both passes' uniforms) for `drive`, the peak
/// alpha `peak` ([`peak_alpha`]), the sides' and tops' colors
/// ([`colors`]), the fill and the veil's share ([`veil_share`]).
pub fn rows(
    drive: &Drive,
    peak: f32,
    [side, top]: [[f32; 3]; 2],
    fill: f32,
    veil: f32,
) -> [[f32; 4]; ROWS] {
    let music = unit(drive.music);
    let (radius, presence) = point_layout(fill);
    [
        [
            1.0 / BAND_SPACING_M,
            drive.phase,
            peak,
            MUSIC_FLOOR + (1.0 - MUSIC_FLOOR) * music,
        ],
        [side[0], side[1], side[2], STEADY],
        [top[0], top[1], top[2], BAND_TINT],
        [
            AA_BANDS_PER_PX[0],
            AA_BANDS_PER_PX[1],
            CREST_MEAN,
            drive.twinkle,
        ],
        [FADE_M[0], FADE_M[1], FAR_FLOOR, unit(veil)],
        [JUMP_MIN_M, JUMP_REL, JUMP_SOFT, NEAR_KEEP],
        [CELL_M, OCTAVE_BLEND, AA_CELLS_PER_PX[0], AA_CELLS_PER_PX[1]],
        [radius, presence, REACH, 1.0 + SWELL * music],
        [SIZE[0], SIZE[1], SOFT_CORE, TWINKLE_DEPTH],
    ]
}

/// `rows` for a target whose pixels are `scale` times the eye's in size
/// (the faces layer: 2): the points' and the wave's fades start and end at
/// `scale` times the cells and bands per pixel, so they fade at the same
/// size on the eye's screen.
pub fn scale_fades(mut rows: [[f32; 4]; ROWS], scale: f32) -> [[f32; 4]; ROWS] {
    for (row, col) in [(3, 0), (3, 1), (6, 2), (6, 3)] {
        rows[row][col] *= scale;
    }
    rows
}

/// This frame's [`Frame`] under the options `opts`, with `drive` stepped,
/// the room's `theme`, the key's tint `key` and the immersion `level`.
pub fn frame(
    opts: &DepthLightOptions,
    drive: &Drive,
    theme: Option<Theme>,
    key: [f32; 3],
    level: f32,
) -> Frame {
    Frame {
        rows: rows(
            drive,
            if opts.points {
                peak_alpha(level, opts.gain)
            } else {
                0.0
            },
            colors(theme, key),
            opts.fill,
            veil_share(level, opts.veil),
        ),
        veil: opts.veil,
    }
}

/// What the vertex stage hands on for one vertex of the mesh.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VertexLight {
    /// The clip position, the occluder's own (`EyeReprojection::
    /// vertex_clip`).
    pub clip: Vec4,
    /// The world point.
    pub world: Vec3,
    /// How much of its points the vertex keeps, 0..1.
    pub keep: f32,
    /// The surface's normal turned toward the eye (unit).
    pub normal: Vec3,
    /// The light left at the vertex's distance, 0..1.
    pub bright: f32,
}

/// The vertex stage of the light (`mesh_light` in
/// `env_depth::ENV_DEPTH_WGSL`) for the vertex of `texel` in `r`'s mesh
/// (`r.mesh_step`), with the light's `rows` and the map's stored values
/// through `stored`. The normal: one-sided differences one mesh step along
/// each axis, toward the neighbor nearer in depth (a neighbor clamped to
/// the vertex at the map's border, or without data, does not count), the
/// way to the eye without one along an axis. What the vertex keeps: the
/// largest step to a nearer neighbor thins it to none and to a farther one
/// to [`NEAR_KEEP`], from [`JUMP_SOFT`] of a mass's step ([`JUMP_MIN_M`],
/// [`JUMP_REL`]) to all of it. The light by distance, all of it to
/// [`FADE_M`]'s start and [`FAR_FLOOR`] past its end. Nothing without data.
pub fn vertex_light(
    r: &EyeReprojection,
    rows: &[[f32; 4]; ROWS],
    texel: [u32; 2],
    stored: impl Fn([u32; 2]) -> f32,
) -> VertexLight {
    let dist = r.texel_distance(stored(texel));
    let p = r.mesh_point(texel, dist);
    let clip = r.vertex_clip(texel, stored(texel));
    let s = r.mesh_step;
    let last = [
        r.depth_size[0].saturating_sub(1),
        r.depth_size[1].saturating_sub(1),
    ];
    let [x, y] = texel;
    let neighbors = [
        [x.saturating_sub(s), y],
        [(x + s).min(last[0]), y],
        [x, y.saturating_sub(s)],
        [x, (y + s).min(last[1])],
    ];
    let inside = [x > 0, x < last[0], y > 0, y < last[1]];
    let d = neighbors.map(|t| r.texel_distance(stored(t)));
    let has: [bool; 4] = std::array::from_fn(|k| d[k].is_some() && inside[k]);
    let to_eye = r.eye_pos - p;
    let range = to_eye.length();
    let mut n = to_eye;
    let (mut keep, mut bright) = (0.0, 0.0);
    if let Some(d0) = dist.filter(|_| range > 0.0) {
        let ray = |t: [u32; 2]| r.depth_ray(r.mesh_uv(t));
        let dv = |k: usize| d[k].unwrap_or(-1.0);
        let gap = |k: usize| {
            if has[k] { (dv(k) - d0).abs() } else { 1e30 }
        };
        let q = ray(texel) * d0;
        let side = |a: usize, b: usize| {
            if gap(b) <= gap(a) {
                ray(neighbors[b]) * dv(b) - q
            } else {
                q - ray(neighbors[a]) * dv(a)
            }
        };
        let g = r
            .depth_view
            .inverse()
            .transform_vector3(side(0, 1).cross(side(2, 3)));
        if (has[0] || has[1]) && (has[2] || has[3]) && g.dot(g) > 0.0 {
            n = g;
        }
        let largest = |f: &dyn Fn(f32) -> f32| {
            (0..4)
                .filter(|&k| has[k])
                .map(|k| f(dv(k)))
                .fold(0.0f32, f32::max)
        };
        let nearer = largest(&|dk| d0 - dk);
        let farther = largest(&|dk| dk - d0);
        let [jump, fade] = [rows[5], rows[4]];
        let part = jump[0].max(jump[1] * d0);
        let from = jump[2] * part;
        keep = (1.0 - smoothstep(from, part, nearer))
            * (1.0 - (1.0 - jump[3]) * smoothstep(from, part, farther));
        bright = 1.0 + (fade[2] - 1.0) * smoothstep(fade[0], fade[1], range);
    }
    if n.dot(to_eye) < 0.0 {
        n = -n;
    }
    let len = n.length();
    VertexLight {
        clip,
        world: p,
        keep,
        normal: if len > 0.0 { n / len } else { Vec3::Z },
        bright,
    }
}

/// One PCG step, the points' tile's hash.
pub fn hash(v: u32) -> u32 {
    let s = v.wrapping_mul(747_796_405).wrapping_add(2_891_336_453);
    let w = ((s >> ((s >> 28) + 4)) ^ s).wrapping_mul(277_803_737);
    (w >> 22) ^ w
}

/// Coordinates in the cells' plane `k`: across x (0), y (1) or z (2).
pub fn light_plane(v: Vec3, k: u32) -> Vec2 {
    match k {
        0 => Vec2::new(v.y, v.z),
        1 => Vec2::new(v.x, v.z),
        _ => Vec2::new(v.x, v.y),
    }
}

/// Where the tile is read for plane `k` at octave `o`, as an offset of
/// whole tiles' share (the WGSL's `light_offset`): each plane and octave
/// reads it from its own place, so neighbors do not repeat each other.
pub fn tile_offset(k: u32, o: i32) -> Vec2 {
    let (k, o) = (k as f32, o as f32);
    Vec2::new(0.31 * k + 0.17 * o, 0.57 * k + 0.29 * o)
}

/// Cell `c` of the tile's point, from the cell's hash: its center (cells,
/// anywhere in the cell), its size (0..1 over [`SIZE`]), its twinkle's
/// phase and its rank (the fill keeps the points ranked under the share of
/// cells it asks for).
pub fn tile_point(c: [u32; 2]) -> (Vec2, f32, f32, f32) {
    let a = hash(c[0] + TILE_CELLS * c[1] + 0x5bd1_e995);
    let b = hash(a);
    let unit = |v: u32| (v & 65535) as f32 / 65536.0;
    let center = Vec2::new(c[0] as f32 + unit(a), c[1] as f32 + unit(a >> 16));
    (center, unit(b), unit(b >> 16), unit(hash(b)))
}

/// The tile of soft points the fragment samples ([`TILE`] texels a side,
/// [`TILE_CELLS`] cells, a point in each, wrapping), made once: per texel,
/// RGBA8, R `1 - d / REACH` for `d` the distance (cells) to the nearest
/// point's center, 0 past the reach; G that point's twinkle phase; B its
/// size; A its rank. Row-major, row 0 first.
pub fn points_tile() -> Vec<[u8; 4]> {
    let per = TILE / TILE_CELLS;
    let n = TILE_CELLS as i32;
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    let mut out = Vec::with_capacity((TILE * TILE) as usize);
    for y in 0..TILE {
        for x in 0..TILE {
            let g = Vec2::new((x as f32 + 0.5) / per as f32, (y as f32 + 0.5) / per as f32);
            let (gx, gy) = ((x / per) as i32, (y / per) as i32);
            let mut best = (f32::INFINITY, 0.0, 0.0, 0.0);
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let (cx, cy) = (gx + dx, gy + dy);
                    let wrap = [cx.rem_euclid(n) as u32, cy.rem_euclid(n) as u32];
                    let (center, phase, size, rank) = tile_point(wrap);
                    let shift =
                        Vec2::new((cx - wrap[0] as i32) as f32, (cy - wrap[1] as i32) as f32);
                    let d = (g - center - shift).length();
                    if d < best.0 {
                        best = (d, phase, size, rank);
                    }
                }
            }
            let (d, phase, size, rank) = best;
            out.push([byte(1.0 - d / REACH), byte(phase), byte(size), byte(rank)]);
        }
    }
    out
}

/// The tile read at `uv` (tiles; it repeats) as the GPU's linear filter
/// reads RGBA8 at level 0: the four texels around, by their centers.
pub fn sample_tile(tile: &[[u8; 4]], uv: Vec2) -> Vec4 {
    let t = uv * TILE as f32 - 0.5;
    let base = t.floor();
    let f = t - base;
    let at = |dx: i32, dy: i32| {
        let x = (base.x as i32 + dx).rem_euclid(TILE as i32) as usize;
        let y = (base.y as i32 + dy).rem_euclid(TILE as i32) as usize;
        let [r, g, b, a] = tile[y * TILE as usize + x];
        Vec4::new(r as f32, g as f32, b as f32, a as f32) / 255.0
    };
    let top = at(0, 0).lerp(at(1, 0), f.x);
    let bottom = at(0, 1).lerp(at(1, 1), f.x);
    top.lerp(bottom, f.y)
}

/// A smooth wave with period 1, 1 at whole numbers and 0 halfway: a
/// triangle eased by a smoothstep, the light's cheap stand-in for a
/// cosine (the WGSL's `light_wave`).
pub fn wave(x: f32) -> f32 {
    let t = ((x - x.floor()) * 2.0 - 1.0).abs();
    t * t * (3.0 - 2.0 * t)
}

/// The points of plane `k` at octave `o` at `g` (cells), 0..1, from one
/// read of the tile: the nearest point's disk at its size, the fill's
/// radius ([`point_layout`]), the music's swell and the vertex's keep
/// (`radius_scale`, all but the size), soft from [`SOFT_CORE`] of the
/// radius; none where the point's rank is past the fill's share of cells;
/// twinkling at 1, 2 or 3 times the clock by its size, from its own phase.
/// The WGSL's `plane_points`.
pub fn plane_points(
    rows: &[[f32; 4]; ROWS],
    tile: &[[u8; 4]],
    g: Vec2,
    k: u32,
    o: i32,
    radius_scale: f32,
) -> f32 {
    let [wave_row, point, size_row] = [rows[3], rows[7], rows[8]];
    let s = sample_tile(tile, g / TILE_CELLS as f32 + tile_offset(k, o));
    let d = (1.0 - s.x) * point[2];
    let radius = radius_scale * (size_row[0] + (size_row[1] - size_row[0]) * s.z);
    if s.w >= point[1] || radius <= 1e-4 {
        return 0.0;
    }
    let core = 1.0 - smoothstep(radius * size_row[2], radius, d);
    let rate = 1.0 + (s.z * 2.999).floor();
    let twinkle = 1.0 - size_row[3] + size_row[3] * wave(rate * wave_row[3] + s.y);
    core * twinkle
}

/// The interpolated normal made unit, +Z where it has no length (two
/// opposite normals across a fold), as the WGSL does.
pub fn unit_normal(n: Vec3) -> Vec3 {
    if n.dot(n) > 1e-12 {
        n.normalize()
    } else {
        Vec3::Z
    }
}

/// The points' light at world point `p` seen from `eye` with the
/// interpolated normal `normal` and keep `keep`, where the world moves by
/// `fw` across a pixel (`fwidth`), before the peak, the distance and the
/// wave: the planes and octaves of the WGSL's fragment, one read of the
/// tile each.
pub fn points(
    rows: &[[f32; 4]; ROWS],
    tile: &[[u8; 4]],
    eye: Vec3,
    p: Vec3,
    normal: Vec3,
    keep: f32,
    fw: Vec3,
) -> f32 {
    let [cell_row, point] = [rows[6], rows[7]];
    let n = unit_normal(normal);
    let n2 = n * n;
    let tri = n2 * n2 / n2.dot(n2);
    let l = (p - eye)
        .length()
        .max(1e-3)
        .log2()
        .clamp(OCTAVE_RANGE[0], OCTAVE_RANGE[1]);
    let li = l.floor();
    let next = smoothstep(cell_row[1], 1.0, l - li);
    let radius_scale = point[0] * point[3] * keep.max(0.0).sqrt();
    let (mut sum, mut used) = (0.0, 0.0);
    for j in 0..2 {
        let wo = if j == 1 { next } else { 1.0 - next };
        if wo <= 0.0 {
            continue;
        }
        let o = li as i32 + j;
        let cell = cell_row[0] * 2f32.powi(o);
        for k in 0..3u32 {
            let wk = tri[k as usize] - PLANE_MIN;
            if wk <= 0.0 {
                continue;
            }
            let f = light_plane(fw, k);
            let aa = 1.0 - smoothstep(cell_row[2], cell_row[3], f.x.max(f.y) / cell);
            if aa > 0.0 {
                sum += wo
                    * wk
                    * aa
                    * plane_points(rows, tile, light_plane(p, k) / cell, k, o, radius_scale);
            }
            used += wo * wk;
        }
    }
    sum / used.max(1e-6)
}

/// The wave's term at height `y` (m) with the light's `rows`, before the
/// music and its fade: the crest's profile, 1 on a crest and 0 midway
/// between two, [`wave`] squared. A function of the height alone.
pub fn crest(rows: &[[f32; 4]; ROWS], y: f32) -> f32 {
    let c = wave(y * rows[0][0] - rows[0][1]);
    c * c
}

/// The fragment stage of the light (`fs_mesh_light`) at world point `p`
/// seen from `eye`, with the interpolated normal `normal`, keep `keep` and
/// light by distance `bright`, where the world moves by `fw` across a
/// pixel: premultiplied light in rgb and the veil in alpha, which the blend
/// ([`veil`]) erases by.
pub fn fragment(
    rows: &[[f32; 4]; ROWS],
    tile: &[[u8; 4]],
    eye: Vec3,
    p: Vec3,
    normal: Vec3,
    keep: f32,
    bright: f32,
    fw: Vec3,
) -> [f32; 4] {
    let [light, side, top, wave, fade, ..] = *rows;
    let weight = light[2] * bright;
    let (mut lit, mut up) = (0.0, 0.0);
    if keep > 0.0 && weight > 0.0 {
        up = unit_normal(normal).y.clamp(0.0, 1.0);
        lit = points(rows, tile, eye, p, normal, keep, fw);
    }
    let fade_wave = 1.0 - smoothstep(wave[0], wave[1], fw.y * light[0]);
    let crest = wave[2] + (crest(rows, p.y) - wave[2]) * fade_wave;
    let band = side[3] + (1.0 - side[3]) * crest * light[3];
    let tint = (up + top[3] * crest).clamp(0.0, 1.0);
    let a = (weight * band * lit).clamp(0.0, 1.0);
    let rgb = |k: usize| (side[k] + (top[k] - side[k]) * tint) * a;
    [rgb(0), rgb(1), rgb(2), fade[3]]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env_depth::{
        DepthView, ENV_DEPTH_WGSL, FILTER_SOFT_M, LIGHT_ROW, NEAR_CUT_M, UNIFORM_ROWS, mesh_side,
        mesh_texel,
    };
    use crate::math::{self, Fov};
    use crate::surfaces::{GREEN, TEAL};
    use glam::Quat;
    use std::f32::consts::TAU;

    /// The Quest 3's depth fov (v207).
    const FOV: Fov = Fov {
        left: -0.942,
        right: 0.698,
        up: 0.768,
        down: -0.960,
    };
    const NEAR: f32 = 0.1;
    const MAP: u32 = 320;
    const STEP: u32 = 2;

    /// An eye and its depth camera at `pos`, turned by `rot`, the eye's
    /// view the depth camera's (the parallax between them is millimeters).
    fn eye_at(pos: Vec3, rot: Quat) -> EyeReprojection {
        let depth = DepthView {
            orientation: rot.to_array(),
            position: pos.to_array(),
            fov: FOV,
        };
        EyeReprojection {
            view_proj: math::view_projection(rot.to_array(), pos.to_array(), FOV, 0.05, 100.0),
            eye_pos: pos,
            depth_view: depth.view_matrix(),
            depth_fov: FOV,
            near: NEAR,
            far: f32::INFINITY,
            depth_size: [MAP, MAP],
            target_size: [1680, 1760],
            layer: 0,
            near_cut_m: NEAR_CUT_M,
            flip_v: false,
            filter_edge_m: FILTER_SOFT_M,
            mesh_step: STEP,
        }
    }

    /// The stored value for a depth along the camera's -Z (infinite far).
    fn encode(depth: f32) -> f32 {
        1.0 - NEAR / depth
    }

    /// The map of a scene: `hit` gives the depth along the depth camera's
    /// -Z of the first surface along a world ray (origin, direction scaled
    /// so the camera-space z is -1), `None` for nothing. Read through the
    /// texel's center, as the runtime stores it.
    fn map_of(
        r: &EyeReprojection,
        hit: impl Fn(Vec3, Vec3) -> Option<f32>,
    ) -> impl Fn([u32; 2]) -> f32 {
        let to_world = r.depth_view.inverse();
        let origin = to_world.transform_point3(Vec3::ZERO);
        let (r, size) = (*r, r.depth_size);
        move |t: [u32; 2]| {
            let uv = Vec2::new(
                (t[0] as f32 + 0.5) / size[0] as f32,
                (t[1] as f32 + 0.5) / size[1] as f32,
            );
            let dir = to_world.transform_vector3(r.depth_ray(uv));
            hit(origin, dir).map_or(1.0, encode)
        }
    }

    /// Where the ray `o + t d` meets the plane through `c` with normal
    /// `n`, t > 0.
    fn plane(o: Vec3, d: Vec3, c: Vec3, n: Vec3) -> Option<f32> {
        let t = (c - o).dot(n) / d.dot(n);
        (t > 0.0).then_some(t)
    }

    /// Where it meets the vertical cylinder through `c` of radius `radius`
    /// (no caps), t > 0.
    fn pole(o: Vec3, d: Vec3, c: Vec3, radius: f32) -> Option<f32> {
        let (o2, d2) = (Vec2::new(o.x - c.x, o.z - c.z), Vec2::new(d.x, d.z));
        let (a, b, k) = (d2.dot(d2), 2.0 * o2.dot(d2), o2.dot(o2) - radius * radius);
        let disc = b * b - 4.0 * a * k;
        (disc >= 0.0)
            .then(|| (-b - disc.sqrt()) / (2.0 * a))
            .filter(|&t| t > 0.0)
    }

    fn nearest(hits: impl IntoIterator<Item = Option<f32>>) -> Option<f32> {
        hits.into_iter().flatten().min_by(f32::total_cmp)
    }

    /// The texel of the mesh vertex nearest where the depth camera sees
    /// world point `p` (the mesh's vertices sit on every `STEP`th texel).
    fn vertex_near(r: &EyeReprojection, p: Vec3) -> [u32; 2] {
        let uv = r
            .depth_uv(r.depth_view.transform_point3(p))
            .expect("in view");
        let t = r.texel(uv);
        let side = mesh_side(MAP, STEP);
        let k = |v: u32| (((v as f32) / STEP as f32).round() as u32).min(side - 1);
        [
            mesh_texel(k(t[0]), MAP, STEP),
            mesh_texel(k(t[1]), MAP, STEP),
        ]
    }

    /// The rows at full immersion with the music full, the wave's phase
    /// at 0 and the twinkle at its brightest for a phase-0 point.
    fn lit_rows() -> [[f32; 4]; ROWS] {
        let mut d = Drive::default();
        d.step(1.0, 1.0, 0.0);
        let mut rows = rows(&d, peak_alpha(1.0, 1.0), [TEAL, [1.0, 0.4, 0.9]], FILL, 0.0);
        rows[0][1] = 0.0;
        rows
    }

    /// The points' tile, made once for the tests.
    fn tile() -> &'static [[u8; 4]] {
        static TILE_ONCE: std::sync::OnceLock<Vec<[u8; 4]>> = std::sync::OnceLock::new();
        TILE_ONCE.get_or_init(points_tile)
    }

    /// A seated eye 1.2 m up looking ahead and a little down.
    fn seated() -> EyeReprojection {
        eye_at(Vec3::new(0.0, 1.2, 0.0), Quat::from_rotation_x(-0.25))
    }

    #[test]
    fn the_veil_erases_the_surfaces_by_the_level_and_the_points_add() {
        // A pixel of sky behind a real thing: premultiplied light at
        // alpha 0.6; a point's light on the thing.
        let sky = [0.1, 0.5, 0.4, 0.6];
        let point = [0.05, 0.3, 0.25];
        for (level, kept) in [(0.0, 0.0), (0.5, 0.5), (1.0, 1.0)] {
            let v = veil_share(level, true);
            let out = veil(sky, [point[0], point[1], point[2], v]);
            for k in 0..3 {
                assert_close!(out[k], point[k] + sky[k] * kept);
            }
            // The alpha is the sky's kept share: the point adds no alpha,
            // so at level 0 the layer is clear over the passthrough.
            assert_close!(out[3], sky[3] * kept);
            // The faces pass: the veil alone.
            let faces = veil(sky, [0.0, 0.0, 0.0, v]);
            assert_close!(faces, sky.map(|c| c * kept));
        }
        // Off: the mesh draws first over the clear, at a veil of 1, and
        // its own fold erases what it drew behind.
        assert_close!(veil_share(0.3, false), 1.0);
        assert_close!(
            veil([0.2, 0.2, 0.2, 0.0], [point[0], point[1], point[2], 1.0]),
            [point[0], point[1], point[2], 0.0]
        );
        assert_close!(veil_share(f32::NAN, true), 1.0);
        // The passes: the veil's order only while it lets some light
        // through, the faces pass's mesh only while it erases some.
        let passes = |level: f32, on: bool| veil_passes(on, veil_share(level, on));
        assert_eq!(
            passes(0.0, true),
            VeilPasses {
                surfaces_first: false,
                faces_mesh: true
            }
        );
        assert_eq!(
            passes(0.5, true),
            VeilPasses {
                surfaces_first: true,
                faces_mesh: true
            }
        );
        assert_eq!(
            passes(1.0, true),
            VeilPasses {
                surfaces_first: true,
                faces_mesh: false
            }
        );
        for level in [0.0, 0.5, 1.0] {
            assert_eq!(
                passes(level, false),
                VeilPasses {
                    surfaces_first: false,
                    faces_mesh: true
                }
            );
        }
        // The faces layer's fades, at twice its cells per pixel.
        let base = rows(&Drive::default(), 1.0, [TEAL; 2], FILL, 0.0);
        let half = scale_fades(base, 2.0);
        assert_close!(
            [half[3][0], half[3][1]],
            [2.0 * base[3][0], 2.0 * base[3][1]]
        );
        assert_close!(
            [half[6][2], half[6][3]],
            [2.0 * base[6][2], 2.0 * base[6][3]]
        );
        assert_close!(half[0], base[0]);
        // Where the points draw: the faces layer when there is one, the
        // eye pass then holding the depth and its own veil.
        let draws =
            |level: f32, on: bool, faces: bool| mesh_draws(true, on, veil_share(level, on), faces);
        use MeshDraw::{Depth, Light, Veil};
        let want = |eye, faces, surfaces_first| MeshDraws {
            eye,
            faces,
            surfaces_first,
        };
        assert_eq!(draws(0.0, true, true), want(Depth, Some(Light), false));
        assert_eq!(draws(0.5, true, true), want(Veil, Some(Light), true));
        assert_eq!(draws(1.0, true, true), want(Depth, Some(Light), true));
        assert_eq!(draws(1.0, false, true), want(Depth, Some(Light), false));
        assert_eq!(draws(0.5, true, false), want(Light, None, true));
        assert_eq!(draws(0.0, true, false), want(Light, None, false));
        assert_eq!(
            mesh_draws(false, true, 0.5, true),
            want(Depth, Some(Depth), false)
        );
        // The rows carry it, and the fragment hands it on as its alpha.
        let mut rows = lit_rows();
        rows[4][3] = veil_share(0.25, true);
        let f = fragment(
            &rows,
            tile(),
            Vec3::ZERO,
            Vec3::new(0.0, 0.0, -1.0),
            Vec3::Z,
            1.0,
            1.0,
            Vec3::splat(1e-3),
        );
        assert_close!(f[3], 0.75);
    }

    #[test]
    fn a_point_stays_put_under_depth_jitter_and_a_head_move() {
        let rows = lit_rows();
        let fw = Vec3::splat(5e-4);
        // A surface 1 m out turned 20 degrees from the eye: walk a row of
        // world points across it and keep each one's light.
        let eye = Vec3::new(0.0, 1.2, 0.0);
        let n = Vec3::new(20f32.to_radians().sin(), 0.0, 20f32.to_radians().cos());
        let across = Vec3::Y.cross(n).normalize();
        let base = eye - n;
        let row: Vec<Vec3> = (0..200)
            .map(|i| base + across * (i as f32 * 0.002 - 0.2))
            .collect();
        let light = |e: Vec3, p: Vec3| points(&rows, tile(), e, p, n, 1.0, fw);
        let lit: Vec<f32> = row.iter().map(|&p| light(eye, p)).collect();
        assert!(lit.iter().any(|&v| v > 0.4) && lit.iter().any(|&v| v < 0.01));
        // The map's noise moves each point along its view ray by up to
        // JITTER_TEST_M. On the plane the surface faces that is a few
        // hundredths of a cell where the ray meets the surface squarely,
        // under a tenth anywhere on the row.
        let shift = |p: Vec3| {
            let moved = p + (p - eye).normalize() * JITTER_TEST_M;
            (light_plane(moved, 2) - light_plane(p, 2)).length() / CELL_M
        };
        assert!(shift(base) < 0.04, "{}", shift(base));
        let (mut worst_shift, mut worst) = (0.0f32, 0.0f32);
        for (&p, &v) in row.iter().zip(&lit) {
            worst_shift = worst_shift.max(shift(p));
            let moved = p + (p - eye).normalize() * JITTER_TEST_M;
            worst = worst.max((light(eye, moved) - v).abs());
        }
        eprintln!(
            "{} mm of noise: a point moves {:.3} of a cell at most, the light at a pixel by {worst:.2} at most (at a point's edge)",
            JITTER_TEST_M * 1000.0,
            worst_shift
        );
        assert!(worst_shift < 0.1, "{worst_shift}");
        assert!(worst < 0.45, "{worst}");
        // A head move of 10 cm at 1 m, inside an octave: nothing changes.
        let moved_eye = eye + Vec3::new(0.1, 0.0, 0.0);
        for (&p, &v) in row.iter().zip(&lit) {
            assert_close!(light(moved_eye, p), v);
        }
        // Where the octaves blend (1.8 m), a centimeter's move changes
        // the light only a little: a cross-fade, not a crawl.
        let far: Vec<Vec3> = row.iter().map(|&p| eye + (p - eye) * 1.8).collect();
        let step = Vec3::new(0.01, 0.0, 0.0);
        let changed = far
            .iter()
            .map(|&p| (light(eye + step, p) - light(eye, p)).abs())
            .fold(0.0f32, f32::max);
        assert!(changed < 0.05, "{changed}");
    }

    #[test]
    fn a_small_step_draws_nothing_and_a_large_one_parts_the_masses() {
        let rows = lit_rows();
        let r = eye_at(Vec3::new(0.0, 1.2, 0.0), Quat::IDENTITY);
        // Two things side by side facing the eye, the right one `step`
        // behind the left one at 1.2 m.
        let walk = |step: f32| {
            let map = map_of(&r, move |o, d| {
                let t = plane(o, d, Vec3::new(0.0, 0.0, -1.2), Vec3::Z)?;
                if (o + d * t).x < 0.0 {
                    Some(t)
                } else {
                    plane(o, d, Vec3::new(0.0, 0.0, -1.2 - step), Vec3::Z)
                }
            });
            let mid = vertex_near(&r, Vec3::new(0.0, 1.2, -1.2));
            (0..12)
                .map(|k| [mid[0] + k * STEP - 6 * STEP, mid[1]])
                .map(|t| vertex_light(&r, &rows, t, &map))
                .collect::<Vec<_>>()
        };
        // 10 and 25 cm: one mass. Every vertex keeps all of its points and
        // its normal faces the eye: no line, no thinning, no tilt.
        for step in [0.1, 0.25] {
            for v in walk(step) {
                assert_close!(v.keep, 1.0);
                assert!(v.normal.z > 0.99, "{step}: {v:?}");
                assert_close!(v.bright, 1.0);
            }
        }
        // 1.5 m: two masses. The near side keeps NEAR_KEEP at its edge,
        // the far side none at its own; inside each, all of it.
        let parted = walk(1.5);
        let keeps: Vec<f32> = parted.iter().map(|v| v.keep).collect();
        let edge = parted
            .iter()
            .position(|v| v.world.z < -1.5)
            .expect("the far thing");
        assert_close!(keeps[edge - 1], NEAR_KEEP);
        assert_close!(keeps[edge], 0.0);
        assert_close!(keeps[edge - 3], 1.0);
        assert_close!(keeps[edge + 2], 1.0);
        // The normals on both sides of the edge still face the eye.
        assert!(parted[edge - 1].normal.z > 0.99 && parted[edge].normal.z > 0.99);
        // The step that parts them: half a meter here, a quarter of the
        // distance farther out.
        let part = |d: f32| JUMP_MIN_M.max(JUMP_REL * d);
        assert_close!(part(1.2), 0.5);
        assert_close!(part(4.0), 1.0);
    }

    #[test]
    fn the_fringe_between_two_masses_carries_shrinking_points() {
        // Across the fringe the keep falls from the near side's to none,
        // and a point's radius with its square root: the points shrink and
        // thin out, no line. Take one point of the tile and see how far
        // from its center it still lights at each keep.
        let rows = lit_rows();
        // A point whose neighbors stand at least a cell off, so its own
        // disk is what lights around it.
        let cells = (0..TILE_CELLS).flat_map(|y| (0..TILE_CELLS).map(move |x| [x, y]));
        let spacing = |c: [u32; 2]| {
            let (center, ..) = tile_point(c);
            let n = TILE_CELLS as i32;
            (-1..=1)
                .flat_map(|dy| (-1..=1).map(move |dx| (dx, dy)))
                .filter(|&d| d != (0, 0))
                .map(|(dx, dy)| {
                    let (x, y) = (c[0] as i32 + dx, c[1] as i32 + dy);
                    let w = [x.rem_euclid(n) as u32, y.rem_euclid(n) as u32];
                    let shift = Vec2::new((x - w[0] as i32) as f32, (y - w[1] as i32) as f32);
                    (tile_point(w).0 + shift - center).length()
                })
                .fold(f32::INFINITY, f32::min)
        };
        let lonely = cells
            .max_by(|a, b| spacing(*a).total_cmp(&spacing(*b)))
            .unwrap();
        assert!(spacing(lonely) > 1.0, "{}", spacing(lonely));
        let (center, _, size, _) = tile_point(lonely);
        // The plane's coordinates that read the tile at the center.
        let g = center - tile_offset(2, 0) * TILE_CELLS as f32;
        let scale = |keep: f32| rows[7][0] * rows[7][3] * keep.sqrt();
        let reach = |keep: f32| {
            (0..500)
                .map(|i| i as f32 * 0.001)
                .take_while(|&dx| {
                    plane_points(&rows, tile(), g + Vec2::new(dx, 0.0), 2, 0, scale(keep)) > 0.0
                })
                .count() as f32
                * 0.001
        };
        let radius = |keep: f32| scale(keep) * (SIZE[0] + (SIZE[1] - SIZE[0]) * size);
        for keep in [1.0, NEAR_KEEP, 0.1] {
            // The tile holds the distance in 8 bits and is read through a
            // linear filter: a few thousandths of a cell.
            assert!(
                (reach(keep) - radius(keep)).abs() < 0.01,
                "{keep}: {} vs {}",
                reach(keep),
                radius(keep)
            );
        }
        assert_close!(reach(0.0), 0.0);
        let area = |keep: f32| radius(keep).powi(2);
        assert!((area(NEAR_KEEP) / area(1.0) - NEAR_KEEP).abs() < 1e-5);
    }

    #[test]
    fn the_tile_holds_a_soft_point_in_every_cell_and_wraps() {
        let t = tile();
        assert_eq!(t.len(), (TILE * TILE) as usize);
        let per = (TILE / TILE_CELLS) as f32;
        let mut shared = 0;
        for cy in 0..TILE_CELLS {
            for cx in 0..TILE_CELLS {
                let (center, phase, size, rank) = tile_point([cx, cy]);
                // At the center, all of its reach (the linear filter rounds
                // the cone's tip by under a texel) and its own attributes,
                // but where another point's center lies within a texel or
                // two and the filter blends the two.
                let s = sample_tile(t, center / TILE_CELLS as f32);
                assert!(s.x > 0.9, "{cx} {cy}: {s}");
                let own = (s.y - phase).abs() < 0.01
                    && (s.z - size).abs() < 0.01
                    && (s.w - rank).abs() < 0.01;
                shared += usize::from(!own);
                assert!(center.x - (cx as f32) < 1.0 && center.y - (cy as f32) < 1.0);
            }
        }
        assert!(shared <= 3, "{shared}");
        // Wrapping: the texels across the tile's edge read like neighbors.
        let row = |y: usize| (0..TILE as usize).map(move |x| t[y * TILE as usize + x][0]);
        let jumps = |v: Vec<u8>| v.windows(2).map(|w| w[0].abs_diff(w[1])).max().unwrap();
        let seam: Vec<u8> = (0..TILE as usize)
            .map(|y| t[y * TILE as usize + TILE as usize - 1][0].abs_diff(t[y * TILE as usize][0]))
            .collect();
        assert!(
            seam.iter().max() <= Some(&(jumps(row(7).collect()) + 4)),
            "{seam:?}"
        );
        // The cone falls 255 / (REACH * per) a texel.
        let slope = 255.0 / (REACH * per);
        assert!(jumps(row(100).collect()) as f32 <= slope + 2.0);
        // A point's area: at most a quarter of a cell's per point, as the
        // fill's largest radius allows.
        let (radius, _) = point_layout(FILL_RANGE[1]);
        assert!(radius * SIZE[1] * (1.0 + SWELL) <= REACH);
    }

    #[test]
    fn the_wave_is_a_cheap_cosine() {
        // 1 at whole numbers, 0 halfway, period 1, within about a hundredth
        // of the raised cosine it stands in for, and its square's mean is
        // CREST_MEAN.
        for x in [-3.0f32, -1.0, 0.0, 1.0, 7.0] {
            assert!((wave(x) - 1.0).abs() < 1e-6);
            assert!(wave(x + 0.5) < 1e-6);
        }
        let n = 4000;
        let (mut worst, mut mean) = (0.0f32, 0.0f32);
        for i in 0..n {
            let x = i as f32 / n as f32 * 3.0 - 1.5;
            let cosine = 0.5 + 0.5 * (TAU * x).cos();
            worst = worst.max((wave(x) - cosine).abs());
            assert!((wave(x) - wave(x + 1.0)).abs() < 1e-4);
            mean += wave(x).powi(2) / n as f32;
        }
        assert!(worst < 0.012, "{worst}");
        assert!((mean - CREST_MEAN).abs() < 2e-3, "{mean}");
    }

    #[test]
    fn far_things_keep_a_floor_of_light() {
        let rows = lit_rows();
        let r = eye_at(Vec3::new(0.0, 1.2, 0.0), Quat::IDENTITY);
        let wall_at = |z: f32| {
            let map = map_of(&r, move |o, d| {
                plane(o, d, Vec3::new(0.0, 0.0, -z), Vec3::Z)
            });
            vertex_light(&r, &rows, vertex_near(&r, Vec3::new(0.0, 1.2, -z)), &map)
        };
        assert_close!(wall_at(1.0).bright, 1.0);
        assert_close!(wall_at(FADE_M[0] - 0.01).bright, 1.0);
        let mid = wall_at(f32::midpoint(FADE_M[0], FADE_M[1])).bright;
        assert!((mid - f32::midpoint(1.0, FAR_FLOOR)).abs() < 0.02, "{mid}");
        for z in [3.01, 4.0, 6.0, 9.0] {
            let v = wall_at(z);
            assert_close!(v.bright, FAR_FLOOR);
            assert_close!(v.keep, 1.0);
        }
        // So a far shelf's points have color: the fragment's light there is
        // the floor's share of a near one's.
        let at = |bright: f32| {
            fragment(
                &rows,
                tile(),
                Vec3::ZERO,
                Vec3::new(0.0, 0.0, -3.0),
                Vec3::Z,
                1.0,
                bright,
                Vec3::ZERO,
            )
        };
        let (near, far) = (at(1.0), at(FAR_FLOOR));
        assert!((far[1] - near[1] * FAR_FLOOR).abs() < 1e-5);
    }

    #[test]
    fn a_thin_pole_carries_points_wherever_it_lies() {
        // Poles 3 texels across, 1 m out in front of a wall 3 m out, slid
        // across the texel grid by quarter texels: a vertex always lands
        // on the pole, keeps the near side's share and the full light;
        // the wall's vertices beside it keep none.
        let rows = lit_rows();
        let r = eye_at(Vec3::new(0.0, 1.2, 0.0), Quat::IDENTITY);
        let texel_m = |dist: f32| dist * (FOV.right.tan() - FOV.left.tan()) / MAP as f32;
        let far_wall = |o, d| plane(o, d, Vec3::new(0.0, 0.0, -3.0), Vec3::Z);
        for shift in [0.0, 0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 1.75] {
            let x = shift * texel_m(1.0);
            let c = Vec3::new(x, 0.0, -1.0);
            let radius = 1.5 * texel_m(1.0);
            let map = map_of(&r, move |o, d| {
                nearest([pole(o, d, c, radius), far_wall(o, d)])
            });
            let mid = vertex_near(&r, Vec3::new(x, 1.2, -1.0));
            let row: Vec<VertexLight> = (0..9)
                .map(|k| [mid[0] + k * STEP - 4 * STEP, mid[1]])
                .map(|t| vertex_light(&r, &rows, t, &map))
                .collect();
            let on: Vec<&VertexLight> = row.iter().filter(|v| v.world.z > -1.5).collect();
            assert!(!on.is_empty(), "{shift}");
            for v in &on {
                assert_close!(v.keep, NEAR_KEEP);
                assert_close!(v.bright, 1.0);
            }
            let beside = row
                .iter()
                .filter(|v| v.world.z < -2.5)
                .map(|v| v.keep)
                .fold(1.0f32, f32::min);
            assert_close!(beside, 0.0);
        }
    }

    #[test]
    fn a_texel_with_no_data_keeps_nothing_and_its_neighbors_keep_theirs() {
        let r = seated();
        let rows = lit_rows();
        let c = Vec3::new(0.0, 0.9, -1.0);
        let t = vertex_near(&r, c);
        let good = map_of(&r, |o, d| plane(o, d, c, Vec3::Z));
        assert_close!(vertex_light(&r, &rows, t, &good).keep, 1.0);
        for bad in [1.0, encode(NEAR_CUT_M * 0.5)] {
            let hole = |x: [u32; 2]| if x == t { bad } else { good(x) };
            let v = vertex_light(&r, &rows, t, hole);
            assert_close!([v.keep, v.bright], [0.0, 0.0]);
            assert_close!(v.clip.z, v.clip.w);
            // A neighbor without data: the other side gives the normal and
            // nothing thins.
            let next = [t[0] + STEP, t[1]];
            let hole = |x: [u32; 2]| if x == next { bad } else { good(x) };
            let v = vertex_light(&r, &rows, t, hole);
            assert_close!(v.keep, 1.0);
            assert!(v.normal.z > 0.99, "{v:?}");
        }
    }

    #[test]
    fn the_points_cover_about_the_fill_and_fade_out_when_small() {
        let mut d = Drive::default();
        d.step(0.0, 0.0, 0.0);
        for fill in [0.1, FILL, FILL_RANGE[1]] {
            let rows = rows(&d, 1.0, [TEAL; 2], fill, 0.0);
            let scale = rows[7][0] * rows[7][3];
            // Over 40 x 40 cells of the plane facing z, sampled finely.
            let (mut covered, mut n) = (0, 0);
            for i in 0..400 {
                for j in 0..400 {
                    let g = Vec2::new(i as f32 * 0.1 + 0.05, j as f32 * 0.1 + 0.05);
                    covered += usize::from(plane_points(&rows, tile(), g, 2, 0, scale) > 0.0);
                    n += 1;
                }
            }
            let share = covered as f32 / n as f32;
            // Overlapping points cover a little less than their areas.
            assert!(share > 0.7 * fill && share < 1.05 * fill, "{fill}: {share}");
        }
        // Cells a few pixels small: the points fade out.
        let rows = lit_rows();
        let p = Vec3::new(0.013, 1.007, -1.0);
        let small = points(
            &rows,
            tile(),
            Vec3::new(0.0, 1.2, 0.0),
            p,
            Vec3::Z,
            1.0,
            Vec3::splat(0.03),
        );
        assert_close!(small, 0.0);
        // The fill's layout: a point in every cell down to the smallest
        // radius, fewer below it; the largest swollen point within half a
        // cell.
        let (radius, presence) = point_layout(FILL);
        assert_close!(presence, 1.0);
        assert!(radius * SIZE[1] * (1.0 + SWELL) <= 0.5);
        let (low, fewer) = point_layout(FILL_RANGE[0]);
        assert_close!(low, RADIUS_MIN);
        assert!(fewer < 1.0);
        let (high, _) = point_layout(1.0);
        assert!(high * SIZE[1] * (1.0 + SWELL) <= 0.5 + 1e-6);
        // The soft profile's share of the disk, by numbers.
        let k = 2000;
        let numeric: f32 = (0..k)
            .map(|i| {
                let x = (i as f32 + 0.5) / k as f32;
                (1.0 - smoothstep(SOFT_CORE, 1.0, x)) * 2.0 * x / k as f32
            })
            .sum();
        assert!((numeric - profile_share(SOFT_CORE)).abs() < 1e-3);
    }

    #[test]
    fn far_or_near_a_cell_keeps_its_size_on_screen() {
        let rows = lit_rows();
        let eye = Vec3::ZERO;
        // At 1, 2 and 4 m the cells are 1, 2 and 4 times CELL_M: the same
        // angle. Across an octave's boundary the light is continuous.
        for d in [1.0f32, 2.0, 4.0] {
            let l = d.log2();
            assert_close!(CELL_M * 2f32.powi(l.floor() as i32), CELL_M * d);
        }
        let at = |z: f32| {
            points(
                &rows,
                tile(),
                eye,
                Vec3::new(0.0123, 0.0456, -z),
                Vec3::Z,
                1.0,
                Vec3::splat(1e-5),
            )
        };
        let (below, above) = (at(2.0 - 1e-4), at(2.0 + 1e-4));
        assert!((below - above).abs() < 2e-3, "{below} {above}");
    }

    #[test]
    fn the_light_rises_with_immersion_and_the_gain() {
        assert_close!(peak_alpha(0.0, 1.0), ALPHA_LOW);
        assert_close!(peak_alpha(1.0, 1.0), ALPHA_HIGH);
        assert_close!(peak_alpha(0.5, 1.0), f32::midpoint(ALPHA_LOW, ALPHA_HIGH));
        assert_close!(peak_alpha(2.0, 1.0), ALPHA_HIGH);
        assert_close!(peak_alpha(f32::NAN, 1.0), ALPHA_LOW);
        assert_close!(peak_alpha(1.0, 10.0), GAIN_MAX * ALPHA_HIGH);
        assert_close!(peak_alpha(1.0, 0.0), 0.0);
        // The mean light at a quarter covered stays well under a lit
        // surface's full alpha.
        let mean = FILL * profile_share(SOFT_CORE) * (1.0 - TWINKLE_DEPTH / 2.0);
        assert!(ALPHA_HIGH * mean < crate::immerse::ALPHA_FULL / 4.0);
    }

    #[test]
    fn the_wave_runs_through_the_points_and_fades_when_thin() {
        // A top (its points on the horizontal plane, which the height's
        // change across a pixel does not touch), one color for both, so
        // the light out is the points times the wave.
        let mut rows = lit_rows();
        rows[2] = [rows[1][0], rows[1][1], rows[1][2], BAND_TINT];
        let eye = Vec3::new(0.0, 1.0, 0.0);
        let at = |rows: &[[f32; 4]; ROWS], p: Vec3, fwy: f32| {
            fragment(
                rows,
                tile(),
                eye,
                p,
                Vec3::Y,
                1.0,
                1.0,
                Vec3::new(1e-5, fwy, 1e-5),
            )[1]
        };
        let lit = (0..400)
            .map(|i| Vec3::new(i as f32 * 0.001, 0.0, -0.5))
            .max_by(|a, b| at(&rows, *a, 1e-5).total_cmp(&at(&rows, *b, 1e-5)))
            .expect("a point");
        let crest_light = at(&rows, lit, 1e-5);
        assert!(crest_light > 0.05, "{crest_light}");
        // The wave moved half a band puts the point in a trough.
        let mut trough = rows;
        trough[0][1] = 0.5;
        let crest_band = STEADY + (1.0 - STEADY) * rows[0][3];
        let ratio = at(&trough, lit, 1e-5) / crest_light;
        assert!((ratio - STEADY / crest_band).abs() < 1e-3, "{ratio}");
        // A band a few pixels thin: the wave's mean, the points unchanged.
        let mean_band = STEADY + (1.0 - STEADY) * CREST_MEAN * rows[0][3];
        let ratio = at(&rows, lit, 1.0) / crest_light;
        assert!((ratio - mean_band / crest_band).abs() < 1e-3, "{ratio}");
        assert!((crest(&rows, 0.0) - 1.0).abs() < 1e-6);
        assert!(crest(&rows, BAND_SPACING_M / 2.0) < 1e-6);
        let n = 1000;
        let m: f32 = (0..n)
            .map(|k| crest(&rows, k as f32 / n as f32 * BAND_SPACING_M))
            .sum::<f32>()
            / n as f32;
        assert!((m - CREST_MEAN).abs() < 1e-4, "{m}");
    }

    #[test]
    fn the_drive_drifts_twinkles_with_the_music_and_wraps() {
        let mut quiet = Drive::default();
        let mut loud = Drive::default();
        for _ in 0..72 {
            quiet.step(1.0 / 72.0, 0.0, 0.0);
            loud.step(1.0 / 72.0, 1.0, 0.0);
        }
        assert!(
            (quiet.phase() - DRIFT_BANDS_S[0]).abs() < 1e-4,
            "{}",
            quiet.phase()
        );
        assert!((quiet.twinkle() - TWINKLE_HZ).abs() < 1e-4);
        assert!(loud.phase() > 0.9 * DRIFT_BANDS_S[1], "{}", loud.phase());
        assert!(loud.twinkle() > 1.9 * TWINKLE_HZ);
        assert!(loud.music() > 0.99);
        // The beat speeds both.
        let (mut a, mut b) = (loud, loud);
        a.step(0.5, 1.0, 0.0);
        b.step(0.5, 1.0, 1.0);
        let moved = |x: f32, from: f32| (x - from).rem_euclid(1.0);
        assert!(
            (moved(b.phase(), loud.phase()) - 2.0 * moved(a.phase(), loud.phase())).abs() < 1e-4
        );
        // Both wrap in 0..1, the twinkle's integer rates keep a point's
        // twinkle continuous across the wrap.
        for _ in 0..1000 {
            a.step(0.1, 1.0, 1.0);
            assert!((0.0..1.0).contains(&a.phase()) && (0.0..1.0).contains(&a.twinkle()));
        }
        for rate in [1.0f32, 2.0, 3.0] {
            let tw = |t: f32| (TAU * (rate * t + 0.3)).sin();
            assert!((tw(0.0) - tw(1.0)).abs() < 1e-4);
        }
        // Bad input moves nothing.
        let mut f = Drive::default();
        f.step(f32::NAN, f32::NAN, f32::INFINITY);
        assert_eq!(f, Drive::default());
    }

    #[test]
    fn the_colors_follow_the_theme() {
        let key = [0.9, 0.3, 1.0];
        assert!(uses_key(Some(Theme::Aurora)));
        assert_close!(colors(Some(Theme::Aurora), key), [TEAL, key]);
        assert!(!uses_key(Some(Theme::Drift)));
        assert_close!(colors(Some(Theme::Drift), key), [TEAL, TEAL]);
        assert_close!(colors(Some(Theme::Classic), key), [GREEN, GREEN]);
        assert_close!(colors(None, key), [GREEN, GREEN]);
        for t in Theme::ALL {
            let c = t.look(KIND_OTHER).expect("other").color;
            assert_close!(colors(Some(t), key)[0], surface_color(c, KIND_OTHER, key));
        }
        assert_eq!(
            colors_line(Some(Theme::Aurora)),
            "depth light: teal sides, the key's tint on top (theme aurora)"
        );
        assert_eq!(
            colors_line(None),
            "depth light: other's own sides, the same on top (no theme saved)"
        );
        assert_eq!(band(Some(Theme::Aurora)), crate::surface_fx::BAND_RMS);
        assert_eq!(band(Some(Theme::Tide)), BAND_BASS);
        assert_eq!(band(None), crate::surface_fx::BAND_RMS);
        let audio = Audio {
            rms: 0.2,
            bass: 0.7,
            mid: 0.4,
            high: 1.5,
            ..Audio::default()
        };
        assert_close!(band_level(&audio, BAND_BASS), 0.7);
        assert_close!(band_level(&audio, BAND_MID), 0.4);
        assert_close!(band_level(&audio, BAND_HIGH), 1.0);
        assert_close!(band_level(&audio, crate::surface_fx::BAND_RMS), 0.2);
    }

    #[test]
    fn the_rows_pack_what_the_shader_reads() {
        let mut d = Drive::default();
        d.step(0.5, 0.5, 0.0);
        let rows = rows(&d, 0.2, [[0.1, 0.2, 0.3], [0.4, 0.5, 0.6]], FILL, 0.7);
        let (radius, presence) = point_layout(FILL);
        assert_close!(
            rows[0],
            [
                1.0 / BAND_SPACING_M,
                d.phase(),
                0.2,
                MUSIC_FLOOR + (1.0 - MUSIC_FLOOR) * d.music()
            ]
        );
        assert_close!(rows[1], [0.1, 0.2, 0.3, STEADY]);
        assert_close!(rows[2], [0.4, 0.5, 0.6, BAND_TINT]);
        assert_close!(
            rows[3],
            [
                AA_BANDS_PER_PX[0],
                AA_BANDS_PER_PX[1],
                CREST_MEAN,
                d.twinkle()
            ]
        );
        assert_close!(rows[4], [FADE_M[0], FADE_M[1], FAR_FLOOR, 0.7]);
        assert_close!(rows[5], [JUMP_MIN_M, JUMP_REL, JUMP_SOFT, NEAR_KEEP]);
        assert_close!(
            rows[6],
            [CELL_M, OCTAVE_BLEND, AA_CELLS_PER_PX[0], AA_CELLS_PER_PX[1]]
        );
        assert_close!(rows[7], [radius, presence, REACH, 1.0 + SWELL * d.music()]);
        assert_close!(rows[8], [SIZE[0], SIZE[1], SOFT_CORE, TWINKLE_DEPTH]);
        const {
            assert!(FADE_M[0] < FADE_M[1] && AA_BANDS_PER_PX[0] < AA_BANDS_PER_PX[1]);
            assert!(AA_CELLS_PER_PX[0] < AA_CELLS_PER_PX[1] && SIZE[0] < SIZE[1]);
            assert!(REACH <= 0.5 && JUMP_SOFT < 1.0);
        };
        // The rows end the occluder's uniform, from `light` on.
        assert_eq!(LIGHT_ROW + ROWS, UNIFORM_ROWS);
        let module = naga::front::wgsl::parse_str(ENV_DEPTH_WGSL).expect("WGSL parses");
        let (_, ty) = module
            .types
            .iter()
            .find(|(_, t)| t.name.as_deref() == Some("EnvEye"))
            .expect("struct EnvEye");
        let naga::TypeInner::Struct { members, span } = &ty.inner else {
            panic!("EnvEye is not a struct");
        };
        assert_eq!(*span as usize, UNIFORM_ROWS * 16);
        let offset = |name: &str| {
            members
                .iter()
                .find(|m| m.name.as_deref() == Some(name))
                .map(|m| m.offset as usize)
                .unwrap_or_else(|| panic!("member {name}"))
        };
        for (k, name) in [
            "light",
            "light_side",
            "light_top",
            "light_wave",
            "light_fade",
            "light_jump",
            "light_cell",
            "light_point",
            "light_size",
        ]
        .into_iter()
        .enumerate()
        {
            assert_eq!(offset(name), (LIGHT_ROW + k) * 16, "{name}");
        }
        assert_eq!(offset("mesh"), crate::env_depth::MESH_ROW * 16);
        // The WGSL's constants are these.
        for line in [
            format!("const PLANE_MIN: f32 = {PLANE_MIN:?};"),
            format!("const OCTAVE_MIN: f32 = {:?};", OCTAVE_RANGE[0]),
            format!("const OCTAVE_MAX: f32 = {:?};", OCTAVE_RANGE[1]),
            format!("const TILE_CELLS: f32 = {:?};", TILE_CELLS as f32),
        ] {
            assert!(ENV_DEPTH_WGSL.contains(&line), "{line}");
        }
        // The hash is PCG's: a known value.
        assert_eq!(hash(0), 129_708_002);
    }

    #[test]
    fn the_knobs_parse_clamp_and_log() {
        let knobs = |pairs: &[(&str, &str)]| {
            let map: std::collections::HashMap<String, String> = pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect();
            DepthLightOptions::from_knobs(move |name| map.get(name).cloned())
        };
        assert_eq!(knobs(&[]), DepthLightOptions::default());
        let d = DepthLightOptions::default();
        assert!(d.on && d.veil);
        assert_close!([d.gain, d.fill], [1.0, FILL]);
        assert!(!knobs(&[(KNOB, " 0 ")]).on);
        assert!(knobs(&[(KNOB, "yes")]).on);
        assert!(!knobs(&[(KNOB_VEIL, "0")]).veil);
        assert!(knobs(&[]).points && !knobs(&[(KNOB_POINTS, "0")]).points);
        let no_points = frame(
            &knobs(&[(KNOB_POINTS, "0")]),
            &Drive::default(),
            None,
            [1.0; 3],
            1.0,
        );
        assert_close!(no_points.rows[0][2], 0.0);
        assert!(
            knobs(&[(KNOB_POINTS, "0")])
                .line(None)
                .contains("no points")
        );
        assert!(knobs(&[(KNOB_VEIL, "1")]).veil);
        assert_close!(knobs(&[(KNOB_GAIN, "2.5")]).gain, 2.5);
        assert_close!(knobs(&[(KNOB_GAIN, "50")]).gain, GAIN_MAX);
        assert_close!(knobs(&[(KNOB_GAIN, "nan")]).gain, 1.0);
        assert_close!(knobs(&[(KNOB_FILL, "0.1")]).fill, 0.1);
        assert_close!(knobs(&[(KNOB_FILL, "0.9")]).fill, FILL_RANGE[1]);
        assert_close!(knobs(&[(KNOB_FILL, "0")]).fill, FILL_RANGE[0]);
        assert_close!(knobs(&[(KNOB_FILL, "x")]).fill, FILL);
        // It needs the mesh, and the occluder, and not the gray.
        assert_eq!(blocked(true, false, 2), None);
        assert!(blocked(true, false, 0).is_some_and(|w| w.contains("envdepthmesh 0")));
        assert!(blocked(false, false, 2).is_some_and(|w| w.contains("envdepth")));
        assert!(blocked(true, true, 2).is_some_and(|w| w.contains("envdepthshow")));
        let on = DepthLightOptions::default();
        let off = knobs(&[(KNOB, "0")]);
        assert!(on.lit(None) && !on.lit(Some("x")) && !off.lit(None));
        let line = on.line(None);
        assert!(line.starts_with("depth light: on, points · gain 1 · fill 0.25 · veil on"));
        assert!(knobs(&[(KNOB_VEIL, "0")]).line(None).contains("veil off"));
        assert!(
            off.line(None)
                .starts_with("depth light: off (debug.fosfora.depthlight 0)")
        );
        let mesh0 = blocked(true, false, 0);
        assert!(
            on.line(mesh0)
                .starts_with("depth light: off, it needs the occluder mesh")
        );
        assert_eq!(off.line(mesh0), off.line(None));
        let change = on.change_line(&off, None);
        assert!(change.starts_with("depth light: on") && change.contains("(was: off"));
        // The frame: the veil by the level, or 1 with it off.
        let drive = Drive::default();
        let f = frame(&on, &drive, Some(Theme::Aurora), [1.0; 3], 0.75);
        assert!(f.veil);
        assert_close!(f.rows[4][3], 0.25);
        let f = frame(&knobs(&[(KNOB_VEIL, "0")]), &drive, None, [1.0; 3], 0.75);
        assert!(!f.veil);
        assert_close!(f.rows[4][3], 1.0);
    }
}
