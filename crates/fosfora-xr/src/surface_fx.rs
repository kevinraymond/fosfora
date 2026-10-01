//! The surfaces pass (board #3472, steps D1 and D2 of
//! `docs/xr/SURFACES_DESIGN.md`): every room surface whose behavior is a
//! surface shader is drawn as one lit quad on its acting face
//! (`surfaces::acting_face`), the lane picking the shader. Plain numbers
//! in, so it builds and tests on the desktop; `gfx.rs` draws it with
//! [`SURFACE_FX_WGSL`] from the rows [`rows`] packs, one uniform slot per
//! lit face (up to [`MAX_SLOTS`]) and one draw each, in the eye pass right
//! after the occluders, where the floor ripple drew: behind a hand or a
//! chair in front of the surface, under the embers.
//!
//! **The slot.** [`UNIFORM_ROWS`] rows, `struct SurfaceFx` in the WGSL:
//! the four corners ([`Face::corners`] at [`LIFT_M`]); `face` (the face's
//! two in-plane half extents, the lift, the behavior id); `params`
//! (strength, the color index, the audio band, the speed); `color` (the
//! color, the peak alpha); `audio` (rms, bass, the beat envelope, the
//! surfaces' clock); `audio2` (mid, high, the downbeat envelope, the bar
//! phase); `shape` (the rings' origin u, v and glow, the streamlines' and
//! the curls' feature size); then the rings, one row per ring slot
//! (origin u, v, radius, intensity). Every position is in the face's
//! (u, v): meters along its two axes from its center, which the vertex
//! stage hands the fragment, so one shader works on any face at any
//! orientation. The audio band ([`BAND_RMS`] to [`BAND_HIGH`]) is the
//! level the streamlines, the curls and the pulse follow; the rings follow
//! the ripple's own bass.
//!
//! **Rings** (id 4): the floor ripple's look (`ripple.rs`: the ring width,
//! the glow, the peak alpha 0.25 times `ripplegain`, the ripple's warm
//! white on a floor) on any face. One [`Ripple`] state is kept (rings born
//! on the beat, the origin low-passed) and its rings are written into
//! every rings face's rows: on a floor the origin is the head projected
//! onto the face, as the ripple had it; on any other face it is the face's
//! point nearest the head ([`rings_origin`]), so a table's rings start at
//! the edge nearest the chair.
//!
//! **Streamlines** (id 5): lines of light flowing across the face along a
//! curl-noise field, the mockup's tabletop. The field is the curl of a
//! stream function ψ, two octaves of value noise ([`stream`]) at
//! [`STREAM_FEATURE_M`] (twice that on a floor), drifting slowly with the
//! clock: `v = (∂ψ/∂v, -∂ψ/∂u)`, divergence-free by construction. The
//! lines are ψ's contours, which are the field's streamlines: a contour
//! every 1 / [`STREAM_CONTOURS_PER_M`] of ψ gives about 10 lines per meter
//! where the field runs at its median speed, denser where it runs faster,
//! so every other line thins out where they would crowd past about 14 a
//! meter and a line between each pair fills in where they would thin under
//! about 6 ([`line_weight`]); each is [`STREAM_LINE_M`] wide with a soft
//! edge. Along them, streaks of light advected along `v` at the speed (two
//! phases cross-faded, the usual texture advection), so a still frame
//! shows thin, curving, directional lines and the moving one shows them
//! flow. The brightness is the surface's color at up to
//! [`STREAM_PEAK_ALPHA`] times the strength, scaled by the band's level
//! with a floor of [`STREAM_RMS_FLOOR`] (a quiet track still shows the
//! flow), faded over [`STREAM_EDGE_M`] toward the face's edges.
//!
//! **Curls** (id 6): the streamlines' field tighter and faster, read as
//! swirls, the mockup's chairs. Three octaves ([`curl_stream`]) at
//! [`CURL_FEATURE_M`] ([`CURL_FLOOR_FEATURE_M`] on a floor), drifting
//! [`CURL_DRIFT`] cells a second, the streaks at [`CURL_SPEED_M_S`]. The
//! lines are drawn as the streamlines' (the same contour spacing and
//! tiers: a 0.4 m chair seat shows 4 to 6 closed curls), plus a faint fill
//! inside each closed contour, the smoothed sign of ψ ([`curl_fill`]) at
//! [`CURL_FILL`] of the line's alpha, so a curl reads as a disc of light
//! with a bright rim. The same peak alpha and band floor as the
//! streamlines.
//!
//! **Pulse** (id 7): a whole-face glow for frames, lamps and anything
//! small: a radial falloff from the face's center ([`pulse_radial`], 1 at
//! the center, [`PULSE_EDGE`] at the edges, the 5 cm edge fade kept) times
//! the downbeat envelope ([`downbeat_step`]: 1 on the downbeat, a 0.6 s
//! time constant) over a floor of [`PULSE_FLOOR`] times the band's level
//! ([`pulse_level`]), so a frame breathes with the bar and never goes
//! black while the music plays. Peak alpha [`PULSE_PEAK_ALPHA`].
//!
//! **The clock** ([`Clock`]) runs at 1 + the beat envelope, so the flow
//! and the drift double on the beat and settle back, without the jumps a
//! speed multiplied into the time would make.

use glam::{Vec2, Vec3};

use crate::ripple::{MAX_RINGS, Ripple};
use crate::surfaces::{Face, KIND_FLOOR, SurfaceBehavior, palette};

/// Lit faces the pass holds, one per obstacle box.
pub const MAX_SLOTS: usize = crate::surfaces::SURFACE_LANE_ROWS;
/// How far off the face the quad sits (m), plus the pipeline's depth bias:
/// the ripple's, as the surface's occluder writes depth at the face.
pub const LIFT_M: f32 = crate::ripple::LIFT_M;
/// Rows of [`rows`], `struct SurfaceFx` in [`SURFACE_FX_WGSL`]: the
/// corners, `face`, `params`, `color`, `audio`, `audio2`, `shape`, then
/// the rings.
pub const UNIFORM_ROWS: usize = 10 + MAX_RINGS;
/// The row of the first ring.
const RINGS_ROW: usize = 10;
/// The streamlines' alpha at full strength and full rms.
// 0.30 read "too faint" on the real desk (Kevin, worn, Sep 30).
pub const STREAM_PEAK_ALPHA: f32 = 0.55;
/// How fast the streaks run along the field (m/s of the clock), doubled
/// at a full beat envelope by the clock.
pub const STREAM_SPEED_M_S: f32 = 0.15;
/// The field's feature size (m): a table's, and a floor's twice it.
pub const STREAM_FEATURE_M: f32 = 0.35;
pub const STREAM_FLOOR_FEATURE_M: f32 = 2.0 * STREAM_FEATURE_M;
/// Contours of ψ per meter of ψ: times |∇ψ| (about 1 at its median), the
/// lines per meter across the flow.
pub const STREAM_CONTOURS_PER_M: f32 = 10.0;
/// Where every other line thins out (lines per meter, from and to) and
/// where a line between each pair fills in (to and from, falling).
pub const STREAM_THIN_FROM: f32 = 12.0;
pub const STREAM_THIN_TO: f32 = 20.0;
pub const STREAM_FILL_FROM: f32 = 7.0;
pub const STREAM_FILL_TO: f32 = 4.0;
/// A line's width (m) and its soft edge (m) at arm's length.
pub const STREAM_LINE_M: f32 = 0.006;
pub const STREAM_LINE_SOFT_M: f32 = 0.0015;
/// The fade toward the face's edges (m).
pub const STREAM_EDGE_M: f32 = 0.05;
/// The band level's floor for the streamlines and the curls.
pub const STREAM_RMS_FLOOR: f32 = 0.6;
/// The field's drift (noise cells per second of the clock): the lines
/// morph slowly while the streaks run along them.
pub const STREAM_DRIFT: f32 = 0.04;
/// The streaks: their size along a line (m), how far one phase carries
/// them before it fades out (m), and a line's brightness between them.
pub const STREAK_M: f32 = 0.08;
pub const STREAK_TRAVEL_M: f32 = 0.3;
pub const STREAK_BASE: f32 = 0.35;
/// The curls' feature size (m), a floor's, their drift (cells per second
/// of the clock) and their streaks' speed (m/s of the clock).
pub const CURL_FEATURE_M: f32 = 0.12;
pub const CURL_FLOOR_FEATURE_M: f32 = 0.25;
pub const CURL_DRIFT: f32 = 0.1;
pub const CURL_SPEED_M_S: f32 = 0.3;
/// The curls' alpha at full strength and full level: the streamlines'.
pub const CURL_PEAK_ALPHA: f32 = STREAM_PEAK_ALPHA;
/// The fill inside a curl at most, as a fraction of a line's light, and
/// the |ψ| (in feature sizes) over which it rises: ψ / (feature * soft) is
/// 1 at about 70 % of the way to its plateau.
pub const CURL_FILL: f32 = 0.25;
pub const CURL_FILL_SOFT: f32 = 0.25;
/// The pulse's alpha at full strength and a full envelope.
pub const PULSE_PEAK_ALPHA: f32 = 0.45;
/// The pulse's light at the face's edges against its center's.
pub const PULSE_EDGE: f32 = 0.35;
/// The pulse's envelope never drops under this times the band's level.
pub const PULSE_FLOOR: f32 = 0.15;
/// The downbeat envelope's decay (per second): a 0.6 s time constant.
pub const DOWNBEAT_DECAY_PER_S: f32 = 1.6;
/// The audio band a surface follows (`params.z`): the rms, the bass, the
/// mid, the high (the larger of presence and brilliance).
pub const BAND_RMS: u32 = 0;
pub const BAND_BASS: u32 = 1;
pub const BAND_MID: u32 = 2;
pub const BAND_HIGH: u32 = 3;
/// How many bands there are.
pub const BANDS: u32 = 4;

