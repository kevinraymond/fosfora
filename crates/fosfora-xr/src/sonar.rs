//! The sweep (board #5511): on each downbeat a thin shell of light leaves
//! the wearer and travels out through the room, and where it crosses a
//! thing the depth light draws (`depth_light.rs`), that thing shows a
//! bright line: its points at one distance from where the sweep began. As
//! the shell moves on, the line crawls over the thing and traces its shape,
//! the way a contour line on a map does. A prototype behind a knob.
//!
//! **Where it is drawn.** On the depth light's mesh, in the passes and the
//! order the depth light already uses: the faces layer when there is one,
//! else the eye pass. No pass, target or texture of its own. On a frame a
//! sweep or a standing line shows, the mesh draws with
//! `fs_mesh_light_sonar` (`env_depth::ENV_DEPTH_WGSL`) in `fs_mesh_light`'s
//! place: the same points, wave and veil, and the sweep over them. On
//! every other frame, and always with the knob at 0, it draws with
//! `fs_mesh_light`, which is as it was before the sweep.
//!
//! **A sweep** ([`Sweep`]). Its origin is the head's position when it
//! started, fixed in the room from then on, so turning or moving the head
//! does not move a line already traveling. Its speed is fixed at its start
//! too ([`speed`]): [`SPAN_M`] in one bar of four beats at the song's
//! tempo, between [`SPEED_M_S`], so sweeps follow each other [`SPAN_M`]
//! apart; [`SPEED_DEFAULT_M_S`] before the first tempo lock. Its radius is
//! its speed times its age.
//!
//! **When one starts** ([`Sonar::step`]). On a downbeat the flash limit
//! let through (`flash.rs`), while the music is not quiet. Never less than
//! [`MIN_GAP_S`] after the last start, whatever asks, and never with
//! [`SWEEPS`] alive. A sweep is alive until its radius reaches the end of
//! its fade ([`FADE_M`]). `sonartest` starts one every that many seconds
//! with no music, under the same two limits.
//!
//! **The line** ([`shade`], [`half_width`]). A crest at the shell's
//! radius, [`HALF_WIDTH_M`] to each side near the wearer and
//! [`NOISE_WIDTHS`] times the depth map's noise at its radius where that is
//! more (the noise model from the recordings, `docs/xr/MEASURED.md`: 0.002
//! times the squared range). Behind the crest a tail: the light the shell
//! left falls to nothing over the distance the shell travels in
//! `sonartail` seconds. A point under the crest or the tail swells
//! ([`POINT_SWELL`], as far as half a cell lets the largest) and brightens
//! to the line's peak whatever the distance had dimmed it to, so the tail
//! is mostly the points'. The line itself is drawn solid, and its tail as
//! a faint glow ([`TAIL_GLOW`]). The whole of a sweep's light fades with
//! its radius, all there to [`FADE_M`]'s start and gone by its end. The
//! colors are the depth light's: the side's color, the crest tinted toward
//! the top's ([`CREST_TINT`]), which is the key's tint where the theme has
//! one.
//!
//! **Off the mesh's staircase** ([`solid_share`]). The solid light fades
//! out toward a mass's edge ([`SOLID_KEEP`]) and on a triangle seen
//! edge-on ([`FACING`]): the fringe that leans back at a step between two
//! depths. A line there would trace the mesh's steps and not the thing,
//! and it is where the map changes most from one frame to the next. The
//! points fade there by themselves, as they did.
//!
//! **Tension** ([`Contours`]). While `buildup` is over the level the
//! immersion reads as tension (`immerse::BUILD_FROM`), standing lines come
//! up between the sweeps: lines of equal distance from where the head was
//! when they came up, [`SPACING_M`] apart. As the build grows a second set
//! fades in halfway between them and then a third between those
//! ([`SET_TENSION`]), so the spacing tightens to a quarter without a line
//! moving. The finer sets only show near ([`SET_FAR_M`]), where a line is
//! still thin against its spacing. On a drop they fade in [`DROP_FADE_S`]
//! and stay away for [`DROP_QUIET_S`], and one sweep at full strength
//! leaves ([`DROP_STRENGTH`] against a downbeat's [`BEAT_STRENGTH`]).
//!
//! **The pattern limit.** Regular stripes are a hazard of their own, so
//! the standing lines never flash, reverse or stand still: they drift
//! outward at [`DRIFT_M_S`] all the time they show, the way the sweeps
//! travel, and the drift never turns back. Their strength changes only by
//! slow ramps ([`CONTOUR_FADE_S`] from nothing to all, the tension the
//! sets read at most [`TENSION_SLEW_PER_S`]), never on a beat; the drop's
//! fade is the one short one. They are faint: [`CONTOUR_SHARE`] of a
//! sweep's line, and never over an alpha of [`CONTOUR_ALPHA_MAX`] whatever
//! the gain. The finest spacing is [`SPACING_M`] / 4 and may not go under
//! [`SPACING_FLOOR_M`]: a surface that faces the wearer and fills the view
//! shows at most [`LINES_AT_1M`] of them at any distance
//! ([`lines_across`]).
//!
//! **The flash limit** (`flash.rs`). A sweep starts on a trigger the
//! limiter let through, as the floor's rings do, and then runs by itself;
//! the standing lines read `buildup` as the limiter hands it on. A place
//! sees a line pass at most once per [`MIN_GAP_S`], so at most twice in
//! any second: two flashes, on a surface that faces the wearer and lights
//! all at once. A standing line is under the change the rule counts.
//! `flash_room.rs` counts both on the depth light's Rust copy, with the
//! music's own light.
//!
//! **Cost.** Per fragment of the depth light's mesh, on a frame a sweep or
//! a standing line shows: two more derivatives, a cross product, three dot
//! products, a division and two smoothsteps for the solid light's share;
//! per live sweep a square root, a smoothstep and about a dozen multiplies
//! and adds; for the standing lines a square root, two divisions, two
//! floors, two smoothsteps and about thirty multiplies and adds; then two
//! more color mixes and about fifteen multiplies and adds for the blend.
//! No texture read, no loop (the three sweeps are three calls), no array.
//!
//! **Knobs**, read at launch and polled once a second
//! ([`SonarOptions::from_knobs`]): `debug.fosfora.sonar 0|1|2` (0 off, the
//! depth light as it was; 1 the depth light's things; 2 is for the scanned
//! room too, which is not built, so it draws as 1; default 1 on this
//! branch), `sonarspeed` (a factor on the speed), `sonarwidth` (on the
//! lines' width), `sonargain` (brightness, 0 to [`GAIN_MAX`]), `sonartail`
//! (the tail, s), `sonarcontours` (the standing lines' strength, 0 to 1; 0
//! never shows them), and for unworn captures `sonartest <s>` (a sweep
//! every that many seconds whatever the music does) and `sonarbuild
//! <0..1>` (the build the standing lines read held there whatever the
//! music does).
//!
//! **What it cannot do.** It draws only where the depth light draws: with
//! geometry first on, not on the scanned walls, floor and tables. It needs
//! downbeats: a song the beat tracker has not locked onto gets no sweeps.
//! The line is as coarse as the depth map's mesh, and a thing narrower
//! than a mesh cell may miss it. A surface that faces the origin squarely
//! is all at one distance, so it lights at once instead of showing a line.
//! A surface seen nearly edge-on (the floor far away, a wall seen along
//! its length) carries the points' share and no solid line. Between two
//! things a sweep shows nothing.
//!
//! Plain numbers in, so it builds and tests on the desktop: [`shade`]
//! mirrors the WGSL's `sonar_light` and [`solid_share`] the fragment's
//! fade, and `tests/depth_light_gpu.rs` runs the shader against them.

use glam::Vec3;

use crate::depth_light::smoothstep;
use crate::immerse::BUILD_FROM;

/// The knobs' names.
pub const KNOB: &str = "debug.fosfora.sonar";
pub const KNOB_SPEED: &str = "debug.fosfora.sonarspeed";
pub const KNOB_WIDTH: &str = "debug.fosfora.sonarwidth";
pub const KNOB_GAIN: &str = "debug.fosfora.sonargain";
pub const KNOB_TAIL: &str = "debug.fosfora.sonartail";
pub const KNOB_CONTOURS: &str = "debug.fosfora.sonarcontours";
pub const KNOB_TEST: &str = "debug.fosfora.sonartest";
pub const KNOB_BUILD: &str = "debug.fosfora.sonarbuild";

