//! The depth light (board #5439): what the live depth map still occludes is
//! drawn as a shape lit by the room's light instead of a black cutout.
//!
//! **What is lit.** The depth occluder's mesh (`env_depth.rs`,
//! `envdepthmesh`) in the eye pass, where it still occludes. With geometry
//! first on (`geom_first.rs`) that is exactly what no room box explains: a
//! chair, a vacuum, a pile, a person, the wearer's own legs. The mesh
//! writes the same depth as before and now color too. The hands are not
//! part of it, since the runtime removes them from the map and the hand
//! mesh occludes them. The faces pass keeps its depth-only copy.
//!
//! **The look** ([`fragment`]). Bands of light at even heights, one every
//! [`BAND_SPACING_M`], wrap the object and drift up with the music, over a
//! steady share ([`STEADY`]) of the light, so a trough is dim rather than
//! dark. A band at one height bends over the object's form, which reads
//! with one eye. Its profile is a squared raised cosine, mean
//! [`CREST_MEAN`]. Each vertex has a normal from the map's gradient
//! ([`vertex_light`]). The light shades from 1 where the surface faces the
//! eye to [`SHADE_FLOOR`] edge-on, and a surface that faces up (a seat, a
//! shelf, a knee) takes the top's color with the bands at [`TOP_CONTRAST`],
//! so a whole top does not flash as a band passes its height. The colors
//! are the theme's ([`colors`]). The sides take the color the theme gives
//! other surfaces, the tops the key's tint where the theme uses it. Aurora
//! draws teal sides and tops in the key's tint; a room with no theme draws
//! other's own green on both.
//!
//! **The silhouette.** It fades to dark against dark. The gradient across a
//! silhouette spans the depth jump, so the normal there lies nearly edge-on
//! to the eye and the facing fade ([`GRAZE`]) takes the light to 0 at the
//! last vertex on the object and at the first one behind it. The light
//! fades out over the mesh cell inside the silhouette, and the fringe
//! triangles that lean back to the background carry none. A vertex whose
//! distance jumps from a neighbor's by more than [`JUMP`] has none either,
//! which keeps something thinner than two mesh steps dark wherever it lies
//! on the texel grid, instead of lit in some places and not in others as
//! the head moves (its two neighbors along a row both land on the
//! background, so the gradient between them misses the jump). On recorded
//! frames of a real office the two other treatments, a rim brightening
//! toward the silhouette and the fringe lit along with the object, drew the
//! mesh's stair steps in light, so the coarse edge was still the first
//! thing seen. Dark against dark leaves the steps visible only where a
//! bright curtain of sky passes right behind the edge.
//!
//! **Stable in time.** The bands are a function of the world height alone,
//! so a surface keeps its pattern while the map jitters. Noise moves a
//! point along a nearly level eye ray, which changes its height by a small
//! fraction of the noise. On three office recordings with the head still,
//! the height under a texel within 3 m moved between read-backs by 0.4 to
//! 1.2 cm at the 95th percentile, a tenth of a band at most. The vertex's
//! share uses smooth fades only: the facing ([`GRAZE`]) and the distance
//! from the eye ([`FADE_M`], all of it to 1.5 m and none past 3 m, where
//! the map's noise grows and clutter is too far to matter). A vertex
//! without data, or next to one, has none. A band narrower than a few
//! pixels fades into its mean ([`AA_BANDS_PER_PX`]) instead of aliasing.
//! The drift is integrated on the CPU ([`Drive`]), so the phase moves
//! continuously, faster with the music, and wraps at a whole band.
//!
//! **How bright** ([`peak_alpha`]). Premultiplied over the cleared target,
//! as the surfaces' light is. The peak alpha rises with the immersion level
//! from [`ALPHA_LOW`] with the room plain (barely there over the real
//! object) to [`ALPHA_HIGH`] at full immersion, about a third of a lit
//! surface's full alpha, so the sky and the surfaces stay the subject.
//! `depthlightgain` multiplies it. The crests follow the band the theme
//! gives other surfaces ([`band`]; the rms for Aurora), smoothed
//! ([`MUSIC_ATTACK_S`], [`MUSIC_RELEASE_S`]), and keep [`MUSIC_FLOOR`] of
//! their light in silence.
//!
//! **Draw order.** With the light on, the eye pass draws the box and hand
//! occluders first and the map's mesh after them, so the mesh lights only
//! what is in front of every other occluder. A hand or a box in front of a
//! lit object hides its light. The depth buffer ends the same, since every
//! occluder keeps the nearest depth (compare `Less` or `LessEqual`)
//! whatever the order. Off, the order is as before.
//!
//! **Cost.** Per vertex of the eye mesh (161 x 161, 26K per eye at the
//! default step 2): four more texel loads (the neighbors), four rays, a
//! cross product, a rotation, two square roots, a division and three
//! smoothsteps, in the vertex stage where the map is already read. None of
//! it feeds the position, so a binning pass that computes positions alone
//! skips it. Per fragment: one `fwidth`, one cosine, one smoothstep and
//! about twenty other arithmetic operations, blended; no texture, no loop,
//! no discard and no `frag_depth`, so early depth testing works as before.
//! A person 1 m away covers about 15 % of an eye's pixels and a quarter at
//! 0.7 m, a vacuum's handle 0.6 m away about 1 %. With the whole map lit
//! (geometry first off, or no boxes) it is every pixel the map covers.
//!
//! **Knobs**, read at launch and polled once a second
//! ([`DepthLightOptions::from_knobs`]): `debug.fosfora.depthlight 0|1`
//! (default 1; 0 is the frame as it was, with depth only, no color write
//! and the old order) and `depthlightgain` (0 to [`GAIN_MAX`], default 1).
//! It needs the occluder's mesh. With the full-screen pass
//! (`envdepthmesh 0`, or `envdepthfilter 0` or 1) or the gray diagnostic
//! (`envdepthshow`) it is off, and the log says so ([`blocked`]).
//! `geomfirstshow`'s tint replaces it while on.
//!
//! **What it does not do.** It does not change what occludes. The depth
//! written, geometry first's rule and the hand removal are as before. It
//! does not sharpen the coarse edge, only darkens it. It lights what the
//! map sees, so where no box explains a wall (no scan, roam, geometry first
//! off) the wall within 3 m is lit too. Past 3 m an object is still a dark
//! shape. So is anything narrower than about six depth texels (4 cm at 1 m,
//! 2.5 cm at 0.6 m), where no vertex has all four neighbors on it.
//!
//! Plain numbers in, so it builds and tests on the desktop:
//! [`vertex_light`] mirrors the WGSL's `mesh_light` and [`fragment`] its
//! `fs_mesh_light` (`env_depth::ENV_DEPTH_WGSL`), and
//! `tests/depth_light_gpu.rs` runs both on a desktop GPU against them.

