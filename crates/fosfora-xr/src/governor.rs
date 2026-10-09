//! The thermal governor (board #3789): steps the cloud density down while
//! the frame is over budget and back up while it is comfortably under, so
//! a headset that throttles after an hour worn (its GPU held at 456 MHz
//! instead of 640, `docs/xr/MEASURED.md`, "Worn A/B of the frame cost")
//! gives ground instead of dropping to 24-30 fps, and takes it back once
//! it cools. Plain numbers, so the decision logic builds and tests on the
//! desktop; `app.rs` feeds it once a frame and applies its density.
//!
//! **The signal** is the runtime's app GPU time
//! (`XR_META_performance_metrics`), the mean over the last second
//! ([`GpuWindow`]). Without the counters it falls back to the frame
//! loop's one-second window: fps and long frames ([`Signal::Frames`]).
//!
//! **The budget** is the display period times a margin: over above
//! [`GOV_DOWN_FRAC`] of it, under below [`GOV_UP_FRAC`]; between the two
//! the governor holds (the hysteresis band). Over for [`GOV_DOWN_S`] steps
//! the density down by [`GOV_STEP`], to [`GOV_FLOOR`] at the lowest; under
//! for [`GOV_UP_S`] steps it back up, to the wearer's own setting (the
//! ceiling) at the highest. Each step restarts the clock, so a run of
//! over-budget frames takes one step per [`GOV_DOWN_S`].
//!
//! **The second stage** (a seam for board #3788): over budget for another
//! [`GOV_DOWN_S`] at the floor raises [`Level::Starved`], which a cheaper
//! face update rate can follow later; it is the first thing given back.

/// Over budget this long (seconds) takes one step down.
pub const GOV_DOWN_S: f32 = 3.0;
/// Under budget this long (seconds) takes one step back up.
pub const GOV_UP_S: f32 = 20.0;
/// Over budget: the smoothed GPU time above this fraction of the display
/// period (`debug.fosfora.govdown`; 12.8 ms at 72 Hz).
pub const GOV_DOWN_FRAC: f32 = 0.92;
/// Under budget: below this fraction of the period (`debug.fosfora.govup`;
/// 10.4 ms at 72 Hz).
pub const GOV_UP_FRAC: f32 = 0.75;
/// The density one step moves.
pub const GOV_STEP: f32 = 0.1;
/// The governor never takes the density below this (a wearer's own lower
/// setting stands; the governor then has nothing to give).
pub const GOV_FLOOR: f32 = 0.3;
/// The span the GPU time is smoothed over (seconds).
pub const GPU_WINDOW_S: f32 = 1.0;
/// A gap this long between two steps (seconds: the session paused, the
/// headset taken off) restarts the clock, so a run from before it does not
/// count the time nothing ran.
pub const GAP_S: f32 = 0.5;
/// The fallback without GPU time: over when the frame rate falls below
/// this fraction of the display rate, or a second holds this many long
/// frames; under only with no long frame and the rate at
/// [`FRAMES_UNDER_FRAC`] of the display's or above (fps cannot say how
/// far under the budget a frame is, so this is the best it can do).
pub const FRAMES_OVER_FRAC: f32 = 0.95;
pub const FRAMES_OVER_LONG: u32 = 3;
pub const FRAMES_UNDER_FRAC: f32 = 0.98;

/// What the governor watches this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Signal {
    /// The app GPU time, smoothed (ms).
    Gpu(f32),
    /// No GPU time: the frame loop's last one-second window.
    Frames { fps: f32, long: u32 },
}

/// Where a signal falls against the budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Budget {
    Over,
    Within,
    Under,
}

/// How far the governor has given ground.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// At the wearer's setting.
    Free,
    /// The density below the wearer's setting.
    Governed,
    /// At the floor and still over budget: the next thing to give is
    /// outside the density (board #3788, the faces' update rate).
    Starved,
}

