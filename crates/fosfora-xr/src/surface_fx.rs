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
//! the curls' feature size, the spectrum's sign, bar count, glow and bar
//! fill); the rings, one row per ring slot (origin u, v, radius,
//! intensity); then the spectrum's bar heights, four to a row (zeros on
//! any other face). Every position is in the face's
//! (u, v): meters along its two axes from its center, which the vertex
//! stage hands the fragment, so one shader works on any face at any
//! orientation (the contours also take the fragment's world position, and
//! their `shape` is the land's clock, level, breath and traveling line's
//! height, with the high land's color and that line's light in the first
//! ring's row). The audio band ([`BAND_RMS`] to [`BAND_HIGH`]) is the
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
//! **Spectrum** (id 3): the wall spectrum (`canvas.rs`, which keeps the
//! bars and the wall pick), folded into the pass in D2: the canvas's own
//! pipeline and draw are gone. Only the wall `canvas::WallPick` chooses is
//! a spectrum slot; its face is oriented by `canvas::spectrum_face` (v up
//! the wall, the bars along u times the sign in `shape.x`, low bands on
//! the wearer's left). The look is the canvas's: warm white with the
//! color index 0 whatever the kind, the cap line, the base glow, peak
//! alpha 0.25 times `canvasgain`.
//!
//! **Pulse** (id 7): a whole-face glow for frames, lamps and anything
//! small: a radial falloff from the face's center ([`pulse_radial`], 1 at
//! the center, [`PULSE_EDGE`] at the edges, the 5 cm edge fade kept) times
//! the downbeat envelope ([`downbeat_step`]: 1 on the downbeat, a 0.6 s
//! time constant) over a floor of [`PULSE_FLOOR`] times the band's level
//! ([`pulse_level`]), so a frame breathes with the bar and never goes
//! black while the music plays. Peak alpha [`PULSE_PEAK_ALPHA`].
//!
//! **Contours** (id 16, board #5533): contour lines, as on a topographic
//! map, over one landscape that fills the room. The land is a height field
//! in world space ([`land`]): three sines of the fragment's position, two
//! of them with their phase bent by a fourth and a fifth, so the hills are
//! not a grid. Every face samples it where it stands, so a line that
//! reaches a wall's edge goes on across the next wall, the floor or the
//! ceiling, and no face fades toward its edges. A line runs wherever the
//! height crosses a multiple of [`LAND_INTERVAL`]. Its distance in meters
//! comes from the height's own gradient along the face, worked out with
//! the height at no extra sine, so a line is [`STREAM_LINE_M`] wide on a
//! steep slope and on a gentle one. Every [`LAND_INDEX_EVERY`]th line is
//! an index line, always drawn; the lines between show as the level rises
//! past [`LAND_FEW_FROM`] and are dimmer ([`LAND_BETWEEN`]). The color
//! runs from the surface's own on low land to [`LAND_HIGH_COLOR`] on high
//! land. A fragment costs ten sines and cosines, five dot products and
//! three smoothsteps, with no texture read, no hash and no loop, against
//! the streamlines' eight lattice hashes off a line and sixteen on one.
//!
//! **The land moves gently** ([`Terrain`], stepped once a frame). It
//! drifts on its own clock, which the beat does not speed up, a few
//! centimeters a second. The music reaches it only through values that
//! glide: the band's level over [`LAND_LEVEL_S`] and the bass over
//! [`LAND_BREATH_S`]. The level raises the land by up to [`LAND_TIDE`], so
//! the lines spread outward from the peaks like a tide over a map, lights
//! the lines between and widens every line a little. The bass swells the
//! relief by [`LAND_BREATH`]. On a downbeat, at most once in
//! [`LAND_TRAVEL_GAP_S`], one faint line sets out from the low land and
//! climbs to the peaks over [`LAND_TRAVEL_S`], fading in and out.
//!
//! **The clock** ([`Clock`]) runs at 1 + the beat envelope, so the flow
//! and the drift double on the beat and settle back, without the jumps a
//! speed multiplied into the time would make.

use glam::{Vec2, Vec3};

use crate::canvas::{Canvas, MAX_BARS};
use crate::ripple::{MAX_RINGS, Ripple};
use crate::surfaces::{Face, KIND_FLOOR, SurfaceBehavior, surface_color};

/// Lit faces the pass holds, one per obstacle box.
pub const MAX_SLOTS: usize = crate::surfaces::SURFACE_LANE_ROWS;
/// How far off the face the quad sits (m), plus the pipeline's depth bias:
/// the ripple's, as the surface's occluder writes depth at the face.
pub const LIFT_M: f32 = crate::ripple::LIFT_M;
/// Rows of [`rows`], `struct SurfaceFx` in [`SURFACE_FX_WGSL`]: the
/// corners, `face`, `params`, `color`, `audio`, `audio2`, `shape`, the
/// rings, then the spectrum's bars.
pub const UNIFORM_ROWS: usize = BARS_ROW + MAX_BARS / 4;
/// The row of the first ring and of the first four bars.
const RINGS_ROW: usize = 10;
const BARS_ROW: usize = RINGS_ROW + MAX_RINGS;
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
/// The contours' alpha at full strength and full level: the streamlines'.
pub const CONTOUR_PEAK_ALPHA: f32 = STREAM_PEAK_ALPHA;
/// The height between two contour lines (the land's height runs from -1
/// to 1), and how many lines from one index line to the next.
pub const LAND_INTERVAL: f32 = 0.125;
pub const LAND_INDEX_EVERY: u32 = 5;
/// How far a full level raises the land (two intervals), and how much a
/// full bass swells its relief.
pub const LAND_TIDE: f32 = 0.25;
pub const LAND_BREATH: f32 = 0.12;
/// The level the lines between the index lines start to show at and are
/// all lit by, and their light against an index line's.
pub const LAND_FEW_FROM: f32 = 0.15;
pub const LAND_FEW_TO: f32 = 0.6;
pub const LAND_BETWEEN: f32 = 0.6;
/// A line's width with the music quiet against its width at full level,
/// and its light.
pub const LAND_THIN: f32 = 0.6;
pub const LAND_FLOOR: f32 = 0.35;
/// The gentlest slope a line's width is worked out with (1/m): on a
/// summit's flat the line stays a line.
pub const LAND_SLOPE_MIN: f32 = 0.15;
/// The tint runs from the low land's color at minus this height to the
/// high land's at plus it.
pub const LAND_TINT_HALF: f32 = 0.5;
/// The high land's color; the low land's is the surface's own.
pub const LAND_HIGH_COLOR: [f32; 3] = crate::surfaces::WARM_WHITE;
/// The land's waves (rad/m), each sine's weight, how far the first two
/// bend (rad), the bending waves, each phase's drift (rad per second of
/// the land's clock) and the second and third sines' offsets: the WGSL's
/// `land` literals.
const LAND_K: [Vec3; 3] = [
    Vec3::new(1.52, 0.72, 1.04),
    Vec3::new(-1.12, 1.68, 2.08),
    Vec3::new(3.12, -2.48, 1.92),
];
const LAND_WEIGHT: [f32; 3] = [0.5, 0.32, 0.18];
const LAND_BEND: [f32; 2] = [1.1, 0.8];
const LAND_Q: [Vec3; 2] = [Vec3::new(-0.56, 0.88, 1.28), Vec3::new(1.6, -0.48, 0.72)];
const LAND_DRIFT: [f32; 5] = [0.13, 0.07, -0.11, -0.05, 0.09];
const LAND_OFFSET: [f32; 2] = [1.7, 4.1];
/// The land's clock wraps here (s): every drift is a whole number of
/// hundredths of a radian a second, so every phase has come round.
pub const LAND_CLOCK_WRAP_S: f64 = 200.0 * std::f64::consts::PI;
/// How long the level and the bass take to reach the land (s, one pole).
pub const LAND_LEVEL_S: f32 = 1.5;
pub const LAND_BREATH_S: f32 = 0.8;
/// The traveling line: how long it climbs (s), the least time from one
/// setting out to the next (s), the heights it climbs between, its light
/// at most against an index line's, and the level under which none sets
/// out.
pub const LAND_TRAVEL_S: f32 = 7.0;
pub const LAND_TRAVEL_GAP_S: f32 = 12.0;
pub const LAND_TRAVEL_HEIGHTS: [f32; 2] = [-0.9, 0.9];
pub const LAND_TRAVEL_LIGHT: f32 = 0.35;
pub const LAND_TRAVEL_MIN_LEVEL: f32 = 0.25;
/// The audio band a surface follows (`params.z`): the rms, the bass, the
/// mid, the high (the larger of presence and brilliance).
pub const BAND_RMS: u32 = 0;
pub const BAND_BASS: u32 = 1;
pub const BAND_MID: u32 = 2;
pub const BAND_HIGH: u32 = 3;
/// How many bands there are.
pub const BANDS: u32 = 4;
/// What the hand menu calls each band (board #3472, D3), in band order.
pub const BAND_NAMES: [&str; BANDS as usize] = ["rms", "bass", "mid", "high"];