/// Rows of [`Sonar::rows`], the end of `struct EnvEye` from `sonar` on
/// (`depth_light::SONAR_ROW`).
pub const ROWS: usize = 12;
/// Sweeps alive at once at most.
pub const SWEEPS: usize = 3;
/// The distance a sweep crosses in one bar (m).
pub const SPAN_M: f32 = 4.0;
/// The slowest and the fastest a sweep travels (m/s): a bar of 4 s and of
/// 1.33 s, 60 and 180 BPM.
pub const SPEED_M_S: [f32; 2] = [1.0, 3.0];
/// A sweep's speed before the first tempo lock (m/s): 120 BPM's.
pub const SPEED_DEFAULT_M_S: f32 = 2.0;
/// `sonarspeed`'s range.
pub const SPEED_FACTOR: [f32; 2] = [0.25, 4.0];
/// The shortest time between two starts (s).
pub const MIN_GAP_S: f32 = 0.7;
/// The radius (m) where a sweep's light starts to fall and where it is
/// gone and the sweep ends.
pub const FADE_M: [f32; 2] = [1.5, 4.5];
/// A line's half-width near the wearer (m).
pub const HALF_WIDTH_M: f32 = 0.02;
/// The depth map's noise along the view ray (m) per squared meter of range
/// (`docs/xr/MEASURED.md`), and how many times it a line's half-width is at
/// least.
pub const NOISE_M_PER_M2: f32 = 0.002;
pub const NOISE_WIDTHS: f32 = 3.0;
/// `sonarwidth`'s range.
pub const WIDTH_FACTOR: [f32; 2] = [0.25, 4.0];
/// The tail (s): the light behind the crest is gone where the shell was
/// this long ago. And `sonartail`'s range.
pub const TAIL_S: f32 = 0.35;
pub const TAIL_RANGE_S: [f32; 2] = [0.0, 2.0];
/// The tail's solid glow where it starts, as a share of the line's light.
pub const TAIL_GLOW: f32 = 0.12;
/// A sweep's strength from a downbeat (and from `sonartest`) and from a
/// drop.
pub const BEAT_STRENGTH: f32 = 0.7;
pub const DROP_STRENGTH: f32 = 1.0;
/// A drop this soon after a start (s) lifts that sweep to the drop's
/// strength instead of waiting for the gap.
pub const DROP_LIFT_S: f32 = 0.25;
/// How long a drop that could not start its sweep keeps asking (s).
pub const DROP_WAIT_S: f32 = 1.0;
/// The line's peak alpha with the room plain (immersion level 0) and with
/// it black, at a strength of 1.
pub const ALPHA_LOW: f32 = 0.25;
pub const ALPHA_HIGH: f32 = 0.9;
/// `sonargain`'s range.
pub const GAIN_MAX: f32 = 4.0;
/// How far a point under the line swells: its radius grows by up to this
/// share, as far as half a cell lets the largest.
pub const POINT_SWELL: f32 = 0.35;
/// How far the crest tints toward the top's color.
pub const CREST_TINT: f32 = 0.6;
/// How squarely a triangle of the mesh must face the eye (the cosine of
/// the angle between its normal and the way to the eye) for the sweep's
/// solid light to start showing on it and to be all there: a triangle seen
/// within about 7 degrees of edge-on carries none, one past about 17 all.
pub const FACING: [f32; 2] = [0.12, 0.3];
/// What a vertex must keep of its points for the solid light to start
/// showing and to be all there: none at a mass's edge
/// (`depth_light::NEAR_KEEP`), all inside the mass.
pub const SOLID_KEEP: [f32; 2] = [crate::depth_light::NEAR_KEEP, 1.0];

/// The standing lines' spacing with the build just begun (m). The second
/// set halves it and the third halves it again.
pub const SPACING_M: f32 = 0.6;
/// The finest spacing allowed (m): the pattern limit's floor.
pub const SPACING_FLOOR_M: f32 = 0.12;
/// The tension over which the second set and the third set fade in.
pub const SET_TENSION: [[f32; 2]; 2] = [[0.15, 0.45], [0.5, 0.85]];
/// The distance (m) over which the second set and the third set fade out.
pub const SET_FAR_M: [[f32; 2]; 2] = [[2.5, 3.5], [1.5, 2.5]];
/// A standing line's light at full strength, as a share of a sweep's line
/// at strength 1.
pub const CONTOUR_SHARE: f32 = 0.1;
/// The most alpha a standing line gets, whatever the gain: under the
/// change of luminance the flash rule counts (`flash::GENERAL_DELTA`) in
/// any color, so a line that drifts over a surface which faces the wearer,
/// lighting all of it at once, is no transition by itself.
pub const CONTOUR_ALPHA_MAX: f32 = 0.09;
/// A standing line's half-width near the wearer (m).
pub const CONTOUR_HALF_WIDTH_M: f32 = 0.02;
/// The tension at which the standing lines are at full strength.
pub const CONTOUR_FULL_AT: f32 = 0.3;
/// The standing lines' ramp from nothing to all and back (s).
pub const CONTOUR_FADE_S: f32 = 1.5;
/// The most the tension the sets read moves in a second.
pub const TENSION_SLEW_PER_S: f32 = 0.25;
/// A drop: the standing lines gone in this long (s), and away for this
/// long after it.
pub const DROP_FADE_S: f32 = 0.3;
pub const DROP_QUIET_S: f32 = 4.0;
/// The standing lines' drift outward (m/s), and the least of it while the
/// flash limit slows the looks' own motion (`flash::MOTION_BUSY`).
pub const DRIFT_M_S: f32 = 0.12;
pub const DRIFT_SLOWEST: f32 = 0.5;
/// The most standing lines a surface 1 m away that fills the view may
/// show.
pub const LINES_AT_1M: u32 = 8;
/// A line counts as showing from this share of a full one's light.
pub const LINE_SHOWS: f32 = 0.25;
/// The view's half angles (degrees, across and up) the count is made for:
/// the Quest 3's, about 104 by 96 degrees.
pub const VIEW_HALF_DEG: [f32; 2] = [52.0, 48.0];