/// The audio a slot carries: the rms, the bass, the mid and the high
/// (0..1), the beat envelope (1 on the beat, decaying), the surfaces'
/// [`Clock`], the downbeat envelope ([`downbeat_step`]) and the bar phase
/// (0..1 over the bar).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Audio {
    pub rms: f32,
    pub bass: f32,
    pub beat: f32,
    pub clock: f32,
    pub mid: f32,
    pub high: f32,
    pub downbeat: f32,
    pub bar_phase: f32,
}

/// The downbeat envelope after `dt` seconds from `env`: 1 when the
/// downbeat `fired` this frame, else decaying at [`DOWNBEAT_DECAY_PER_S`].
pub fn downbeat_step(env: f32, fired: bool, dt: f32) -> f32 {
    let env = if fired { 1.0 } else { env };
    env * (-DOWNBEAT_DECAY_PER_S * dt.max(0.0)).exp()
}

/// The surfaces' clock: seconds at 1 + the beat envelope, kept in f64 so a
/// long session does not lose the frame's step.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Clock(f64);

impl Clock {
    /// Advance by `dt` seconds at a beat envelope of `beat` (0..1).
    pub fn advance(&mut self, dt: f32, beat: f32) {
        self.0 += f64::from(dt.max(0.0) * (1.0 + beat.clamp(0.0, 1.0)));
    }

    pub fn seconds(&self) -> f32 {
        self.0 as f32
    }
}

/// One lit face: the face the behavior acts on, the surface's kind (its
/// color), its lane's behavior and strength, and the audio band it follows
/// ([`BAND_RMS`] to [`BAND_HIGH`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Slot {
    pub face: Face,
    pub kind: u32,
    pub behavior: SurfaceBehavior,
    pub strength: f32,
    pub band: u32,
}

/// `p`'s (u, v) on `face`: meters along its two axes from its center, the
/// coordinates the fragment works in.
pub fn face_uv(face: &Face, p: Vec3) -> Vec2 {
    let d = p - face.center;
    Vec2::new(d.dot(face.axes[0]), d.dot(face.axes[1]))
}

/// Where rings spread from on `face`, in its (u, v), for the smoothed head
/// `head`: on a floor the head projected onto it (under the chair, as the
/// ripple had it, even past the face's edge); on any other face the face's
/// point nearest the head (a table's edge nearest the chair, a wall's
/// point level with the eyes).
pub fn rings_origin(face: &Face, kind: u32, head: Vec3) -> Vec2 {
    let p = if kind == KIND_FLOOR {
        head
    } else {
        face.nearest(head)
    };
    face_uv(face, p)
}

/// A floor's face as the ripple drew it: level (a slightly tilted scene
/// floor gets a level quad, its axes flattened, the extents with them) and
/// its top raised to `stage_top` when that is higher, so the stage floor's
/// occluder never hides it.
pub fn level_floor(face: Face, stage_top: Option<f32>) -> Face {
    let flat = |v: Vec3| Vec3::new(v.x, 0.0, v.z);
    let (a, b) = (flat(face.axes[0]), flat(face.axes[1]));
    let y = stage_top.map_or(face.center.y, |s| s.max(face.center.y));
    Face {
        normal: Vec3::Y,
        center: Vec3::new(face.center.x, y, face.center.z),
        axes: [a.normalize_or(face.axes[0]), b.normalize_or(face.axes[1])],
        half: [face.half[0] * a.length(), face.half[1] * b.length()],
    }
}

/// The color a slot draws in: the surface's palette color, but the rings
/// on a floor keep the ripple's warm white, so the floor looks as it did.
pub fn color_of(kind: u32, behavior: SurfaceBehavior) -> [f32; 3] {
    if behavior == SurfaceBehavior::Rings && kind == KIND_FLOOR {
        crate::ripple::COLOR
    } else {
        palette(kind)
    }
}

/// The uniform rows of `slot` at time `t` (`struct SurfaceFx` in
/// [`SURFACE_FX_WGSL`], the layout in the module docs). `ripple` is the
/// rings' state; without it a rings slot draws nothing (no rings, no
/// glow).
pub fn rows(
    slot: &Slot,
    audio: Audio,
    ripple: Option<&Ripple>,
    t: f32,
) -> [[f32; 4]; UNIFORM_ROWS] {
    let mut rows = [[0.0f32; 4]; UNIFORM_ROWS];
    for (row, c) in rows.iter_mut().zip(slot.face.corners(LIFT_M)) {
        *row = [c.x, c.y, c.z, 1.0];
    }
    let face = &slot.face;
    rows[4] = [
        face.half[0],
        face.half[1],
        LIFT_M,
        slot.behavior.id() as f32,
    ];
    let color = color_of(slot.kind, slot.behavior);
    let (speed, peak) = match slot.behavior {
        SurfaceBehavior::Rings => (
            ripple.map_or(crate::ripple::SPEED_M_S, |r| r.speed),
            crate::ripple::PEAK_ALPHA * ripple.map_or(0.0, |r| r.gain),
        ),
        SurfaceBehavior::Curls => (CURL_SPEED_M_S, CURL_PEAK_ALPHA),
        SurfaceBehavior::Pulse => (0.0, PULSE_PEAK_ALPHA),
        _ => (STREAM_SPEED_M_S, STREAM_PEAK_ALPHA),
    };
    rows[5] = [
        slot.strength.clamp(0.0, 1.0),
        slot.kind as f32,
        slot.band.min(BANDS - 1) as f32,
        speed,
    ];
    rows[6] = [color[0], color[1], color[2], peak];
    rows[7] = [audio.rms, audio.bass, audio.beat, audio.clock];
    rows[8] = [audio.mid, audio.high, audio.downbeat, audio.bar_phase];
    let floor = slot.kind == KIND_FLOOR;
    match slot.behavior {
        SurfaceBehavior::Rings => {
            if let Some(r) = ripple {
                let origin = r
                    .head()
                    .map_or(Vec2::ZERO, |h| rings_origin(face, slot.kind, h));
                rows[9] = [origin.x, origin.y, r.glow(), 0.0];
                for (row, ring) in rows[RINGS_ROW..].iter_mut().zip(r.slots()) {
                    if let Some((ring, (radius, i))) =
                        ring.and_then(|g| g.at(t, r.speed).map(|a| (g, a)))
                    {
                        let o = rings_origin(face, slot.kind, ring.head);
                        *row = [o.x, o.y, radius, i];
                    }
                }
            }
        }
        SurfaceBehavior::Curls => {
            let feature = if floor {
                CURL_FLOOR_FEATURE_M
            } else {
                CURL_FEATURE_M
            };
            rows[9] = [0.0, 0.0, 0.0, feature];
        }
        SurfaceBehavior::Pulse => {}
        _ => {
            let feature = if floor {
                STREAM_FLOOR_FEATURE_M
            } else {
                STREAM_FEATURE_M
            };
            rows[9] = [0.0, 0.0, 0.0, feature];
        }
    }
    rows
}