use std::f32::consts::TAU;

use glam::{Vec3, Vec4};

use crate::env_depth::EyeReprojection;
use crate::surface_fx::{Audio, BAND_BASS, BAND_HIGH, BAND_MID};
use crate::surfaces::{COLOR_KEY, KIND_OTHER, SurfaceBehavior, color_name, surface_color};
use crate::themes::Theme;

/// The knobs' names.
pub const KNOB: &str = "debug.fosfora.depthlight";
pub const KNOB_GAIN: &str = "debug.fosfora.depthlightgain";

/// Rows of [`rows`], the end of `struct EnvEye` from `light` on
/// (`env_depth::LIGHT_ROW`).
pub const ROWS: usize = 6;
/// The height of one band of light (m): crests 12 cm apart, about eight on
/// a seated person's torso and legs.
pub const BAND_SPACING_M: f32 = 0.12;
/// The light between crests, as a share of a crest's.
pub const STEADY: f32 = 0.35;
/// The bands' contrast on a surface that faces straight up, as a share of
/// a side's.
pub const TOP_CONTRAST: f32 = 0.3;
/// The mean of the crest's profile, a squared raised cosine: what a band
/// fades into where it is too thin to draw.
pub const CREST_MEAN: f32 = 0.375;
/// The shading's floor: the light on a surface seen edge-on, as a share of
/// one that faces the eye.
pub const SHADE_FLOOR: f32 = 0.5;
/// The facing (the cosine between the normal and the way to the eye) where
/// the light starts to fade in and where it is all there: none at 81
/// degrees and past, all of it at 53 and under.
pub const GRAZE: [f32; 2] = [0.15, 0.6];
/// The distance from the eye (m) where the light starts to fade out and
/// where it is gone.
pub const FADE_M: [f32; 2] = [1.5, 3.0];
/// The largest jump from the vertex's distance to a neighbor's, as a share
/// of the vertex's, where the light starts to fade out and where it is
/// gone: a silhouette's jump. The facing fade catches most of them; this
/// catches a vertex on something thinner than two mesh steps, whose two
/// neighbors along an axis both lie on the background, so the gradient
/// between them does not see the jump. The noise on a surface within 3 m
/// moves a neighbor by a percent or two of the distance (the recordings).
pub const JUMP: [f32; 2] = [0.06, 0.12];
/// Bands per pixel (the `fwidth` of the band count) where the bands start
/// to fade into their mean and where they are gone: six pixels a band and
/// two.
pub const AA_BANDS_PER_PX: [f32; 2] = [1.0 / 6.0, 0.5];
/// The peak alpha with the room plain (immersion level 0) and at full
/// immersion: barely there over the real object, then a third of a lit
/// surface's full alpha (`immerse::ALPHA_FULL`).
pub const ALPHA_LOW: f32 = 0.05;
pub const ALPHA_HIGH: f32 = 0.33;
/// `depthlightgain`'s range.
pub const GAIN_MAX: f32 = 4.0;
/// The crests' light in silence, as a share of a loud moment's.
pub const MUSIC_FLOOR: f32 = 0.4;
/// The music's smoothing: the time constants (s) while the band's level
/// rises and while it falls.
pub const MUSIC_ATTACK_S: f32 = 0.05;
pub const MUSIC_RELEASE_S: f32 = 0.3;
/// The drift (bands a second, up) in silence and at a full band; the beat
/// envelope adds up to as much again, as it speeds the surfaces' clock.
pub const DRIFT_BANDS_S: [f32; 2] = [0.05, 0.25];

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
}