fn unit(x: f32) -> f32 {
    if x.is_finite() {
        x.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// What the knobs ask.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SonarOptions {
    /// `debug.fosfora.sonar`: 0 off, 1 the depth light's things, 2 the
    /// scanned room too (not built: draws as 1).
    pub mode: u32,
    /// `debug.fosfora.sonarspeed`: a factor on a sweep's speed.
    pub speed: f32,
    /// `debug.fosfora.sonarwidth`: a factor on the lines' width.
    pub width: f32,
    /// `debug.fosfora.sonargain`: multiplies the peak alpha.
    pub gain: f32,
    /// `debug.fosfora.sonartail`: the tail (s).
    pub tail_s: f32,
    /// `debug.fosfora.sonarcontours`: the standing lines' strength.
    pub contours: f32,
    /// `debug.fosfora.sonartest`: a sweep every this many seconds whatever
    /// the music does, never under [`MIN_GAP_S`]; `None` unset or 0.
    pub test_s: Option<f32>,
    /// `debug.fosfora.sonarbuild`: the build the standing lines read, 0..1,
    /// held there whatever the music does; `None` unset, the music's.
    pub build: Option<f32>,
}

impl Default for SonarOptions {
    fn default() -> Self {
        Self {
            mode: 1,
            speed: 1.0,
            width: 1.0,
            gain: 1.0,
            tail_s: TAIL_S,
            contours: 1.0,
            test_s: None,
            build: None,
        }
    }
}

impl SonarOptions {
    /// The options the eight knobs ask, each read through `prop` (a system
    /// property by name, `None` unset): an unreadable number is the
    /// default, one out of range the nearest end. `sonar` is 0, 1 or 2;
    /// `sonartest` is off unset, unreadable or not above 0; `sonarbuild`
    /// is the music's unset or unreadable.
    pub fn from_knobs(prop: impl Fn(&str) -> Option<String>) -> Self {
        let d = Self::default();
        let number = |name: &str| {
            prop(name)
                .and_then(|v| v.trim().parse::<f32>().ok())
                .filter(|v| v.is_finite())
        };
        Self {
            mode: number(KNOB).map_or(d.mode, |v| v.clamp(0.0, 2.0).round() as u32),
            speed: number(KNOB_SPEED)
                .map_or(d.speed, |v| v.clamp(SPEED_FACTOR[0], SPEED_FACTOR[1])),
            width: number(KNOB_WIDTH)
                .map_or(d.width, |v| v.clamp(WIDTH_FACTOR[0], WIDTH_FACTOR[1])),
            gain: number(KNOB_GAIN).map_or(d.gain, |v| v.clamp(0.0, GAIN_MAX)),
            tail_s: number(KNOB_TAIL)
                .map_or(d.tail_s, |v| v.clamp(TAIL_RANGE_S[0], TAIL_RANGE_S[1])),
            contours: number(KNOB_CONTOURS).map_or(d.contours, |v| v.clamp(0.0, 1.0)),
            test_s: number(KNOB_TEST)
                .filter(|&v| v > 0.0)
                .map(|v| v.max(MIN_GAP_S)),
            build: number(KNOB_BUILD).map(|v| v.clamp(0.0, 1.0)),
        }
    }

    /// Whether the sweep draws at all.
    pub fn on(&self) -> bool {
        self.mode > 0
    }

    /// The state in a few words, for the log.
    pub fn describe(&self) -> String {
        if !self.on() {
            return format!("off ({KNOB} 0): the depth light as it was");
        }
        let room = if self.mode > 1 {
            " (2 asked: the scanned room is not built, drawn as 1)"
        } else {
            ""
        };
        let test = self.test_s.map_or_else(String::new, |s| {
            format!(" \u{b7} a test sweep every {s} s ({KNOB_TEST})")
        });
        let build = self.build.map_or_else(String::new, |b| {
            format!(" \u{b7} the build held at {b} ({KNOB_BUILD})")
        });
        format!(
            "on, the depth light's things{room} \u{b7} speed x{} ({SPAN_M} m a bar, {} to {} m/s) \u{b7} width x{} ({} m each side near) \u{b7} gain {} \u{b7} tail {} s \u{b7} contours {} \u{b7} starts {MIN_GAP_S} s apart at least, {SWEEPS} alive at most{test}{build}",
            self.speed,
            SPEED_M_S[0],
            SPEED_M_S[1],
            self.width,
            HALF_WIDTH_M,
            self.gain,
            self.tail_s,
            self.contours,
        )
    }

    /// The launch line.
    pub fn line(&self) -> String {
        format!("sonar: {}", self.describe())
    }

    /// The line for a change from `before`.
    pub fn change_line(&self, before: &Self) -> String {
        format!("{} (was: {})", self.line(), before.describe())
    }
}

/// A sweep's speed (m/s) at a beat of `beat_period_s` seconds (`None`
/// before the first tempo lock) with `sonarspeed` at `factor`: [`SPAN_M`]
/// in a bar of four beats, between [`SPEED_M_S`], times the factor.
pub fn speed(beat_period_s: Option<f32>, factor: f32) -> f32 {
    let base = beat_period_s
        .filter(|p| p.is_finite() && *p > 0.0)
        .map_or(SPEED_DEFAULT_M_S, |p| {
            (SPAN_M / (4.0 * p)).clamp(SPEED_M_S[0], SPEED_M_S[1])
        });
    let factor = if factor.is_finite() {
        factor.clamp(SPEED_FACTOR[0], SPEED_FACTOR[1])
    } else {
        1.0
    };
    base * factor
}

/// A line's half-width (m) at `range_m` from the wearer, `near_m` near and
/// [`NOISE_WIDTHS`] times the map's noise there where that is more, times
/// `sonarwidth` (`factor`).
pub fn half_width(near_m: f32, range_m: f32, factor: f32) -> f32 {
    near_m.max(NOISE_WIDTHS * NOISE_M_PER_M2 * range_m * range_m) * factor
}

/// What is left of a sweep's light at radius `radius_m`: all of it to
/// [`FADE_M`]'s start, none from its end.
pub fn fade(radius_m: f32) -> f32 {
    1.0 - smoothstep(FADE_M[0], FADE_M[1], radius_m)
}

/// The line's peak alpha at immersion level `level` (0..1) and gain
/// `gain`, at a strength of 1.
pub fn peak_alpha(level: f32, gain: f32) -> f32 {
    let gain = if gain.is_finite() {
        gain.clamp(0.0, GAIN_MAX)
    } else {
        0.0
    };
    (ALPHA_LOW + (ALPHA_HIGH - ALPHA_LOW) * unit(level)) * gain
}

/// The tension in `buildup`: its share over [`BUILD_FROM`], 0..1, as the
/// immersion reads it.
pub fn tension(buildup: f32) -> f32 {
    ((unit(buildup) - BUILD_FROM) / (1.0 - BUILD_FROM)).max(0.0)
}

/// The strength of the second and the third set of standing lines at
/// `tension`.
pub fn set_weights(tension: f32) -> [f32; 2] {
    let t = unit(tension);
    SET_TENSION.map(|[from, to]| smoothstep(from, to, t))
}

/// What a standing line `index` lines out (at the finest spacing, from the
/// drift) carries at `range_m`, as a share of a full one: every fourth all
/// of it, the second set (halfway between those) and the third (the odd
/// ones) their weight in `sets`, less what their distance takes
/// ([`SET_FAR_M`]). Before the fade by distance all lines share.
pub fn line_weight(index: i32, range_m: f32, sets: [f32; 2]) -> f32 {
    let far =
        |k: usize| 1.0 - unit((range_m - SET_FAR_M[k][0]) / (SET_FAR_M[k][1] - SET_FAR_M[k][0]));
    match index.rem_euclid(4) {
        0 => 1.0,
        2 => sets[0] * far(0),
        _ => sets[1] * far(1),
    }
}

/// How many standing lines show on a surface that faces the wearer
/// `near_m` away and fills the view, at `tension`: the lines between its
/// nearest point and the view's far corner ([`VIEW_HALF_DEG`]) that carry
/// at least [`LINE_SHOWS`] of a full line's light, the fade by distance
/// included. The most over the drift.
pub fn lines_across(near_m: f32, tension: f32) -> u32 {
    let [tx, ty] = VIEW_HALF_DEG.map(|d| d.to_radians().tan());
    let far_m = near_m * (1.0 + tx * tx + ty * ty).sqrt();
    let fine = SPACING_M / 4.0;
    let sets = set_weights(tension);
    (0..16)
        .map(|k| {
            let drift = SPACING_M * k as f32 / 16.0;
            let first = ((near_m - drift) / fine).ceil() as i32;
            let last = ((far_m - drift) / fine).floor() as i32;
            (first..=last)
                .filter(|&n| {
                    let d = drift + n as f32 * fine;
                    line_weight(n, d, sets) * fade(d) >= LINE_SHOWS
                })
                .count() as u32
        })
        .max()
        .unwrap_or(0)
}

/// One sweep.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sweep {
    /// Where the head was when it started (world).
    pub origin: Vec3,
    /// Seconds since.
    pub age_s: f32,
    /// Its speed (m/s), fixed at its start.
    pub speed: f32,
    /// Its strength, 0..1.
    pub strength: f32,
}

impl Sweep {
    /// The shell's radius now (m).
    pub fn radius(&self) -> f32 {
        self.speed * self.age_s
    }
}

/// What started a sweep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cause {
    Downbeat,
    Drop,
    Test,
}

/// A frame's inputs to [`Sonar::step`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Input {
    /// Seconds since the last frame.
    pub dt: f32,
    /// A downbeat and a drop the flash limit let through this frame.
    pub downbeat: bool,
    pub drop: bool,
    /// Seconds a beat at the song's tempo, `None` before the first lock.
    pub beat_period_s: Option<f32>,
    /// The analysis's `buildup`, as the flash limit hands it on.
    pub buildup: f32,
    /// The music is quiet (silence, a pause): no sweep starts on a beat
    /// and the standing lines go.
    pub quiet: bool,
    /// The head's position (world).
    pub head: Vec3,
    /// The share of their speed the looks' own motion runs at
    /// (`flash::Limiter::motion`).
    pub motion: f32,
}

/// The standing lines' state.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Contours {
    /// Their strength, 0..1, ramped.
    pub strength: f32,
    /// The tension the sets read, slewed.
    pub tension: f32,
    /// What is left of a drop's quiet (s).
    pub quiet_s: f32,
    /// Where the head was when they came up (world).
    pub origin: Vec3,
    /// Their drift outward, 0..[`SPACING_M`] (it wraps by whole patterns).
    pub drift_m: f32,
}

/// The sweeps and the standing lines, stepped once a frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sonar {
    sweeps: [Option<Sweep>; SWEEPS],
    /// Seconds since the last start.
    since_start_s: f32,
    /// What is left of a drop's wait for its sweep (s), 0 with none
    /// waiting.
    drop_wait_s: f32,
    /// The test knob's clock (s).
    test_clock_s: f32,
    contours: Contours,
}

impl Default for Sonar {
    fn default() -> Self {
        Self {
            sweeps: [None; SWEEPS],
            since_start_s: f32::INFINITY,
            drop_wait_s: 0.0,
            test_clock_s: 0.0,
            contours: Contours::default(),
        }
    }
}

impl Sonar {
    /// The sweeps alive, oldest slot first.
    pub fn sweeps(&self) -> impl Iterator<Item = &Sweep> {
        self.sweeps.iter().flatten()
    }

    /// The standing lines.
    pub fn contours(&self) -> &Contours {
        &self.contours
    }