/// A lattice value in -1..1 for cell `(x, y)` and `seed`: an integer hash,
/// the WGSL's `hash` bit for bit (24 bits, so the float is exact).
fn hash(x: i32, y: i32, seed: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343)
        ^ (y as u32).wrapping_mul(0xd816_3841)
        ^ seed.wrapping_mul(0xcb1a_b31f);
    h = (h ^ (h >> 16)).wrapping_mul(0x7feb_352d);
    h = (h ^ (h >> 15)).wrapping_mul(0x846c_a68b);
    h ^= h >> 16;
    (h >> 8) as f32 * (2.0 / 16_777_215.0) - 1.0
}

/// Value noise at `p` (cells) and its gradient, the WGSL's `noise`: the
/// lattice values blended with the quintic fade, whose continuous second
/// derivative keeps the curl's divergence at zero across the cells.
pub fn noise(p: Vec2, seed: u32) -> (f32, Vec2) {
    let i = p.floor();
    let f = p - i;
    let (x, y) = (i.x as i32, i.y as i32);
    let u = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
    let du = 30.0 * f * f * (f * (f - 2.0) + 1.0);
    let a = hash(x, y, seed);
    let b = hash(x.wrapping_add(1), y, seed);
    let c = hash(x, y.wrapping_add(1), seed);
    let d = hash(x.wrapping_add(1), y.wrapping_add(1), seed);
    let (k1, k2, k3) = (b - a, c - a, a - b - c + d);
    (
        a + k1 * u.x + k2 * u.y + k3 * u.x * u.y,
        du * Vec2::new(k1 + k3 * u.y, k2 + k3 * u.x),
    )
}

/// The second and third octaves' offsets (cells) and each octave's drift
/// direction: the WGSL's `stream` and `curl_stream` literals.
const OCTAVE2_OFFSET: Vec2 = Vec2::new(17.31, 5.13);
const OCTAVE3_OFFSET: Vec2 = Vec2::new(-9.27, 31.7);
const DRIFT1: Vec2 = Vec2::new(0.8, 0.6);
const DRIFT2: Vec2 = Vec2::new(-0.6, 0.8);
const DRIFT3: Vec2 = Vec2::new(-0.8, -0.6);

/// The stream function ψ (m) at face point `uv` (m) for a field of
/// `feature` size at `clock`, and its gradient (dimensionless): two
/// octaves of [`noise`], the second at twice the frequency and half the
/// weight, each drifting its own way. The WGSL's `stream`.
pub fn stream(uv: Vec2, feature: f32, clock: f32) -> (f32, Vec2) {
    let l = feature.max(1e-3);
    let (n1, g1) = noise(uv / l + DRIFT1 * (STREAM_DRIFT * clock), 1);
    let (n2, g2) = noise(
        2.0 * uv / l + OCTAVE2_OFFSET + DRIFT2 * (STREAM_DRIFT * clock),
        2,
    );
    (l * n1 + 0.25 * l * n2, g1 + 0.5 * g2)
}

/// The curls' stream function ψ (m) and its gradient: [`stream`]'s shape
/// with a third octave (four times the frequency, a sixteenth of the
/// value, a quarter of the gradient), its own seeds and the faster
/// [`CURL_DRIFT`]. The WGSL's `curl_stream`.
pub fn curl_stream(uv: Vec2, feature: f32, clock: f32) -> (f32, Vec2) {
    let l = feature.max(1e-3);
    let drift = CURL_DRIFT * clock;
    let (n1, g1) = noise(uv / l + DRIFT1 * drift, 5);
    let (n2, g2) = noise(2.0 * uv / l + OCTAVE2_OFFSET + DRIFT2 * drift, 6);
    let (n3, g3) = noise(4.0 * uv / l + OCTAVE3_OFFSET + DRIFT3 * drift, 7);
    (
        l * (n1 + 0.25 * n2 + 0.0625 * n3),
        g1 + 0.5 * g2 + 0.25 * g3,
    )
}

/// The curls' fill for ψ on a field of `feature` size: the sign of ψ,
/// smoothed (0 on the zero contour, toward ±1 inside a curl), as the WGSL's
/// `curls_light` takes its magnitude.
pub fn curl_fill(psi: f32, feature: f32) -> f32 {
    let x = psi / (feature.max(1e-3) * CURL_FILL_SOFT);
    x / (x * x + 1.0).sqrt()
}

/// The pulse's radial falloff at face point `uv` on a face of half extents
/// `half`: 1 at the center, [`PULSE_EDGE`] at the edges and past them,
/// smoothly between. The WGSL's `pulse_light` before the envelope.
pub fn pulse_radial(uv: Vec2, half: [f32; 2]) -> f32 {
    let r = (uv / Vec2::from(half).max(Vec2::splat(1e-3))).length();
    let t = r.clamp(0.0, 1.0);
    let s = t * t * (3.0 - 2.0 * t);
    1.0 + (PULSE_EDGE - 1.0) * s
}

/// The pulse's envelope: the downbeat envelope over a floor of
/// [`PULSE_FLOOR`] times the band's `level`.
pub fn pulse_level(downbeat: f32, level: f32) -> f32 {
    downbeat.max(PULSE_FLOOR * level.clamp(0.0, 1.0))
}

/// A contour's weight (the WGSL's `line_weight`): contour `m` of ψ at half
/// the spacing (`round(2 ψ STREAM_CONTOURS_PER_M)`) where the full spacing
/// runs `rho` lines per meter. Every fourth is always drawn, the other
/// even ones thin out between [`STREAM_THIN_FROM`] and [`STREAM_THIN_TO`],
/// the odd ones (between each pair) fill in from [`STREAM_FILL_FROM`] down
/// to [`STREAM_FILL_TO`].
pub fn line_weight(m: i32, rho: f32) -> f32 {
    let smooth = |a: f32, b: f32, x: f32| {
        let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    };
    match m.rem_euclid(4) {
        0 => 1.0,
        2 => 1.0 - smooth(STREAM_THIN_FROM, STREAM_THIN_TO, rho),
        _ => 1.0 - smooth(STREAM_FILL_TO, STREAM_FILL_FROM, rho),
    }
}

/// The lines per meter across the flow the weighted contours draw where
/// the full spacing runs `rho`: the tiers' densities times their weights.
pub fn lines_per_m(rho: f32) -> f32 {
    0.5 * rho * (line_weight(0, rho) + line_weight(2, rho)) + rho * line_weight(1, rho)
}

/// The flow at `uv`: the curl of [`stream`], `(∂ψ/∂v, -∂ψ/∂u)` (m/m; the
/// shader runs the streaks along its direction at the slot's speed).
pub fn flow(uv: Vec2, feature: f32, clock: f32) -> Vec2 {
    let (_, g) = stream(uv, feature, clock);
    Vec2::new(g.y, -g.x)
}

/// The surfaces pass's shader: group 0 is the eye pass's shared camera
/// (`view_proj`), group 1 one slot's rows ([`rows`]). The vertex stage
/// places the quad from its corners and hands the fragment its (u, v) on
/// the face in meters; the fragment switches on the behavior id and
/// writes premultiplied light in the slot's color, at most its peak alpha.
pub const SURFACE_FX_WGSL: &str = r"
struct Eye { view_proj: mat4x4<f32> }
struct SurfaceFx {
    // -a-b, +a-b, +a+b, -a+b around the face
    corners: array<vec4<f32>, 4>,
    // x, y the half extents along the face's axes (m), z the lift (m),
    // w the behavior id
    face: vec4<f32>,
    // x strength 0..1, y the color index, z the audio band (0 rms, 1 bass,
    // 2 mid, 3 high), w the speed (m/s)
    params: vec4<f32>,
    // rgb the color, w the peak alpha
    color: vec4<f32>,
    // x rms, y bass, z the beat envelope, w the surfaces' clock (s)
    audio: vec4<f32>,
    // x mid, y high, z the downbeat envelope, w the bar phase
    audio2: vec4<f32>,
    // rings: x, y their origin (m), z the glow; streamlines and curls: w
    // the feature size (m)
    shape: vec4<f32>,
    // origin u, v (m), radius (m), intensity; 0 intensity for an empty slot
    rings: array<vec4<f32>, 8>,
}
@group(0) @binding(0) var<uniform> eye: Eye;
@group(1) @binding(0) var<uniform> fx: SurfaceFx;