impl Default for DepthLightOptions {
    fn default() -> Self {
        Self {
            on: true,
            gain: 1.0,
        }
    }
}

impl DepthLightOptions {
    /// The options the two knobs ask, each read through `prop` (a system
    /// property by name, `None` unset): `depthlight` off only at `0`; an
    /// unreadable gain is the default, one out of range the nearest end.
    pub fn from_knobs(prop: impl Fn(&str) -> Option<String>) -> Self {
        let d = Self::default();
        Self {
            on: prop(KNOB).as_deref().map(str::trim) != Some("0"),
            gain: prop(KNOB_GAIN)
                .and_then(|v| v.trim().parse::<f32>().ok())
                .filter(|v| v.is_finite())
                .map_or(d.gain, |v| v.clamp(0.0, GAIN_MAX)),
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
            "on · gain {} · peak alpha {ALPHA_LOW} with the room plain to {ALPHA_HIGH} at full immersion · a band every {BAND_SPACING_M} m · fades out {} to {} m from the eye",
            self.gain, FADE_M[0], FADE_M[1],
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

/// The peak alpha at immersion level `level` (0..1) and gain `gain`: from
/// [`ALPHA_LOW`] at level 0 to [`ALPHA_HIGH`] at 1, times the gain.
pub fn peak_alpha(level: f32, gain: f32) -> f32 {
    let gain = if gain.is_finite() {
        gain.clamp(0.0, GAIN_MAX)
    } else {
        0.0
    };
    (ALPHA_LOW + (ALPHA_HIGH - ALPHA_LOW) * unit(level)) * gain
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

/// The audio band (`surface_fx::BAND_*`) the crests follow under `theme`:
/// the one it gives other surfaces; with no theme, other's default
/// behavior's.
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

/// The music the light follows and the bands' phase, stepped once a frame.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Drive {
    music: f32,
    phase: f32,
}

impl Drive {
    /// Advance by `dt` seconds with the band's level `level` and the beat
    /// envelope `beat` (both 0..1): the music one-pole toward the level
    /// ([`MUSIC_ATTACK_S`] rising, [`MUSIC_RELEASE_S`] falling), the phase
    /// on by [`DRIFT_BANDS_S`] at that music, times one plus the beat,
    /// wrapped to a whole band.
    pub fn step(&mut self, dt: f32, level: f32, beat: f32) {
        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
        let level = unit(level);
        let tau = if level > self.music {
            MUSIC_ATTACK_S
        } else {
            MUSIC_RELEASE_S
        };
        self.music += (level - self.music) * (1.0 - (-dt / tau).exp());
        let speed = DRIFT_BANDS_S[0] + (DRIFT_BANDS_S[1] - DRIFT_BANDS_S[0]) * self.music;
        self.phase = (self.phase + dt * speed * (1.0 + unit(beat))).fract();
    }

    /// The smoothed music, 0..1.
    pub fn music(&self) -> f32 {
        self.music
    }

    /// The bands' phase, 0..1 of a band.
    pub fn phase(&self) -> f32 {
        self.phase
    }
}

/// The light's rows of `struct EnvEye` (from `light` on, written at
/// `env_depth::LIGHT_ROW` of the eye pass's uniform) for `drive`, the peak
/// alpha `peak` ([`peak_alpha`]) and the sides' and tops' colors
/// ([`colors`]).
pub fn rows(drive: &Drive, peak: f32, [side, top]: [[f32; 3]; 2]) -> [[f32; 4]; ROWS] {
    [
        [
            1.0 / BAND_SPACING_M,
            drive.phase,
            peak,
            MUSIC_FLOOR + (1.0 - MUSIC_FLOOR) * unit(drive.music),
        ],
        [side[0], side[1], side[2], STEADY],
        [top[0], top[1], top[2], TOP_CONTRAST],
        [FADE_M[0], FADE_M[1], GRAZE[0], GRAZE[1]],
        [
            SHADE_FLOOR,
            AA_BANDS_PER_PX[0],
            AA_BANDS_PER_PX[1],
            CREST_MEAN,
        ],
        [JUMP[0], JUMP[1], 0.0, 0.0],
    ]
}

/// What the vertex stage hands on for one vertex of the mesh.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VertexLight {
    /// The clip position, the occluder's own (`EyeReprojection::
    /// vertex_clip`).
    pub clip: Vec4,
    /// The world point.
    pub world: Vec3,
    /// The vertex's share of the light, 0..1.
    pub weight: f32,
    /// How far the surface faces up, 0..1.
    pub up: f32,
}

/// The vertex stage of the light (`mesh_light` in
/// `env_depth::ENV_DEPTH_WGSL`) for the vertex of `texel` in `r`'s mesh
/// (`r.mesh_step`), with the light's `rows` and the map's stored values
/// through `stored`: the occluder's clip position, the world point, and
/// the share from the normal (the gradient one mesh step either way along
/// each axis, clamped to the map), the facing, the distance from the eye
/// and the largest jump to a neighbor ([`JUMP`]). No share without data at
/// the vertex or a neighbor.
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
    let d = neighbors.map(|t| r.texel_distance(stored(t)));
    // The shader multiplies a missing distance's -1 in; its result is not
    // used then.
    let q = |k: usize| r.depth_ray(r.mesh_uv(neighbors[k])) * d[k].unwrap_or(-1.0);
    let n = r
        .depth_view
        .inverse()
        .transform_vector3((q(1) - q(0)).cross(q(3) - q(2)));
    let to_eye = r.eye_pos - p;
    let (range, len) = (to_eye.length(), n.length());
    let (mut weight, mut up) = (0.0, 0.0);
    if dist.is_some() && d.iter().all(Option::is_some) && len > 0.0 && range > 0.0 {
        let facing = n.dot(to_eye) / (len * range);
        let ny = if facing < 0.0 { -n.y } else { n.y };
        up = (ny / len).clamp(0.0, 1.0);
        let f = facing.abs();
        let [fade, shape, jumps] = [rows[3], rows[4], rows[5]];
        let d0 = dist.unwrap_or(0.0);
        let jump = d
            .iter()
            .flatten()
            .map(|&x| (x - d0).abs())
            .fold(0.0, f32::max)
            / d0;
        weight = smoothstep(fade[2], fade[3], f)
            * (shape[0] + (1.0 - shape[0]) * f)
            * (1.0 - smoothstep(fade[0], fade[1], range))
            * (1.0 - smoothstep(jumps[0], jumps[1], jump));
    }
    VertexLight {
        clip,
        world: p,
        weight,
        up,
    }
}

/// The bands' term at height `y` (m) with the light's `rows`, before the
/// music and the contrast: the crest's profile, 1 on a crest and 0 midway
/// between two. A function of the height alone.
pub fn crest(rows: &[[f32; 4]; ROWS], y: f32) -> f32 {
    let h = y * rows[0][0] - rows[0][1];
    let c = 0.5 + 0.5 * (TAU * h).cos();
    c * c
}

/// The fragment stage of the light (`fs_mesh_light`): premultiplied RGBA
/// at world height `world_y`, with the interpolated share `weight` and
/// facing up `up`, where the band count changes by `fw` across a pixel
/// (`fwidth`).
pub fn fragment(rows: &[[f32; 4]; ROWS], world_y: f32, weight: f32, up: f32, fw: f32) -> [f32; 4] {
    let [light, side, top, _, shape, _] = *rows;
    let contrast = (1.0 - smoothstep(shape[1], shape[2], fw)) * (1.0 + (top[3] - 1.0) * up);
    let band = (shape[3] + (crest(rows, world_y) - shape[3]) * contrast) * light[3];
    let steady = side[3];
    let a = (light[2] * weight * (steady + (1.0 - steady) * band)).clamp(0.0, 1.0);
    let rgb = |k: usize| (side[k] + (top[k] - side[k]) * up) * a;
    [rgb(0), rgb(1), rgb(2), a]
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
    use glam::{Quat, Vec2};

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

    fn lit_rows() -> [[f32; 4]; ROWS] {
        let mut d = Drive::default();
        d.step(1.0, 1.0, 0.0);
        let mut rows = rows(&d, peak_alpha(1.0, 1.0), [TEAL, [1.0, 0.4, 0.9]]);
        rows[0][1] = 0.0;
        rows
    }

    /// A seated eye 1.2 m up looking ahead and a little down.
    fn seated() -> EyeReprojection {
        eye_at(Vec3::new(0.0, 1.2, 0.0), Quat::from_rotation_x(-0.25))
    }

    /// A wall 3.5 m ahead, beyond the light's reach.
    fn wall(o: Vec3, d: Vec3) -> Option<f32> {
        plane(o, d, Vec3::new(0.0, 0.0, -3.5), Vec3::Z)
    }

    #[test]
    fn a_vertical_pole_is_lit_down_its_middle_and_dark_at_its_sides() {
        // A pole 6 cm across, 0.6 m away: the vacuum's handle.
        let r = seated();
        let c = Vec3::new(0.0, 0.0, -0.6);
        let map = map_of(&r, |o, d| nearest([pole(o, d, c, 0.03), wall(o, d)]));
        let rows = lit_rows();
        let middle = vertex_light(&r, &rows, vertex_near(&r, Vec3::new(0.0, 1.0, -0.57)), &map);
        // Facing the eye, near: almost all of its share, and a side.
        assert!(middle.weight > 0.9, "{middle:?}");
        assert!(middle.up < 0.05, "{middle:?}");
        assert!((middle.world - Vec3::new(0.0, 1.0, -0.57)).length() < 0.02);
        // Its light at a crest is the peak, between two the steady share.
        let at = |y: f32| fragment(&rows, y, middle.weight, 0.0, 0.01)[3];
        let peak = rows[0][2] * middle.weight;
        assert!((at(0.0) - peak).abs() < 1e-4, "{} {peak}", at(0.0));
        assert!((at(BAND_SPACING_M / 2.0) - peak * STEADY).abs() < 1e-4);
        // Its edge, where the gradient spans the jump to the wall: none.
        let edge = vertex_light(&r, &rows, vertex_near(&r, Vec3::new(0.03, 1.0, -0.6)), &map);
        assert!(edge.weight < 0.05, "{edge:?}");
        // The wall beside it: past the light's reach.
        let beside = vertex_light(&r, &rows, vertex_near(&r, Vec3::new(0.4, 1.0, -3.5)), &map);
        assert_close!(beside.weight, 0.0);
    }

    #[test]
    fn something_thinner_than_two_mesh_steps_stays_dark_in_every_alignment() {
        // Poles in front of a wall 2 m out, slid across the texel grid by
        // quarter texels: a vertex on a pole narrower than two steps has
        // both neighbors along the row on the wall, so the gradient
        // between them sees no jump; the jump to the vertex itself does.
        let rows = lit_rows();
        let r = eye_at(Vec3::new(0.0, 1.2, 0.0), Quat::IDENTITY);
        let near_wall = |o, d| plane(o, d, Vec3::new(0.0, 0.0, -2.0), Vec3::Z);
        let texel_m = |dist: f32| dist * (FOV.right.tan() - FOV.left.tan()) / MAP as f32;
        let brightest = |dist: f32, width_texels: f32, shift: f32| {
            let x = shift * texel_m(dist);
            let c = Vec3::new(x, 0.0, -dist);
            let radius = width_texels * texel_m(dist) / 2.0;
            let map = map_of(&r, move |o, d| {
                nearest([pole(o, d, c, radius), near_wall(o, d)])
            });
            let mid = vertex_near(&r, Vec3::new(x, 1.2, -dist));
            // Only the vertices on the pole: the wall behind is lit too.
            (0..9)
                .map(|k| [mid[0] + k * STEP - 4 * STEP, mid[1]])
                .filter(|&t| r.texel_distance(map(t)).is_some_and(|d| d < dist + 0.1))
                .map(|t| vertex_light(&r, &rows, t, &map).weight)
                .fold(0.0, f32::max)
        };
        for dist in [0.6, 1.0] {
            for shift in [0.0, 0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 1.75] {
                // Three texels across: dark wherever it lies.
                assert_close!(brightest(dist, 3.0, shift), 0.0);
                // Seven: lit wherever it lies.
                assert!(brightest(dist, 7.0, shift) > 0.9, "{dist} {shift}");
            }
        }
    }

    #[test]
    fn a_slanted_face_is_shaded_by_its_facing_and_a_top_takes_its_color() {
        let r = seated();
        // A board leaning back 45 degrees 1 m ahead, facing the eye and up.
        let n = Vec3::new(0.0, 1.0, 1.0).normalize();
        let c = Vec3::new(0.0, 0.9, -1.0);
        let map = map_of(&r, |o, d| nearest([plane(o, d, c, n), wall(o, d)]));
        let rows = lit_rows();
        let v = vertex_light(&r, &rows, vertex_near(&r, c), &map);
        let to_eye = (r.eye_pos - v.world).normalize();
        let facing = n.dot(to_eye);
        let want =
            smoothstep(GRAZE[0], GRAZE[1], facing) * (SHADE_FLOOR + (1.0 - SHADE_FLOOR) * facing);
        assert!((v.weight - want).abs() < 0.02, "{v:?} want {want}");
        assert!((v.up - n.y).abs() < 0.02, "{v:?}");
        // Half a top: the colors half each, the bands at half their way to
        // the top's contrast.
        let [r_, g, b, a] = fragment(&rows, 0.0, 1.0, 0.5, 0.01);
        let mid = |k: usize| f32::midpoint(rows[1][k], rows[2][k]) * a;
        assert_close!([r_, g, b], [mid(0), mid(1), mid(2)]);
        // A level top barely flashes: crest against trough.
        let top = |y: f32| fragment(&rows, y, 1.0, 1.0, 0.01)[3];
        let side = |y: f32| fragment(&rows, y, 1.0, 0.0, 0.01)[3];
        let swing = |f: &dyn Fn(f32) -> f32| f(0.0) - f(BAND_SPACING_M / 2.0);
        assert!((swing(&top) - TOP_CONTRAST * swing(&side)).abs() < 1e-4);
    }

    #[test]
    fn at_a_silhouette_the_light_fades_to_none_and_the_fringe_carries_none() {
        // A box's front face 1 m ahead, a wall 2 m ahead, still in the
        // light's reach; walk the mesh row through the face's middle out
        // across its right edge.
        let r = seated();
        let box_hit = |o: Vec3, d: Vec3| {
            let t = plane(o, d, Vec3::new(0.0, 0.0, -1.0), Vec3::Z)?;
            let p = o + d * t;
            (p.x.abs() < 0.25 && (0.6..1.2).contains(&p.y)).then_some(t)
        };
        let near_wall = |o, d| plane(o, d, Vec3::new(0.0, 0.0, -2.0), Vec3::Z);
        let map = map_of(&r, |o, d| nearest([box_hit(o, d), near_wall(o, d)]));
        let rows = lit_rows();
        let start = vertex_near(&r, Vec3::new(0.0, 0.9, -1.0));
        let mut seen = Vec::new();
        for k in 0..40 {
            let t = [start[0] + k * STEP, start[1]];
            let v = vertex_light(&r, &rows, t, &map);
            let on_box = r.texel_distance(map(t)).is_some_and(|d| d < 1.5);
            seen.push((on_box, v.weight));
        }
        let last_on = seen.iter().rposition(|s| s.0).expect("the box");
        assert!(last_on > 3 && last_on + 1 < seen.len());
        // Inside: all of it (the face looks at the eye).
        assert!(seen[last_on - 2].1 > 0.9, "{seen:?}");
        // The last vertex on the box and the first behind it: none, so
        // the fringe between them is dark at both ends.
        assert!(seen[last_on].1 < 0.02, "{seen:?}");
        assert!(seen[last_on + 1].1 < 0.02, "{seen:?}");
        // And the light falls off toward the edge, never rises.
        assert!(seen[last_on - 1].1 <= seen[last_on - 2].1 + 1e-4);
        // One more vertex out, the wall is lit again, at its distance.
        assert!(seen[last_on + 2].1 > 0.5, "{seen:?}");
    }

    #[test]
    fn far_away_the_light_is_gone_and_it_fades_in_between() {
        let rows = lit_rows();
        let wall_at = |z: f32| {
            let r = eye_at(Vec3::new(0.0, 1.2, 0.0), Quat::IDENTITY);
            let map = map_of(&r, move |o, d| {
                plane(o, d, Vec3::new(0.0, 0.0, -z), Vec3::Z)
            });
            vertex_light(&r, &rows, vertex_near(&r, Vec3::new(0.0, 1.2, -z)), &map).weight
        };
        assert!(wall_at(1.0) > 0.99);
        assert!(wall_at(FADE_M[0] - 0.01) > 0.99);
        let mid = wall_at(f32::midpoint(FADE_M[0], FADE_M[1]));
        assert!((mid - 0.5).abs() < 0.02, "{mid}");
        assert_close!(wall_at(FADE_M[1] + 0.01), 0.0);
        assert_close!(wall_at(5.0), 0.0);
    }

    #[test]
    fn a_texel_with_no_data_or_too_near_has_no_light_nor_do_its_neighbors() {
        let r = seated();
        let rows = lit_rows();
        let c = Vec3::new(0.0, 0.9, -1.0);
        let t = vertex_near(&r, c);
        let wall1m = |o, d| plane(o, d, c, Vec3::Z);
        let good = map_of(&r, wall1m);
        assert!(vertex_light(&r, &rows, t, &good).weight > 0.9);
        for bad in [1.0, encode(NEAR_CUT_M * 0.5)] {
            // At the vertex: no light, and on the far plane.
            let hole = |x: [u32; 2]| if x == t { bad } else { good(x) };
            let v = vertex_light(&r, &rows, t, hole);
            assert_close!([v.weight, v.up], [0.0, 0.0]);
            assert_close!(v.clip.z, v.clip.w);
            // At a neighbor one step away: no light either.
            let next = [t[0] + STEP, t[1]];
            let hole = |x: [u32; 2]| if x == next { bad } else { good(x) };
            assert_close!(vertex_light(&r, &rows, t, hole).weight, 0.0);
            // Two steps away it is not a neighbor.
            let far = [t[0] + 2 * STEP, t[1]];
            let hole = |x: [u32; 2]| if x == far { bad } else { good(x) };
            assert!(vertex_light(&r, &rows, t, hole).weight > 0.9);
        }
    }

    #[test]
    fn the_light_rises_with_immersion_and_the_gain() {
        assert_close!(peak_alpha(0.0, 1.0), ALPHA_LOW);
        assert_close!(peak_alpha(1.0, 1.0), ALPHA_HIGH);
        assert_close!(peak_alpha(0.5, 1.0), f32::midpoint(ALPHA_LOW, ALPHA_HIGH));
        assert_close!(peak_alpha(2.0, 1.0), ALPHA_HIGH);
        assert_close!(peak_alpha(f32::NAN, 1.0), ALPHA_LOW);
        assert_close!(peak_alpha(1.0, 2.0), 2.0 * ALPHA_HIGH);
        assert_close!(peak_alpha(1.0, 10.0), GAIN_MAX * ALPHA_HIGH);
        assert_close!(peak_alpha(1.0, 0.0), 0.0);
        // A third of a lit surface's full alpha at full immersion; barely
        // there with the room plain.
        assert!((ALPHA_HIGH - crate::immerse::ALPHA_FULL / 3.0).abs() < 0.01);
        const { assert!(ALPHA_LOW < ALPHA_HIGH / 5.0) };
        // The fragment's alpha scales with the peak: the rows' z.
        let mut rows = lit_rows();
        let full = fragment(&rows, 0.0, 1.0, 0.0, 0.01)[3];
        rows[0][2] = peak_alpha(0.0, 1.0);
        let plain = fragment(&rows, 0.0, 1.0, 0.0, 0.01)[3];
        assert!((plain / full - ALPHA_LOW / ALPHA_HIGH).abs() < 1e-5);
    }

    #[test]
    fn the_same_world_point_keeps_its_pattern_while_the_map_jitters() {
        let rows = lit_rows();
        // The pattern is the height's alone: wherever the map puts a
        // point, the same height gets the same band.
        for y in [0.13, 0.5, 0.91, 1.37] {
            for w in [0.3, 1.0] {
                let a = fragment(&rows, y, w, 0.0, 0.01);
                let b = fragment(&rows, y + 1e-7, w, 0.0, 0.01);
                assert!((a[3] - b[3]).abs() < 1e-5);
            }
        }
        // A seated person's chest 0.8 m ahead, the map jittering by up to
        // 4 mm per texel and frame (the recordings' near noise): the
        // vertex of one texel keeps its height within a millimeter or so,
        // a small share of a band, and its light share barely moves.
        let r = seated();
        let c = Vec3::new(0.0, 1.0, -0.8);
        let n = Vec3::new(0.2, 0.0, 1.0).normalize();
        let t = vertex_near(&r, c);
        let clean = map_of(&r, |o, d| plane(o, d, c, n));
        let base = vertex_light(&r, &rows, t, &clean);
        let mut seed = 0x2545_f491_u32;
        let mut noise = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed as f32 / u32::MAX as f32 - 0.5) * 0.008
        };
        let (mut worst_y, mut worst_w, mut sum_w) = (0.0f32, 0.0f32, 0.0f32);
        let frames = 500;
        for _ in 0..frames {
            // Each texel its own noise this frame, the vertex's and its
            // four neighbors' alike.
            let jitter: Vec<(u32, u32, f32)> = (0..9)
                .flat_map(|k| (0..9).map(move |j| (k, j)))
                .map(|(k, j)| (k, j, noise()))
                .collect();
            let jittered = |x: [u32; 2]| {
                let (k, j) = (
                    (x[0] + 4).saturating_sub(t[0]),
                    (x[1] + 4).saturating_sub(t[1]),
                );
                let e = jitter
                    .iter()
                    .find(|&&(a, b, _)| (a, b) == (k, j))
                    .map_or(0.0, |&(.., e)| e);
                encode(r.texel_distance(clean(x)).expect("data") + e)
            };
            let v = vertex_light(&r, &rows, t, jittered);
            worst_y = worst_y.max((v.world.y - base.world.y).abs());
            worst_w = worst_w.max((v.weight - base.weight).abs());
            sum_w += (v.weight - base.weight).abs();
        }
        let mean_w = sum_w / frames as f32;
        eprintln!(
            "jitter up to 4 mm: height moved at most {:.2} mm ({:.3} of a band); share {:.3}, moved {mean_w:.4} on average, {worst_w:.3} at most",
            worst_y * 1000.0,
            worst_y * rows[0][0],
            base.weight
        );
        assert!(worst_y < 0.002, "{worst_y}");
        assert!(
            worst_y * rows[0][0] < 0.02,
            "a band moved {}",
            worst_y * rows[0][0]
        );
        assert!(mean_w < 0.03, "mean {mean_w}");
        assert!(worst_w < 0.15, "worst {worst_w}");
    }

    #[test]
    fn a_band_too_thin_to_draw_fades_into_its_mean() {
        let rows = lit_rows();
        let at = |y: f32, fw: f32| fragment(&rows, y, 1.0, 0.0, fw)[3];
        // Six pixels a band and wider: the whole swing.
        let swing = |fw: f32| at(0.0, fw) - at(BAND_SPACING_M / 2.0, fw);
        let full = swing(AA_BANDS_PER_PX[0]);
        assert!(full > 0.1);
        assert!((swing(0.01) - full).abs() < 1e-6);
        // Two pixels a band and thinner: the mean, flat.
        assert_close!(swing(AA_BANDS_PER_PX[1]), 0.0);
        assert_close!(swing(2.0), 0.0);
        let mean = rows[0][2] * (STEADY + (1.0 - STEADY) * CREST_MEAN * rows[0][3]);
        assert!((at(0.3, 1.0) - mean).abs() < 1e-5);
        // In between, part of it.
        let mid = swing(f32::midpoint(AA_BANDS_PER_PX[0], AA_BANDS_PER_PX[1]));
        assert!(mid > 0.0 && mid < full);
        // The profile's mean is CREST_MEAN.
        let n = 1000;
        let mean: f32 = (0..n)
            .map(|k| crest(&rows, k as f32 / n as f32 * BAND_SPACING_M))
            .sum::<f32>()
            / n as f32;
        assert!((mean - CREST_MEAN).abs() < 1e-4, "{mean}");
    }

    #[test]
    fn the_drive_drifts_up_with_the_music_and_wraps() {
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
        assert!(loud.phase() > 0.9 * DRIFT_BANDS_S[1], "{}", loud.phase());
        assert!(loud.music() > 0.99);
        // A crest at height y one step later sits higher: the bands rise.
        let before = rows(&loud, 1.0, [TEAL; 2]);
        let y0 = (1.0 + before[0][1]) / before[0][0];
        loud.step(0.1, 1.0, 0.0);
        let after = rows(&loud, 1.0, [TEAL; 2]);
        let y1 = (1.0 + after[0][1]) / after[0][0];
        assert!(y1 > y0);
        assert!((crest(&before, y0) - 1.0).abs() < 1e-4);
        assert!((crest(&after, y1) - 1.0).abs() < 1e-4);
        // The beat speeds it as it speeds the surfaces' clock.
        let (mut a, mut b) = (loud, loud);
        a.step(0.5, 1.0, 0.0);
        b.step(0.5, 1.0, 1.0);
        let moved = |d: &Drive| (d.phase() - loud.phase()).rem_euclid(1.0);
        assert!((moved(&b) - 2.0 * moved(&a)).abs() < 1e-4);
        // It wraps at a whole band and stays in 0..1.
        for _ in 0..1000 {
            a.step(0.1, 1.0, 1.0);
            assert!((0.0..1.0).contains(&a.phase()));
        }
        // The music falls slower than it rises.
        let mut d = Drive::default();
        d.step(MUSIC_ATTACK_S, 1.0, 0.0);
        assert!((d.music() - (1.0 - (-1.0f32).exp())).abs() < 1e-5);
        let mut e = Drive {
            music: 1.0,
            phase: 0.0,
        };
        e.step(MUSIC_ATTACK_S, 0.0, 0.0);
        assert!(e.music() > 0.8);
        // Bad input moves nothing.
        let mut f = Drive::default();
        f.step(f32::NAN, f32::NAN, f32::INFINITY);
        assert_eq!(f, Drive::default());
    }

    #[test]
    fn the_colors_follow_the_theme() {
        let key = [0.9, 0.3, 1.0];
        // Aurora: teal, the color it gives other surfaces, and the key's
        // tint on top, as its sky.
        assert!(uses_key(Some(Theme::Aurora)));
        assert_close!(colors(Some(Theme::Aurora), key), [TEAL, key]);
        // Drift: teal for other surfaces, no key anywhere.
        assert!(!uses_key(Some(Theme::Drift)));
        assert_close!(colors(Some(Theme::Drift), key), [TEAL, TEAL]);
        // Classic and no theme: other's own green.
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
        // The band: the theme's for other surfaces (Tide's pulse is the
        // bass), else other's default (the curls: the rms).
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
        let rows = rows(&d, 0.2, [[0.1, 0.2, 0.3], [0.4, 0.5, 0.6]]);
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
        assert_close!(rows[2], [0.4, 0.5, 0.6, TOP_CONTRAST]);
        assert_close!(rows[3], [FADE_M[0], FADE_M[1], GRAZE[0], GRAZE[1]]);
        assert_close!(
            rows[4],
            [
                SHADE_FLOOR,
                AA_BANDS_PER_PX[0],
                AA_BANDS_PER_PX[1],
                CREST_MEAN
            ]
        );
        assert_close!(rows[5], [JUMP[0], JUMP[1], 0.0, 0.0]);
        // Every fade runs low to high, as smoothstep needs.
        const { assert!(FADE_M[0] < FADE_M[1] && GRAZE[0] < GRAZE[1]) };
        const { assert!(AA_BANDS_PER_PX[0] < AA_BANDS_PER_PX[1] && JUMP[0] < JUMP[1]) };
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
            "light_fade",
            "light_shape",
            "light_jump",
        ]
        .into_iter()
        .enumerate()
        {
            assert_eq!(offset(name), (LIGHT_ROW + k) * 16, "{name}");
        }
        assert_eq!(offset("mesh"), crate::env_depth::MESH_ROW * 16);
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
        // On by default; off only at 0.
        assert_eq!(knobs(&[]), DepthLightOptions::default());
        assert!(knobs(&[]).on);
        assert!(!knobs(&[(KNOB, " 0 ")]).on);
        assert!(knobs(&[(KNOB, "1")]).on);
        assert!(knobs(&[(KNOB, "yes")]).on);
        assert_close!(knobs(&[(KNOB_GAIN, "2.5")]).gain, 2.5);
        assert_close!(knobs(&[(KNOB_GAIN, "50")]).gain, GAIN_MAX);
        assert_close!(knobs(&[(KNOB_GAIN, "-1")]).gain, 0.0);
        assert_close!(knobs(&[(KNOB_GAIN, "nan")]).gain, 1.0);
        assert_close!(knobs(&[(KNOB_GAIN, "x")]).gain, 1.0);
        // It needs the mesh, and the occluder, and not the gray.
        assert_eq!(blocked(true, false, 2), None);
        assert!(blocked(true, false, 0).is_some_and(|w| w.contains("envdepthmesh 0")));
        assert!(blocked(false, false, 2).is_some_and(|w| w.contains("envdepth")));
        assert!(blocked(true, true, 2).is_some_and(|w| w.contains("envdepthshow")));
        let on = DepthLightOptions::default();
        let off = knobs(&[(KNOB, "0")]);
        assert!(on.lit(None));
        assert!(!on.lit(Some("x")));
        assert!(!off.lit(None));
        assert!(on.line(None).starts_with("depth light: on · gain 1"));
        assert!(
            on.line(None)
                .contains("peak alpha 0.05 with the room plain to 0.33")
        );
        assert!(
            off.line(None)
                .starts_with("depth light: off (debug.fosfora.depthlight 0)")
        );
        let mesh0 = blocked(true, false, 0);
        assert!(
            on.line(mesh0)
                .starts_with("depth light: off, it needs the occluder mesh")
        );
        // Asked off, the reason does not matter.
        assert_eq!(off.line(mesh0), off.line(None));
        let change = on.change_line(&off, None);
        assert!(change.starts_with("depth light: on") && change.contains("(was: off"));
    }
}