    /// Whether anything draws this frame.
    pub fn shows(&self) -> bool {
        self.sweeps().next().is_some() || self.contours.strength > 0.0
    }

    /// Nothing alive and nothing showing: the knob at 0, or the depth
    /// light off. The gap since the last start keeps counting from here.
    pub fn clear(&mut self) {
        *self = Self {
            since_start_s: self.since_start_s,
            ..Self::default()
        };
    }

    /// Advance one frame under `opts`. Returns what started a sweep this
    /// frame, if one did.
    ///
    /// The sweeps age, and one whose radius passed [`FADE_M`]'s end is
    /// gone. A start is asked by a downbeat with the music not quiet, by a
    /// drop (which keeps asking for [`DROP_WAIT_S`]; one within
    /// [`DROP_LIFT_S`] of the last start lifts that sweep instead) and by
    /// the test knob's clock. It is given when the last start is at least
    /// [`MIN_GAP_S`] old and fewer than [`SWEEPS`] are alive: from the
    /// head, at [`speed`], at the drop's strength when a drop asks.
    ///
    /// The standing lines: the tension the sets read follows [`tension`]
    /// of `buildup` (of the `sonarbuild` knob's value when set, quiet or
    /// not) at most [`TENSION_SLEW_PER_S`]; the strength ramps
    /// toward the lesser of that and the tension as it is, over
    /// [`CONTOUR_FULL_AT`], taking [`CONTOUR_FADE_S`] for the whole way; a
    /// drop takes it down in [`DROP_FADE_S`] and holds it there for
    /// [`DROP_QUIET_S`]. Coming up from nothing they center on the head.
    /// The drift moves on by [`DRIFT_M_S`] times `motion`, never less than
    /// [`DRIFT_SLOWEST`] of it and never back.
    pub fn step(&mut self, opts: &SonarOptions, input: Input) -> Option<Cause> {
        let dt = if input.dt.is_finite() {
            input.dt.max(0.0)
        } else {
            0.0
        };
        self.since_start_s += dt;
        if !opts.on() {
            self.clear();
            return None;
        }
        for slot in &mut self.sweeps {
            if let Some(s) = slot {
                s.age_s += dt;
                if s.radius() >= FADE_M[1] {
                    *slot = None;
                }
            }
        }
        // A drop: lift a sweep that just left, or ask for one.
        self.drop_wait_s = (self.drop_wait_s - dt).max(0.0);
        if input.drop {
            let young = self
                .sweeps
                .iter_mut()
                .flatten()
                .filter(|s| s.age_s <= DROP_LIFT_S)
                .min_by(|a, b| a.age_s.total_cmp(&b.age_s));
            match young {
                Some(s) => s.strength = DROP_STRENGTH,
                None => self.drop_wait_s = DROP_WAIT_S,
            }
        }
        let test = opts.test_s.is_some_and(|period| {
            self.test_clock_s += dt;
            let due = self.test_clock_s >= period;
            if due {
                self.test_clock_s = 0.0;
            }
            due
        });
        if opts.test_s.is_none() {
            self.test_clock_s = 0.0;
        }
        let cause = if self.drop_wait_s > 0.0 {
            Some(Cause::Drop)
        } else if input.downbeat && !input.quiet {
            Some(Cause::Downbeat)
        } else if test {
            Some(Cause::Test)
        } else {
            None
        };
        let free = self.sweeps.iter().position(Option::is_none);
        let started = match (cause, free) {
            (Some(cause), Some(k)) if self.since_start_s >= MIN_GAP_S => {
                self.sweeps[k] = Some(Sweep {
                    origin: input.head,
                    age_s: 0.0,
                    speed: speed(input.beat_period_s, opts.speed),
                    strength: if cause == Cause::Drop {
                        DROP_STRENGTH
                    } else {
                        BEAT_STRENGTH
                    },
                });
                self.since_start_s = 0.0;
                self.drop_wait_s = 0.0;
                Some(cause)
            }
            _ => None,
        };
        self.step_contours(opts, &input, dt);
        started
    }

    fn step_contours(&mut self, opts: &SonarOptions, input: &Input, dt: f32) {
        let c = &mut self.contours;
        if input.drop {
            c.quiet_s = DROP_QUIET_S;
        }
        let dropped = c.quiet_s > 0.0;
        c.quiet_s = (c.quiet_s - dt).max(0.0);
        // The test knob's build needs no music.
        let (buildup, quiet) = match opts.build {
            Some(held) => (held, false),
            None => (input.buildup, input.quiet),
        };
        let raw = if dropped || quiet || opts.contours <= 0.0 {
            0.0
        } else {
            tension(buildup)
        };
        let slew = TENSION_SLEW_PER_S * dt;
        c.tension += (raw - c.tension).clamp(-slew, slew);
        let target = (raw.min(c.tension) / CONTOUR_FULL_AT).min(1.0);
        let ramp = dt / if dropped { DROP_FADE_S } else { CONTOUR_FADE_S };
        let was = c.strength;
        c.strength = unit(c.strength + (target - c.strength).clamp(-ramp, ramp));
        if c.strength <= 0.0 {
            // Unseen: the sets start over with the next build.
            if dropped {
                c.tension = 0.0;
            }
        } else if was <= 0.0 {
            c.origin = input.head;
        }
        let motion = if input.motion.is_finite() {
            input.motion.clamp(DRIFT_SLOWEST, 1.0)
        } else {
            1.0
        };
        c.drift_m = (c.drift_m + DRIFT_M_S * motion * dt).rem_euclid(SPACING_M);
    }

    /// This frame's rows of `struct EnvEye` (from `sonar` on) under `opts`
    /// at the immersion `level`: all 0 with the knob at 0 or nothing to
    /// show, and the fragment then takes the path it took before the
    /// sweep. Row 0 the line's peak alpha, the tail's glow, the points'
    /// swell and the crest's tint; row 1 the facing (squared) and the keep
    /// between which the solid light comes in ([`solid_share`]); then per
    /// sweep its origin and radius, and its strength at that
    /// radius, 1 over its half-width and 1 over its tail's length; then the
    /// standing lines' origin and drift, their strength, lines per meter
    /// and sets, their width and fade, and where the finer sets fade.
    pub fn rows(&self, opts: &SonarOptions, level: f32) -> [[f32; 4]; ROWS] {
        let mut rows = [[0.0f32; 4]; ROWS];
        let peak = peak_alpha(level, opts.gain);
        if !opts.on() || !self.shows() || peak <= 0.0 {
            return rows;
        }
        rows[0] = [peak, TAIL_GLOW, POINT_SWELL, CREST_TINT];
        rows[1] = [
            FACING[0] * FACING[0],
            FACING[1] * FACING[1],
            SOLID_KEEP[0],
            SOLID_KEEP[1],
        ];
        for (k, s) in self.sweeps.iter().enumerate() {
            let Some(s) = s else { continue };
            let radius = s.radius();
            let tail_m = s.speed * opts.tail_s;
            rows[2 + 2 * k] = [s.origin.x, s.origin.y, s.origin.z, radius];
            rows[3 + 2 * k] = [
                unit(s.strength) * fade(radius),
                1.0 / half_width(HALF_WIDTH_M, radius, opts.width),
                if tail_m > 1e-6 { 1.0 / tail_m } else { 1e6 },
                0.0,
            ];
        }
        let c = &self.contours;
        let [second, third] = set_weights(c.tension);
        let far = |k: usize| [SET_FAR_M[k][0], 1.0 / (SET_FAR_M[k][1] - SET_FAR_M[k][0])];
        let (far2, far3) = (far(0), far(1));
        rows[8] = [c.origin.x, c.origin.y, c.origin.z, c.drift_m];
        rows[9] = [
            (CONTOUR_SHARE * unit(opts.contours) * unit(c.strength)).min(CONTOUR_ALPHA_MAX / peak),
            4.0 / SPACING_M,
            second,
            third,
        ];
        rows[10] = [
            CONTOUR_HALF_WIDTH_M * opts.width,
            NOISE_WIDTHS * NOISE_M_PER_M2 * opts.width,
            FADE_M[0],
            FADE_M[1],
        ];
        rows[11] = [far3[0], far3[1], far2[0], far2[1]];
        rows
    }
}

/// A line's profile `u` half-widths from its middle: 1 there, 0 from one
/// half-width out, eased.
pub fn crest(u: f32) -> f32 {
    1.0 - smoothstep(0.0, 1.0, u.abs())
}

