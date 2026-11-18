//! Photosensitive safety (board #5451): the rule every look must meet, the
//! count that checks it, and the limiter every audio-driven light goes
//! through so it holds.
//!
//! **The rule** (WCAG 2.3.1, three flashes or below threshold): no more
//! than three general flashes and no more than three red flashes in any
//! one-second period. A general flash is a pair of opposing changes in
//! relative luminance of [`GENERAL_DELTA`] or more of the maximum, where
//! the darker state is below [`DARK_BELOW`]. A red flash is a pair of
//! opposing transitions involving a saturated red: `R / (R + G + B)` at
//! least [`RED_SHARE`], and `(R - G - B) x 320` changing by more than
//! [`RED_DELTA`]. The rule exempts flashes over less than a quarter of a
//! 10 degree field; in a headset the walls and the sky fill the view, so
//! nothing here takes that exemption.
//!
//! **The count** ([`Counter`]). One counter per region of the view, fed the
//! region's mean linear color once a frame. Each signal (the relative
//! luminance, and the red value `max(0, R - G - B) x 320`) runs through a
//! zigzag with the rule's threshold: a transition is a swing of at least
//! the threshold from the last extreme the other way, the first one from
//! the lowest or highest value seen so far. A general transition counts
//! when the darker of its two ends is under [`DARK_BELOW`]; a red one when
//! either end is a saturated red. A flash is two transitions, so a second
//! that holds more than [`MAX_TRANSITIONS`] fails. The window is the
//! frames of one second, ending at each frame in turn ([`in_window`]), as
//! the usual flash analyzers count it.
//!
//! **The limiter** ([`Limiter`]). One place between the analysis and
//! everything the audio lights. `app.rs` hands it the frame's hop as the
//! analysis made it, and every look reads what comes out: the features
//! (every level limited, the triggers let through or held, the beat's
//! phase held), the mel bands, and the values the frame works out from
//! them before they light anything (the beat and downbeat envelopes, the
//! spectrum's bars and glow, the key's tint, the depth light's music:
//! [`follower`]). One thing may read around it, behind a knob: the edge
//! bars' heights (below). It holds the guarantee in one rule: every value
//! it lets through moves in one shared direction at a time. In a rise any
//! value may go up or hold, none may go down; in a fall the other way
//! round. The direction turns at most [`TURNS_PER_WINDOW`] times in any
//! second, and each direction lasts [`MIN_PHASE_S`] at least.
//!
//! **Why that holds the count.** A look's light in a region goes up with
//! the levels it reads: more bass, a brighter curtain; more onset, a
//! brighter sky. Sums, products, curves and the brightest of several such
//! values all go up with them too. So within one direction every region's
//! light only rises, or only falls, and it can change direction only where
//! the limiter turns. Four turns in a second leave at most five
//! runs, so at most five transitions: two and a half flashes, general or
//! red, whatever the music does, on every theme and on any look added later
//! that reads the music this way. The sixth transition the rule allows is
//! left for what the looks do on their own: Aurora's curtains drift across
//! a region with the music held still. While three of the second's four
//! turns are spent the looks' clocks run at half speed ([`Limiter::motion`])
//! so the drift stays inside it.
//!
//! **Keeping the show.** A rise takes a value up at once, so a hit lands on
//! its own frame. A beat turns the direction to a rise as it fires when the
//! turns allow, and the direction falls again once something wants to
//! fall. A level falls quickly after a sparse hit and more slowly the more
//! often it is hit ([`RELEASE_S`], by its own pace, [`PACE_HZ`]), so a run
//! of hi-hats rides on a swell instead of strobing. The beats come first
//! (board #5557): the limiter keeps the beats' period, and a rise a level
//! asks for between two beats is made only when the light can fall again
//! and the next beat still has its two turns ([`Limiter::spares_the_beat`]).
//! Without that the levels' asks spent the turns, and on real songs 5 to
//! 41 % of the beats went through. Beats up to 120 BPM all
//! go through, each one rise and one fall. Faster, every other beat goes
//! through (half time, [`HALF_TIME_S`]), and the beats held back leave the
//! light as it is for a moment ([`BEAT_FALL_S`]) so the next one has its
//! turns. What the wearer loses: within a direction, a value that wants to
//! go the other way waits for the next turn, so in busy music the light
//! moves in steps a few times a second; fast beats land at half time; and
//! nothing pulses on the beat's phase (Aurora's 10 % beat breathing, the
//! world effects' beat-phase envelopes), since the phase runs on its own
//! clock and no direction could hold it. The spectral shape, the key and the
//! pans glide ([`LEVEL_GLIDE_S`]), and a port that moves its shapes with the
//! music reads its levels gliding ([`Limiter::glided`]), so a zoom swells
//! instead of snapping. In busy passages the looks drift at half speed.
//!
//! **Triggers.** The light a beat starts runs on by itself: a ring spreads,
//! a burst of embers fades. Those cannot follow the direction, so a beat
//! goes through only when the direction may rise now and fall again within
//! [`BEAT_FALL_S`], and at least [`BEAT_GAP_S`] after the last one;
//! downbeats and drops have their own gaps. A ring's rise and fall then
//! land on the turns the beat used where it starts, under the wearer.
//!
//! **What it does not cover.** The real room behind the passthrough: the
//! limiter dims nothing it does not draw. The looks' own motion with the
//! music held still (measured: under one flash a second, `flash_room.rs`).
//! A delayed copy of the light (the edge bars' tunnel shows the bars of the
//! last 0.6 s, rung by rung) and light a beat set going (a ring far from
//! where it started) follow the turns only where they start; they are
//! bounded by the beats that go through and measured (with the edge bars'
//! heights free the rungs keep the free bars, below). So is a bar's tip:
//! a bar is brightest at its top, with a cap line, so a region the tip
//! rises through sees a little light pass. A look that maps the music to a
//! hue or a position rather than to light, so that more of the music can
//! mean less light, is outside the proof and has to be measured as the
//! themes are; so is a look built on the bar's clock, which passes as the
//! analysis has it. Shards are the first such look to need more than the
//! measuring: they move their cell edges with the onsets, and on a ceiling,
//! the nearest face, that took one song to 3.5 flashes a second at full
//! strength. While the limiter is on they draw there at no more than
//! `surfaces::CEILING_SHARDS_LIMITED` (board #5533); off, as the wearer
//! set them. And the edge bars' heights when they are free, which
//! do not pass through here at all (next).
//!
//! **The edge bars' heights** (board #5532, [`BARS_KNOB`]). Behind the
//! limiter a bar keeps the limiter's time, not the music's. On the eleven
//! songs measured, 41 to 76 % of the rises a bar makes never show, the
//! ones that do wait up to 264 to 417 ms (the 95th percentile), and at 124
//! BPM a third of the beats go through (`MEASURED.md`). Along the room's
//! edges that reads as bars out of time with the music. So
//! `debug.fosfora.barsfree`, read at launch, says what the edge bars'
//! heights read ([`bars_free_from_knob`]; unset, [`BARS_FREE_DEFAULT`]).
//!
//! At 0 they read the spectrum's bars as the limiter shows them, and
//! everything above holds for the edge bars as it is written.
//!
//! At 1 they read bars of their own, made from the mel bands as the
//! analysis has them (`edge_bars::Heights`). A bar is at the analysis's
//! height on the frame of each of its own hits, whatever the direction,
//! so the guarantee above does not cover the edge bars' light: it is
//! bounded and measured instead. Only the heights are free. The edge bars
//! read nothing else of the music, and the wall spectrum's bars and glow,
//! the features, the mel bands every look reads and the triggers stay
//! behind the limiter as they are at 0.
//!
//! Bounded: a bar on an edge is a thin strip, at most 0.12 of its face's
//! smaller side and 2 to 20 cm tall. It rises at once and falls at
//! `edge_bars::FREE_RELEASE_PER_S`, 1.25 of the tallest bar's height a
//! second, half the wall spectrum's speed. A region dims only as fast as
//! its bars fall, so the faster a band is hit, the less its bars change
//! the light of a region, and past some rate the change is under the
//! rule's tenth and counts for nothing.
//!
//! Measured (`flash_room.rs`), in the room of the measurement at immersion
//! level 1, each mode forced on every surface. The strips alone, on the
//! face or standing, count hits up to 2 a second in the themes' colors and
//! nothing faster, which is 2 flashes. In white, brighter than any color a
//! bar takes and with nothing in front of them, they count hits up to 2.75
//! a second, which is 3. The tunnels alone count half a flash. Each theme
//! whole, on pulse trains from 4 to 16 a second and trains of short hits
//! from 1.5 to 6, is never over three and never over the most it counts
//! with the bars limited. On the eleven songs no theme is over three, in
//! its own edge modes or with any mode forced. In the themes' own modes
//! and with the tunnels forced, no run counts more free than limited. With
//! strips forced on every surface, six runs of 110 count half a flash
//! more, and five of those are Aurora going from 2.5 to 3, the worst case.
//! At the wall spectrum's fall speed, hits three or more times a second
//! took a region beside a strip to 3.5 and 4 flashes, which is why the
//! strips' bars fall at half of it. The tunnels' rungs keep bars that fall
//! at the wall spectrum's speed, since a rung is a small share of its face
//! and the tunnels count the same either way.
//!
//! Not covered at 1: a room that puts a tall strip nearer the eye than the
//! measured one does, so that its bars fill more of a region, and
//! `debug.fosfora.edgebarsgain` or `edgebarsheight` turned up. The white
//! count sits on the three with nothing to spare. That is why the default
//! stays 0 until both have been worn.
//!
//! **On or off** (board #5534, [`decide`]). The limit is the wearer's
//! choice, and it is on unless they turn it off: a new wearer is warned and
//! asked before the first show (`flash_choice.rs`), and the hand menu's
//! "Limit flashing" row and the voice change it later, saved in
//! `hand_menu.json`. A wearer who has not answered has the limit. The
//! knob, `debug.fosfora.flashlimit 0|1`, overrides the wearer's setting
//! either way while it is set, for an A/B and for unworn tests; unset, the
//! setting rules. The log says which of the two decided ([`line`]).