/// Band `band`'s name as the hand menu shows it ([`BAND_NAMES`]); `?` past
/// the last band.
pub fn band_name(band: u32) -> &'static str {
    BAND_NAMES.get(band as usize).copied().unwrap_or("?")
}

/// The audio a slot carries: the rms, the bass, the mid and the high
/// (0..1), the beat envelope (1 on the beat, decaying), the surfaces'
/// [`Clock`], the downbeat envelope ([`downbeat_step`]) and the bar phase
/// (0..1 over the bar); and the key's tint (`surfaces::key_tint`), the
/// color of index `surfaces::COLOR_KEY`.
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
    pub key_tint: [f32; 3],
    /// The contours' land this frame ([`Terrain::land`]).
    pub land: Land,
}

/// The contours' land as a frame draws it ([`Terrain::land`]): its clock
/// (s, wrapped), each band's level as it reached the land, the bass as it
/// did, and the traveling line's height and light (0: none).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Land {
    pub clock: f32,
    pub levels: [f32; BANDS as usize],
    pub breath: f32,
    pub travel: f32,
    pub travel_light: f32,
}

/// The contours' land from frame to frame (board #5533): its own clock,
/// the music as it reaches it, and the one line that travels. One for the
/// room, since the land is one.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Terrain {
    clock: f64,
    levels: [f32; BANDS as usize],
    breath: f32,
    /// The traveling line's age (s).
    travel: Option<f32>,
    /// Seconds since the last one set out.
    since: f32,
}

impl Terrain {
    /// `dt` seconds on. The clock runs at `motion` (the flash limit's
    /// share of the looks' own speed, 1 as a rule) and not with the beat.
    /// `levels` are the bands' (rms, bass, mid, high) as the flash limit
    /// lets them through; each glides to the land over [`LAND_LEVEL_S`],
    /// the bass also as the breath over [`LAND_BREATH_S`]. A `downbeat`
    /// sends the traveling line out when none travels, the last set out
    /// [`LAND_TRAVEL_GAP_S`] ago or more and the rms has reached
    /// [`LAND_TRAVEL_MIN_LEVEL`].
    pub fn step(&mut self, dt: f32, motion: f32, levels: [f32; BANDS as usize], downbeat: bool) {
        let dt = dt.max(0.0);
        self.clock =
            (self.clock + f64::from(dt * motion.clamp(0.0, 1.0))).rem_euclid(LAND_CLOCK_WRAP_S);
        let toward = |now: f32, want: f32, secs: f32| {
            now + (want.clamp(0.0, 1.0) - now) * (1.0 - (-dt / secs).exp())
        };
        for (now, want) in self.levels.iter_mut().zip(levels) {
            *now = toward(*now, want, LAND_LEVEL_S);
        }
        self.breath = toward(self.breath, levels[BAND_BASS as usize], LAND_BREATH_S);
        self.since += dt;
        self.travel = self
            .travel
            .map(|age| age + dt)
            .filter(|age| *age < LAND_TRAVEL_S);
        if downbeat
            && self.travel.is_none()
            && self.since >= LAND_TRAVEL_GAP_S
            && self.levels[BAND_RMS as usize] >= LAND_TRAVEL_MIN_LEVEL
        {
            self.travel = Some(0.0);
            self.since = 0.0;
        }
    }

    /// The land as this frame draws it. The traveling line climbs evenly
    /// between [`LAND_TRAVEL_HEIGHTS`] and its light is a half sine over
    /// its climb, squared: it leaves and arrives at no light.
    pub fn land(&self) -> Land {
        let (travel, travel_light) = self.travel.map_or((0.0, 0.0), |age| {
            let x = (age / LAND_TRAVEL_S).clamp(0.0, 1.0);
            let [from, to] = LAND_TRAVEL_HEIGHTS;
            let s = (std::f32::consts::PI * x).sin();
            (from + (to - from) * x, LAND_TRAVEL_LIGHT * s * s)
        });
        Land {
            clock: self.clock as f32,
            levels: self.levels,
            breath: self.breath,
            travel,
            travel_light,
        }
    }
}

/// The land's height at world point `p` (m) at `clock` (s), about -1 to 1,
/// and its gradient (1/m). The WGSL's `land`.
pub fn land(p: Vec3, clock: f32) -> (f32, Vec3) {
    let [k1, k2, k3] = LAND_K;
    let [q1, q2] = LAND_Q;
    let [w1, w2, w3] = LAND_WEIGHT;
    let t1 = q1.dot(p) + LAND_DRIFT[0] * clock;
    let f1 = k1.dot(p) + LAND_BEND[0] * t1.sin() + LAND_DRIFT[1] * clock;
    let t2 = q2.dot(p) + LAND_DRIFT[2] * clock;
    let f2 = k2.dot(p) + LAND_BEND[1] * t2.sin() + LAND_DRIFT[3] * clock + LAND_OFFSET[0];
    let f3 = k3.dot(p) + LAND_DRIFT[4] * clock + LAND_OFFSET[1];
    (
        w1 * f1.sin() + w2 * f2.sin() + w3 * f3.sin(),
        w1 * f1.cos() * (k1 + LAND_BEND[0] * t1.cos() * q1)
            + w2 * f2.cos() * (k2 + LAND_BEND[1] * t2.cos() * q2)
            + w3 * f3.cos() * k3,
    )
}