/// The tail's light `behind_m` behind the shell (negative ahead of it,
/// where there is none) with 1 over the tail's length `inv_tail`: 1 at the
/// shell, falling as a square to 0 at the tail's end.
pub fn tail(behind_m: f32, inv_tail: f32) -> f32 {
    if behind_m < 0.0 {
        return 0.0;
    }
    let t = (1.0 - behind_m * inv_tail).clamp(0.0, 1.0);
    t * t
}

/// The sweep's light at world point `p`, from `rows` ([`Sonar::rows`]):
/// the lines' light (the sweeps' crests and the standing lines, the
/// brightest of them) and the tails' glow, each 0..1. The WGSL's
/// `sonar_light`.
pub fn shade(rows: &[[f32; 4]; ROWS], p: Vec3) -> [f32; 2] {
    let (mut line, mut glow) = (0.0f32, 0.0f32);
    for k in 0..SWEEPS {
        let (at, shape) = (rows[2 + 2 * k], rows[3 + 2 * k]);
        if shape[0] <= 0.0 {
            continue;
        }
        let behind = at[3] - (p - Vec3::new(at[0], at[1], at[2])).length();
        line = line.max(shape[0] * crest(behind * shape[1]));
        glow = glow.max(shape[0] * tail(behind, shape[2]));
    }
    let [at, lines, width, far] = [rows[8], rows[9], rows[10], rows[11]];
    if lines[0] > 0.0 {
        let d = (p - Vec3::new(at[0], at[1], at[2])).length();
        let x = (d - at[3]) * lines[1];
        let n = (x + 0.5).floor();
        let m = n - 4.0 * (n * 0.25).floor();
        let near = |from: f32, inv: f32| 1.0 - ((d - from) * inv).clamp(0.0, 1.0);
        let w = if m < 0.5 {
            1.0
        } else if m > 1.5 && m < 2.5 {
            lines[2] * near(far[2], far[3])
        } else {
            lines[3] * near(far[0], far[1])
        };
        let half = width[0].max(width[1] * d * d);
        let core = crest((x - n) / (lines[1] * half));
        let fade = 1.0 - smoothstep(width[2], width[3], d);
        line = line.max(lines[0] * w * core * fade);
    }
    [line, glow]
}