use std::collections::VecDeque;

/// A general transition: a change of relative luminance of this much (of
/// the 0..1 maximum)...
pub const GENERAL_DELTA: f32 = 0.1;
/// ...where the darker state is below this.
pub const DARK_BELOW: f32 = 0.8;
/// A saturated red: `R / (R + G + B)` at least this.
pub const RED_SHARE: f32 = 0.8;
/// A red transition: `max(0, R - G - B) x RED_SCALE` changing by more than
/// this.
pub const RED_DELTA: f32 = 20.0;
pub const RED_SCALE: f32 = 320.0;
/// The window the rule counts in (s): a second, the frames of one second
/// (72 at 72 Hz). Two events a whole second apart or more are never in one
/// window; [`in_window`] says which are.
pub const WINDOW_S: f64 = 1.0;

/// Whether an event `age` seconds old is in the window that ends now: the
/// second's frames, so one exactly a second old (72 frames at 72 Hz) is
/// out. The 0.1 ms is far under any frame and keeps the frame clock's
/// rounding from moving an event across the edge.
pub fn in_window(age: f64) -> bool {
    age < WINDOW_S - 1e-4
}
/// The most flashes the rule allows in a window, and the transitions that
/// make them.
pub const MAX_FLASHES: u32 = 3;
pub const MAX_TRANSITIONS: u32 = 2 * MAX_FLASHES;

/// Relative luminance of a linear color (the sRGB primaries' weights).
pub fn luminance(rgb: [f32; 3]) -> f32 {
    0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2]
}

/// Whether a linear color is a saturated red.
pub fn saturated_red(rgb: [f32; 3]) -> bool {
    let sum = rgb[0] + rgb[1] + rgb[2];
    sum > 1e-6 && rgb[0] / sum >= RED_SHARE
}

/// The red value the rule compares: `max(0, R - G - B) x 320`.
pub fn red_value(rgb: [f32; 3]) -> f32 {
    (rgb[0] - rgb[1] - rgb[2]).max(0.0) * RED_SCALE
}

/// One signal's transitions: a zigzag with a threshold, each end carrying
/// whether it may count (the dark end for a general flash, a saturated red
/// for a red one).
#[derive(Debug, Clone, Copy)]
struct Zigzag {
    /// 1 rising, -1 falling, 0 before the first transition.
    dir: i8,
    /// The extreme since the last transition, and whether it may count.
    ext: (f32, bool),
    /// Before the first transition: the lowest and highest values seen.
    lo: (f32, bool),
    hi: (f32, bool),
    started: bool,
}

impl Zigzag {
    const fn new() -> Self {
        Self {
            dir: 0,
            ext: (0.0, false),
            lo: (0.0, false),
            hi: (0.0, false),
            started: false,
        }
    }

    /// Feed one value with its flag; `Some(counts)` on a transition, where
    /// `counts` says whether the rule counts it, given `pair`, which decides
    /// from the two ends' values and flags.
    fn push(
        &mut self,
        v: f32,
        flag: bool,
        delta: f32,
        strict: bool,
        pair: impl Fn((f32, bool), (f32, bool)) -> bool,
    ) -> Option<bool> {
        let at = (v, flag);
        let swing = |d: f32| if strict { d > delta } else { d >= delta };
        if !self.started {
            self.started = true;
            self.lo = at;
            self.hi = at;
            return None;
        }
        match self.dir {
            0 => {
                if v < self.lo.0 {
                    self.lo = at;
                }
                if v > self.hi.0 {
                    self.hi = at;
                }
                if swing(v - self.lo.0) {
                    let counts = pair(self.lo, at);
                    (self.dir, self.ext) = (1, at);
                    Some(counts)
                } else if swing(self.hi.0 - v) {
                    let counts = pair(self.hi, at);
                    (self.dir, self.ext) = (-1, at);
                    Some(counts)
                } else {
                    None
                }
            }
            1 => {
                if v >= self.ext.0 {
                    self.ext = at;
                    None
                } else if swing(self.ext.0 - v) {
                    let counts = pair(self.ext, at);
                    (self.dir, self.ext) = (-1, at);
                    Some(counts)
                } else {
                    None
                }
            }
            _ => {
                if v <= self.ext.0 {
                    self.ext = at;
                    None
                } else if swing(v - self.ext.0) {
                    let counts = pair(self.ext, at);
                    (self.dir, self.ext) = (1, at);
                    Some(counts)
                } else {
                    None
                }
            }
        }
    }
}

/// One kind of flash's transitions in the rolling window.
#[derive(Debug, Clone)]
struct Track {
    zig: Zigzag,
    times: VecDeque<f64>,
    worst: u32,
    worst_at: f64,
    total: u32,
}

impl Track {
    fn new() -> Self {
        Self {
            zig: Zigzag::new(),
            times: VecDeque::new(),
            worst: 0,
            worst_at: 0.0,
            total: 0,
        }
    }

    fn count(&mut self, t: f64) {
        self.times.push_back(t);
        while self.times.front().is_some_and(|&s| !in_window(t - s)) {
            self.times.pop_front();
        }
        self.total += 1;
        let n = self.times.len() as u32;
        if n > self.worst {
            self.worst = n;
            self.worst_at = t;
        }
    }
}

/// The rule's count for one region of the view.
#[derive(Debug, Clone)]
pub struct Counter {
    general: Track,
    red: Track,
}

impl Default for Counter {
    fn default() -> Self {
        Self::new()
    }
}

/// The most transitions a region saw in any window, general and red, and
/// when (s).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Worst {
    pub general: u32,
    pub general_at: f64,
    pub red: u32,
    pub red_at: f64,
}

impl Worst {
    /// The flashes those transitions make (two to a flash; an odd one is
    /// half of the next).
    pub fn general_flashes(&self) -> f32 {
        self.general as f32 / 2.0
    }

    pub fn red_flashes(&self) -> f32 {
        self.red as f32 / 2.0
    }

    /// Whether the rule holds: at most three of each.
    pub fn passes(&self) -> bool {
        self.general <= MAX_TRANSITIONS && self.red <= MAX_TRANSITIONS
    }

    /// The worse of two, kind by kind.
    #[must_use]
    pub fn max(self, other: Self) -> Self {
        let (general, general_at) = if other.general > self.general {
            (other.general, other.general_at)
        } else {
            (self.general, self.general_at)
        };
        let (red, red_at) = if other.red > self.red {
            (other.red, other.red_at)
        } else {
            (self.red, self.red_at)
        };
        Self {
            general,
            general_at,
            red,
            red_at,
        }
    }
}

impl Counter {
    pub fn new() -> Self {
        Self {
            general: Track::new(),
            red: Track::new(),
        }
    }

    /// The region's mean linear color `rgb` at time `t` (s, increasing).
    pub fn push(&mut self, t: f64, rgb: [f32; 3]) {
        let rgb = rgb.map(|c| if c.is_finite() { c.max(0.0) } else { 0.0 });
        let l = luminance(rgb);
        let dark = |a: (f32, bool), b: (f32, bool)| a.0.min(b.0) < DARK_BELOW;
        if self.general.zig.push(l, false, GENERAL_DELTA, false, dark) == Some(true) {
            self.general.count(t);
        }
        let red = |a: (f32, bool), b: (f32, bool)| a.1 || b.1;
        let counted = self
            .red
            .zig
            .push(red_value(rgb), saturated_red(rgb), RED_DELTA, true, red);
        if counted == Some(true) {
            self.red.count(t);
        }
    }

    /// The most transitions in any window so far.
    pub fn worst(&self) -> Worst {
        Worst {
            general: self.general.worst,
            general_at: self.general.worst_at,
            red: self.red.worst,
            red_at: self.red.worst_at,
        }
    }

    /// Every counted transition so far, general and red.
    pub fn totals(&self) -> (u32, u32) {
        (self.general.total, self.red.total)
    }
}

/// The direction every limited value may move in this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Values may rise or hold.
    Rise,
    /// Values may fall or hold.
    Fall,
}

