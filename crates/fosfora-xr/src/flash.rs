//! Photosensitive safety (board #5451): the rule every look must meet, and
//! the count that checks it.
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
}