/// One step the governor took.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Change {
    /// The density before and after (equal when only the level moved).
    pub from: f32,
    pub to: f32,
    /// The level after the step.
    pub level: Level,
    /// A step down (over budget) or up.
    pub down: bool,
    /// How long the budget held before the step (seconds).
    pub held_s: f32,
}

/// The over and under margins (fractions of the display period).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Margins {
    pub down: f32,
    pub up: f32,
}

impl Default for Margins {
    fn default() -> Self {
        Self {
            down: GOV_DOWN_FRAC,
            up: GOV_UP_FRAC,
        }
    }
}

impl Margins {
    /// The over and under limits for a display period (ms).
    pub fn limits_ms(self, period_ms: f32) -> (f32, f32) {
        (self.down * period_ms, self.up * period_ms)
    }

    /// Where `signal` falls against the budget for `period_ms`.
    pub fn budget(self, signal: Signal, period_ms: f32) -> Budget {
        match signal {
            Signal::Gpu(ms) => {
                let (over, under) = self.limits_ms(period_ms);
                if ms > over {
                    Budget::Over
                } else if ms < under {
                    Budget::Under
                } else {
                    Budget::Within
                }
            }
            Signal::Frames { fps, long } => {
                let hz = 1000.0 / period_ms;
                if fps < FRAMES_OVER_FRAC * hz || long >= FRAMES_OVER_LONG {
                    Budget::Over
                } else if long == 0 && fps >= FRAMES_UNDER_FRAC * hz {
                    Budget::Under
                } else {
                    Budget::Within
                }
            }
        }
    }
}

/// The density the governor runs the cloud at, under the wearer's own.
#[derive(Debug, Clone, Copy)]
pub struct Governor {
    margins: Margins,
    /// The wearer's setting: the most the governor gives back.
    ceiling: f32,
    density: f32,
    starved: bool,
    /// The budget the signal has held since this time (seconds).
    run: Option<(Budget, f32)>,
    /// The last step's time.
    last_s: Option<f32>,
}

impl Governor {
    pub fn new(margins: Margins, ceiling: f32) -> Self {
        Self {
            margins,
            ceiling,
            density: ceiling,
            starved: false,
            run: None,
            last_s: None,
        }
    }

    pub fn margins(&self) -> Margins {
        self.margins
    }

    /// The wearer's setting.
    pub fn ceiling(&self) -> f32 {
        self.ceiling
    }

    /// The density to run the cloud at.
    pub fn density(&self) -> f32 {
        self.density
    }

    /// The density while it is below the wearer's setting (what the
    /// stepper shows, marked), else `None`.
    pub fn governed(&self) -> Option<f32> {
        (self.density < self.ceiling - 1e-4).then_some(self.density)
    }

    pub fn level(&self) -> Level {
        if self.starved {
            Level::Starved
        } else if self.governed().is_some() {
            Level::Governed
        } else {
            Level::Free
        }
    }

    /// The wearer set the density (the stepper): it is the new ceiling and
    /// the density, the governor's debt cleared and its clock restarted.
    pub fn set_ceiling(&mut self, ceiling: f32) {
        self.ceiling = ceiling;
        self.density = ceiling;
        self.starved = false;
        self.run = None;
    }

    /// No signal this frame (no GPU time and no frame window yet): the
    /// clock restarts, nothing moves.
    pub fn idle(&mut self) {
        self.run = None;
    }