/// Direction changes allowed in any window ([`in_window`]): four turns
/// make at most five runs, and so at most five transitions, two and a half
/// flashes, which leaves one transition's room for what the looks do on
/// their own (curtains drifting across a region).
pub const TURNS_PER_WINDOW: usize = 4;
/// The shortest a direction lasts (s), so the turns spread out.
pub const MIN_PHASE_S: f32 = 0.1;
/// How far a value must be from where it is held (0..1) to ask for the
/// other direction.
pub const SWITCH_GAP: f32 = 0.04;
/// The shortest time between two beats, two downbeats and two drops let
/// through (s). 0.38 s lets every beat through up to 157 BPM; faster, about
/// every other one.
pub const BEAT_GAP_S: f64 = 0.38;
pub const DOWNBEAT_GAP_S: f64 = 0.75;
/// When the light a beat starts has fallen (s after the beat): a beat goes
/// through only if the direction may turn back down by then, and a beat
/// held back keeps the light from turning up until then.
pub const BEAT_FALL_S: f64 = 0.25;
/// Beats closer than the first (s, 125 BPM) go through every other one
/// (half time) until they are further apart than the second (120 BPM).
pub const HALF_TIME_S: [f64; 2] = [0.48, 0.5];
/// The beat periods the limiter keeps turns for (s, 300 to 40 BPM): a
/// period outside them is no beat to wait for.
pub const BEAT_PERIOD_S: [f64; 2] = [0.2, 1.5];
/// How much of a new period the kept one takes each beat, and how far a
/// period may be from the kept one and still be the same tempo (a share
/// of it).
const PERIOD_FOLLOW: f64 = 0.25;
const PERIOD_SAME: f64 = 0.2;
/// How early the next beat may fire against the kept period (s): two
/// frames at 72 Hz, the analysis's own jitter. A turn that would leave the
/// beat no room if it came this early is not made.
pub const BEAT_EARLY_S: f64 = 0.03;
pub const DROP_GAP_S: f64 = 2.0;
/// A level's fall time constant (s) at a sparse pace and at a dense one:
/// a single hit clears at once, a fast run of them rides on a swell.
pub const RELEASE_S: [f32; 2] = [0.05, 0.4];
/// The pace (a level's own hits a second) over which its fall lengthens
/// from the first to the second.
pub const PACE_HZ: [f32; 2] = [2.5, 5.0];
/// How fast the pace forgets (s).
const PACE_TAU_S: f32 = 1.0;
/// The share of their speed the looks' clocks run at while the turns are
/// nearly spent ([`Limiter::motion`]), and how fast they glide there and
/// back (s).
pub const MOTION_BUSY: f32 = 0.5;
pub const MOTION_GLIDE_S: f32 = 0.25;
/// How fast the levels a look that moves its shapes reads follow the
/// music (s, [`Limiter::glided`]).
pub const SHAPE_GLIDE_S: f32 = 0.5;
/// How fast a level that glides follows the music (s): the spectral shape,
/// the key, the pans, which the looks map to hues and places, so a hue
/// drifts instead of jumping.
pub const LEVEL_GLIDE_S: f32 = 0.3;
/// A level's rise of this much from its last low is a hit (the general
/// flash's threshold on a 0..1 value).
const HIT_DELTA: f32 = 0.1;

/// The beat's phase while the limiter runs: held at its end, as long
/// after a beat, so nothing pulses on the phase between the beats it lets
/// through.
pub const BEAT_PHASE_HELD: f32 = 0.999;

/// The knob, `debug.fosfora.flashlimit`: 0 (off) or 1 (on) overrides the
/// wearer's setting while it is set; unset or anything else, the setting
/// rules.
pub const KNOB: &str = "debug.fosfora.flashlimit";

/// What decided whether the limiter runs (board #5534).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum By {
    /// The knob, set to 0 or 1.
    Knob,
    /// The wearer's saved setting.
    Wearer,
    /// Nothing is saved: the wearer has not chosen, so the limit is on.
    NotAsked,
}

/// Whether the limiter runs, and what decided it: the knob's value when it
/// is `0` or `1`, else the wearer's `setting` (`hand_menu.json`'s
/// `limit_flashing`), else on.
pub fn decide(knob: Option<&str>, setting: Option<bool>) -> (bool, By) {
    match (knob.map(str::trim), setting) {
        (Some("0"), _) => (false, By::Knob),
        (Some("1"), _) => (true, By::Knob),
        (_, Some(on)) => (on, By::Wearer),
        (_, None) => (true, By::NotAsked),
    }
}

/// The edge bars' heights' knob, `debug.fosfora.barsfree` (board #5532,
/// the module docs): 0 the bars as the limiter shows them, 1 bars of their
/// own that follow the analysis (`edge_bars::Heights`). Read at launch.
pub const BARS_KNOB: &str = "debug.fosfora.barsfree";
/// What the edge bars' heights do with the knob unset: limited. The one
/// place to change once both have been worn.
pub const BARS_FREE_DEFAULT: bool = false;

/// Whether the edge bars' heights follow the analysis, from the knob's
/// value: `1` yes, `0` no, anything else [`BARS_FREE_DEFAULT`].
pub fn bars_free_from_knob(value: Option<&str>) -> bool {
    match value.map(str::trim) {
        Some("1") => true,
        Some("0") => false,
        _ => BARS_FREE_DEFAULT,
    }
}

/// The log line: the limiter's state, what decided it (board #5534), and
/// the edge bars' heights.
pub fn line(on: bool, by: By, bars_free: bool) -> String {
    let limit = if on {
        format!(
            "flash limit: on · every audio-driven light turns at most {TURNS_PER_WINDOW} times a second \
             (each direction at least {:.0} ms), beats {:.0} ms apart at least",
            MIN_PHASE_S * 1000.0,
            BEAT_GAP_S * 1000.0
        )
    } else {
        "flash limit: OFF, the music drives the light unlimited".to_owned()
    };
    let by = match by {
        By::Knob => format!("{KNOB} {} overrides the wearer's setting", u8::from(on)),
        By::Wearer => "the wearer's setting".to_owned(),
        By::NotAsked => "the wearer has not chosen".to_owned(),
    };
    let bars = if bars_free {
        format!(
            "the edge bars' heights follow the analysis, falling {} heights a second ({BARS_KNOB} 1)",
            crate::edge_bars::FREE_RELEASE_PER_S
        )
    } else if on {
        format!("the edge bars' heights limited ({BARS_KNOB} 0)")
    } else {
        format!("the edge bars' heights as the wall spectrum's ({BARS_KNOB} 0)")
    };
    format!("{limit} · {by} · {bars}")
}

/// The audio's triggers this frame.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Triggers {
    pub beat: bool,
    pub downbeat: bool,
    pub drop: bool,
}

/// The audio features' slots the limiter treats by name (the order of the
/// core's `AudioFeatures`, which is frozen).
pub mod slot {
    pub const COUNT: usize = 85;
    pub const SUB_BASS: usize = 0;
    pub const BRILLIANCE: usize = 6;
    pub const RMS: usize = 7;
    pub const KICK: usize = 8;
    pub const ONSET: usize = 15;
    pub const BEAT: usize = 16;
    pub const BEAT_PHASE: usize = 17;
    pub const BEAT_STRENGTH: usize = 19;
    pub const LOUDNESS_M: usize = 46;
    pub const DOWNBEAT: usize = 52;
    pub const BAR_PHASE: usize = 53;
    pub const BEAT_IN_BAR: usize = 54;
    pub const DROP: usize = 60;
    pub const PERCUSSIVE_ENERGY: usize = 61;
    pub const HARMONIC_ENERGY: usize = 62;
    pub const BAR_INDEX: usize = 81;
    pub const BEAT_INDEX: usize = 82;
}

/// What the limiter does with a feature slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A level: limited, and it may ask for the other direction.
    Drive,
    /// A level that follows the direction gliding ([`LEVEL_GLIDE_S`]) and
    /// does not ask for it (the spectral shape, the key, the tempo, the
    /// pans: the looks map them to hues and places more than to light, and
    /// their wobble would only turn the direction more often).
    Level,
    /// A trigger: let through at most once per its gap.
    Trigger,
    /// A clock or a counter: passed as it is (the beat's phase holds at
    /// its end after a beat that was not let through).
    Clock,
}

/// How the limiter treats feature slot `i`.
pub fn kind(i: usize) -> Kind {
    use slot::*;
    match i {
        SUB_BASS..=KICK
        | ONSET
        | BEAT_STRENGTH
        | LOUDNESS_M
        | PERCUSSIVE_ENERGY
        | HARMONIC_ENERGY => Kind::Drive,
        BEAT | DOWNBEAT | DROP => Kind::Trigger,
        BEAT_PHASE | BAR_PHASE | BEAT_IN_BAR | BAR_INDEX | BEAT_INDEX => Kind::Clock,
        _ => Kind::Level,
    }
}

/// The values the frame works out from the audio after the analysis, each
/// of which the limiter follows ([`Limiter::follow`]): the beat and
/// downbeat envelopes, the depth light's music, the wall spectrum's glow,
/// the key's tint (three) and the spectrum's bars (`canvas::MAX_BARS`).
pub mod follower {
    pub const BEAT_ENV: usize = 0;
    pub const DOWNBEAT_ENV: usize = 1;
    pub const DEPTH_MUSIC: usize = 2;
    pub const BAR_GLOW: usize = 3;
    pub const TINT: usize = 4;
    pub const BARS: usize = TINT + 3;
    /// The features' beat, downbeat and drop slots (the limiter's own: 1
    /// from a trigger let through until the light falls).
    pub const TRIGGERS: usize = BARS + crate::canvas::MAX_BARS;
    /// The depth light's meters: the bass, the mids and the highs
    /// (board #5543, `depth_light.rs`).
    pub const DEPTH_BANDS: usize = TRIGGERS + 3;
    pub const COUNT: usize = DEPTH_BANDS + 3;
}