/// The contours' light, 0 to 1, where the land is `height` high and
/// `slope` steep along the face (1/m, [`land`]'s gradient less its part
/// along the face's normal), for a land of `level`, `breath` and
/// traveling line `travel` (its height and its light), on a pixel `px`
/// meters across: the WGSL's `contours_light`.
pub fn contour_light(
    height: f32,
    slope: f32,
    level: f32,
    breath: f32,
    travel: [f32; 2],
    px: f32,
) -> f32 {
    let smooth = |a: f32, b: f32, x: f32| {
        let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    };
    let level = level.clamp(0.0, 1.0);
    let amp = 1.0 + LAND_BREATH * breath.clamp(0.0, 1.0);
    let slope = (amp * slope).max(LAND_SLOPE_MIN);
    let h = amp * height + LAND_TIDE * level;
    let v = h / LAND_INTERVAL;
    let m = v.round();
    let soft = STREAM_LINE_SOFT_M.max(px);
    let half_w = 0.5 * STREAM_LINE_M * (LAND_THIN + (1.0 - LAND_THIN) * level);
    let line = |d: f32| 1.0 - smooth(half_w - soft, half_w + soft, d);
    let weight = if (m as i32).rem_euclid(LAND_INDEX_EVERY as i32) == 0 {
        1.0
    } else {
        LAND_BETWEEN * smooth(LAND_FEW_FROM, LAND_FEW_TO, level)
    };
    let light = (weight * line((v - m).abs() * LAND_INTERVAL / slope))
        .max(travel[1] * line((h - travel[0]).abs() / slope));
    light * (LAND_FLOOR + (1.0 - LAND_FLOOR) * level)
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

/// One lit face: the face the behavior acts on, the surface's kind, its
/// lane's behavior and strength, and the lane's color index and audio band
/// (`surfaces::lane_params`): the color 0 the kind's own, 1 to 7 the
/// surface palette, 8 the key's tint (`surfaces::surface_color`); the band
/// [`BAND_RMS`] to [`BAND_HIGH`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Slot {
    pub face: Face,
    pub kind: u32,
    pub behavior: SurfaceBehavior,
    pub strength: f32,
    pub color: u32,
    pub band: u32,
}

/// What a spectrum slot draws: the bars' heights and the bottom glow (a
/// [`Canvas`]'s, or what the flash limiter lets through of them,
/// `flash::follower`), the canvas's gain, and the sign of its face's `u`
/// (`canvas::spectrum_face`).
#[derive(Debug, Clone, Copy)]
pub struct Spectrum<'a> {
    pub heights: &'a [f32],
    pub glow: f32,
    pub gain: f32,
    pub sign: f32,
}