    /// Once a frame: `now_s` the frame time, `signal` the load,
    /// `period_ms` the display period. Returns the step taken, if any.
    pub fn step(&mut self, now_s: f32, signal: Signal, period_ms: f32) -> Option<Change> {
        if self.last_s.is_some_and(|last| now_s - last > GAP_S) {
            self.run = None;
        }
        self.last_s = Some(now_s);
        if period_ms.is_nan() || period_ms <= 0.0 {
            self.run = None;
            return None;
        }
        let budget = self.margins.budget(signal, period_ms);
        let since = match self.run {
            Some((b, since)) if b == budget => since,
            _ => {
                self.run = Some((budget, now_s));
                now_s
            }
        };
        let held_s = now_s - since;
        let from = self.density;
        let change = |to: f32, level: Level, down: bool| Change {
            from,
            to,
            level,
            down,
            held_s,
        };
        match budget {
            Budget::Within => None,
            Budget::Over if held_s >= GOV_DOWN_S => {
                self.run = Some((budget, now_s));
                // Tenths, so the steps land on the stepper's grid and float
                // noise never makes a step short of the floor.
                let to = round_cents(from - GOV_STEP).max(GOV_FLOOR);
                if to < from - 1e-4 {
                    self.density = to;
                    Some(change(to, self.level(), true))
                } else if !self.starved && from <= GOV_FLOOR + 1e-4 {
                    self.starved = true;
                    Some(change(from, Level::Starved, true))
                } else {
                    None
                }
            }
            Budget::Under if held_s >= GOV_UP_S => {
                self.run = Some((budget, now_s));
                if self.starved {
                    self.starved = false;
                    return Some(change(from, self.level(), false));
                }
                let to = round_cents(from + GOV_STEP).min(self.ceiling);
                (to > from + 1e-4).then(|| {
                    self.density = to;
                    change(to, self.level(), false)
                })
            }
            Budget::Over | Budget::Under => None,
        }
    }
}

fn round_cents(v: f32) -> f32 {
    (v * 100.0).round() / 100.0
}

/// The app GPU time over the last [`GPU_WINDOW_S`]: one sample per frame
/// the runtime reports it for.
#[derive(Debug, Clone, Default)]
pub struct GpuWindow {
    samples: std::collections::VecDeque<(f32, f32)>,
}

impl GpuWindow {
    /// Add the frame at `now_s` with `gpu_ms`, dropping what is older than
    /// the window.
    pub fn push(&mut self, now_s: f32, gpu_ms: f32) {
        self.samples.push_back((now_s, gpu_ms));
        self.prune(now_s);
    }

    /// The mean over the window ending at `now_s`, `None` without a sample
    /// in it (the counters off, or every query failed).
    pub fn mean(&mut self, now_s: f32) -> Option<f32> {
        self.prune(now_s);
        let n = self.samples.len();
        (n > 0).then(|| self.samples.iter().map(|s| s.1).sum::<f32>() / n as f32)
    }