const BEHAVIOR_RINGS: u32 = 4u;
const BEHAVIOR_STREAMLINES: u32 = 5u;
const BEHAVIOR_CURLS: u32 = 6u;
const BEHAVIOR_PULSE: u32 = 7u;
const BAND_BASS: u32 = 1u;
const BAND_MID: u32 = 2u;
const BAND_HIGH: u32 = 3u;
// The ripple's ring width and glow radius (m).
const RING_WIDTH: f32 = 0.15;
const GLOW_RADIUS: f32 = 0.6;
// The streamlines' contours per meter of the stream function, line width
// and soft edge (m), edge fade (m), level floor, drift (cells/s), and the
// streaks' size, travel per phase (m) and the line between them.
const CONTOURS_PER_M: f32 = 10.0;
const THIN_FROM: f32 = 12.0;
const THIN_TO: f32 = 20.0;
const FILL_FROM: f32 = 7.0;
const FILL_TO: f32 = 4.0;
const LINE_M: f32 = 0.006;
const LINE_SOFT_M: f32 = 0.0015;
const EDGE_M: f32 = 0.05;
const RMS_FLOOR: f32 = 0.6;
const DRIFT: f32 = 0.04;
const STREAK_M: f32 = 0.08;
const STREAK_TRAVEL_M: f32 = 0.3;
const STREAK_BASE: f32 = 0.35;
// The curls' drift (cells/s), their fill (a fraction of a line's light)
// and the |psi| (in feature sizes) it rises over.
const CURL_DRIFT: f32 = 0.1;
const CURL_FILL: f32 = 0.25;
const CURL_FILL_SOFT: f32 = 0.25;
// The pulse's light at the edges and its envelope's floor (times the
// band's level).
const PULSE_EDGE: f32 = 0.35;
const PULSE_FLOOR: f32 = 0.15;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VsOut {
    var order = array<u32, 6>(0u, 1u, 2u, 0u, 2u, 3u);
    var signs = array<vec2<f32>, 4>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0),
    );
    let k = order[i];
    var out: VsOut;
    out.pos = eye.view_proj * vec4<f32>(fx.corners[k].xyz, 1.0);
    out.uv = signs[k] * fx.face.xy;
    return out;
}

// The field library: a lattice hash, value noise with its gradient, the
// stream functions and their curl (surface_fx.rs has the same in Rust).
fn hash(c: vec2<i32>, seed: u32) -> f32 {
    var h = (bitcast<u32>(c.x) * 0x8da6b343u) ^ (bitcast<u32>(c.y) * 0xd8163841u)
        ^ (seed * 0xcb1ab31fu);
    h = (h ^ (h >> 16u)) * 0x7feb352du;
    h = (h ^ (h >> 15u)) * 0x846ca68bu;
    h = h ^ (h >> 16u);
    return f32(h >> 8u) * (2.0 / 16777215.0) - 1.0;
}

// Value noise at p (cells): x the value, yz its gradient.
fn noise(p: vec2<f32>, seed: u32) -> vec3<f32> {
    let i = floor(p);
    let f = p - i;
    let c = vec2<i32>(i);
    let u = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
    let du = 30.0 * f * f * (f * (f - 2.0) + 1.0);
    let a = hash(c, seed);
    let b = hash(c + vec2<i32>(1, 0), seed);
    let d = hash(c + vec2<i32>(0, 1), seed);
    let e = hash(c + vec2<i32>(1, 1), seed);
    let k1 = b - a;
    let k2 = d - a;
    let k3 = a - b - d + e;
    return vec3<f32>(
        a + k1 * u.x + k2 * u.y + k3 * u.x * u.y,
        du * vec2<f32>(k1 + k3 * u.y, k2 + k3 * u.x),
    );
}

// The stream function at uv (m) for a field of `feature` size: x psi (m),
// yz its gradient.
fn stream(uv: vec2<f32>, feature: f32, clock: f32) -> vec3<f32> {
    let l = max(feature, 1e-3);
    let n1 = noise(uv / l + vec2<f32>(0.8, 0.6) * (DRIFT * clock), 1u);
    let n2 = noise(
        2.0 * uv / l + vec2<f32>(17.31, 5.13) + vec2<f32>(-0.6, 0.8) * (DRIFT * clock),
        2u,
    );
    return vec3<f32>(l * n1.x + 0.25 * l * n2.x, n1.yz + 0.5 * n2.yz);
}

// The curls' stream function: a third octave, their own seeds, a faster
// drift.
fn curl_stream(uv: vec2<f32>, feature: f32, clock: f32) -> vec3<f32> {
    let l = max(feature, 1e-3);
    let drift = CURL_DRIFT * clock;
    let n1 = noise(uv / l + vec2<f32>(0.8, 0.6) * drift, 5u);
    let n2 = noise(2.0 * uv / l + vec2<f32>(17.31, 5.13) + vec2<f32>(-0.6, 0.8) * drift, 6u);
    let n3 = noise(4.0 * uv / l + vec2<f32>(-9.27, 31.7) + vec2<f32>(-0.8, -0.6) * drift, 7u);
    return vec3<f32>(
        l * (n1.x + 0.25 * n2.x + 0.0625 * n3.x),
        n1.yz + 0.5 * n2.yz + 0.25 * n3.yz,
    );
}

// The curl of the stream function: the flow, divergence-free.
fn curl(grad: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(grad.y, -grad.x);
}

// A contour's weight by its tier (contour m of the half spacing): every
// fourth always, the other even ones thin out where the lines crowd, the
// odd ones fill in where they thin (rho: lines per meter at the spacing).
fn line_weight(m: i32, rho: f32) -> f32 {
    let tier = ((m % 4) + 4) % 4;
    if tier == 0 {
        return 1.0;
    }
    if tier == 2 {
        return 1.0 - smoothstep(THIN_FROM, THIN_TO, rho);
    }
    return 1.0 - smoothstep(FILL_TO, FILL_FROM, rho);
}

fn band_level() -> f32 {
    let band = u32(fx.params.z + 0.5);
    if band == BAND_BASS {
        return fx.audio.y;
    }
    if band == BAND_MID {
        return fx.audio2.x;
    }
    if band == BAND_HIGH {
        return fx.audio2.y;
    }
    return fx.audio.x;
}

// The fade over EDGE_M toward the face's edges.
fn edge_fade(uv: vec2<f32>) -> f32 {
    return smoothstep(0.0, EDGE_M, fx.face.x - abs(uv.x))
        * smoothstep(0.0, EDGE_M, fx.face.y - abs(uv.y));
}

fn rings_light(uv: vec2<f32>) -> f32 {
    var light = 0.0;
    for (var k = 0u; k < 8u; k++) {
        let r = fx.rings[k];
        // Empty and cut slots are 0: skip them (uniform across the draw).
        if r.w <= 0.0 {
            continue;
        }
        let d = (distance(uv, r.xy) - r.z) / RING_WIDTH;
        light += r.w * exp(-d * d);
    }
    let g = distance(uv, fx.shape.xy) / GLOW_RADIUS;
    light += fx.shape.z * exp(-g * g);
    return clamp(light, 0.0, 1.0);
}

// The contours of the stream function s (x psi, yz its gradient) as lines
// of light, 0..1. Meters to the nearest contour of psi at half the
// spacing: the lines are the field's streamlines, thin and even in width
// wherever the field runs, weighted by their tier so their density stays
// even.
fn contour_line(s: vec3<f32>, px: f32) -> f32 {
    let g = max(length(s.yz), 1e-4);
    let rho = CONTOURS_PER_M * g;
    let q = 2.0 * s.x * CONTOURS_PER_M;
    let m = round(q);
    let d = abs(q - m) / (2.0 * rho);
    let soft = max(LINE_SOFT_M, px);
    let half_w = 0.5 * LINE_M;
    return line_weight(i32(m), rho) * (1.0 - smoothstep(half_w - soft, half_w + soft, d));
}

// Streaks along the lines, advected along the flow at the speed: two
// phases half a travel apart, each fading in and out; the line's
// brightness, STREAK_BASE between them.
fn streaks(uv: vec2<f32>, s: vec3<f32>) -> f32 {
    let dir = curl(s.yz) / max(length(s.yz), 1e-4);
    let travel = fx.params.w * fx.audio.w / STREAK_TRAVEL_M;
    var streak = 0.0;
    for (var k = 0u; k < 2u; k++) {
        let tau = fract(travel + 0.5 * f32(k));
        let p = (uv - dir * (tau * STREAK_TRAVEL_M)) / STREAK_M;
        let n = noise(p, 3u + k).x;
        streak += (1.0 - abs(2.0 * tau - 1.0)) * smoothstep(-0.2, 0.6, n);
    }
    return STREAK_BASE + (1.0 - STREAK_BASE) * streak;
}