/// The share of the sweep's solid light (the line and its tail's glow) a
/// fragment shows, from `rows`: by how squarely its triangle, whose normal
/// is `face` (any length), faces the eye `to_eye` away ([`FACING`]), and
/// by what its vertex keeps, `keep` ([`SOLID_KEEP`]). The fringe that
/// leans back at a step between two depths is seen edge-on, and a mass's
/// edge keeps half: a line there would trace the mesh's staircase and not
/// the thing. As in the WGSL's `fs_mesh_light_sonar`.
pub fn solid_share(rows: &[[f32; 4]; ROWS], face: Vec3, to_eye: Vec3, keep: f32) -> f32 {
    let toward = face.dot(to_eye);
    let facing = toward * toward / (face.dot(face) * to_eye.dot(to_eye)).max(1e-30);
    smoothstep(rows[1][0], rows[1][1], facing) * smoothstep(rows[1][2], rows[1][3], keep)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FPS: f32 = 72.0;
    const DT: f32 = 1.0 / FPS;
    const HEAD: Vec3 = Vec3::new(0.0, 1.2, 0.0);

    fn input() -> Input {
        Input {
            dt: DT,
            downbeat: false,
            drop: false,
            beat_period_s: Some(0.5),
            buildup: 0.0,
            quiet: false,
            head: HEAD,
            motion: 1.0,
        }
    }

    /// Whether `a` is within `tol` of `b`: for values that went through a
    /// frame clock or a length, past `assert_close!`'s exact compare.
    fn within(a: f32, b: f32, tol: f32) -> bool {
        (a - b).abs() <= tol
    }

    /// `secs` of frames, `at` making each frame's input from its index;
    /// the times a sweep started and what started it.
    fn run(
        sonar: &mut Sonar,
        opts: &SonarOptions,
        secs: f32,
        at: impl Fn(usize) -> Input,
    ) -> Vec<(f32, Cause)> {
        let mut starts = Vec::new();
        for n in 0..(secs * FPS).round() as usize {
            if let Some(cause) = sonar.step(opts, at(n)) {
                starts.push((n as f32 * DT, cause));
            }
        }
        starts
    }

    /// A downbeat on every `every`th frame.
    fn downbeats(every: usize) -> impl Fn(usize) -> Input {
        move |n| Input {
            downbeat: n.is_multiple_of(every),
            ..input()
        }
    }

    /// One sweep from the head, `radius` m out at 2 m/s, at `strength`.
    fn one_sweep(radius: f32, strength: f32) -> Sonar {
        Sonar {
            sweeps: [
                Some(Sweep {
                    origin: HEAD,
                    age_s: radius / 2.0,
                    speed: 2.0,
                    strength,
                }),
                None,
                None,
            ],
            since_start_s: radius / 2.0,
            ..Sonar::default()
        }
    }

    #[test]
    fn a_sweep_starts_on_a_downbeat_from_the_head_and_keeps_its_origin() {
        let mut sonar = Sonar::default();
        let opts = SonarOptions::default();
        // No beat: nothing, for as long as it lasts.
        assert!(run(&mut sonar, &opts, 5.0, |_| input()).is_empty());
        assert!(!sonar.shows());
        assert_close!(sonar.rows(&opts, 1.0), [[0.0f32; 4]; ROWS]);
        // A downbeat: one sweep from the head, at 120 BPM's 2 m/s.
        let started = sonar.step(
            &opts,
            Input {
                downbeat: true,
                ..input()
            },
        );
        assert_eq!(started, Some(Cause::Downbeat));
        let s = *sonar.sweeps().next().expect("a sweep");
        assert_close!(s.origin.to_array(), HEAD.to_array());
        assert_close!(s.speed, 2.0);
        assert_close!(s.strength, BEAT_STRENGTH);
        // The head moves away; the sweep's origin stays, and its radius is
        // its speed times its age.
        for _ in 0..36 {
            sonar.step(
                &opts,
                Input {
                    head: Vec3::new(1.0, 1.5, -2.0),
                    ..input()
                },
            );
        }
        let s = *sonar.sweeps().next().expect("still alive");
        assert_close!(s.origin.to_array(), HEAD.to_array());
        assert!(within(s.radius(), 1.0, 1e-4), "{}", s.radius());
        let rows = sonar.rows(&opts, 1.0);
        assert_close!(rows[2][..3], HEAD.to_array());
        assert!(within(rows[2][3], 1.0, 1e-4));
        // Gone once its radius passes the fade's end.
        run(&mut sonar, &opts, 2.0, |_| input());
        assert_eq!(sonar.sweeps().count(), 0);
    }

    #[test]
    fn starts_keep_the_gap_whatever_asks() {
        let opts = SonarOptions::default();
        // A downbeat every frame: a start every 0.7 s and no sooner.
        let mut sonar = Sonar::default();
        let starts = run(&mut sonar, &opts, 6.0, downbeats(1));
        assert!(starts.len() >= 8, "{starts:?}");
        for pair in starts.windows(2) {
            assert!(pair[1].0 - pair[0].0 >= MIN_GAP_S - 1e-3, "{pair:?}");
        }
        // The test knob under the gap is held to it, and it needs no beat.
        let test = SonarOptions::from_knobs(|name| (name == KNOB_TEST).then(|| "0.2".to_owned()));
        assert_eq!(test.test_s, Some(MIN_GAP_S));
        let mut sonar = Sonar::default();
        let starts = run(&mut sonar, &test, 5.0, |_| Input {
            quiet: true,
            ..input()
        });
        assert!(starts.len() >= 6 && starts.iter().all(|s| s.1 == Cause::Test));
        for pair in starts.windows(2) {
            assert!(pair[1].0 - pair[0].0 >= MIN_GAP_S - 1e-3, "{pair:?}");
        }
        // A drop right behind a downbeat's sweep lifts it instead of
        // starting another; a later one waits for the gap.
        let mut sonar = Sonar::default();
        let starts = run(&mut sonar, &opts, 1.5, |n| Input {
            downbeat: n == 0,
            drop: n == 6 || n == 30,
            ..input()
        });
        assert_eq!(starts.len(), 2, "{starts:?}");
        assert_eq!((starts[0].1, starts[1].1), (Cause::Downbeat, Cause::Drop));
        assert!(starts[1].0 >= MIN_GAP_S - 1e-3);
        let strengths: Vec<f32> = sonar.sweeps().map(|s| s.strength).collect();
        assert_close!(strengths, vec![DROP_STRENGTH, DROP_STRENGTH]);
        // A drop that cannot start its sweep stops asking after a second.
        let mut sonar = one_sweep(0.8, BEAT_STRENGTH);
        sonar.since_start_s = -5.0;
        let starts = run(&mut sonar, &opts, 6.0, |n| Input {
            drop: n == 0,
            ..input()
        });
        assert!(starts.is_empty(), "{starts:?}");
    }

    #[test]
    fn at_most_three_are_alive_and_none_starts_in_silence() {
        // At the slowest speed a sweep lives 18 s: with a downbeat every
        // second three start, and the fourth waits for the first to end.
        let slow = SonarOptions {
            speed: SPEED_FACTOR[0],
            ..SonarOptions::default()
        };
        let mut sonar = Sonar::default();
        let (mut most, mut starts, mut by_17) = (0, 0, 0);
        for n in 0..(30.0 * FPS) as usize {
            let fired = sonar.step(
                &slow,
                Input {
                    downbeat: n.is_multiple_of(72),
                    beat_period_s: Some(1.0),
                    ..input()
                },
            );
            starts += usize::from(fired.is_some());
            most = most.max(sonar.sweeps().count());
            if n == 17 * 72 {
                by_17 = starts;
            }
        }
        assert_eq!(most, SWEEPS);
        assert_eq!(by_17, 3);
        assert!((5..=6).contains(&starts), "{starts}");
        // Quiet: downbeats start nothing.
        let mut sonar = Sonar::default();
        let opts = SonarOptions::default();
        let quiet = run(&mut sonar, &opts, 4.0, |n| Input {
            quiet: true,
            downbeat: n.is_multiple_of(72),
            ..input()
        });
        assert!(quiet.is_empty());
        // The knob at 0: nothing, and what was alive is gone.
        let mut sonar = Sonar::default();
        run(&mut sonar, &opts, 0.5, downbeats(1000));
        assert_eq!(sonar.sweeps().count(), 1);
        let off = SonarOptions {
            mode: 0,
            ..SonarOptions::default()
        };
        assert!(run(&mut sonar, &off, 2.0, downbeats(1)).is_empty());
        assert!(!sonar.shows());
        assert_close!(sonar.rows(&off, 1.0), [[0.0f32; 4]; ROWS]);
    }

    #[test]
    fn the_speed_crosses_four_meters_a_bar_within_its_range() {
        // 120 BPM: a bar of 2 s. 100 BPM: 2.4 s.
        assert_close!(speed(Some(0.5), 1.0), 2.0);
        assert_close!(speed(Some(0.6), 1.0), SPAN_M / 2.4);
        // 40 BPM and 240 BPM: the ends.
        assert_close!(speed(Some(1.5), 1.0), SPEED_M_S[0]);
        assert_close!(speed(Some(0.25), 1.0), SPEED_M_S[1]);
        // No tempo yet, or a senseless one.
        assert_close!(speed(None, 1.0), SPEED_DEFAULT_M_S);
        assert_close!(speed(Some(0.0), 1.0), SPEED_DEFAULT_M_S);
        assert_close!(speed(Some(f32::NAN), 1.0), SPEED_DEFAULT_M_S);
        // The knob's factor, within its range.
        assert_close!(speed(Some(0.5), 0.5), 1.0);
        assert_close!(speed(Some(0.5), 100.0), 2.0 * SPEED_FACTOR[1]);
        // Sweeps a bar apart are 4 m apart.
        let mut sonar = Sonar::default();
        let opts = SonarOptions::default();
        run(&mut sonar, &opts, 2.0 + DT, downbeats(144));
        let radii: Vec<f32> = sonar.sweeps().map(Sweep::radius).collect();
        assert_eq!(radii.len(), 2, "{radii:?}");
        assert!(within((radii[0] - radii[1]).abs(), SPAN_M, 1e-3));
    }

    #[test]
    fn the_line_is_a_crest_with_a_tail_behind_it() {
        // One sweep from the head at 2 m/s, 1 m out, the tail 0.35 s.
        let opts = SonarOptions::default();
        let s = BEAT_STRENGTH;
        let rows = one_sweep(1.0, s).rows(&opts, 1.0);
        assert_close!(rows[2], [0.0, 1.2, 0.0, 1.0]);
        let at = |d: f32| shade(&rows, HEAD + Vec3::new(0.6, 0.0, -0.8) * d);
        let near = |got: [f32; 2], want: [f32; 2]| {
            assert!(
                within(got[0], want[0], 1e-3) && within(got[1], want[1], 1e-3),
                "{got:?} {want:?}"
            );
        };
        // On the shell: the crest, and the tail starts there.
        near(at(1.0), [s, s]);
        // Half a half-width either side: half the crest.
        assert!(within(at(1.0 + 0.5 * HALF_WIDTH_M)[0], 0.5 * s, 1e-3));
        assert!(within(at(1.0 - 0.5 * HALF_WIDTH_M)[0], 0.5 * s, 1e-3));
        // Ahead of the shell: nothing yet, past a half-width.
        near(at(1.0 + HALF_WIDTH_M + 0.001), [0.0, 0.0]);
        near(at(1.5), [0.0, 0.0]);
        // Behind it: no line, the tail falling as a square over 0.7 m.
        near(at(0.65), [0.0, s * 0.25]);
        near(at(0.3 - 0.001), [0.0, 0.0]);
        assert_close!(crest(0.0), 1.0);
        assert_close!(crest(-0.5), 0.5);
        assert_close!(crest(1.0), 0.0);
        assert_close!(tail(-0.01, 2.0), 0.0);
        assert_close!(tail(0.25, 2.0), 0.25);
        assert_close!(tail(0.6, 2.0), 0.0);
        // The light at a place over time rises once and falls once.
        let mut sonar = Sonar::default();
        let mut seen = Vec::new();
        for n in 0..200 {
            sonar.step(
                &opts,
                Input {
                    downbeat: n == 0,
                    ..input()
                },
            );
            let [line, glow] = shade(&sonar.rows(&opts, 1.0), HEAD + Vec3::X * 1.3);
            seen.push(line.max(TAIL_GLOW * glow));
        }
        let most = seen.iter().copied().fold(0.0, f32::max);
        let top = seen
            .iter()
            .position(|v| v.to_bits() == most.to_bits())
            .expect("a peak");
        assert!(most > 0.9 * TAIL_GLOW * s, "{most}");
        assert!(seen[..=top].windows(2).all(|w| w[1] >= w[0] - 1e-6));
        assert!(seen[top..].windows(2).all(|w| w[1] <= w[0] + 1e-6));
        assert_close!(*seen.last().expect("frames"), 0.0);
        // No tail asked: the crest alone.
        let none = SonarOptions {
            tail_s: 0.0,
            ..opts
        };
        let rows = one_sweep(1.0, s).rows(&none, 1.0);
        near(shade(&rows, HEAD + Vec3::X * 0.9), [0.0, 0.0]);
        near(shade(&rows, HEAD + Vec3::X), [s, s]);
    }

    #[test]
    fn the_line_widens_with_the_maps_noise_and_fades_with_distance() {
        // Near: the near width. Far: three times the noise model.
        assert_close!(half_width(HALF_WIDTH_M, 0.5, 1.0), HALF_WIDTH_M);
        assert_close!(half_width(HALF_WIDTH_M, 1.5, 1.0), HALF_WIDTH_M);
        assert_close!(half_width(HALF_WIDTH_M, 3.0, 1.0), 0.054);
        assert_close!(half_width(HALF_WIDTH_M, 4.0, 1.0), 0.096);
        assert_close!(half_width(HALF_WIDTH_M, 3.0, 2.0), 0.108);
        // The recordings' scatter: 0.5 cm near, never under it.
        for range in [0.5f32, 1.0, 2.0, 3.0, 4.0, 4.5] {
            let noise = (NOISE_M_PER_M2 * range * range).max(0.005);
            assert!(
                half_width(HALF_WIDTH_M, range, 1.0) >= 2.9 * noise,
                "{range} m"
            );
        }
        // The light: all there to 1.5 m, half way at 3 m, gone at 4.5 m.
        assert_close!(fade(1.0), 1.0);
        assert_close!(fade(FADE_M[0]), 1.0);
        assert_close!(fade(3.0), 0.5);
        assert_close!(fade(FADE_M[1]), 0.0);
        // In the rows: the strength at the radius, 1 over the width there
        // and over the tail's length.
        let opts = SonarOptions::default();
        let rows = one_sweep(3.0, BEAT_STRENGTH).rows(&opts, 1.0);
        assert_close!(rows[0], [ALPHA_HIGH, TAIL_GLOW, POINT_SWELL, CREST_TINT]);
        assert_close!(rows[2][3], 3.0);
        assert_close!(rows[3][0], BEAT_STRENGTH * 0.5);
        assert!(within(rows[3][1], 1.0 / 0.054, 1e-3));
        assert_close!(rows[3][2], 1.0 / (2.0 * TAIL_S));
        assert_close!(
            rows[1],
            [
                FACING[0] * FACING[0],
                FACING[1] * FACING[1],
                SOLID_KEEP[0],
                SOLID_KEEP[1]
            ]
        );
        let p = HEAD + Vec3::X * 3.0;
        assert!(within(shade(&rows, p)[0], BEAT_STRENGTH * 0.5, 1e-3));
        // The solid light: all of it on a triangle that faces the eye
        // inside a mass, none on one seen edge-on (the fringe that leans
        // back at a step), none at a mass's edge.
        let to_eye = Vec3::new(0.0, 0.0, 2.0);
        let leaning = |cos: f32| Vec3::new((1.0 - cos * cos).sqrt(), 0.0, cos) * 3.0;
        assert_close!(solid_share(&rows, Vec3::Z, to_eye, 1.0), 1.0);
        assert_close!(solid_share(&rows, -Vec3::Z * 0.01, to_eye, 1.0), 1.0);
        assert_close!(solid_share(&rows, leaning(FACING[1]), to_eye, 1.0), 1.0);
        assert_close!(solid_share(&rows, leaning(FACING[0]), to_eye, 1.0), 0.0);
        assert_close!(solid_share(&rows, leaning(0.05), to_eye, 1.0), 0.0);
        let part = solid_share(&rows, leaning(0.2), to_eye, 1.0);
        assert!(part > 0.05 && part < 0.6, "{part}");
        assert_close!(solid_share(&rows, Vec3::Z, to_eye, SOLID_KEEP[0]), 0.0);
        assert_close!(solid_share(&rows, Vec3::Z, to_eye, 0.75), 0.5);
        assert_close!(solid_share(&rows, Vec3::ZERO, to_eye, 1.0), 0.0);
        // The peak by the level and the gain.
        assert_close!(peak_alpha(0.0, 1.0), ALPHA_LOW);
        assert_close!(peak_alpha(1.0, 1.0), ALPHA_HIGH);
        assert_close!(peak_alpha(1.0, 9.0), ALPHA_HIGH * GAIN_MAX);
        assert_close!(peak_alpha(1.0, f32::NAN), 0.0);
        let dark = SonarOptions { gain: 0.0, ..opts };
        assert_close!(one_sweep(3.0, 1.0).rows(&dark, 1.0), [[0.0f32; 4]; ROWS]);
    }

    /// A build: `buildup` held for `secs`.
    fn build(sonar: &mut Sonar, opts: &SonarOptions, buildup: f32, secs: f32) {
        run(sonar, opts, secs, |_| Input { buildup, ..input() });
    }

    #[test]
    fn the_contours_come_up_with_the_build_and_tighten_by_halves() {
        let opts = SonarOptions::default();
        // Under the level that reads as tension: none.
        assert_close!(tension(BUILD_FROM), 0.0);
        assert_close!(tension(0.2), 0.0);
        assert!(within(tension(0.7), 0.5, 1e-5));
        assert_close!(tension(1.0), 1.0);
        let mut sonar = Sonar::default();
        build(&mut sonar, &opts, BUILD_FROM, 5.0);
        assert_close!(sonar.contours().strength, 0.0);
        assert!(!sonar.shows());
        // The sets: the first alone, then the second, then the third.
        assert_close!(set_weights(0.0), [0.0, 0.0]);
        assert!(within(set_weights(0.3)[0], 0.5, 1e-5));
        assert_close!(set_weights(0.45), [1.0, 0.0]);
        assert!(within(set_weights(0.675)[1], 0.5, 1e-5));
        assert_close!(set_weights(1.0), [1.0, 1.0]);
        // A full build: the strength comes up as a slow ramp, the tension
        // the sets read slower still.
        let mut sonar = Sonar::default();
        let mut before = *sonar.contours();
        let (mut steepest, mut fastest) = (0.0f32, 0.0f32);
        for _ in 0..(8.0 * FPS) as usize {
            sonar.step(
                &opts,
                Input {
                    buildup: 1.0,
                    ..input()
                },
            );
            let now = *sonar.contours();
            steepest = steepest.max(now.strength - before.strength);
            fastest = fastest.max(now.tension - before.tension);
            before = now;
        }
        assert!(steepest <= DT / CONTOUR_FADE_S + 1e-6, "{steepest}");
        assert!(fastest <= TENSION_SLEW_PER_S * DT + 1e-6, "{fastest}");
        assert!(within(sonar.contours().strength, 1.0, 1e-4));
        assert!(within(sonar.contours().tension, 1.0, 1e-4));
        // The rows: a third of a sweep's line at most, four lines a
        // pattern, both finer sets in, centered where the head was.
        let rows = sonar.rows(&opts, 1.0);
        assert!(within(rows[9][0], CONTOUR_SHARE, 1e-4));
        assert_close!(rows[9][1], 4.0 / SPACING_M);
        assert!(within(rows[9][2], 1.0, 1e-4) && within(rows[9][3], 1.0, 1e-4));
        assert_close!(rows[8][..3], HEAD.to_array());
        // Lines at the finest spacing near, from the drift out; between
        // two, nothing.
        let drift = rows[8][3];
        let fine = SPACING_M / 4.0;
        let at = |d: f32| shade(&rows, HEAD + Vec3::X * d)[0];
        for n in 4..9 {
            let d = drift + n as f32 * fine;
            let want = CONTOUR_SHARE * fade(d);
            assert!(within(at(d), want, 2e-3), "line {n}: {}", at(d));
            assert!(within(at(d + 0.5 * fine), 0.0, 1e-4));
        }
        // Far: the third set is gone by 2.5 m and the second by 3.5 m,
        // every fourth line stays.
        let n = ((3.6 - drift) / fine).ceil() as i32;
        let n = n + (4 - n.rem_euclid(4)) % 4;
        for (k, share) in [(0, 1.0), (1, 0.0), (2, 0.0), (3, 0.0)] {
            let d = drift + (n + k) as f32 * fine;
            let want = CONTOUR_SHARE * fade(d) * share;
            assert!(within(at(d), want, 2e-3), "line {}: {}", n + k, at(d));
            assert_close!(line_weight(n + k, d, [1.0, 1.0]), share);
        }
        // Faint at any gain: the alpha never passes the cap.
        for gain in [0.5, 1.0, 2.0, GAIN_MAX] {
            let loud = SonarOptions { gain, ..opts };
            let rows = sonar.rows(&loud, 1.0);
            let alpha = rows[0][0] * rows[9][0];
            assert!(alpha <= CONTOUR_ALPHA_MAX + 1e-6, "gain {gain}: {alpha}");
            assert!(alpha <= CONTOUR_SHARE * rows[0][0] + 1e-6);
        }
        const { assert!(CONTOUR_ALPHA_MAX < crate::flash::GENERAL_DELTA) };
        // The knob at 0 never shows them.
        let none = SonarOptions {
            contours: 0.0,
            ..opts
        };
        let mut sonar = Sonar::default();
        build(&mut sonar, &none, 1.0, 6.0);
        assert!(!sonar.shows());
        // The test knob's build: held whatever the music does, in
        // silence too, by the same slow ramps.
        let held = SonarOptions {
            build: Some(0.7),
            ..opts
        };
        let mut sonar = Sonar::default();
        run(&mut sonar, &held, 8.0, |_| Input {
            quiet: true,
            ..input()
        });
        assert!(within(sonar.contours().strength, 1.0, 1e-4));
        assert!(within(sonar.contours().tension, 0.5, 1e-3));
    }

    #[test]
    fn the_spacing_has_a_floor_and_a_full_view_shows_few_lines() {
        // The finest spacing the sets can make.
        let finest = SPACING_M / 4.0;
        assert!(finest >= SPACING_FLOOR_M, "{finest}");
        // No tension, however high, makes a finer set or a stronger one.
        for t in [0.0, 0.5, 1.0, 2.0, f32::INFINITY, f32::NAN] {
            let [second, third] = set_weights(t);
            assert!((0.0..=1.0).contains(&second) && (0.0..=1.0).contains(&third));
        }
        // A surface 1 m away filling the view: at most 8 at the tightest.
        let tight = [lines_across(1.0, 1.0), lines_across(3.0, 1.0)];
        let loose = [lines_across(1.0, 0.0), lines_across(3.0, 0.0)];
        eprintln!(
            "standing lines across a full view: at 1 m {} loosest and {} tightest, at 3 m {} and {}",
            loose[0], tight[0], loose[1], tight[1]
        );
        assert!(tight[0] <= LINES_AT_1M, "{tight:?}");
        assert!(loose[0] <= tight[0] && loose[1] <= tight[1]);
        // At 3 m the finer sets have faded: fewer than at 1 m.
        assert!(tight[1] < tight[0], "{tight:?}");
        // At no distance more than that.
        let mut most = (0, 0.0);
        for k in 0..45 {
            let near = 0.2 + 0.1 * k as f32;
            let n = lines_across(near, 1.0);
            if n > most.0 {
                most = (n, near);
            }
        }
        eprintln!("the most at any distance: {} at {:.1} m", most.0, most.1);
        assert!(most.0 <= LINES_AT_1M, "{most:?}");
        // A line is thin against its spacing wherever its set shows.
        for (k, set) in SET_FAR_M.iter().enumerate() {
            let spacing = SPACING_M / [2.0, 4.0][k];
            let width = 2.0 * half_width(CONTOUR_HALF_WIDTH_M, set[0], 1.0);
            assert!(width <= 0.5 * spacing, "set {k}: {width} of {spacing}");
        }
    }

    #[test]
    fn the_drift_runs_outward_and_never_reverses() {
        let opts = SonarOptions::default();
        let mut sonar = Sonar::default();
        // A build that wobbles, the limiter's motion wobbling too, a NaN,
        // a negative and a 0 among them, and a drop in the middle.
        let mut total = 0.0f32;
        let mut before = sonar.contours().drift_m;
        let mut slowest = f32::INFINITY;
        for n in 0..(20.0 * FPS) as usize {
            let t = n as f32 * DT;
            let motion = match n % 7 {
                0 => f32::NAN,
                1 => -1.0,
                2 => 0.0,
                _ => 0.5 + 0.5 * (t * 3.0).sin().abs(),
            };
            sonar.step(
                &opts,
                Input {
                    buildup: 0.7 + 0.3 * (t * 2.0).sin(),
                    drop: n == 700,
                    motion,
                    ..input()
                },
            );
            let now = sonar.contours().drift_m;
            assert!((0.0..SPACING_M).contains(&now), "{now}");
            // Outward by a step, or wrapped by a whole pattern.
            let step = (now - before).rem_euclid(SPACING_M);
            assert!(step > 0.0 && step <= DRIFT_M_S * DT * 1.01, "{step}");
            slowest = slowest.min(step);
            total += step;
            before = now;
        }
        assert!(
            slowest >= DRIFT_SLOWEST * DRIFT_M_S * DT * 0.99,
            "{slowest}"
        );
        assert!(total > 0.5 * DRIFT_M_S * 20.0);
        // A wrap changes nothing: the same lines a whole pattern on.
        let mut sonar = Sonar::default();
        build(&mut sonar, &opts, 1.0, 8.0);
        let rows = sonar.rows(&opts, 1.0);
        let mut wrapped = rows;
        wrapped[8][3] = rows[8][3] + SPACING_M;
        for k in 0..200 {
            let p = HEAD + Vec3::Z * (0.5 + 0.01 * k as f32);
            let (a, b) = (shade(&rows, p), shade(&wrapped, p));
            assert!(within(a[0], b[0], 2e-3), "{a:?} {b:?}");
        }
        // A line's place moves out with the drift.
        let line = rows[8][3] + SPACING_M;
        let mut later = rows;
        later[8][3] += 0.01;
        let at = |rows: &[[f32; 4]; ROWS], d: f32| shade(rows, HEAD + Vec3::Z * d)[0];
        assert!(at(&rows, line) > 0.9 * CONTOUR_SHARE);
        assert!(within(at(&later, line + 0.01), at(&rows, line), 1e-3));
        assert!(at(&later, line) < 0.6 * at(&rows, line));
    }

    #[test]
    fn a_drop_fades_the_contours_and_sends_a_full_sweep() {
        let opts = SonarOptions::default();
        let mut sonar = Sonar::default();
        build(&mut sonar, &opts, 1.0, 8.0);
        assert!(within(sonar.contours().strength, 1.0, 1e-4));
        // The drop: a sweep at full strength leaves at once.
        let started = sonar.step(
            &opts,
            Input {
                drop: true,
                downbeat: true,
                buildup: 1.0,
                ..input()
            },
        );
        assert_eq!(started, Some(Cause::Drop));
        assert_close!(
            sonar.sweeps().next().expect("the sweep").strength,
            DROP_STRENGTH
        );
        // The lines: gone in 0.3 s, a ramp and no step, with the build
        // still reading full.
        let mut before = sonar.contours().strength;
        let mut frames = 1;
        while sonar.contours().strength > 0.0 {
            sonar.step(
                &opts,
                Input {
                    buildup: 1.0,
                    ..input()
                },
            );
            let now = sonar.contours().strength;
            assert!(before - now <= DT / DROP_FADE_S + 1e-6);
            before = now;
            frames += 1;
            assert!(frames < 100);
        }
        assert!(within(frames as f32 * DT, DROP_FADE_S, 0.03), "{frames}");
        // Away for the drop's quiet, then back slowly if the build is.
        build(&mut sonar, &opts, 1.0, DROP_QUIET_S - DROP_FADE_S - 0.2);
        assert_close!(sonar.contours().strength, 0.0);
        build(&mut sonar, &opts, 1.0, 0.5);
        let s = sonar.contours().strength;
        assert!(s > 0.0 && s < 0.3, "{s}");
        // Without a drop, a build that ends takes them away in the slow
        // ramp.
        let mut sonar = Sonar::default();
        build(&mut sonar, &opts, 1.0, 8.0);
        build(&mut sonar, &opts, 0.0, 0.5 * CONTOUR_FADE_S);
        assert!(within(sonar.contours().strength, 0.5, 0.02));
        build(&mut sonar, &opts, 0.0, 0.5 * CONTOUR_FADE_S + 0.1);
        assert_close!(sonar.contours().strength, 0.0);
        // Quiet takes them away the same way.
        let mut sonar = Sonar::default();
        build(&mut sonar, &opts, 1.0, 8.0);
        run(&mut sonar, &opts, CONTOUR_FADE_S + 0.1, |_| Input {
            buildup: 1.0,
            quiet: true,
            ..input()
        });
        assert_close!(sonar.contours().strength, 0.0);
        // They center on where the head is when they come up, and stay.
        let mut sonar = Sonar::default();
        run(&mut sonar, &opts, 3.0, |_| Input {
            buildup: 1.0,
            head: Vec3::new(2.0, 1.0, 1.0),
            ..input()
        });
        assert_close!(sonar.contours().origin.to_array(), [2.0, 1.0, 1.0]);
        run(&mut sonar, &opts, 1.0, |_| Input {
            buildup: 1.0,
            ..input()
        });
        assert_close!(sonar.contours().origin.to_array(), [2.0, 1.0, 1.0]);
    }

    #[test]
    fn the_knobs_read_within_their_ranges_and_say_so() {
        let d = SonarOptions::default();
        assert_eq!(SonarOptions::from_knobs(|_| None), d);
        assert_eq!((d.mode, d.test_s, d.build), (1, None, None));
        let set = |pairs: &'static [(&'static str, &'static str)]| {
            SonarOptions::from_knobs(move |name| {
                pairs
                    .iter()
                    .find(|(k, _)| *k == name)
                    .map(|(_, v)| (*v).to_owned())
            })
        };
        let o = set(&[
            (KNOB, "2"),
            (KNOB_SPEED, "0.5"),
            (KNOB_WIDTH, "9"),
            (KNOB_GAIN, "-1"),
            (KNOB_TAIL, "0.6"),
            (KNOB_CONTOURS, "0.4"),
            (KNOB_TEST, "2"),
            (KNOB_BUILD, "0.7"),
        ]);
        assert_eq!(o.mode, 2);
        assert_close!(
            [o.speed, o.width, o.gain, o.tail_s, o.contours],
            [0.5, WIDTH_FACTOR[1], 0.0, 0.6, 0.4]
        );
        assert_eq!(o.test_s, Some(2.0));
        assert_eq!(o.build, Some(0.7));
        assert!(o.describe().contains("not built"));
        assert!(o.describe().contains("the build held at 0.7"));
        assert_eq!(set(&[(KNOB_BUILD, "7")]).build, Some(1.0));
        assert_eq!(set(&[(KNOB_BUILD, "x")]).build, None);
        // Unreadable: the default. 0: off.
        assert_eq!(set(&[(KNOB, "x"), (KNOB_TAIL, "nan")]), d);
        let off = set(&[(KNOB, "0")]);
        assert!(!off.on());
        assert_eq!(
            off.line(),
            "sonar: off (debug.fosfora.sonar 0): the depth light as it was"
        );
        assert_eq!(set(&[(KNOB_TEST, "0")]).test_s, None);
        assert!(d.line().starts_with("sonar: on, the depth light's things"));
        assert!(off.change_line(&d).contains("(was: on,"));
    }
}