    fn prune(&mut self, now_s: f32) {
        while self
            .samples
            .front()
            .is_some_and(|s| now_s - s.0 > GPU_WINDOW_S)
        {
            self.samples.pop_front();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 72 Hz.
    const PERIOD: f32 = 1000.0 / 72.0;
    const OVER: f32 = 15.0;
    const UNDER: f32 = 9.0;
    const BETWEEN: f32 = 11.5;

    /// Run the governor at 72 frames a second from `t0` for `secs` with a
    /// constant GPU time; returns every change and the end time.
    fn run(g: &mut Governor, t0: f32, secs: f32, ms: f32) -> (Vec<Change>, f32) {
        let mut changes = Vec::new();
        let frames = (secs * 72.0).round() as u32;
        let mut t = t0;
        for i in 0..frames {
            t = t0 + i as f32 / 72.0;
            changes.extend(g.step(t, Signal::Gpu(ms), PERIOD));
        }
        (changes, t + 1.0 / 72.0)
    }

    fn densities(changes: &[Change]) -> Vec<f32> {
        changes.iter().map(|c| c.to).collect()
    }

    #[test]
    fn limits_at_72_hz_match_the_brief() {
        let (over, under) = Margins::default().limits_ms(PERIOD);
        assert!((over - 12.78).abs() < 0.01, "{over}");
        assert!((under - 10.42).abs() < 0.01, "{under}");
    }

    #[test]
    fn holds_while_under_budget() {
        let mut g = Governor::new(Margins::default(), 1.0);
        let (changes, _) = run(&mut g, 0.0, 120.0, UNDER);
        assert!(changes.is_empty());
        assert_close!(g.density(), 1.0);
        assert_eq!(g.level(), Level::Free);
    }

    #[test]
    fn steps_down_after_three_seconds_over() {
        let mut g = Governor::new(Margins::default(), 1.0);
        let (changes, t) = run(&mut g, 0.0, 2.9, OVER);
        assert!(changes.is_empty());
        let (changes, t) = run(&mut g, t, 0.2, OVER);
        assert_eq!(changes.len(), 1);
        let c = changes[0];
        assert!(c.down);
        assert_close!([c.from, c.to], [1.0, 0.9]);
        assert!(c.held_s >= GOV_DOWN_S);
        assert_eq!(g.level(), Level::Governed);
        // One step per 3 s while it stays over.
        let (changes, _) = run(&mut g, t, 6.0, OVER);
        assert_close!(densities(&changes), [0.8, 0.7]);
    }

    #[test]
    fn does_not_oscillate_at_the_boundary() {
        let mut g = Governor::new(Margins::default(), 1.0);
        // Inside the hysteresis band, near either edge, for minutes.
        let (over, under) = Margins::default().limits_ms(PERIOD);
        let mut t = 0.0;
        for ms in [over - 0.05, under + 0.05, BETWEEN] {
            let (changes, end) = run(&mut g, t, 120.0, ms);
            assert!(changes.is_empty(), "{ms} ms: {changes:?}");
            t = end;
        }
        // Over for 2 s then back inside the band, repeatedly: the clock
        // restarts each time, so no step.
        for _ in 0..30 {
            let (a, end) = run(&mut g, t, 2.0, OVER);
            let (b, end) = run(&mut g, end, 1.0, BETWEEN);
            assert!(a.is_empty() && b.is_empty());
            t = end;
        }
        assert_close!(g.density(), 1.0);
        // Governed, then inside the band: it holds where it is, neither
        // down nor up.
        let (changes, t) = run(&mut g, t, 3.1, OVER);
        assert_close!(densities(&changes), [0.9]);
        let (changes, _) = run(&mut g, t, 120.0, BETWEEN);
        assert!(changes.is_empty());
        assert_close!(g.density(), 0.9);
    }

    #[test]
    fn climbs_back_after_twenty_seconds_under() {
        let mut g = Governor::new(Margins::default(), 1.0);
        let (_, t) = run(&mut g, 0.0, 6.1, OVER);
        assert_close!(g.density(), 0.8);
        let (changes, t) = run(&mut g, t, 19.9, UNDER);
        assert!(changes.is_empty());
        let (changes, t) = run(&mut g, t, 0.2, UNDER);
        assert_eq!(changes.len(), 1);
        assert!(!changes[0].down);
        assert_close!(changes[0].to, 0.9);
        let (changes, _) = run(&mut g, t, 60.0, UNDER);
        assert_close!(densities(&changes), [1.0]);
        assert_eq!(g.level(), Level::Free);
    }

    #[test]
    fn respects_the_floor_and_starves_there() {
        let mut g = Governor::new(Margins::default(), 1.0);
        let (changes, t) = run(&mut g, 0.0, 21.1, OVER);
        assert_close!(densities(&changes), [0.9, 0.8, 0.7, 0.6, 0.5, 0.4, 0.3]);
        assert_close!(g.density(), GOV_FLOOR);
        assert_eq!(g.level(), Level::Governed);
        // Still over at the floor: the second stage, once.
        let (changes, t) = run(&mut g, t, 30.0, OVER);
        assert_eq!(changes.len(), 1);
        assert_close!([changes[0].from, changes[0].to], [0.3, 0.3]);
        assert_eq!(changes[0].level, Level::Starved);
        assert_eq!(g.level(), Level::Starved);
        assert_close!(g.density(), GOV_FLOOR);
        // Under again: the second stage is given back first, then the
        // density.
        let (changes, _) = run(&mut g, t, 40.1, UNDER);
        assert_eq!(changes.len(), 2);
        assert_close!(densities(&changes), [0.3, 0.4]);
        assert_eq!(changes[0].level, Level::Governed);
    }

    #[test]
    fn respects_the_ceiling() {
        let mut g = Governor::new(Margins::default(), 0.65);
        let (changes, t) = run(&mut g, 0.0, 12.1, OVER);
        assert_close!(densities(&changes), [0.55, 0.45, 0.35, 0.3]);
        let (changes, _) = run(&mut g, t, 200.0, UNDER);
        assert_close!(densities(&changes), [0.4, 0.5, 0.6, 0.65]);
        assert_close!(g.density(), 0.65);
        assert_eq!(g.level(), Level::Free);
    }

    #[test]
    fn a_wearer_setting_below_the_floor_stands() {
        let mut g = Governor::new(Margins::default(), 0.2);
        let (changes, _) = run(&mut g, 0.0, 3.1, OVER);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].level, Level::Starved);
        assert_close!(g.density(), 0.2);
    }