fn streamlines_light(uv: vec2<f32>, px: f32) -> f32 {
    let s = stream(uv, fx.shape.w, fx.audio.w);
    let line = contour_line(s, px);
    if line <= 0.001 {
        return 0.0;
    }
    let level = max(RMS_FLOOR, clamp(band_level(), 0.0, 1.0));
    return line * streaks(uv, s) * level * edge_fade(uv);
}

// The streamlines' lines on the curls' field, over a faint fill inside
// each closed contour: the smoothed sign of psi, so a curl reads as a disc
// of light with a bright rim.
fn curls_light(uv: vec2<f32>, px: f32) -> f32 {
    let s = curl_stream(uv, fx.shape.w, fx.audio.w);
    let x = s.x / (max(fx.shape.w, 1e-3) * CURL_FILL_SOFT);
    var light = CURL_FILL * abs(x) / sqrt(x * x + 1.0);
    let line = contour_line(s, px);
    if line > 0.001 {
        light = max(light, line * streaks(uv, s));
    }
    let level = max(RMS_FLOOR, clamp(band_level(), 0.0, 1.0));
    return light * level * edge_fade(uv);
}

// A whole-face glow: brightest at the center, on the downbeat envelope
// over a floor of the band's level.
fn pulse_light(uv: vec2<f32>) -> f32 {
    let r = length(uv / max(fx.face.xy, vec2<f32>(1e-3)));
    let radial = mix(1.0, PULSE_EDGE, smoothstep(0.0, 1.0, r));
    let env = max(fx.audio2.z, PULSE_FLOOR * clamp(band_level(), 0.0, 1.0));
    return radial * env * edge_fade(uv);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // The pixel's footprint on the face (m), for the lines' antialiasing:
    // taken before any branch, where the derivatives are defined.
    let px = length(fwidth(in.uv));
    let id = u32(fx.face.w + 0.5);
    var light = 0.0;
    if id == BEHAVIOR_RINGS {
        light = rings_light(in.uv);
    } else if id == BEHAVIOR_STREAMLINES {
        light = streamlines_light(in.uv, px);
    } else if id == BEHAVIOR_CURLS {
        light = curls_light(in.uv, px);
    } else if id == BEHAVIOR_PULSE {
        light = pulse_light(in.uv);
    }
    let a = fx.color.w * clamp(fx.params.x, 0.0, 1.0) * clamp(light, 0.0, 1.0);
    return vec4<f32>(fx.color.rgb * a, a);
}
";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surfaces::{KIND_CEILING, KIND_TABLE, KIND_WALL, acting_face};
    use glam::Quat;

    const HEAD: Vec3 = Vec3::new(0.3, 1.2, 0.4);

    /// A table as the runtime reports one: local +Z up, top at 0.75 m.
    fn table() -> Face {
        acting_face(
            Vec3::new(0.0, 0.4, -0.8),
            Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2),
            Vec3::new(0.6, 0.4, 0.35),
            HEAD,
        )
    }

    /// A scene floor plane at y = 0 (local +Z up), 4 x 3 m.
    fn floor() -> Face {
        acting_face(
            Vec3::new(1.0, -0.02, -1.0),
            Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2),
            Vec3::new(2.0, 1.5, 0.02),
            HEAD,
        )
    }

    fn slot(face: Face, kind: u32, behavior: SurfaceBehavior) -> Slot {
        Slot {
            face,
            kind,
            behavior,
            strength: 1.0,
            band: behavior.default_band(),
        }
    }

    fn module() -> naga::Module {
        let module = naga::front::wgsl::parse_str(SURFACE_FX_WGSL).expect("surface fx WGSL parses");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::default(),
        )
        .validate(&module)
        .expect("surface fx WGSL validates");
        module
    }

    /// A named `const` of the WGSL, as a float.
    fn wgsl_const(module: &naga::Module, name: &str) -> f32 {
        let (_, c) = module
            .constants
            .iter()
            .find(|(_, c)| c.name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("const {name}"));
        match module.global_expressions[c.init] {
            naga::Expression::Literal(naga::Literal::F32(v)) => v,
            naga::Expression::Literal(naga::Literal::U32(v)) => v as f32,
            ref e => panic!("{name}: {e:?}"),
        }
    }

    #[test]
    fn the_shader_validates_and_matches_the_rows_and_the_constants() {
        let module = module();
        let (_, ty) = module
            .types
            .iter()
            .find(|(_, t)| t.name.as_deref() == Some("SurfaceFx"))
            .expect("struct SurfaceFx");
        let naga::TypeInner::Struct { span, .. } = ty.inner else {
            panic!("SurfaceFx is not a struct");
        };
        assert_eq!(span as usize, UNIFORM_ROWS * 16);
        assert_eq!(MAX_RINGS, 8, "rings: array<vec4<f32>, 8>");
        let pairs = [
            ("BEHAVIOR_RINGS", SurfaceBehavior::Rings.id() as f32),
            (
                "BEHAVIOR_STREAMLINES",
                SurfaceBehavior::Streamlines.id() as f32,
            ),
            ("BEHAVIOR_CURLS", SurfaceBehavior::Curls.id() as f32),
            ("BEHAVIOR_PULSE", SurfaceBehavior::Pulse.id() as f32),
            ("BAND_BASS", BAND_BASS as f32),
            ("BAND_MID", BAND_MID as f32),
            ("BAND_HIGH", BAND_HIGH as f32),
            ("RING_WIDTH", crate::ripple::WIDTH_M),
            ("GLOW_RADIUS", crate::ripple::GLOW_RADIUS_M),
            ("CONTOURS_PER_M", STREAM_CONTOURS_PER_M),
            ("THIN_FROM", STREAM_THIN_FROM),
            ("THIN_TO", STREAM_THIN_TO),
            ("FILL_FROM", STREAM_FILL_FROM),
            ("FILL_TO", STREAM_FILL_TO),
            ("LINE_M", STREAM_LINE_M),
            ("LINE_SOFT_M", STREAM_LINE_SOFT_M),
            ("EDGE_M", STREAM_EDGE_M),
            ("RMS_FLOOR", STREAM_RMS_FLOOR),
            ("DRIFT", STREAM_DRIFT),
            ("STREAK_M", STREAK_M),
            ("STREAK_TRAVEL_M", STREAK_TRAVEL_M),
            ("STREAK_BASE", STREAK_BASE),
            ("CURL_DRIFT", CURL_DRIFT),
            ("CURL_FILL", CURL_FILL),
            ("CURL_FILL_SOFT", CURL_FILL_SOFT),
            ("PULSE_EDGE", PULSE_EDGE),
            ("PULSE_FLOOR", PULSE_FLOOR),
        ];
        for (name, value) in pairs {
            assert_close!(wgsl_const(&module, name), value);
        }
        // The Rust twin's octave constants are the WGSL's literals.
        for v in [OCTAVE2_OFFSET, OCTAVE3_OFFSET, DRIFT1, DRIFT2, DRIFT3] {
            let lit = format!("vec2<f32>({:?}, {:?})", v.x, v.y);
            assert!(SURFACE_FX_WGSL.contains(&lit), "{lit}");
        }
    }

    #[test]
    fn the_rows_carry_the_face_the_behavior_and_the_audio() {
        let face = table();
        let audio = Audio {
            rms: 0.3,
            bass: 0.6,
            beat: 0.5,
            clock: 12.5,
            mid: 0.2,
            high: 0.1,
            downbeat: 0.7,
            bar_phase: 0.25,
        };
        let s = Slot {
            strength: 0.7,
            ..slot(face, KIND_TABLE, SurfaceBehavior::Streamlines)
        };
        let rows = rows(&s, audio, None, 0.0);
        // The corners: the table's top (0.75 m) lifted, in order around it.
        for (row, c) in rows.iter().zip(face.corners(LIFT_M)) {
            assert_close!(*row, [c.x, c.y, c.z, 1.0]);
        }
        assert!(
            rows[..4]
                .iter()
                .all(|r| (r[1] - (0.75 + LIFT_M)).abs() < 1e-5)
        );
        let mut half = [rows[4][0], rows[4][1]];
        half.sort_by(f32::total_cmp);
        assert_close!(half, [0.4, 0.6]);
        assert_close!(rows[4][2..], [LIFT_M, 5.0]);
        assert_close!(rows[5], [0.7, KIND_TABLE as f32, 0.0, STREAM_SPEED_M_S]);
        let blue = crate::surfaces::palette(KIND_TABLE);
        assert_close!(rows[6], [blue[0], blue[1], blue[2], STREAM_PEAK_ALPHA]);
        assert_close!(rows[7], [0.3, 0.6, 0.5, 12.5]);
        assert_close!(rows[8], [0.2, 0.1, 0.7, 0.25]);
        assert_close!(rows[9], [0.0, 0.0, 0.0, STREAM_FEATURE_M]);
        assert!(rows[RINGS_ROW..].iter().all(|r| r[3] == 0.0));
        // The corners' (u, v) are the half extents, as the vertex stage
        // hands them to the fragment.
        let signs = [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]];
        for (c, s) in face.corners(0.0).iter().zip(signs) {
            let uv = face_uv(&face, *c);
            assert_close!([uv.x, uv.y], [s[0] * face.half[0], s[1] * face.half[1]]);
        }
        // A floor's streamlines are twice the size, violet; a strength
        // past 1 is clamped.
        let s = Slot {
            strength: 3.0,
            ..slot(floor(), KIND_FLOOR, SurfaceBehavior::Streamlines)
        };
        let rows = super::rows(&s, audio, None, 0.0);
        assert_close!(rows[5][0], 1.0);
        assert_close!(rows[9][3], STREAM_FLOOR_FEATURE_M);
        assert_close!(rows[6][..3], crate::surfaces::palette(KIND_FLOOR));
    }

    #[test]
    fn the_curls_and_pulse_rows_carry_their_shapes() {
        use crate::surfaces::{KIND_FRAME, KIND_OTHER};
        let audio = Audio {
            rms: 0.3,
            bass: 0.6,
            beat: 0.5,
            clock: 12.5,
            mid: 0.2,
            high: 0.1,
            downbeat: 0.7,
            bar_phase: 0.25,
        };
        // Curls on a chair (a volume, other): its top, the rms by default.
        let face = table();
        let s = Slot {
            strength: 0.8,
            ..slot(face, KIND_OTHER, SurfaceBehavior::Curls)
        };
        let rows = rows(&s, audio, None, 0.0);
        for (row, c) in rows.iter().zip(face.corners(LIFT_M)) {
            assert_close!(*row, [c.x, c.y, c.z, 1.0]);
        }
        assert_close!(rows[4], [face.half[0], face.half[1], LIFT_M, 6.0]);
        assert_close!(
            rows[5],
            [0.8, KIND_OTHER as f32, BAND_RMS as f32, CURL_SPEED_M_S]
        );
        let green = palette(KIND_OTHER);
        assert_close!(rows[6], [green[0], green[1], green[2], CURL_PEAK_ALPHA]);
        assert_close!(rows[7], [0.3, 0.6, 0.5, 12.5]);
        assert_close!(rows[8], [0.2, 0.1, 0.7, 0.25]);
        assert_close!(rows[9], [0.0, 0.0, 0.0, CURL_FEATURE_M]);
        assert_close!(rows[RINGS_ROW..], [[0.0f32; 4]; MAX_RINGS]);
        // On a floor the curls are larger; the band is the lane's, up to
        // the high.
        let s = Slot {
            band: BAND_MID,
            ..slot(floor(), KIND_FLOOR, SurfaceBehavior::Curls)
        };
        let rows = super::rows(&s, audio, None, 0.0);
        assert_close!(rows[9][3], CURL_FLOOR_FEATURE_M);
        assert_close!(rows[5][2], BAND_MID as f32);
        let s = Slot {
            band: 9,
            ..slot(floor(), KIND_FLOOR, SurfaceBehavior::Curls)
        };
        assert_close!(super::rows(&s, audio, None, 0.0)[5][2], BAND_HIGH as f32);
        // The pulse on a frame: the bass by default, no speed, no shape,
        // the frame's amber at the pulse's peak alpha.
        let s = slot(face, KIND_FRAME, SurfaceBehavior::Pulse);
        let rows = super::rows(&s, audio, None, 0.0);
        assert_close!(rows[4][3], 7.0);
        assert_close!(rows[5], [1.0, KIND_FRAME as f32, BAND_BASS as f32, 0.0]);
        let amber = palette(KIND_FRAME);
        assert_close!(rows[6], [amber[0], amber[1], amber[2], PULSE_PEAK_ALPHA]);
        assert_close!(rows[8], [0.2, 0.1, 0.7, 0.25]);
        assert_close!(rows[9], [0.0; 4]);
        assert_close!(rows[RINGS_ROW..], [[0.0f32; 4]; MAX_RINGS]);
    }

    #[test]
    fn the_curls_fill_is_zero_on_a_contour_and_tends_to_one_inside() {
        let l = CURL_FEATURE_M;
        assert_close!(curl_fill(0.0, l), 0.0);
        // Deep inside a curl (|psi| of a feature size): within 5 % of 1,
        // signed like psi.
        assert!(curl_fill(l, l) > 0.95, "{}", curl_fill(l, l));
        assert!(curl_fill(-l, l) < -0.95);
        // Rising with |psi|, odd.
        let mut last = -1.0;
        for i in -40..=40 {
            let f = curl_fill(i as f32 * 0.005, l);
            assert!(f > last, "{i}");
            assert!((f + curl_fill(-i as f32 * 0.005, l)).abs() < 1e-6);
            last = f;
        }
        // At a curl's center (a local extremum of psi) the fill is most of
        // the way up: find one on the field and check it.
        let h = 0.004;
        let psi = |p: Vec2| curl_stream(p, l, 0.0).0;
        let mut best = (0.0f32, Vec2::ZERO);
        for i in 0..100 {
            for j in 0..100 {
                let p = Vec2::new(i as f32, j as f32) * h;
                if psi(p).abs() > best.0.abs() {
                    best = (psi(p), p);
                }
            }
        }
        assert!(curl_fill(best.0, l).abs() > 0.9, "{best:?}");
    }

    /// The closed curls of `psi` (a field sampled on an `n` x `n` grid)
    /// whose contours `spacing` apart enclose them: the components of each
    /// superlevel set `{psi >= k spacing}` with nothing of the next level
    /// inside, for both signs.
    fn closed_curls(psi: &[f32], n: usize, spacing: f32) -> usize {
        let mut count = 0;
        let mut seen = vec![false; psi.len()];
        let mut stack = Vec::new();
        for sign in [1.0, -1.0] {
            let q: Vec<i32> = psi
                .iter()
                .map(|v| (sign * v / spacing).floor() as i32)
                .collect();
            let (lo, hi) = (*q.iter().min().unwrap(), *q.iter().max().unwrap());
            for k in lo..=hi {
                seen.fill(false);
                for start in 0..q.len() {
                    if q[start] < k || seen[start] {
                        continue;
                    }
                    let mut higher = false;
                    seen[start] = true;
                    stack.push(start);
                    while let Some(p) = stack.pop() {
                        higher |= q[p] > k;
                        let (x, y) = (p % n, p / n);
                        let around = [
                            (x + 1 < n).then(|| p + 1),
                            (x > 0).then(|| p - 1),
                            (y + 1 < n).then(|| p + n),
                            (y > 0).then(|| p - n),
                        ];
                        for o in around.into_iter().flatten() {
                            if q[o] >= k && !seen[o] {
                                seen[o] = true;
                                stack.push(o);
                            }
                        }
                    }
                    if !higher {
                        count += 1;
                    }
                }
            }
        }
        count
    }

    #[test]
    fn a_chair_seat_shows_four_to_six_closed_curls() {
        // A contour closes round a curl's center where the half spacing's
        // lines fill in (the field is slow there): count the curls those
        // contours enclose over 4 x 4 m and scale to a 0.4 m seat.
        let (n, h) = (500usize, 0.008f32);
        let side = n as f32 * h;
        for clock in [0.0, 40.0] {
            let psi: Vec<f32> = (0..n * n)
                .map(|k| {
                    let p = Vec2::new((k % n) as f32, (k / n) as f32) * h;
                    curl_stream(p, CURL_FEATURE_M, clock).0
                })
                .collect();
            let curls = closed_curls(&psi, n, 0.5 / STREAM_CONTOURS_PER_M);
            let per_seat = curls as f32 * 0.16 / (side * side);
            assert!((4.0..=6.0).contains(&per_seat), "{clock}: {per_seat}");
        }
    }

    #[test]
    fn the_curls_field_is_divergence_free_and_its_gradient_is_psis() {
        let h = 1e-3;
        for feature in [CURL_FEATURE_M, CURL_FLOOR_FEATURE_M] {
            for p in [
                Vec2::new(0.11, -0.23),
                Vec2::new(0.57, 0.31),
                Vec2::new(-1.3, 0.8),
            ] {
                let psi = |q: Vec2| curl_stream(q, feature, 3.0).0;
                let (_, g) = curl_stream(p, feature, 3.0);
                let num = Vec2::new(
                    (psi(p + Vec2::X * h) - psi(p - Vec2::X * h)) / (2.0 * h),
                    (psi(p + Vec2::Y * h) - psi(p - Vec2::Y * h)) / (2.0 * h),
                );
                assert!(g.distance(num) < 1e-2 * (1.0 + g.length()), "{g} vs {num}");
                // The curl of the gradient: tangent to the contours, and its
                // divergence vanishes against the field's rate of change.
                let v = |q: Vec2| {
                    let (_, g) = curl_stream(q, feature, 3.0);
                    Vec2::new(g.y, -g.x)
                };
                let div = (v(p + Vec2::X * h).x - v(p - Vec2::X * h).x) / (2.0 * h)
                    + (v(p + Vec2::Y * h).y - v(p - Vec2::Y * h).y) / (2.0 * h);
                assert!(div.abs() < 5e-2 / feature, "{feature} {p}: {div}");
            }
        }
    }

    #[test]
    fn the_pulse_jumps_on_the_downbeat_and_decays_in_two_seconds() {
        let dt = 1.0 / 72.0;
        let mut env = downbeat_step(0.0, true, 0.0);
        assert_close!(env, 1.0);
        // After a time constant (1 / 1.6 s), 1 / e of it.
        let mut t = 0.0;
        while t < 1.0 / DOWNBEAT_DECAY_PER_S - 1e-4 {
            env = downbeat_step(env, false, dt);
            t += dt;
        }
        assert!((env - (-1.0f32).exp()).abs() < 0.02, "{env}");
        // Under 0.05 by 2 s.
        while t < 2.0 {
            env = downbeat_step(env, false, dt);
            t += dt;
        }
        assert!(env < 0.05 && env > 0.03, "{env}");
        // The next downbeat restarts it; no time, no decay.
        assert_close!(downbeat_step(env, true, 0.0), 1.0);
        assert_close!(downbeat_step(0.5, false, -1.0), 0.5);
        // The envelope's floor: the band's level, so a frame never goes
        // black while the music plays.
        assert_close!(pulse_level(0.0, 0.8), PULSE_FLOOR * 0.8);
        assert_close!(pulse_level(0.9, 0.8), 0.9);
        assert_close!(pulse_level(0.0, 3.0), PULSE_FLOOR);
        // The falloff: 1 at the center, PULSE_EDGE at the edges and past
        // them, falling along the way.
        let half = [0.3, 0.5];
        assert_close!(pulse_radial(Vec2::ZERO, half), 1.0);
        assert_close!(pulse_radial(Vec2::new(0.3, 0.0), half), PULSE_EDGE);
        assert_close!(pulse_radial(Vec2::new(0.0, -0.5), half), PULSE_EDGE);
        assert_close!(pulse_radial(Vec2::new(0.3, 0.5), half), PULSE_EDGE);
        let mut last = 1.0 + 1e-6;
        for i in 0..=30 {
            let r = pulse_radial(Vec2::new(i as f32 * 0.01, 0.0), half);
            assert!(r < last, "{i}");
            last = r;
        }
    }

    #[test]
    fn the_rings_rows_are_the_ripples_on_a_floor() {
        let mut r = Ripple::new(2.0, 0.5);
        r.update(0.0, 0.01, HEAD, true, 1.0, 0.8);
        let face = floor();
        let rows = rows(
            &slot(face, KIND_FLOOR, SurfaceBehavior::Rings),
            Audio::default(),
            Some(&r),
            0.5,
        );
        assert_close!(rows[4][3], 4.0);
        // The ripple's warm white at its peak alpha times the gain.
        let c = crate::ripple::COLOR;
        assert_close!(rows[6], [c[0], c[1], c[2], crate::ripple::PEAK_ALPHA * 0.5]);
        assert_close!(rows[5][3], 2.0);
        // The origin under the head, on the floor's (u, v): the floor's
        // center is (1, 0, -1), so the head at (0.3, _, 0.4) is 0.7 m to
        // -x and 1.4 m to +z of it, whatever the axes' order and signs.
        let o = Vec2::new(rows[9][0], rows[9][1]);
        let world = face.center + face.axes[0] * o.x + face.axes[1] * o.y;
        assert!(
            world.abs_diff_eq(Vec3::new(HEAD.x, 0.0, HEAD.z), 1e-5),
            "{world}"
        );
        assert_close!(rows[9][2], r.glow());
        assert!(r.glow() > 0.0);
        // One live ring from there, 1 m out at 0.5 s.
        let live: Vec<_> = rows[RINGS_ROW..]
            .iter()
            .filter(|row| row[3] > 0.0)
            .collect();
        assert_eq!(live.len(), 1);
        assert_close!(live[0][..3], [o.x, o.y, 1.0]);
        assert!((live[0][3] - (-0.5 / crate::ripple::DECAY_S).exp()).abs() < 1e-6);
        // The light the shader sums is the ripple's: one ring 1 m out.
        let light = |p: Vec3| {
            let uv = face_uv(&face, p);
            let rings: f32 = rows[RINGS_ROW..]
                .iter()
                .filter(|r| r[3] > 0.0)
                .map(|r| {
                    let d = (uv.distance(Vec2::new(r[0], r[1])) - r[2]) / crate::ripple::WIDTH_M;
                    r[3] * (-d * d).exp()
                })
                .sum();
            let g = uv.distance(o) / crate::ripple::GLOW_RADIUS_M;
            rings + rows[9][2] * (-g * g).exp()
        };
        for p in [
            Vec3::new(HEAD.x + 1.0, 0.0, HEAD.z),
            Vec3::new(HEAD.x, 0.0, HEAD.z - 0.7),
            Vec3::new(1.5, 0.0, -2.0),
        ] {
            let expect = r.intensity(0.5, Vec2::new(p.x, p.z));
            assert!((light(p) - expect).abs() < 1e-5, "{p}");
        }
        // Without the ripple state (the `ripple` knob off): nothing lit.
        let off = rows_without_ripple(face);
        assert!(off[RINGS_ROW..].iter().all(|r| r[3] == 0.0));
        assert_close!(off[9][2], 0.0);
        assert_close!(off[6][3], 0.0);
    }

    fn rows_without_ripple(face: Face) -> [[f32; 4]; UNIFORM_ROWS] {
        rows(
            &slot(face, KIND_FLOOR, SurfaceBehavior::Rings),
            Audio::default(),
            None,
            0.5,
        )
    }

    #[test]
    fn the_rings_on_a_table_start_at_the_edge_nearest_the_head() {
        let mut r = Ripple::default();
        r.update(0.0, 0.01, HEAD, true, 1.0, 0.0);
        let face = table();
        // The table's top spans x -0.6..0.6, z -1.2..-0.4 at 0.75 m; the
        // head at (0.3, 1.2, 0.4) is past its near edge: the origin is
        // (0.3, 0.75, -0.4), on the edge.
        let o = rings_origin(&face, KIND_TABLE, HEAD);
        let world = face.center + face.axes[0] * o.x + face.axes[1] * o.y;
        assert!(
            world.abs_diff_eq(Vec3::new(0.3, 0.75, -0.4), 1e-5),
            "{world}"
        );
        let rows = rows(
            &slot(face, KIND_TABLE, SurfaceBehavior::Rings),
            Audio::default(),
            Some(&r),
            0.2,
        );
        assert_close!([rows[9][0], rows[9][1]], [o.x, o.y]);
        let live: Vec<_> = rows[RINGS_ROW..]
            .iter()
            .filter(|row| row[3] > 0.0)
            .collect();
        assert_eq!(live.len(), 1);
        assert_close!(live[0][..2], [o.x, o.y]);
        // The table's own color, not the ripple's warm white.
        assert_close!(rows[6][..3], crate::surfaces::palette(KIND_TABLE));
        // A floor would have put it under the head, past the table.
        let under = rings_origin(&face, KIND_FLOOR, HEAD);
        assert!(under.distance(o) > 0.7, "{under} {o}");
        // A wall: the point level with the eyes; a ceiling: over the head.
        let wall = acting_face(
            Vec3::new(0.0, 1.25, -2.0),
            Quat::IDENTITY,
            Vec3::new(2.0, 1.25, 0.02),
            HEAD,
        );
        let o = rings_origin(&wall, KIND_WALL, HEAD);
        let p = wall.center + wall.axes[0] * o.x + wall.axes[1] * o.y;
        assert!(p.abs_diff_eq(Vec3::new(0.3, 1.2, -1.98), 1e-5), "{p}");
        let ceiling = acting_face(
            Vec3::new(0.0, 2.6, 0.0),
            Quat::from_rotation_x(std::f32::consts::FRAC_PI_2),
            Vec3::new(3.0, 3.0, 0.02),
            HEAD,
        );
        assert!(ceiling.normal.abs_diff_eq(Vec3::NEG_Y, 1e-5));
        let o = rings_origin(&ceiling, KIND_CEILING, HEAD);
        let p = ceiling.center + ceiling.axes[0] * o.x + ceiling.axes[1] * o.y;
        assert!(p.abs_diff_eq(Vec3::new(0.3, 2.58, 0.4), 1e-5), "{p}");
    }

    #[test]
    fn a_floor_is_leveled_and_kept_above_the_stage_floor() {
        // A floor tilted 2 degrees, its top 3 cm under the stage floor's.
        let rot = Quat::from_rotation_z(2f32.to_radians())
            * Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
        let face = acting_face(
            Vec3::new(0.0, -0.05, 0.0),
            rot,
            Vec3::new(2.0, 1.5, 0.02),
            HEAD,
        );
        let level = level_floor(face, Some(0.0));
        assert!(level.normal.abs_diff_eq(Vec3::Y, 1e-6));
        assert_close!(level.center.y, 0.0);
        for a in level.axes {
            assert!(a.y.abs() < 1e-6 && (a.length() - 1.0).abs() < 1e-5, "{a}");
        }
        // The extents shrink with the flattened axes, as the ripple's quad.
        assert!((level.half[0] - 2.0 * 2f32.to_radians().cos()).abs() < 1e-4);
        assert!((level.half[1] - 1.5).abs() < 1e-5);
        // A floor above the stage floor keeps its height.
        let high = level_floor(face, Some(-0.5));
        assert!((high.center.y - face.center.y).abs() < 1e-6);
        assert!((level_floor(face, None).center.y - face.center.y).abs() < 1e-6);
    }

    #[test]
    fn the_curl_is_divergence_free_and_runs_along_the_lines() {
        let h = 1e-3;
        for feature in [STREAM_FEATURE_M, STREAM_FLOOR_FEATURE_M] {
            for clock in [0.0, 7.3, 250.0] {
                for (i, p) in [
                    Vec2::new(0.11, -0.23),
                    Vec2::new(0.57, 0.31),
                    Vec2::new(-1.3, 0.8),
                    Vec2::new(2.2, -3.1),
                    Vec2::new(0.0, 0.0),
                ]
                .into_iter()
                .enumerate()
                {
                    let v = |q: Vec2| flow(q, feature, clock);
                    let dvx = (v(p + Vec2::X * h).x - v(p - Vec2::X * h).x) / (2.0 * h);
                    let dvy = (v(p + Vec2::Y * h).y - v(p - Vec2::Y * h).y) / (2.0 * h);
                    // Against the field's own rate of change, which is
                    // about 1 / feature.
                    let scale = (dvx.abs() + dvy.abs()).max(1.0 / feature);
                    assert!(
                        (dvx + dvy).abs() < 2e-2 * scale,
                        "point {i} feature {feature} clock {clock}: div {} vs {scale}",
                        dvx + dvy
                    );
                    // The flow is tangent to psi's contours, the lines.
                    let (_, g) = stream(p, feature, clock);
                    assert!(v(p).dot(g).abs() < 1e-5);
                }
            }
        }
    }

    #[test]
    fn the_gradient_is_the_stream_functions_derivative() {
        // The analytic gradient the lines' width and the flow use against
        // central differences of psi, across cell borders too.
        let h = 1e-3;
        for p in [
            Vec2::new(0.35 * 3.0 + 1e-4, 0.2),
            Vec2::new(-0.9, 0.35 * 2.0 - 1e-4),
            Vec2::new(1.234, -0.567),
        ] {
            let psi = |q: Vec2| stream(q, STREAM_FEATURE_M, 3.0).0;
            let (_, g) = stream(p, STREAM_FEATURE_M, 3.0);
            let num = Vec2::new(
                (psi(p + Vec2::X * h) - psi(p - Vec2::X * h)) / (2.0 * h),
                (psi(p + Vec2::Y * h) - psi(p - Vec2::Y * h)) / (2.0 * h),
            );
            assert!(g.distance(num) < 5e-3 * (1.0 + g.length()), "{g} vs {num}");
        }
    }

    #[test]
    fn the_lines_run_eight_to_twelve_a_meter() {
        // The contours at the full spacing run CONTOURS_PER_M |grad psi|
        // lines per meter, a wide spread (3 to 23 between the tenth and the
        // ninetieth percentile over a table's field); the tiers even it
        // out: the median in the brief's 8 to 12 (10 on both), 6 to 14
        // between the tenth and the ninety-fifth percentile (only around
        // the field's stagnation points, where the contours ring a point,
        // do they thin further), so the lines are neither a haze nor
        // sparse.
        for feature in [STREAM_FEATURE_M, STREAM_FLOOR_FEATURE_M] {
            let mut density: Vec<f32> = Vec::new();
            for i in 0..80 {
                for j in 0..80 {
                    let p = Vec2::new(i as f32, j as f32) * 0.05 - Vec2::splat(2.0);
                    let rho = STREAM_CONTOURS_PER_M * stream(p, feature, 0.0).1.length();
                    density.push(lines_per_m(rho));
                }
            }
            density.sort_by(f32::total_cmp);
            let at = |q: f32| density[((density.len() - 1) as f32 * q) as usize];
            assert!((8.0..=12.0).contains(&at(0.5)), "{feature}: {}", at(0.5));
            assert!(at(0.1) >= 6.0 && at(0.95) <= 14.0, "{feature}");
        }
        // The tiers: every fourth contour always; the even ones between
        // gone where the lines crowd; the odd ones only where they thin.
        for m in [-8, -4, 0, 4, 12] {
            assert_close!(line_weight(m, 50.0), 1.0);
            assert_close!(line_weight(m, 1.0), 1.0);
        }
        for m in [-6, -2, 2, 6] {
            assert_close!(line_weight(m, 10.0), 1.0);
            assert_close!(line_weight(m, STREAM_THIN_TO), 0.0);
        }
        for m in [-3, -1, 1, 3, 5] {
            assert_close!(line_weight(m, STREAM_FILL_TO), 1.0);
            assert_close!(line_weight(m, 10.0), 0.0);
        }
        assert_close!(lines_per_m(10.0), 10.0);
    }

    #[test]
    fn the_noise_is_smooth_bounded_and_seeded() {
        let mut seen = [f32::MAX, f32::MIN];
        let mut differ = 0;
        for i in 0..200 {
            let p = Vec2::new(i as f32 * 0.137 - 10.0, i as f32 * 0.071 + 3.0);
            let (v, _) = noise(p, 1);
            assert!((-1.0..=1.0).contains(&v), "{v}");
            seen = [seen[0].min(v), seen[1].max(v)];
            if (noise(p, 2).0 - v).abs() > 0.01 {
                differ += 1;
            }
        }
        assert!(seen[0] < -0.4 && seen[1] > 0.4, "{seen:?}");
        // Another seed, another field.
        assert!(differ > 180, "{differ}");
        // Lattice points take the hash itself: continuous across cells.
        let (v, g) = noise(Vec2::new(3.0, -2.0), 1);
        assert_close!(v, hash(3, -2, 1));
        assert_close!([g.x, g.y], [0.0, 0.0]);
        let below = noise(Vec2::new(3.0 - 1e-4, -2.0), 1).0;
        assert!((below - v).abs() < 1e-3);
    }

    #[test]
    fn the_clock_runs_faster_on_the_beat() {
        let mut c = Clock::default();
        for _ in 0..72 {
            c.advance(1.0 / 72.0, 0.0);
        }
        assert!((c.seconds() - 1.0).abs() < 1e-5);
        for _ in 0..72 {
            c.advance(1.0 / 72.0, 1.0);
        }
        assert!((c.seconds() - 3.0).abs() < 1e-5);
        // Never backward.
        c.advance(-1.0, 5.0);
        assert!((c.seconds() - 3.0).abs() < 1e-5);
    }
}