/// The mel bands the limiter holds (the analysis makes 64).
pub const MELS: usize = 64;

/// How a level moves within the direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pace {
    /// Up at once, down by its own pace ([`RELEASE_S`]), and it may ask for
    /// the other direction: the levels that carry the light.
    Drive,
    /// The same, without asking: the mel bands, which the bars follow.
    Follow,
    /// Gliding both ways ([`LEVEL_GLIDE_S`]), without asking: the levels
    /// the looks map to a hue or a place rather than to light.
    Glide,
}

/// One level's state: where it is held, whether it has a value yet, its
/// own hits (a zigzag on its raw value) and their pace.
#[derive(Debug, Clone, Copy, Default)]
struct LevelState {
    y: f32,
    primed: bool,
    rising: bool,
    ext: f32,
    pace: f32,
}

/// The photosensitive limiter (board #5451): one place every audio-driven
/// light passes through, so the light cannot flash more than three times
/// a second (the module docs).
#[derive(Debug, Clone)]
pub struct Limiter {
    on: bool,
    phase: Phase,
    since_s: f32,
    now_s: f64,
    /// When the direction turned, the last second's.
    turns: VecDeque<f64>,
    /// The largest pending move up and down after last frame's limiting,
    /// over the values that may ask.
    up: f32,
    down: f32,
    levels: Vec<LevelState>,
    /// The share of their speed the looks' clocks run at ([`Self::motion`]).
    motion: f32,
    /// Every level gliding after its limited value ([`SHAPE_GLIDE_S`]),
    /// within the direction: what a look that moves its shapes with the
    /// music reads ([`Self::glided`]).
    glide: [f32; slot::COUNT],
    followers: Vec<(f32, bool)>,
    last_beat: f64,
    last_downbeat: f64,
    last_drop: f64,
    /// Whether the last beat the audio fired was let through, when it
    /// fired, and whether the beats come too fast for each to go through
    /// (then every other one does: half time).
    beat_kept: bool,
    last_raw_beat: f64,
    half_time: bool,
    /// The beats' period as the analysis fires them (s), following it
    /// ([`PERIOD_FOLLOW`]); 0 while there is none to go by.
    period: f64,
    /// Until when a beat held back keeps the light from turning up: its hit
    /// rides on the light as it is, and the turns are left for the next
    /// beat.
    hold_until: f64,
    /// Beats let through and held back since the start.
    pub beats_kept: u32,
    pub beats_held: u32,
}

impl Limiter {
    pub fn new(on: bool) -> Self {
        Self {
            on,
            phase: Phase::Rise,
            since_s: 0.0,
            now_s: 0.0,
            turns: VecDeque::new(),
            up: 0.0,
            down: 0.0,
            levels: vec![LevelState::default(); slot::COUNT + MELS],
            glide: [0.0; slot::COUNT],
            motion: 1.0,
            followers: vec![(0.0, false); follower::COUNT],
            last_beat: f64::NEG_INFINITY,
            last_downbeat: f64::NEG_INFINITY,
            last_drop: f64::NEG_INFINITY,
            beat_kept: true,
            last_raw_beat: f64::NEG_INFINITY,
            half_time: false,
            period: 0.0,
            hold_until: f64::NEG_INFINITY,
            beats_kept: 0,
            beats_held: 0,
        }
    }

    pub fn on(&self) -> bool {
        self.on
    }

    /// Turn it on or off; either way it starts over from the next values.
    pub fn set_on(&mut self, on: bool) {
        if on != self.on {
            *self = Self {
                beats_kept: self.beats_kept,
                beats_held: self.beats_held,
                ..Self::new(on)
            };
        }
    }

    /// The direction this frame.
    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// The turns in the last second, this frame's included.
    pub fn turns_last_second(&self) -> usize {
        self.turns
            .iter()
            .filter(|&&t| in_window(self.now_s - t))
            .count()
    }

    /// Start a frame `dt` seconds after the last: decide the direction
    /// from the moves last frame left pending and this frame's beat, and
    /// let the triggers through that their gaps allow. Off, the triggers
    /// pass as they are.
    pub fn begin(&mut self, dt: f32, raw: Triggers) -> Triggers {
        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
        self.now_s += f64::from(dt);
        self.since_s += dt;
        if !self.on {
            return raw;
        }
        let now = self.now_s;
        while self.turns.front().is_some_and(|&t| !in_window(now - t)) {
            self.turns.pop_front();
        }
        let room = TURNS_PER_WINDOW.saturating_sub(self.turns.len());
        let settled = self.since_s >= MIN_PHASE_S - 1e-6;
        let pass = |last: &mut f64, fired: bool, gap: f64| {
            let ok = fired && now - *last >= gap - 1e-9;
            if ok {
                *last = now;
            }
            ok
        };
        // A beat starts light that runs on by itself (the rings, a burst):
        // it goes through only when the light may rise now and fall again
        // by the time a hit's light has fallen ([`BEAT_FALL_S`]), so its
        // rise and fall are the frame's own turns, not more.
        let later = self
            .turns
            .iter()
            .filter(|&&t| in_window(now + BEAT_FALL_S - t))
            .count();
        let beat_room = match self.phase {
            Phase::Rise => later < TURNS_PER_WINDOW,
            Phase::Fall => room >= 1 && settled && later + 1 < TURNS_PER_WINDOW,
        };
        // Beats faster than two a second (each takes two turns) go
        // through every other one, a downbeat always when it fits.
        if raw.beat {
            let period = now - self.last_raw_beat;
            self.last_raw_beat = now;
            self.period = if !(BEAT_PERIOD_S[0]..=BEAT_PERIOD_S[1]).contains(&period) {
                0.0
            } else if self.period > 0.0 && (period - self.period).abs() <= PERIOD_SAME * self.period
            {
                self.period + (period - self.period) * PERIOD_FOLLOW
            } else {
                period
            };
            if period < HALF_TIME_S[0] {
                self.half_time = true;
            } else if period > HALF_TIME_S[1] {
                self.half_time = false;
            }
        }
        let skip = self.half_time && self.beat_kept && !raw.downbeat;
        let gated = Triggers {
            beat: beat_room && !skip && pass(&mut self.last_beat, raw.beat, BEAT_GAP_S),
            downbeat: beat_room && pass(&mut self.last_downbeat, raw.downbeat, DOWNBEAT_GAP_S),
            drop: pass(&mut self.last_drop, raw.drop, DROP_GAP_S),
        };
        if raw.beat {
            self.beat_kept = gated.beat;
            if gated.beat {
                self.beats_kept += 1;
            } else {
                self.beats_held += 1;
                self.hold_until = now + BEAT_FALL_S;
            }
        }
        let turn = if gated.beat {
            // The beat rises now, and holds the rise for the shortest
            // phase before anything may fall.
            let turn = self.phase == Phase::Fall;
            self.since_s = 0.0;
            self.phase = Phase::Rise;
            if turn {
                self.turns.push_back(now);
            }
            false
        } else {
            // A fall is always the levels' to ask for. A rise between
            // two beats is made only when it leaves the next beat its
            // turns ([`Self::spares_the_beat`]).
            let wants = match self.phase {
                Phase::Rise => self.down >= SWITCH_GAP,
                Phase::Fall => {
                    now >= self.hold_until
                        && (gated.downbeat || (self.up >= SWITCH_GAP && self.spares_the_beat(now)))
                }
            };
            wants && settled && room >= 1
        };
        if turn {
            self.phase = match self.phase {
                Phase::Rise => Phase::Fall,
                Phase::Fall => Phase::Rise,
            };
            self.since_s = 0.0;
            self.turns.push_back(now);
        }
        // The looks' own motion slows while the turns are nearly spent.
        let busy = self.turns.len() + 1 >= TURNS_PER_WINDOW;
        let target = if busy { MOTION_BUSY } else { 1.0 };
        self.motion += (target - self.motion) * (1.0 - (-dt / MOTION_GLIDE_S).exp());
        self.up = 0.0;
        self.down = 0.0;
        gated
    }

    /// When the next beat that goes through is due, by the kept period:
    /// the next one the analysis fires, or the one after it in half time
    /// when the last one went through. `None` with no period to go by, or
    /// once the beats have stopped coming.
    fn next_beat(&self, now: f64) -> Option<f64> {
        if self.period <= 0.0 || now - self.last_raw_beat > 1.5 * self.period {
            return None;
        }
        let beats = if self.half_time && self.beat_kept {
            2.0
        } else {
            1.0
        };
        Some(self.last_raw_beat + beats * self.period)
    }