    #[test]
    fn resets_on_a_wearer_step() {
        let mut g = Governor::new(Margins::default(), 1.0);
        let (_, t) = run(&mut g, 0.0, 2.5, OVER);
        // The wearer steps it mid-run: a new ceiling, the clock restarted.
        g.set_ceiling(0.65);
        assert_close!(g.density(), 0.65);
        let (changes, t) = run(&mut g, t, 2.9, OVER);
        assert!(changes.is_empty());
        let (changes, t) = run(&mut g, t, 5.0, OVER);
        assert_close!(densities(&changes), [0.55, 0.45]);
        assert_eq!(g.level(), Level::Governed);
        // A step on the stepper while governed clears the debt.
        g.set_ceiling(0.5);
        assert_eq!(g.level(), Level::Free);
        assert_close!([g.ceiling(), g.density()], [0.5, 0.5]);
        let (changes, _) = run(&mut g, t, 100.0, UNDER);
        assert!(changes.is_empty());
    }

    #[test]
    fn falls_back_to_the_frame_window() {
        let m = Margins::default();
        let frames = |fps, long| Signal::Frames { fps, long };
        assert_eq!(m.budget(frames(72.0, 0), PERIOD), Budget::Under);
        assert_eq!(m.budget(frames(71.0, 1), PERIOD), Budget::Within);
        assert_eq!(m.budget(frames(71.5, 3), PERIOD), Budget::Over);
        assert_eq!(m.budget(frames(57.0, 15), PERIOD), Budget::Over);
        let mut g = Governor::new(m, 1.0);
        let mut changes = Vec::new();
        for i in 0..(72 * 4) {
            changes.extend(g.step(i as f32 / 72.0, frames(30.0, 30), PERIOD));
        }
        assert_close!(densities(&changes), [0.9]);
    }

    #[test]
    fn a_pause_restarts_the_clock() {
        let mut g = Governor::new(Margins::default(), 1.0);
        let (changes, t) = run(&mut g, 0.0, 2.5, OVER);
        assert!(changes.is_empty());
        // A minute with no frame, then over again: the run starts over.
        let (changes, t) = run(&mut g, t + 60.0, 2.9, OVER);
        assert!(changes.is_empty());
        let (changes, _) = run(&mut g, t, 0.2, OVER);
        assert_close!(densities(&changes), [0.9]);
    }

    #[test]
    fn no_period_is_no_step() {
        let mut g = Governor::new(Margins::default(), 1.0);
        for i in 0..1000 {
            assert!(g.step(i as f32, Signal::Gpu(OVER), 0.0).is_none());
        }
        assert_close!(g.density(), 1.0);
    }

    #[test]
    fn gpu_window_is_the_last_second_mean() {
        let mut w = GpuWindow::default();
        assert!(w.mean(0.0).is_none());
        for i in 0..72 {
            w.push(i as f32 / 72.0, 10.0);
        }
        for i in 72..108 {
            w.push(i as f32 / 72.0, 16.0);
        }
        // The last second: 36 frames at 10 ms, 36 at 16 (plus the edge).
        let m = w.mean(107.0 / 72.0).unwrap();
        assert!((12.8..13.2).contains(&m), "{m}");
        // A second with no sample: none.
        assert!(w.mean(10.0).is_none());
    }
}