impl<'a> Spectrum<'a> {
    /// `canvas`'s bars as they are, on a face whose `u` has sign `sign`.
    pub fn of(canvas: &'a Canvas, sign: f32) -> Self {
        Self {
            heights: canvas.heights(),
            glow: canvas.glow(),
            gain: canvas.gain,
            sign,
        }
    }
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

/// The color a slot draws in: its color index's (`surfaces::surface_color`,
/// `key` the key's tint), but with index 0 the rings on a floor keep the
/// ripple's warm white, so the floor looks as it did, and the spectrum the
/// canvas's on any face.
pub fn color_of(kind: u32, behavior: SurfaceBehavior, color: u32, key: [f32; 3]) -> [f32; 3] {
    match behavior {
        SurfaceBehavior::Rings if color == 0 && kind == KIND_FLOOR => crate::ripple::COLOR,
        SurfaceBehavior::Spectrum if color == 0 => crate::canvas::COLOR,
        _ => surface_color(color, kind, key),
    }
}

/// The uniform rows of `slot` at time `t` (`struct SurfaceFx` in
/// [`SURFACE_FX_WGSL`], the layout in the module docs). `ripple` is the
/// rings' state and `spectrum` the bars; without them a rings or a
/// spectrum slot draws nothing.
pub fn rows(
    slot: &Slot,
    audio: Audio,
    ripple: Option<&Ripple>,
    spectrum: Option<Spectrum<'_>>,
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
    let color = color_of(slot.kind, slot.behavior, slot.color, audio.key_tint);
    let (speed, peak) = match slot.behavior {
        SurfaceBehavior::Rings => (
            ripple.map_or(crate::ripple::SPEED_M_S, |r| r.speed),
            crate::ripple::PEAK_ALPHA * ripple.map_or(0.0, |r| r.gain),
        ),
        SurfaceBehavior::Curls => (CURL_SPEED_M_S, CURL_PEAK_ALPHA),
        SurfaceBehavior::Pulse => (0.0, PULSE_PEAK_ALPHA),
        SurfaceBehavior::Contours => (0.0, CONTOUR_PEAK_ALPHA),
        SurfaceBehavior::Spectrum => (
            0.0,
            crate::canvas::PEAK_ALPHA * spectrum.map_or(0.0, |s| s.gain),
        ),
        _ => (STREAM_SPEED_M_S, STREAM_PEAK_ALPHA),
    };
    rows[5] = [
        slot.strength.clamp(0.0, 1.0),
        slot.color as f32,
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
        SurfaceBehavior::Spectrum => {
            if let Some(s) = spectrum {
                let heights = s.heights;
                rows[9] = [
                    s.sign,
                    heights.len() as f32,
                    s.glow,
                    crate::canvas::BAR_FILL,
                ];
                for (i, h) in heights.iter().take(MAX_BARS).enumerate() {
                    rows[BARS_ROW + i / 4][i % 4] = *h;
                }
            }
        }
        SurfaceBehavior::Pulse => {}
        SurfaceBehavior::Contours => {
            let land = audio.land;
            rows[9] = [
                land.clock,
                land.levels[slot.band.min(BANDS - 1) as usize],
                land.breath,
                land.travel,
            ];
            let [r, g, b] = LAND_HIGH_COLOR;
            rows[RINGS_ROW] = [r, g, b, land.travel_light];
        }
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
    // the feature size (m); spectrum: x the sign of u toward the wearer's
    // right, y the bar count, z the bottom glow, w the bar fill; contours:
    // x the land's clock (s), y its level, z its breath, w the traveling
    // line's height
    shape: vec4<f32>,
    // origin u, v (m), radius (m), intensity; 0 intensity for an empty slot
    // (contours: the first row is the high land's color, w the traveling
    // line's light)
    rings: array<vec4<f32>, 8>,
    // the spectrum's bar heights 0..1, four per row, from the wearer's left
    bars: array<vec4<f32>, 16>,
}
@group(0) @binding(0) var<uniform> eye: Eye;
@group(1) @binding(0) var<uniform> fx: SurfaceFx;

const BEHAVIOR_SPECTRUM: u32 = 3u;
const BEHAVIOR_RINGS: u32 = 4u;
const BEHAVIOR_STREAMLINES: u32 = 5u;
const BEHAVIOR_CURLS: u32 = 6u;
const BEHAVIOR_PULSE: u32 = 7u;
const BEHAVIOR_CONTOURS: u32 = 16u;
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
// The contours' land: the height between two lines and the lines from one
// index line to the next, how far the level raises it and the bass swells
// it, the level the lines between show from and to and their light, a
// quiet line's width and light, the gentlest slope a width is worked out
// with (1/m), and the height the tint runs to either side of 0.
const LAND_INTERVAL: f32 = 0.125;
const LAND_INDEX_EVERY: u32 = 5u;
const LAND_TIDE: f32 = 0.25;
const LAND_BREATH: f32 = 0.12;
const LAND_FEW_FROM: f32 = 0.15;
const LAND_FEW_TO: f32 = 0.6;
const LAND_BETWEEN: f32 = 0.6;
const LAND_THIN: f32 = 0.6;
const LAND_FLOOR: f32 = 0.35;
const LAND_SLOPE_MIN: f32 = 0.15;
const LAND_TINT_HALF: f32 = 0.5;
// The spectrum's (the canvas's): the softness of a bar's sides (fraction
// of its slot) and of its top and cap (fraction of the wall's height),
// and the bottom glow's height.
const SIDE_SOFT: f32 = 0.12;
const TOP_SOFT: f32 = 0.015;
const CAP_WIDTH: f32 = 0.012;
const GLOW_HEIGHT: f32 = 0.03;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    // The point of the face in the world (m), for the contours' land.
    @location(1) world: vec3<f32>,
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
    out.world = fx.corners[k].xyz;
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

// The land: x its height at world point p (m) at clock tau (s), about -1
// to 1, yzw its gradient (1/m). Three sines of the position, the first two
// with their phase bent by another, each drifting its own way.
fn land(p: vec3<f32>, tau: f32) -> vec4<f32> {
    let k1 = vec3<f32>(1.52, 0.72, 1.04);
    let q1 = vec3<f32>(-0.56, 0.88, 1.28);
    let k2 = vec3<f32>(-1.12, 1.68, 2.08);
    let q2 = vec3<f32>(1.6, -0.48, 0.72);
    let k3 = vec3<f32>(3.12, -2.48, 1.92);
    let t1 = dot(q1, p) + 0.13 * tau;
    let f1 = dot(k1, p) + 1.1 * sin(t1) + 0.07 * tau;
    let t2 = dot(q2, p) + -0.11 * tau;
    let f2 = dot(k2, p) + 0.8 * sin(t2) + -0.05 * tau + 1.7;
    let f3 = dot(k3, p) + 0.09 * tau + 4.1;
    return vec4<f32>(
        0.5 * sin(f1) + 0.32 * sin(f2) + 0.18 * sin(f3),
        0.5 * cos(f1) * (k1 + 1.1 * cos(t1) * q1)
            + 0.32 * cos(f2) * (k2 + 0.8 * cos(t2) * q2)
            + 0.18 * cos(f3) * k3,
    );
}

// The contours: x a line's light where the land's height, raised by the
// level and swelled by the bass, crosses a multiple of the interval, or
// where it crosses the traveling line's height; y how high the land is,
// 0 on low land to 1 on high, for the tint. The distance to a line in
// meters is the height's distance over the slope along the face, so a
// line is as wide on a steep slope as on a gentle one. Index lines are
// always drawn; the lines between show as the level rises.
fn contours_light(world: vec3<f32>, px: f32) -> vec2<f32> {
    let level = clamp(fx.shape.y, 0.0, 1.0);
    let amp = 1.0 + LAND_BREATH * clamp(fx.shape.z, 0.0, 1.0);
    let l = land(world, fx.shape.x);
    let n = normalize(cross(
        fx.corners[1].xyz - fx.corners[0].xyz,
        fx.corners[3].xyz - fx.corners[0].xyz,
    ));
    let across = dot(l.yzw, n);
    let slope = max(amp * sqrt(max(dot(l.yzw, l.yzw) - across * across, 0.0)), LAND_SLOPE_MIN);
    let h = amp * l.x + LAND_TIDE * level;
    let v = h / LAND_INTERVAL;
    let m = round(v);
    let soft = max(LINE_SOFT_M, px);
    let half_w = 0.5 * LINE_M * mix(LAND_THIN, 1.0, level);
    let every = i32(LAND_INDEX_EVERY);
    var weight = LAND_BETWEEN * smoothstep(LAND_FEW_FROM, LAND_FEW_TO, level);
    if ((i32(m) % every) + every) % every == 0 {
        weight = 1.0;
    }
    let d = abs(v - m) * LAND_INTERVAL / slope;
    let dt = abs(h - fx.shape.w) / slope;
    let light = max(
        weight * (1.0 - smoothstep(half_w - soft, half_w + soft, d)),
        fx.rings[0].w * (1.0 - smoothstep(half_w - soft, half_w + soft, dt)),
    );
    return vec2<f32>(
        light * mix(LAND_FLOOR, 1.0, level),
        smoothstep(-LAND_TINT_HALF, LAND_TINT_HALF, l.x),
    );
}

// The wall spectrum: bars of light climbing the face, the bar index along
// u times the sign (low bands on the wearer's left), the height along v,
// brightest at the bar's top with a cap line, over a glow along the
// bottom.
fn spectrum_light(uv: vec2<f32>) -> f32 {
    let n = max(fx.shape.y, 1.0);
    let across = 0.5 + 0.5 * fx.shape.x * uv.x / max(fx.face.x, 1e-4);
    let v = 0.5 + 0.5 * uv.y / max(fx.face.y, 1e-4);
    let x = clamp(across, 0.0, 0.99999) * n;
    let i = min(u32(x), u32(n) - 1u);
    let f = fract(x);
    let gap = (1.0 - fx.shape.w) * 0.5;
    let side = smoothstep(gap, gap + SIDE_SOFT, f)
        * (1.0 - smoothstep(1.0 - gap - SIDE_SOFT, 1.0 - gap, f));
    let h = fx.bars[i / 4u][i % 4u];
    // Below the top, brighter toward it: light climbing the wall.
    let below = 1.0 - smoothstep(h - TOP_SOFT, h, v);
    let grad = mix(0.2, 1.0, clamp(v / max(h, 1e-3), 0.0, 1.0));
    let c = (v - h) / CAP_WIDTH;
    let cap = exp(-c * c) * smoothstep(0.0, 0.02, h);
    let light = side * (0.7 * below * grad + 0.6 * cap) + fx.shape.z * exp(-v / GLOW_HEIGHT);
    return clamp(light, 0.0, 1.0);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // The pixel's footprint on the face (m), for the lines' antialiasing:
    // taken before any branch, where the derivatives are defined.
    let px = length(fwidth(in.uv));
    let id = u32(fx.face.w + 0.5);
    var light = 0.0;
    var rgb = fx.color.rgb;
    if id == BEHAVIOR_CONTOURS {
        // Low land in the surface's color, high land in the land's own.
        let c = contours_light(in.world, px);
        light = c.x;
        rgb = mix(rgb, fx.rings[0].rgb, c.y);
    } else if id == BEHAVIOR_RINGS {
        light = rings_light(in.uv);
    } else if id == BEHAVIOR_STREAMLINES {
        light = streamlines_light(in.uv, px);
    } else if id == BEHAVIOR_CURLS {
        light = curls_light(in.uv, px);
    } else if id == BEHAVIOR_PULSE {
        light = pulse_light(in.uv);
    } else if id == BEHAVIOR_SPECTRUM {
        light = spectrum_light(in.uv);
    }
    let a = fx.color.w * clamp(fx.params.x, 0.0, 1.0) * clamp(light, 0.0, 1.0);
    return vec4<f32>(rgb * a, a);
}
";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surfaces::{KIND_CEILING, KIND_TABLE, KIND_WALL, acting_face, palette};
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
            color: 0,
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
    fn every_band_has_a_name_and_nothing_else_does() {
        assert_eq!(band_name(BAND_RMS), "rms");
        assert_eq!(band_name(BAND_BASS), "bass");
        assert_eq!(band_name(BAND_MID), "mid");
        assert_eq!(band_name(BAND_HIGH), "high");
        assert_eq!(BAND_NAMES.len(), BANDS as usize);
        assert_eq!(BAND_HIGH, BANDS - 1);
        assert_eq!(band_name(BANDS), "?");
        assert_eq!(band_name(u32::MAX), "?");
        // Each behavior's default band has a name.
        for b in SurfaceBehavior::ALL {
            assert_ne!(band_name(b.default_band()), "?", "{}", b.name());
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
        assert_eq!(MAX_BARS, 16 * 4, "bars: array<vec4<f32>, 16>");
        let pairs = [
            ("BEHAVIOR_RINGS", SurfaceBehavior::Rings.id() as f32),
            (
                "BEHAVIOR_STREAMLINES",
                SurfaceBehavior::Streamlines.id() as f32,
            ),
            ("BEHAVIOR_SPECTRUM", SurfaceBehavior::Spectrum.id() as f32),
            ("BEHAVIOR_CURLS", SurfaceBehavior::Curls.id() as f32),
            ("BEHAVIOR_PULSE", SurfaceBehavior::Pulse.id() as f32),
            ("BEHAVIOR_CONTOURS", SurfaceBehavior::Contours.id() as f32),
            ("LAND_INTERVAL", LAND_INTERVAL),
            ("LAND_INDEX_EVERY", LAND_INDEX_EVERY as f32),
            ("LAND_TIDE", LAND_TIDE),
            ("LAND_BREATH", LAND_BREATH),
            ("LAND_FEW_FROM", LAND_FEW_FROM),
            ("LAND_FEW_TO", LAND_FEW_TO),
            ("LAND_BETWEEN", LAND_BETWEEN),
            ("LAND_THIN", LAND_THIN),
            ("LAND_FLOOR", LAND_FLOOR),
            ("LAND_SLOPE_MIN", LAND_SLOPE_MIN),
            ("LAND_TINT_HALF", LAND_TINT_HALF),
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
            ("SIDE_SOFT", crate::canvas::SIDE_SOFT),
            ("TOP_SOFT", crate::canvas::TOP_SOFT),
            ("CAP_WIDTH", crate::canvas::CAP_WIDTH),
            ("GLOW_HEIGHT", crate::canvas::GLOW_HEIGHT),
        ];
        for (name, value) in pairs {
            assert_close!(wgsl_const(&module, name), value);
        }
        // The Rust twin's octave constants are the WGSL's literals.
        for v in [OCTAVE2_OFFSET, OCTAVE3_OFFSET, DRIFT1, DRIFT2, DRIFT3] {
            let lit = format!("vec2<f32>({:?}, {:?})", v.x, v.y);
            assert!(SURFACE_FX_WGSL.contains(&lit), "{lit}");
        }
        // And the land's: its waves, weights, bends, drifts and offsets.
        for (name, v) in ["k1", "k2", "k3"]
            .iter()
            .zip(LAND_K)
            .chain(["q1", "q2"].iter().zip(LAND_Q))
        {
            let lit = format!("let {name} = vec3<f32>({:?}, {:?}, {:?});", v.x, v.y, v.z);
            assert!(SURFACE_FX_WGSL.contains(&lit), "{lit}");
        }
        let [w1, w2, w3] = LAND_WEIGHT;
        let [b1, b2] = LAND_BEND;
        let d = LAND_DRIFT;
        let [o1, o2] = LAND_OFFSET;
        for lit in [
            format!("let t1 = dot(q1, p) + {:?} * tau;", d[0]),
            format!("let f1 = dot(k1, p) + {b1:?} * sin(t1) + {:?} * tau;", d[1]),
            format!("let t2 = dot(q2, p) + {:?} * tau;", d[2]),
            format!(
                "let f2 = dot(k2, p) + {b2:?} * sin(t2) + {:?} * tau + {o1:?};",
                d[3]
            ),
            format!("let f3 = dot(k3, p) + {:?} * tau + {o2:?};", d[4]),
            format!("{w1:?} * sin(f1) + {w2:?} * sin(f2) + {w3:?} * sin(f3),"),
            format!("{w1:?} * cos(f1) * (k1 + {b1:?} * cos(t1) * q1)"),
            format!("+ {w2:?} * cos(f2) * (k2 + {b2:?} * cos(t2) * q2)"),
            format!("+ {w3:?} * cos(f3) * k3,"),
        ] {
            assert!(SURFACE_FX_WGSL.contains(&lit), "{lit}");
        }
        // Every drift a whole number of hundredths: the clock's wrap
        // brings every phase round.
        for r in LAND_DRIFT {
            assert_close!((r * 100.0).round(), r * 100.0);
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
            key_tint: [0.9, 0.2, 0.4],
            ..Audio::default()
        };
        let s = Slot {
            strength: 0.7,
            ..slot(face, KIND_TABLE, SurfaceBehavior::Streamlines)
        };
        let rows = rows(&s, audio, None, None, 0.0);
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
        assert_close!(rows[5], [0.7, 0.0, 0.0, STREAM_SPEED_M_S]);
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
        let rows = super::rows(&s, audio, None, None, 0.0);
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
            key_tint: [0.9, 0.2, 0.4],
            ..Audio::default()
        };
        // Curls on a chair (a volume, other): its top, the rms by default.
        let face = table();
        let s = Slot {
            strength: 0.8,
            ..slot(face, KIND_OTHER, SurfaceBehavior::Curls)
        };
        let rows = rows(&s, audio, None, None, 0.0);
        for (row, c) in rows.iter().zip(face.corners(LIFT_M)) {
            assert_close!(*row, [c.x, c.y, c.z, 1.0]);
        }
        assert_close!(rows[4], [face.half[0], face.half[1], LIFT_M, 6.0]);
        assert_close!(rows[5], [0.8, 0.0, BAND_RMS as f32, CURL_SPEED_M_S]);
        let green = palette(KIND_OTHER);
        assert_close!(rows[6], [green[0], green[1], green[2], CURL_PEAK_ALPHA]);
        assert_close!(rows[7], [0.3, 0.6, 0.5, 12.5]);
        assert_close!(rows[8], [0.2, 0.1, 0.7, 0.25]);
        assert_close!(rows[9], [0.0, 0.0, 0.0, CURL_FEATURE_M]);
        assert_close!(rows[RINGS_ROW..], [[0.0f32; 4]; UNIFORM_ROWS - RINGS_ROW]);
        // On a floor the curls are larger; the band is the lane's, up to
        // the high.
        let s = Slot {
            band: BAND_MID,
            ..slot(floor(), KIND_FLOOR, SurfaceBehavior::Curls)
        };
        let rows = super::rows(&s, audio, None, None, 0.0);
        assert_close!(rows[9][3], CURL_FLOOR_FEATURE_M);
        assert_close!(rows[5][2], BAND_MID as f32);
        let s = Slot {
            band: 9,
            ..slot(floor(), KIND_FLOOR, SurfaceBehavior::Curls)
        };
        assert_close!(
            super::rows(&s, audio, None, None, 0.0)[5][2],
            BAND_HIGH as f32
        );
        // The pulse on a frame: the bass by default, no speed, no shape,
        // the frame's amber at the pulse's peak alpha.
        let s = slot(face, KIND_FRAME, SurfaceBehavior::Pulse);
        let rows = super::rows(&s, audio, None, None, 0.0);
        assert_close!(rows[4][3], 7.0);
        assert_close!(rows[5], [1.0, 0.0, BAND_BASS as f32, 0.0]);
        let amber = palette(KIND_FRAME);
        assert_close!(rows[6], [amber[0], amber[1], amber[2], PULSE_PEAK_ALPHA]);
        assert_close!(rows[8], [0.2, 0.1, 0.7, 0.25]);
        assert_close!(rows[9], [0.0; 4]);
        assert_close!(rows[RINGS_ROW..], [[0.0f32; 4]; UNIFORM_ROWS - RINGS_ROW]);
    }

    #[test]
    fn the_spectrum_rows_carry_the_bars_the_sign_and_the_canvas_look() {
        use crate::canvas::{BAR_FILL, Canvas, spectrum_face};
        let mut c = Canvas::new(6, 0.5);
        let mel: Vec<f32> = (0..6).map(|i| 0.4 + 0.08 * i as f32).collect();
        c.update(1.0 / 72.0, &mel);
        // A wall 2 m ahead whose local X runs to the wearer's left.
        let wall = acting_face(
            Vec3::new(0.0, 1.25, -2.0),
            Quat::from_rotation_z(std::f32::consts::PI),
            Vec3::new(2.0, 1.25, 0.02),
            HEAD,
        );
        let (face, sign) = spectrum_face(wall);
        assert_close!(sign, -1.0);
        let s = slot(face, KIND_WALL, SurfaceBehavior::Spectrum);
        let audio = Audio {
            rms: 0.3,
            ..Audio::default()
        };
        let rows = rows(&s, audio, None, Some(Spectrum::of(&c, sign)), 0.0);
        for (row, p) in rows.iter().zip(face.corners(LIFT_M)) {
            assert_close!(*row, [p.x, p.y, p.z, 1.0]);
        }
        assert_close!(rows[4], [2.0, 1.25, LIFT_M, 3.0]);
        assert_close!(rows[5], [1.0, 0.0, BAND_RMS as f32, 0.0]);
        // The canvas's warm white at its peak alpha times the gain.
        let w = crate::canvas::COLOR;
        assert_close!(rows[6], [w[0], w[1], w[2], crate::canvas::PEAK_ALPHA * 0.5]);
        assert_close!(rows[7], [0.3, 0.0, 0.0, 0.0]);
        assert_close!(rows[9], [-1.0, 6.0, c.glow(), BAR_FILL]);
        assert!(c.glow() > 0.0);
        assert_close!(rows[RINGS_ROW..BARS_ROW], [[0.0f32; 4]; MAX_RINGS]);
        let bars: Vec<f32> = rows[BARS_ROW..].iter().flatten().copied().collect();
        assert_close!(bars[..6], c.heights());
        assert_close!(bars[6..], [0.0f32; MAX_BARS - 6]);
        // The loudest bar reaches the top, the others below it in order.
        assert!(bars[5] > 0.999, "{bars:?}");
        // Bar 0, the lowest band, is at the wearer's left: +u here.
        let left = face.center - Vec3::X * 1.9;
        let u = face_uv(&face, left).x;
        assert!(u > 0.0, "{u}");
        assert_eq!(crate::canvas::spectrum_bar(u, 2.0, sign, 6), 0);
        // Another kind (the ceiling test) keeps the warm white; a color
        // index paints it.
        let s = slot(face, KIND_CEILING, SurfaceBehavior::Spectrum);
        let spectrum = Some(Spectrum::of(&c, 1.0));
        assert_close!(rows_of(&s, spectrum)[6][..3], w);
        let s = Slot { color: 6, ..s };
        assert_close!(
            rows_of(&s, spectrum)[6][..3],
            crate::surfaces::SURFACE_PALETTE[5]
        );
        // Without the canvas (`debug.fosfora.canvas 0`): nothing lit, no
        // bars, no other slot carries any.
        let off = rows_of(&s, None);
        assert_close!(off[6][3], 0.0);
        assert_close!(off[BARS_ROW..], [[0.0f32; 4]; MAX_BARS / 4]);
        let other = rows_of(&slot(face, KIND_WALL, SurfaceBehavior::Pulse), spectrum);
        assert_close!(other[BARS_ROW..], [[0.0f32; 4]; MAX_BARS / 4]);
    }

    fn rows_of(s: &Slot, spectrum: Option<Spectrum<'_>>) -> [[f32; 4]; UNIFORM_ROWS] {
        rows(s, Audio::default(), None, spectrum, 0.0)
    }

    #[test]
    fn the_color_index_picks_the_kinds_the_palettes_or_the_keys_color() {
        use crate::surfaces::{COLOR_KEY, KIND_OTHER, SURFACE_PALETTE};
        let audio = Audio {
            key_tint: [0.9, 0.2, 0.4],
            ..Audio::default()
        };
        let face = table();
        for (behavior, kind) in [
            (SurfaceBehavior::Curls, KIND_OTHER),
            (SurfaceBehavior::Streamlines, KIND_TABLE),
            (SurfaceBehavior::Rings, KIND_FLOOR),
        ] {
            for color in 0..=COLOR_KEY + 1 {
                let s = Slot {
                    color,
                    ..slot(face, kind, behavior)
                };
                let rows = rows(&s, audio, None, None, 0.0);
                assert_close!(rows[5][1], color as f32);
                let expect = match color {
                    // Index 0: the kind's color, the rings on a floor the
                    // ripple's warm white.
                    0 if behavior == SurfaceBehavior::Rings => crate::ripple::COLOR,
                    0 => palette(kind),
                    COLOR_KEY => audio.key_tint,
                    c if c < COLOR_KEY => SURFACE_PALETTE[c as usize - 1],
                    // Past the palette: the kind's own.
                    _ => palette(kind),
                };
                assert_close!(rows[6][..3], expect);
            }
        }
        // The rings on a floor take a palette color when one is asked for.
        let s = Slot {
            color: 6,
            ..slot(floor(), KIND_FLOOR, SurfaceBehavior::Rings)
        };
        assert_close!(rows(&s, audio, None, None, 0.0)[6][..3], SURFACE_PALETTE[5]);
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
            None,
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
            None,
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

    /// A wall of the room the contours' tests stand in: 4 m wide, 2.5 m
    /// high, at z = -2, facing the room.
    fn wall() -> Face {
        acting_face(
            Vec3::new(0.0, 1.25, -2.0),
            Quat::IDENTITY,
            Vec3::new(2.0, 1.25, 0.02),
            HEAD,
        )
    }

    /// Board #5533: the land's gradient is its height's, so a line's
    /// width in meters is right; it stays within -1 to 1 and a room holds
    /// both its low land and its high.
    #[test]
    fn the_lands_gradient_is_its_heights_and_a_room_holds_hills_and_hollows() {
        let (mut low, mut high) = (f32::MAX, f32::MIN);
        let eps = 1e-3;
        for i in 0..400 {
            let p = Vec3::new(
                -2.5 + 5.0 * hash(i, 1, 9).abs(),
                2.5 * hash(i, 2, 9).abs(),
                -2.5 + 5.0 * hash(i, 3, 9).abs(),
            );
            let clock = 300.0 * hash(i, 4, 9).abs();
            let (h, g) = land(p, clock);
            assert!(h.abs() <= 1.0 + 1e-5, "{h}");
            (low, high) = (low.min(h), high.max(h));
            for (axis, want) in [Vec3::X, Vec3::Y, Vec3::Z].into_iter().zip(g.to_array()) {
                let d =
                    (land(p + axis * eps, clock).0 - land(p - axis * eps, clock).0) / (2.0 * eps);
                assert!(
                    (d - want).abs() < 0.02,
                    "{p:?} {axis:?}: {d} against {want}"
                );
            }
        }
        assert!(low < -0.7 && high > 0.7, "{low}..{high}");
        // Rolling, not rugged: no slope a line could not follow.
        let steepest: f32 = LAND_K
            .iter()
            .zip(LAND_WEIGHT)
            .zip([LAND_BEND[0], LAND_BEND[1], 0.0])
            .zip([LAND_Q[0], LAND_Q[1], Vec3::ZERO])
            .map(|(((k, w), bend), q)| w * (k.length() + bend * q.length()))
            .sum();
        assert!(steepest < 4.5, "{steepest}");
    }

    /// One landscape, not six: two faces that meet at a corner sample the
    /// same land, so a line that reaches the edge goes on across the next
    /// face. Each face's quad is lifted off its own surface, so the two
    /// quads' edges are the lift's diagonal apart (2.8 cm), and across
    /// that gap the land's height changes by under half the step between
    /// two lines: the line that leaves the wall is the line that arrives
    /// on the floor.
    #[test]
    fn a_contour_line_goes_on_across_a_corner() {
        let (wall, floor) = (wall(), level_floor(floor(), None));
        let audio = Audio::default();
        let on = |face: Face, kind: u32| {
            rows(
                &slot(face, kind, SurfaceBehavior::Contours),
                audio,
                None,
                None,
                0.0,
            )
        };
        // The world point the vertex stage hands the fragment: the corners
        // blended, as the rasterizer blends them.
        let world = |rows: &[[f32; 4]; UNIFORM_ROWS], s: f32, t: f32| {
            let c = |k: usize| Vec3::new(rows[k][0], rows[k][1], rows[k][2]);
            c(0).lerp(c(1), s).lerp(c(3).lerp(c(2), s), t)
        };
        let (w, f) = (on(wall, KIND_WALL), on(floor, KIND_FLOOR));
        // Along the corner, where the wall's face meets the floor's.
        assert_close!(floor.center.y, 0.0);
        for i in 0..=20 {
            let x = -1.0 + 0.15 * i as f32;
            let edge = Vec3::new(x, 0.0, wall.center.z);
            // The same world point reached over each face's own (u, v).
            let on_wall = edge + wall.normal * LIFT_M;
            let on_floor = edge + floor.normal * LIFT_M;
            let (uw, uf) = (face_uv(&wall, on_wall), face_uv(&floor, on_floor));
            let blend = |face: &Face, uv: Vec2| {
                (
                    0.5 + 0.5 * uv.x / face.half[0],
                    0.5 + 0.5 * uv.y / face.half[1],
                )
            };
            let (sw, tw) = blend(&wall, uw);
            let (sf, tf) = blend(&floor, uf);
            assert!((world(&w, sw, tw) - on_wall).length() < 1e-4);
            assert!((world(&f, sf, tf) - on_floor).length() < 1e-4);
            assert_close!((on_wall - on_floor).length(), LIFT_M * 2f32.sqrt());
            for clock in [0.0, 77.0] {
                let (a, b) = (land(on_wall, clock).0, land(on_floor, clock).0);
                assert!(
                    (a - b).abs() < 0.5 * LAND_INTERVAL,
                    "x {x}: {a} on the wall, {b} on the floor"
                );
            }
        }
    }

    /// With the music quiet the lines are few and thin: the index lines
    /// alone. At full level every line is lit, the index lines brighter,
    /// and each is wider. A line is as wide on a steep slope as on a
    /// gentle one.
    #[test]
    fn quiet_shows_the_index_lines_and_full_level_all_of_them() {
        let px = 0.001;
        // On the line of height `m` intervals, less the tide the level adds.
        let on_line = |m: i32, level: f32| m as f32 * LAND_INTERVAL - LAND_TIDE * level;
        let at =
            |m: i32, level: f32| contour_light(on_line(m, level), 1.0, level, 0.0, [0.0; 2], px);
        // Quiet: the index lines at the floor's light, nothing between.
        for m in [-5, 0, 5] {
            assert!(at(m, 0.0) > 0.9 * LAND_FLOOR, "{m}");
        }
        for m in [-3, -1, 1, 2, 4] {
            assert_close!(at(m, 0.0), 0.0);
            assert_close!(at(m, LAND_FEW_FROM), 0.0);
        }
        // Full: every line, an index line brighter than the ones between.
        assert!(at(0, 1.0) > 0.95);
        for m in [-3, -1, 1, 2, 4] {
            assert!((at(m, 1.0) - LAND_BETWEEN * at(0, 1.0)).abs() < 1e-3, "{m}");
        }
        // Between two lines, nothing.
        let between = on_line(0, 1.0) + 0.5 * LAND_INTERVAL;
        assert_close!(contour_light(between, 1.0, 1.0, 0.0, [0.0; 2], px), 0.0);
        // The width: where the light is half an index line's, in meters
        // from the line, at a slope of 1 and of 3.
        let half_width = |level: f32, slope: f32| {
            let peak = contour_light(on_line(0, level), slope, level, 0.0, [0.0; 2], px);
            (0..2000)
                .map(|i| i as f32 * 1e-5)
                .find(|d| {
                    contour_light(
                        on_line(0, level) + d * slope,
                        slope,
                        level,
                        0.0,
                        [0.0; 2],
                        px,
                    ) < 0.5 * peak
                })
                .expect("the line ends")
        };
        let (thin, wide) = (half_width(0.0, 1.0), half_width(1.0, 1.0));
        assert!((wide - 0.5 * STREAM_LINE_M).abs() < 2e-4, "{wide}");
        assert!(
            (thin - 0.5 * LAND_THIN * STREAM_LINE_M).abs() < 2e-4,
            "{thin}"
        );
        assert!((half_width(1.0, 3.0) - wide).abs() < 2e-4);
        // The level raises the land: quiet, the index line runs where the
        // land is 0 high; at full level, on ground lower by the tide.
        assert!(contour_light(0.0, 1.0, 0.0, 0.0, [0.0; 2], px) > 0.9 * LAND_FLOOR);
        assert!(contour_light(-LAND_TIDE, 1.0, 1.0, 0.0, [0.0; 2], px) > 0.9);
        assert_close!(
            contour_light(
                -LAND_TIDE - 0.5 * LAND_INTERVAL,
                1.0,
                1.0,
                0.0,
                [0.0; 2],
                px
            ),
            0.0
        );
        // The traveling line: its own height, its own light, anywhere.
        let off = on_line(0, 1.0) + 0.4 * LAND_INTERVAL;
        let h = off + LAND_TIDE;
        assert_close!(
            contour_light(off, 1.0, 1.0, 0.0, [h, LAND_TRAVEL_LIGHT], px),
            LAND_TRAVEL_LIGHT
        );
        assert_close!(contour_light(off, 1.0, 1.0, 0.0, [h, 0.0], px), 0.0);
    }

    /// Nothing jumps: the music reaches the land gliding, its clock does
    /// not run with the beat, and the one traveling line leaves and
    /// arrives at no light, on a downbeat, one at a time.
    #[test]
    fn the_land_moves_gently() {
        let dt = 1.0 / 72.0;
        let mut t = Terrain::default();
        // The level from nothing to full in one frame: the land takes
        // under 1 % of it, and most of it within three time constants.
        t.step(dt, 1.0, [1.0; 4], false);
        let first = t.land();
        assert!(first.levels.iter().all(|l| *l > 0.0 && *l < 0.01));
        assert!(first.breath > 0.0 && first.breath < 0.02);
        for _ in 0..(3.0 * LAND_LEVEL_S / dt) as usize {
            let before = t.land();
            t.step(dt, 1.0, [1.0; 4], false);
            let now = t.land();
            // A frame moves a line by a small part of the step between
            // two lines.
            assert!(LAND_TIDE * (now.levels[0] - before.levels[0]) < 0.02 * LAND_INTERVAL);
        }
        assert!(t.land().levels[0] > 0.94);
        // Each band its own level.
        let mut bands = Terrain::default();
        for _ in 0..720 {
            bands.step(dt, 1.0, [0.2, 0.9, 0.5, 0.0], false);
        }
        for (now, want) in bands.land().levels.iter().zip([0.2, 0.9, 0.5, 0.0]) {
            assert!((now - want).abs() < 0.01, "{now} against {want}");
        }
        assert!((bands.land().breath - 0.9).abs() < 0.01);
        // The clock: seconds, whatever the beat does; slower with the
        // flash limit's motion; wrapped where every phase comes round.
        let mut c = Terrain::default();
        for _ in 0..72 {
            c.step(dt, 1.0, [0.0; 4], true);
        }
        assert!((c.land().clock - 1.0).abs() < 1e-3);
        for _ in 0..72 {
            c.step(dt, 0.5, [0.0; 4], false);
        }
        assert!((c.land().clock - 1.5).abs() < 1e-3);
        let wrap = LAND_CLOCK_WRAP_S as f32;
        let p = Vec3::new(0.7, 1.1, -1.3);
        assert!((land(p, wrap).0 - land(p, 0.0).0).abs() < 2e-3);
        assert!((land(p, wrap - 0.01).0 - land(p, 0.0).0).abs() < 2e-3);
        // The traveling line. Quiet, a downbeat sends none.
        assert_close!(c.land().travel_light, 0.0);
        let mut t = Terrain::default();
        for _ in 0..(LAND_TRAVEL_GAP_S / dt) as usize + 72 {
            t.step(dt, 1.0, [0.8; 4], false);
        }
        assert_close!(t.land().travel_light, 0.0);
        t.step(dt, 1.0, [0.8; 4], true);
        // It sets out at no light from the low land.
        let start = t.land();
        assert_close!(start.travel_light, 0.0);
        assert_close!(start.travel, LAND_TRAVEL_HEIGHTS[0]);
        let (mut peak, mut last, mut frames) = (0.0f32, start, 0);
        loop {
            // A downbeat on every frame: still one line.
            t.step(dt, 1.0, [0.8; 4], true);
            let now = t.land();
            if now.travel_light == 0.0 && frames > 10 {
                break;
            }
            assert!(now.travel >= last.travel, "it climbs");
            assert!((now.travel_light - last.travel_light).abs() < 0.01);
            peak = peak.max(now.travel_light);
            (last, frames) = (now, frames + 1);
        }
        assert!((peak - LAND_TRAVEL_LIGHT).abs() < 1e-3, "{peak}");
        assert!((frames as f32 * dt - LAND_TRAVEL_S).abs() < 0.1);
        // It arrived near the peaks at almost no light, and the next waits
        // out the gap.
        assert!(last.travel > 0.85 && last.travel_light < 0.01);
        let mut waited = 0;
        while t.land().travel_light == 0.0 && t.land().travel == 0.0 {
            t.step(dt, 1.0, [0.8; 4], true);
            waited += 1;
            assert!(waited < 2000);
        }
        let gap = (frames + waited) as f32 * dt;
        assert!((gap - LAND_TRAVEL_GAP_S).abs() < 0.1, "{gap}");
    }

    #[test]
    fn a_contours_slot_carries_the_land() {
        let land = Land {
            clock: 41.5,
            levels: [0.1, 0.2, 0.3, 0.4],
            breath: 0.6,
            travel: -0.2,
            travel_light: 0.3,
        };
        let audio = Audio {
            land,
            ..Audio::default()
        };
        let s = Slot {
            color: 5,
            band: BAND_MID,
            ..slot(wall(), KIND_WALL, SurfaceBehavior::Contours)
        };
        let rows = rows(&s, audio, None, None, 0.0);
        assert_close!(rows[4][3], SurfaceBehavior::Contours.id() as f32);
        assert_close!(rows[5], [1.0, 5.0, BAND_MID as f32, 0.0]);
        let green = crate::surfaces::GREEN;
        assert_close!(rows[6], [green[0], green[1], green[2], CONTOUR_PEAK_ALPHA]);
        assert_close!(rows[9], [41.5, 0.3, 0.6, -0.2]);
        let [r, g, b] = LAND_HIGH_COLOR;
        assert_close!(rows[RINGS_ROW], [r, g, b, 0.3]);
        for row in &rows[RINGS_ROW + 1..] {
            assert_close!(*row, [0.0; 4]);
        }
        // The band's default is the rms.
        assert_eq!(SurfaceBehavior::Contours.default_band(), BAND_RMS);
    }
}