    /// Whether a rise now that a level asks for, between two beats, leaves
    /// the next beat its own two turns, the rise on the beat and the fall
    /// after it (board #5557).
    ///
    /// The turns are few, four a second, and every level may ask for one.
    /// On real songs the levels asked all the time and took them (3.6 to
    /// 3.9 turns a second on every song measured), so when a beat fired
    /// there was seldom a turn left: 5 to 41 % of the beats went through,
    /// and with the limit on the light no longer moved with the music. So
    /// the beat comes first. A rise between two beats is made only if the
    /// light can also fall again before the beat, a turn more, and the
    /// beat still has its two after that. A fall is never held back: a
    /// light that stays up hides the next beat as surely as a spent turn
    /// does. With no beat to wait for, any level may turn as before.
    ///
    /// This only says which of the allowed turns are made, never more of
    /// them, so the rule holds as it did (the module docs).
    fn spares_the_beat(&self, now: f64) -> bool {
        let Some(beat) = self.next_beat(now) else {
            return true;
        };
        // The beat may come a little early: count the window from there.
        let at = beat - BEAT_EARLY_S;
        // The turns in the window that ends at `end`, this one with them.
        let turns = |end: f64| {
            self.turns.iter().filter(|&&t| in_window(end - t)).count()
                + usize::from(in_window(end - now))
        };
        at - now >= 2.0 * BEAT_FALL_S
            && turns(at) + 2 <= TURNS_PER_WINDOW
            && turns(at + BEAT_FALL_S) + 3 <= TURNS_PER_WINDOW
    }

    /// The share of their speed the looks' clocks run at this frame (the
    /// surfaces' clock, the ports' drifts): 1, gliding to [`MOTION_BUSY`]
    /// while three of the second's four turns are spent. A look's own
    /// motion (a curtain drifting across a region) changes the light
    /// without the music, and the turns leave room for one such change a
    /// second; a slower drift keeps it to that while the music uses the
    /// rest. Off, 1.
    pub fn motion(&self) -> f32 {
        if self.on { self.motion } else { 1.0 }
    }

    /// Note a value's pending moves after limiting: `x` where the audio
    /// has it, `y` where it is held.
    fn ask(&mut self, x: f32, y: f32) {
        self.up = self.up.max(x - y);
        self.down = self.down.max(y - x);
    }

    /// Level `i` (a feature slot, or `slot::COUNT` plus a mel band) to
    /// `x` this frame, `dt` after the last, as `pace` says: in a rise it
    /// goes up (at once, or gliding) and holds where it would fall; in a
    /// fall it goes down (at once at a sparse pace and slower the denser its
    /// own hits, or gliding) and holds where it would rise.
    fn level(&mut self, i: usize, x: f32, dt: f32, pace: Pace) -> f32 {
        let asks = pace == Pace::Drive;
        let glides = pace == Pace::Glide;
        if !x.is_finite() {
            return x;
        }
        let phase = self.phase;
        let s = &mut self.levels[i];
        if !s.primed {
            *s = LevelState {
                y: x,
                primed: true,
                rising: true,
                ext: x,
                pace: 0.0,
            };
            return x;
        }
        // The level's own pace: its hits, a zigzag on the raw value.
        s.pace *= (-dt / PACE_TAU_S).exp();
        if s.rising {
            if x > s.ext {
                s.ext = x;
            } else if s.ext - x >= HIT_DELTA {
                (s.rising, s.ext) = (false, x);
            }
        } else if x < s.ext {
            s.ext = x;
        } else if x - s.ext >= HIT_DELTA {
            (s.rising, s.ext) = (true, x);
            s.pace += 1.0;
        }
        let y = s.y;
        let glide = 1.0 - (-dt / LEVEL_GLIDE_S).exp();
        s.y = match phase {
            Phase::Rise if glides && x > y => y + (x - y) * glide,
            Phase::Fall if glides && x < y => y - (y - x) * glide,
            Phase::Rise => y.max(x),
            Phase::Fall if x < y => {
                let t = ((s.pace - PACE_HZ[0]) / (PACE_HZ[1] - PACE_HZ[0])).clamp(0.0, 1.0);
                let tau = RELEASE_S[0] + (RELEASE_S[1] - RELEASE_S[0]) * t * t * (3.0 - 2.0 * t);
                x.max(y - (y - x) * (1.0 - (-dt / tau).exp()))
            }
            Phase::Fall => y,
        };
        let y = s.y;
        if asks {
            self.ask(x, y);
        }
        y
    }

    /// This frame's features, `dt` after the last, in place: every level
    /// limited, the triggers set to `gated` ([`Self::begin`]), the beat's
    /// phase held at its end after a beat that was not let through, the
    /// other clocks as they are. Off, untouched.
    pub fn features(&mut self, f: &mut [f32; slot::COUNT], dt: f32, gated: Triggers) {
        if !self.on {
            return;
        }
        for i in 0..slot::COUNT {
            f[i] = match kind(i) {
                Kind::Drive => self.level(i, f[i], dt, Pace::Drive),
                Kind::Level => self.level(i, f[i], dt, Pace::Glide),
                Kind::Trigger => {
                    let (fired, k) = match i {
                        slot::BEAT => (gated.beat, 0),
                        slot::DOWNBEAT => (gated.downbeat, 1),
                        _ => (gated.drop, 2),
                    };
                    self.follow(follower::TRIGGERS + k, f32::from(u8::from(fired)))
                }
                Kind::Clock => Self::clock(i, f),
            };
        }
        let a = 1.0 - (-dt.max(0.0) / SHAPE_GLIDE_S).exp();
        for i in (0..slot::COUNT).filter(|&i| matches!(kind(i), Kind::Drive | Kind::Level)) {
            let (g, y) = (self.glide[i], f[i]);
            self.glide[i] = match self.phase {
                Phase::Rise if y > g => g + (y - g) * a,
                Phase::Fall if y < g => g - (g - y) * a,
                _ => g,
            };
        }
    }

    /// `f` (this frame's features as [`Self::features`] left them) with
    /// every level gliding after its value, for a look that moves its
    /// shapes with the music (`surface_port::Port::shapes`): a kaleidoscope
    /// that zooms with the bass sweeps its pattern across every region, so
    /// the bass it reads swells instead of jumping. The glide keeps to the
    /// direction too. Off, untouched.
    pub fn glided(&self, f: &mut [f32; slot::COUNT]) {
        if !self.on {
            return;
        }
        for i in (0..slot::COUNT).filter(|&i| matches!(kind(i), Kind::Drive | Kind::Level)) {
            f[i] = self.glide[i];
        }
    }

    /// Clock slot `i`: the beat's phase held at its end; the bar's clock
    /// and the counts as the analysis has them. A bar is a second or more
    /// at any tempo the analysis reports, and a clock that jumped where the
    /// analysis's runs on would make its own steps in the looks that read
    /// it.
    fn clock(i: usize, f: &[f32; slot::COUNT]) -> f32 {
        if i == slot::BEAT_PHASE {
            BEAT_PHASE_HELD
        } else {
            f[i]
        }
    }

    /// This frame's mel bands, limited as levels in place. Off, untouched.
    pub fn mel(&mut self, mel: &mut [f32], dt: f32) {
        if !self.on {
            return;
        }
        for (k, m) in mel.iter_mut().take(MELS).enumerate() {
            *m = self.level(slot::COUNT + k, *m, dt, Pace::Follow);
        }
    }

    /// Follower `id` ([`follower`]) at `x` this frame: in a rise it takes
    /// `x` where that is higher and holds otherwise, in a fall where that
    /// is lower. Off, `x`.
    pub fn follow(&mut self, id: usize, x: f32) -> f32 {
        if !self.on || !x.is_finite() {
            return x;
        }
        let (y, primed) = self.followers[id];
        let y = if !primed {
            x
        } else {
            match self.phase {
                Phase::Rise => y.max(x),
                Phase::Fall => y.min(x),
            }
        };
        self.followers[id] = (y, true);
        self.ask(x, y);
        y
    }

    /// Followers `id..` at `xs`, in place.
    pub fn follow_all(&mut self, id: usize, xs: &mut [f32]) {
        for (k, x) in xs.iter_mut().enumerate() {
            *x = self.follow(id + k, *x);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FPS: f64 = 72.0;

    /// A counter fed `secs` of a color per frame.
    fn count(secs: f64, color: impl Fn(f64) -> [f32; 3]) -> Counter {
        let mut c = Counter::new();
        let frames = (secs * FPS).round() as u32;
        for i in 0..frames {
            let t = f64::from(i) / FPS;
            c.push(t, color(t));
        }
        c
    }

    fn square(t: f64, hz: f64) -> bool {
        (t * hz).fract() < 0.5
    }

    #[test]
    fn a_square_wave_flashes_at_its_rate_and_three_a_second_pass() {
        // Black to white at 2, 3 and 4 Hz: two transitions a cycle.
        for hz in [2.0, 3.0, 4.0] {
            let c = count(5.0, |t| if square(t, hz) { [1.0; 3] } else { [0.0; 3] });
            let w = c.worst();
            // A second's frames hold the rate's transitions exactly.
            assert_eq!(w.general, (2.0 * hz) as u32, "{hz} Hz: {w:?}");
            assert_eq!(w.red, 0, "white is not red");
        }
        // Two transitions exactly a second apart are not in one window.
        assert!(!in_window(1.0) && in_window(71.0 / 72.0));
        // 2 and 3 Hz pass, 4 Hz fails.
        assert!(
            count(5.0, |t| if square(t, 2.0) { [1.0; 3] } else { [0.0; 3] })
                .worst()
                .passes()
        );
        assert!(
            !count(5.0, |t| if square(t, 4.0) { [1.0; 3] } else { [0.0; 3] })
                .worst()
                .passes()
        );
    }

    #[test]
    fn small_and_bright_changes_are_not_flashes() {
        // A swing under a tenth of the luminance, at 10 Hz.
        let c = count(3.0, |t| if square(t, 10.0) { [0.35; 3] } else { [0.3; 3] });
        assert_eq!(c.worst().general, 0);
        // A big swing between two bright states: the darker over 0.8.
        let c = count(3.0, |t| if square(t, 10.0) { [1.0; 3] } else { [0.85; 3] });
        assert_eq!(c.worst().general, 0);
        // The same swing reaching under 0.8 counts.
        let c = count(3.0, |t| if square(t, 10.0) { [1.0; 3] } else { [0.7; 3] });
        assert!(c.worst().general > MAX_TRANSITIONS);
    }

    #[test]
    fn a_slow_wave_counts_by_its_swings_not_its_speed() {
        // A 4 Hz sine over the whole range fails, a 1 Hz one passes, and a
        // 4 Hz one of 0.06 swings under the threshold.
        let sine = |hz: f64, amp: f32| {
            move |t: f64| [0.5 + amp * (std::f64::consts::TAU * hz * t).sin() as f32; 3]
        };
        assert!(!count(4.0, sine(4.0, 0.4)).worst().passes());
        assert!(count(4.0, sine(1.0, 0.4)).worst().passes());
        assert_eq!(count(4.0, sine(4.0, 0.03)).worst().general, 0);
    }

    #[test]
    fn red_flashes_need_a_saturated_red_and_a_swing_of_twenty() {
        // A saturated red pulsing at 5 Hz: (R - G - B) x 320 from 0 to
        // about 150. Its luminance swings 0.1 too.
        let red = [0.5, 0.02, 0.02];
        assert!(saturated_red(red));
        assert!(red_value(red) > 100.0);
        let c = count(3.0, |t| if square(t, 5.0) { red } else { [0.0; 3] });
        let w = c.worst();
        assert!(w.red > MAX_TRANSITIONS, "{w:?}");
        // A red too faint to change the value by 20: 0.05 x 320 = 16.
        let faint = [0.06, 0.005, 0.005];
        assert!(saturated_red(faint) && red_value(faint) < RED_DELTA);
        let c = count(3.0, |t| if square(t, 5.0) { faint } else { [0.0; 3] });
        assert_eq!(c.worst().red, 0);
        // An orange swinging as much is no red flash: under 0.8 red.
        let orange = [0.8, 0.3, 0.05];
        assert!(!saturated_red(orange));
        let c = count(3.0, |t| if square(t, 5.0) { orange } else { [0.0; 3] });
        assert_eq!(c.worst().red, 0);
        assert!(c.worst().general > MAX_TRANSITIONS);
    }

    #[test]
    fn the_window_is_a_rolling_second() {
        // Three flashes in a second, then a second's pause, then three
        // more: never more than six transitions in one window.
        let burst = |t: f64| {
            let k = (t * 6.0) as u32;
            let on = k.is_multiple_of(2);
            let in_burst = (t % 2.0) < 1.0;
            if in_burst && on { [1.0; 3] } else { [0.0; 3] }
        };
        let w = count(6.0, burst).worst();
        assert!(w.general <= MAX_TRANSITIONS, "{w:?}");
        assert!(w.passes());
        // The same flashes evenly at 4 Hz fail.
        assert!(
            !count(6.0, |t| if square(t, 4.0) { [1.0; 3] } else { [0.0; 3] })
                .worst()
                .passes()
        );
    }

    #[test]
    fn a_steady_or_unreadable_region_never_flashes() {
        assert_eq!(count(3.0, |_| [0.4, 0.1, 0.1]).worst(), Worst::default());
        let c = count(3.0, |t| {
            if square(t, 8.0) {
                [f32::NAN; 3]
            } else {
                [0.0; 3]
            }
        });
        assert_eq!(c.worst(), Worst::default());
        let w = Worst {
            general: 3,
            general_at: 1.0,
            red: 7,
            red_at: 2.0,
        };
        let v = Worst {
            general: 5,
            general_at: 4.0,
            red: 1,
            red_at: 0.5,
        };
        let m = w.max(v);
        assert_eq!((m.general, m.red), (5, 7));
        assert!(!m.passes());
        assert_close!([m.general_flashes(), m.red_flashes()], [2.5, 3.5]);
    }

    /// A small deterministic generator for the tests' inputs.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> f32 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((self.0 >> 40) as f32) / (1u64 << 24) as f32
        }
    }

    /// One frame of input: the features, the mel bands, the followers'
    /// raw values and the triggers.
    struct Input {
        f: [f32; slot::COUNT],
        mel: [f32; MELS],
        followers: [f32; follower::TRIGGERS],
        triggers: Triggers,
    }

    /// Every input at `level`, a beat when `beat`.
    fn input(level: f32, beat: bool) -> Input {
        Input {
            f: [level; slot::COUNT],
            mel: [level; MELS],
            followers: [level; follower::TRIGGERS],
            triggers: Triggers {
                beat,
                downbeat: false,
                drop: false,
            },
        }
    }

    /// What the limiter made of one frame: the direction, the limited
    /// features, mel bands and followers, and the triggers let through.
    struct Output {
        phase: Phase,
        f: [f32; slot::COUNT],
        mel: [f32; MELS],
        followers: [f32; follower::TRIGGERS],
        gated: Triggers,
    }

    fn step(l: &mut Limiter, mut i: Input) -> Output {
        let dt = (1.0 / FPS) as f32;
        let gated = l.begin(dt, i.triggers);
        l.features(&mut i.f, dt, gated);
        l.mel(&mut i.mel, dt);
        l.follow_all(0, &mut i.followers);
        Output {
            phase: l.phase(),
            f: i.f,
            mel: i.mel,
            followers: i.followers,
            gated,
        }
    }

    /// Every limited value of an output, in one list.
    fn values(o: &Output) -> Vec<f32> {
        let levels = (0..slot::COUNT).filter(|&i| matches!(kind(i), Kind::Drive | Kind::Level));
        levels
            .map(|i| o.f[i])
            .chain(o.mel)
            .chain(o.followers)
            .collect()
    }

    /// A named input over time.
    type Source = (String, Box<dyn FnMut(f64) -> Input>);
    /// A named light worked out from the limited values.
    type Light = (&'static str, Box<dyn Fn(&[f32]) -> f32>);

    /// The inputs the tests drive: noise on every value, a pulse train at
    /// `hz` on every value at full depth, and both mixed with beats.
    fn sources() -> Vec<Source> {
        let mut out: Vec<Source> = Vec::new();
        for hz in [2.0, 4.0, 8.0, 12.0, 16.0, 30.0] {
            out.push((
                format!("pulse {hz} Hz"),
                Box::new(move |t: f64| {
                    let k = (t * hz).floor();
                    let prev = ((t - 1.0 / FPS) * hz).floor();
                    let level = if (t * hz).fract() < 0.5 { 1.0 } else { 0.0 };
                    input(level, k > prev)
                }),
            ));
        }
        for seed in 1..4u64 {
            let mut rng = Rng(seed);
            out.push((
                format!("noise {seed}"),
                Box::new(move |_| {
                    let mut i = input(0.0, rng.next() < 0.3);
                    for v in i.f.iter_mut().chain(&mut i.mel).chain(&mut i.followers) {
                        *v = rng.next();
                    }
                    i
                }),
            ));
        }
        out
    }

    #[test]
    fn the_direction_turns_at_most_four_times_in_any_second() {
        for (name, mut source) in sources() {
            let mut l = Limiter::new(true);
            let mut turns = Vec::new();
            let mut last = l.phase();
            let mut since = f64::NEG_INFINITY;
            for n in 0..(20.0 * FPS) as u32 {
                let t = f64::from(n) / FPS;
                let o = step(&mut l, source(t));
                if o.phase != last {
                    // Each direction lasts the shortest phase at least.
                    assert!(t - since >= f64::from(MIN_PHASE_S) - 1e-3, "{name} at {t}");
                    turns.push(t);
                    since = t;
                    last = o.phase;
                }
                let window = turns.iter().filter(|&&s| in_window(t - s)).count();
                assert!(window <= TURNS_PER_WINDOW, "{name}: {window} turns by {t}");
            }
            assert!(
                turns.len() > 10,
                "{name}: it turns at all ({})",
                turns.len()
            );
        }
    }

    #[test]
    fn every_value_moves_with_the_direction_or_holds() {
        for (name, mut source) in sources() {
            let mut l = Limiter::new(true);
            let mut was: Option<Vec<f32>> = None;
            for n in 0..(10.0 * FPS) as u32 {
                let o = step(&mut l, source(f64::from(n) / FPS));
                let now = values(&o);
                if let Some(was) = &was {
                    for (k, (a, b)) in was.iter().zip(&now).enumerate() {
                        let ok = match o.phase {
                            Phase::Rise => b >= a,
                            Phase::Fall => b <= a,
                        };
                        assert!(ok, "{name}: value {k} went {a} -> {b} in {:?}", o.phase);
                    }
                }
                was = Some(now);
            }
        }
    }

    /// The construction: any light that rises with every limited value
    /// (weighted sums, products, the brightest of several, a curve) turns
    /// with the direction alone, so at most five transitions fit in a
    /// second however the music moves.
    #[test]
    fn any_light_that_rises_with_the_values_stays_within_the_rule() {
        let mut weights = Rng(7);
        let lights: Vec<Light> = vec![
            (
                "mean",
                Box::new(|v: &[f32]| v.iter().sum::<f32>() / v.len() as f32),
            ),
            ("one value", Box::new(|v: &[f32]| v[3])),
            (
                "brightest",
                Box::new(|v: &[f32]| v.iter().copied().fold(0.0, f32::max)),
            ),
            (
                "product",
                Box::new(|v: &[f32]| v[0] * v[7] * (1.0 + 0.8 * v[15])),
            ),
            ("curve", Box::new(|v: &[f32]| (v[1] * v[1] * 3.0).min(1.0))),
        ];
        let w: Vec<f32> = (0..200).map(|_| weights.next()).collect();
        let weighted = move |v: &[f32]| {
            let s: f32 = v.iter().zip(&w).map(|(x, w)| x * w).sum();
            s / w.iter().sum::<f32>()
        };
        for (name, mut source) in sources() {
            let mut l = Limiter::new(true);
            let mut counters: Vec<Counter> = (0..=lights.len()).map(|_| Counter::new()).collect();
            for n in 0..(20.0 * FPS) as u32 {
                let t = f64::from(n) / FPS;
                let o = step(&mut l, source(t));
                let v = values(&o);
                for (c, (_, light)) in counters.iter_mut().zip(&lights) {
                    c.push(t, [light(&v); 3]);
                }
                counters[lights.len()].push(t, [weighted(&v); 3]);
            }
            for (c, light) in counters
                .iter()
                .zip(lights.iter().map(|l| l.0).chain(["weighted"]))
            {
                let w = c.worst();
                assert!(
                    w.general <= TURNS_PER_WINDOW as u32 + 1,
                    "{name}, {light}: {w:?}"
                );
            }
        }
    }

    #[test]
    fn a_kick_at_120_bpm_lands_on_its_beat_and_clears_before_the_next() {
        let mut l = Limiter::new(true);
        let kick = |t: f64| {
            // A hit of 80 ms every half second.
            let phase = t % 0.5;
            let level = if phase < 0.08 { 1.0 } else { 0.05 };
            input(level, phase < 1.0 / FPS - 1e-9)
        };
        let mut peaks = 0;
        let mut beats = (0, 0);
        for n in 0..(8.0 * FPS) as u32 {
            let t = f64::from(n) / FPS;
            let i = kick(t);
            let raw = i.f[slot::KICK];
            let beat = i.triggers.beat;
            let o = step(&mut l, i);
            beats.0 += u32::from(beat);
            beats.1 += u32::from(o.gated.beat);
            if t > 1.0 {
                if beat {
                    // On the beat's own frame, at full height.
                    assert_close!(o.f[slot::KICK], raw);
                    peaks += 1;
                }
                // Clear before the next kick.
                if t % 0.5 > 0.4 {
                    assert!(o.f[slot::KICK] < 0.15, "{t}: {}", o.f[slot::KICK]);
                }
            }
        }
        assert_eq!(beats.0, beats.1, "every beat goes through");
        assert!(peaks >= 13, "{peaks}");
    }

    #[test]
    fn a_fast_run_of_hits_rides_on_a_swell() {
        // Hits at 9 a second on one band, a steady kick elsewhere: the
        // band's light stays up between its hits instead of strobing.
        let mut l = Limiter::new(true);
        let mut low = f32::MAX;
        for n in 0..(6.0 * FPS) as u32 {
            let t = f64::from(n) / FPS;
            let mut i = input(0.3, false);
            i.f[slot::BRILLIANCE] = if (t * 9.0).fract() < 0.3 { 1.0 } else { 0.0 };
            let o = step(&mut l, i);
            if t > 2.0 {
                low = low.min(o.f[slot::BRILLIANCE]);
            }
        }
        assert!(low > 0.5, "the swell's low {low}");
    }

    #[test]
    fn beats_go_through_at_most_every_beat_gap_and_the_phase_holds() {
        // 230 BPM: beats every 0.26 s, about every other one goes through.
        let mut l = Limiter::new(true);
        let mut kept = Vec::new();
        for n in 0..(10.0 * FPS) as u32 {
            let t = f64::from(n) / FPS;
            let beat = ((t - 1.0 / FPS) * 230.0 / 60.0).floor() < (t * 230.0 / 60.0).floor();
            let mut i = input(0.5, beat);
            i.triggers.downbeat = beat;
            i.f[slot::BEAT_PHASE] = (t * 230.0 / 60.0).fract() as f32;
            let o = step(&mut l, i);
            assert_close!(o.f[slot::BEAT_PHASE], BEAT_PHASE_HELD);
            // The beat's slot: 1 from a beat let through while the light
            // rises, 0 once it falls.
            if o.gated.beat {
                assert_close!(o.f[slot::BEAT], 1.0);
            }
            if o.phase == Phase::Fall {
                assert_close!(o.f[slot::BEAT], 0.0);
            }
            assert!(o.f[slot::BEAT_PHASE] <= BEAT_PHASE_HELD);
            if o.gated.beat {
                kept.push(t);
            }
        }
        for w in kept.windows(2) {
            assert!(w[1] - w[0] >= BEAT_GAP_S - 1e-6, "{w:?}");
        }
        let (all, through) = (l.beats_kept + l.beats_held, l.beats_kept);
        assert!(
            through * 3 >= all && through * 3 <= 2 * all,
            "{through} of {all}"
        );
    }

    #[test]
    fn off_it_passes_everything_as_it_is() {
        for (name, mut source) in sources() {
            let mut l = Limiter::new(false);
            for n in 0..(3.0 * FPS) as u32 {
                let i = source(f64::from(n) / FPS);
                let (f, mel, followers, triggers) = (i.f, i.mel, i.followers, i.triggers);
                let o = step(&mut l, i);
                assert_eq!(o.gated, triggers, "{name}");
                assert!(o.f.iter().zip(f).all(|(a, b)| a.to_bits() == b.to_bits()));
                assert!(
                    o.mel
                        .iter()
                        .zip(mel)
                        .all(|(a, b)| a.to_bits() == b.to_bits())
                );
                assert!(
                    o.followers
                        .iter()
                        .zip(followers)
                        .all(|(a, b)| a.to_bits() == b.to_bits())
                );
            }
        }
        // Turned on and off, it starts over from the next values.
        let mut l = Limiter::new(true);
        step(&mut l, input(1.0, true));
        l.set_on(false);
        assert!(!l.on());
        l.set_on(true);
        let o = step(&mut l, input(0.2, false));
        assert_close!(o.f[slot::RMS], 0.2);
    }

    /// Board #5534: the knob at 0 or 1 overrides the wearer's setting; unset
    /// or anything else the setting rules; with nothing saved the limit is
    /// on. The line says which decided.
    #[test]
    fn the_knob_overrides_the_wearers_setting_and_the_line_says_which() {
        for setting in [None, Some(true), Some(false)] {
            assert_eq!(decide(Some("0"), setting), (false, By::Knob));
            assert_eq!(decide(Some(" 0 "), setting), (false, By::Knob));
            assert_eq!(decide(Some("1"), setting), (true, By::Knob));
        }
        for knob in [None, Some(""), Some("yes"), Some("2")] {
            assert_eq!(decide(knob, Some(true)), (true, By::Wearer), "{knob:?}");
            assert_eq!(decide(knob, Some(false)), (false, By::Wearer), "{knob:?}");
            // Never asked, or asked and not answered: on.
            assert_eq!(decide(knob, None), (true, By::NotAsked), "{knob:?}");
        }
        // No value of the knob and no state of the file turns it off but
        // the two that say off.
        for knob in [None, Some("0"), Some("1"), Some("off")] {
            for setting in [None, Some(true), Some(false)] {
                let (on, _) = decide(knob, setting);
                let off = knob == Some("0") || (knob != Some("1") && setting == Some(false));
                assert_eq!(on, !off, "{knob:?} {setting:?}");
            }
        }
        // The line: the limit, what decided it, the edge bars' heights.
        assert_eq!(
            line(true, By::NotAsked, false),
            "flash limit: on · every audio-driven light turns at most 4 times a second \
             (each direction at least 100 ms), beats 380 ms apart at least · the wearer has not chosen · \
             the edge bars' heights limited (debug.fosfora.barsfree 0)"
        );
        assert!(line(true, By::Wearer, false).contains("least · the wearer's setting · the edge"));
        assert_eq!(
            line(false, By::Wearer, false),
            "flash limit: OFF, the music drives the light unlimited · the wearer's setting · \
             the edge bars' heights as the wall spectrum's (debug.fosfora.barsfree 0)"
        );
        assert!(line(false, By::Knob, false).starts_with(
            "flash limit: OFF, the music drives the light unlimited · \
             debug.fosfora.flashlimit 0 overrides the wearer's setting · "
        ));
        assert!(
            line(true, By::Knob, false)
                .contains(" · debug.fosfora.flashlimit 1 overrides the wearer's setting · ")
        );
        for by in [By::Knob, By::Wearer, By::NotAsked] {
            for bars in [false, true] {
                let (on, off) = (line(true, by, bars), line(false, by, bars));
                assert!(on.starts_with("flash limit: on"), "{on}");
                assert!(off.starts_with("flash limit: OFF"), "{off}");
                // Only the knob's override names the knob.
                assert_eq!(off.contains(KNOB), by == By::Knob, "{off}");
                let value = format!("{BARS_KNOB} {}", u8::from(bars));
                assert!(on.ends_with(&format!("({value})")), "{on}");
                assert!(off.ends_with(&format!("({value})")), "{off}");
            }
        }
        assert!(line(true, By::Wearer, false).contains("heights limited"));
        assert!(
            line(true, By::Wearer, true)
                .contains("follow the analysis, falling 1.25 heights a second")
        );
        // The edge bars' heights: the knob's 0 or 1, else the default.
        assert!(bars_free_from_knob(Some("1")));
        assert!(bars_free_from_knob(Some(" 1 ")));
        assert!(!bars_free_from_knob(Some("0")));
        for other in [None, Some(""), Some("yes"), Some("2")] {
            assert_eq!(bars_free_from_knob(other), BARS_FREE_DEFAULT, "{other:?}");
        }
    }

    #[test]
    fn the_slots_are_the_core_features() {
        use fosfora_app::audio::features::{AudioFeatures, NUM_FEATURES};
        assert_eq!(slot::COUNT, NUM_FEATURES);
        assert_eq!(MELS, fosfora_app::audio::analyzer::SPECTROGRAM_MELS);
        let at = |set: fn(&mut AudioFeatures)| {
            let mut f = AudioFeatures::default();
            set(&mut f);
            f.as_slice().iter().position(|&v| v > 0.5).expect("a slot")
        };
        assert_eq!(at(|f| f.sub_bass = 1.0), slot::SUB_BASS);
        assert_eq!(at(|f| f.brilliance = 1.0), slot::BRILLIANCE);
        assert_eq!(at(|f| f.rms = 1.0), slot::RMS);
        assert_eq!(at(|f| f.kick = 1.0), slot::KICK);
        assert_eq!(at(|f| f.onset = 1.0), slot::ONSET);
        assert_eq!(at(|f| f.beat = 1.0), slot::BEAT);
        assert_eq!(at(|f| f.beat_phase = 1.0), slot::BEAT_PHASE);
        assert_eq!(at(|f| f.beat_strength = 1.0), slot::BEAT_STRENGTH);
        assert_eq!(at(|f| f.loudness_m = 1.0), slot::LOUDNESS_M);
        assert_eq!(at(|f| f.downbeat = 1.0), slot::DOWNBEAT);
        assert_eq!(at(|f| f.bar_phase = 1.0), slot::BAR_PHASE);
        assert_eq!(at(|f| f.beat_in_bar = 1.0), slot::BEAT_IN_BAR);
        assert_eq!(at(|f| f.drop = 1.0), slot::DROP);
        assert_eq!(at(|f| f.percussive_energy = 1.0), slot::PERCUSSIVE_ENERGY);
        assert_eq!(at(|f| f.harmonic_energy = 1.0), slot::HARMONIC_ENERGY);
        assert_eq!(at(|f| f.bar_index = 1.0), slot::BAR_INDEX);
        assert_eq!(at(|f| f.beat_index = 1.0), slot::BEAT_INDEX);
        // The bands, the rms and the kick drive; the triggers and clocks
        // are their own.
        for i in 0..=slot::KICK {
            assert_eq!(kind(i), Kind::Drive, "{i}");
        }
        assert_eq!(kind(slot::BEAT), Kind::Trigger);
        assert_eq!(kind(slot::BEAT_PHASE), Kind::Clock);
    }

    /// The share of beats that go through by tempo, a hit on each: every
    /// one up to 120 BPM, and every other one (half time) from 125 BPM.
    #[test]
    fn every_beat_goes_through_to_120_bpm_and_every_other_one_faster() {
        for (bpm, share) in [
            (90.0, 1.0),
            (120.0, 1.0),
            (128.0, 0.5),
            (140.0, 0.5),
            (150.0, 0.5),
            (174.0, 0.5),
            (230.0, 0.5),
        ] {
            let mut l = Limiter::new(true);
            let hz = bpm / 60.0;
            let (mut all, mut through) = (0u32, 0u32);
            for n in 0..(30.0 * FPS) as u32 {
                let t = f64::from(n) / FPS;
                // The beat on the first frame of each cycle (the 1e-9 keeps
                // rounding from finding one twice).
                let beat = ((t - 1.0 / FPS) * hz + 1e-9).floor() < (t * hz + 1e-9).floor();
                let level = if (t * hz + 1e-9).fract() < 0.2 {
                    1.0
                } else {
                    0.1
                };
                let o = step(&mut l, input(level, beat));
                if t > 2.0 {
                    all += u32::from(beat);
                    through += u32::from(o.gated.beat);
                }
            }
            let got = f64::from(through) / f64::from(all);
            eprintln!("{bpm} BPM: {through} of {all} beats through ({got:.2})");
            assert!((got - share).abs() < 0.03, "{bpm} BPM: {got}");
        }
    }

    /// Board #5557: the beats keep their turns when other levels ask all
    /// the time, as they do in a real song. A kick on the beat and a run
    /// of hi-hats between the beats, seven a second: before, the hats took
    /// the turns and a third of the beats or fewer went through.
    #[test]
    fn busy_levels_between_the_beats_do_not_take_the_beats_turns() {
        for (bpm, least, most) in [(90.0, 0.95, 1.0), (110.0, 0.95, 1.0), (128.0, 0.47, 0.53)] {
            let mut l = Limiter::new(true);
            let hz = bpm / 60.0;
            let (mut all, mut through) = (0u32, 0u32);
            let mut turns = VecDeque::new();
            let mut phase = l.phase();
            for n in 0..(40.0 * FPS) as u32 {
                let t = f64::from(n) / FPS;
                let beat = ((t - 1.0 / FPS) * hz + 1e-9).floor() < (t * hz + 1e-9).floor();
                // The kick: every level up for a fifth of the beat.
                let kick = if (t * hz + 1e-9).fract() < 0.2 {
                    1.0
                } else {
                    0.1
                };
                let mut i = input(kick, beat);
                // The hats: the onset and the top bands, on their own
                // clock.
                let hat = if (t * 7.0).fract() < 0.3 { 0.9 } else { 0.05 };
                i.f[slot::ONSET] = hat;
                i.f[slot::BRILLIANCE] = hat;
                for m in &mut i.mel[48..] {
                    *m = hat;
                }
                let o = step(&mut l, i);
                if o.phase != phase {
                    phase = o.phase;
                    turns.push_back(t);
                }
                while turns.front().is_some_and(|&x| !in_window(t - x)) {
                    turns.pop_front();
                }
                assert!(turns.len() <= TURNS_PER_WINDOW, "{bpm} BPM at {t}");
                if t > 4.0 {
                    all += u32::from(beat);
                    through += u32::from(o.gated.beat);
                }
            }
            let got = f64::from(through) / f64::from(all);
            eprintln!("{bpm} BPM with hats: {through} of {all} beats through ({got:.2})");
            assert!((least..=most).contains(&got), "{bpm} BPM: {got}");
        }
    }

    #[test]
    fn the_looks_slow_while_the_turns_are_spent_and_shapes_glide() {
        // A calm song: full speed. A busy one (8 Hz pulses): half speed.
        let mut calm = Limiter::new(true);
        let mut busy = Limiter::new(true);
        let (mut slowest, mut glided) = (1.0f32, Vec::new());
        for n in 0..(6.0 * FPS) as u32 {
            let t = f64::from(n) / FPS;
            step(&mut calm, input(0.5, false));
            let level = if (t * 8.0).fract() < 0.5 { 1.0 } else { 0.0 };
            let o = step(&mut busy, input(level, false));
            let mut f = o.f;
            busy.glided(&mut f);
            if t > 2.0 {
                slowest = slowest.min(busy.motion());
                glided.push((f[slot::RMS], o.f[slot::RMS], o.phase));
            }
        }
        assert_close!(calm.motion(), 1.0);
        assert!((slowest - MOTION_BUSY).abs() < 0.05, "{slowest}");
        assert_close!(Limiter::new(false).motion(), 1.0);
        // The gliding view swings less than the levels, and with the
        // direction.
        let swing = |k: usize| {
            let v = glided.iter().map(|g| if k == 0 { g.0 } else { g.1 });
            v.clone().fold(0.0f32, f32::max) - v.fold(1.0f32, f32::min)
        };
        assert!(swing(0) < 0.6 * swing(1), "{} {}", swing(0), swing(1));
        for w in glided.windows(2) {
            let ok = match w[1].2 {
                Phase::Rise => w[1].0 >= w[0].0,
                Phase::Fall => w[1].0 <= w[0].0,
            };
            assert!(ok, "{w:?}");
        }
    }
}
