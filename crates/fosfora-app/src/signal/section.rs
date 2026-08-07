//! Causal section labeling for the Signal broadcast: intro / build / drop / break /
//! steady, with per-transition confidence and dwell measured in bars.
//!
//! This is deliberately a signal-layer component, not an `AudioFeatures` field: it
//! consumes finished features (`buildup`, `drop`, loudness, the bar clock) so the
//! frame ABI stays untouched, and the trait boundary lets an ONNX causal model
//! replace the heuristic later (Workstream D tiers report through
//! [`SectionEstimator::tier`]) without any schema change.
//!
//! `outro` exists in the label set (the schema string set is complete, and offline
//! labelers may use it) but the live heuristic never returns it: causally, an outro
//! is indistinguishable from a break or a quiet steady until the song has already
//! ended. Shipping a low-confidence guess would poison trust in the address.

use crate::audio::features::AudioFeatures;

use super::clock::BarClock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SectionLabel {
    Intro,
    Build,
    Drop,
    Break,
    Outro,
    Steady,
}

impl SectionLabel {
    pub fn as_str(self) -> &'static str {
        match self {
            SectionLabel::Intro => "intro",
            SectionLabel::Build => "build",
            SectionLabel::Drop => "drop",
            SectionLabel::Break => "break",
            SectionLabel::Outro => "outro",
            SectionLabel::Steady => "steady",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SectionState {
    pub label: SectionLabel,
    /// 0..1; every stateful Signal address carries one (consumers may ignore it).
    pub confidence: f32,
}

pub trait SectionEstimator {
    /// Called once per hop, in order.
    fn process(&mut self, f: &AudioFeatures, ts: f64) -> SectionState;
    /// Reported over `/fosfora/v1/status/tier` — "heuristic-v1" now, "onnx-vN" later.
    fn tier(&self) -> &'static str;
}

// Thresholds are named consts, not config: the benchmark harness (Workstream C)
// tunes them against annotated sets; live knobs would make its numbers meaningless.
const BUILD_ENTER: f32 = 0.60;
const BUILD_EXIT: f32 = 0.40;
const BUILD_SUSTAIN_BARS: f64 = 2.0;
const BUILD_MIN_AGE_BARS: f64 = 2.0;
const FAILED_BUILD_MIN_AGE_BARS: f64 = 4.0;
const DROP_MIN_BARS: f64 = 4.0;
const DROP_MAX_BARS: f64 = 8.0;
const DROP_FADE_DELTA: f32 = 0.15;
const DROP_FADE_BARS: u32 = 2;
const INTRO_LOUD: f32 = 0.35;
const INTRO_SUSTAIN_BARS: f64 = 2.0;
const INTRO_MAX_BARS: f64 = 8.0;
const BREAK_COLLAPSE: f32 = 0.20;
const BREAK_BASS_CEILING: f32 = 0.15;
const BREAK_MIN_PREV_AGE_BARS: f64 = 4.0;
const BREAK_RECOVER_WITHIN: f32 = 0.10;
const BREAK_TIMEOUT_BARS: f64 = 16.0;
const SILENCE_LOUD: f32 = 0.03;
const SILENCE_BARS: f64 = 4.0;
/// EMA time constant for the loudness envelope the rules read.
const LOUD_TAU_SECS: f64 = 1.0;
/// Trailing window of per-bar loudness means used as the collapse reference.
const BAR_RING_LEN: usize = 8;

pub struct HeuristicSectionEstimator {
    clock: BarClock,
    prev_ts: Option<f64>,
    prev_bars: f64,
    label: SectionLabel,
    confidence: f32,
    entry_bar: f64,
    entry_loud: f32,

    /// EMA of `loudness_s`.
    loud: f32,

    // Per-bar loudness aggregation.
    cur_bar: i64,
    bar_sum: f64,
    bar_hops: u32,
    /// Trailing ring of finalized per-bar loudness means, newest last.
    bar_means: Vec<f32>,
    /// Consecutive finalized bars whose mean sat below `entry_loud - DROP_FADE_DELTA`.
    faded_bars: u32,
    /// Loudness reference from before a break, for recovery detection.
    pre_break_median: f32,

    // Sustained-condition accumulators, in bars.
    build_cond_bars: f64,
    /// A new build must first see `buildup < BUILD_EXIT` after the previous one —
    /// explicit hysteresis so a plateau hovering at the threshold cannot re-trigger.
    build_armed: bool,
    build_exit_bars: f64,
    intro_loud_bars: f64,
    silence_bars: f64,
}

impl HeuristicSectionEstimator {
    pub fn new() -> Self {
        Self {
            clock: BarClock::new(),
            prev_ts: None,
            prev_bars: 0.0,
            label: SectionLabel::Intro,
            confidence: 0.5,
            entry_bar: 0.0,
            entry_loud: 0.0,
            loud: 0.0,
            cur_bar: 0,
            bar_sum: 0.0,
            bar_hops: 0,
            bar_means: Vec::new(),
            faded_bars: 0,
            pre_break_median: 0.0,
            build_cond_bars: 0.0,
            build_armed: true,
            build_exit_bars: 0.0,
            intro_loud_bars: 0.0,
            silence_bars: 0.0,
        }
    }

    fn enter(&mut self, label: SectionLabel, confidence: f32, bars: f64) {
        self.label = label;
        self.confidence = confidence.clamp(0.0, 1.0);
        self.entry_bar = bars;
        self.entry_loud = self.loud;
        self.faded_bars = 0;
        self.build_cond_bars = 0.0;
        self.build_exit_bars = 0.0;
        self.intro_loud_bars = 0.0;
    }

    fn enter_build(&mut self, buildup: f32, bars: f64) {
        let conf = ((buildup - BUILD_ENTER) / 0.3).clamp(0.3, 1.0);
        self.enter(SectionLabel::Build, conf, bars);
        self.build_armed = false;
    }

    fn build_condition(&self) -> bool {
        self.build_cond_bars >= BUILD_SUSTAIN_BARS
    }

    /// Median of the trailing per-bar loudness means (the collapse reference).
    fn trailing_median(&self) -> Option<f32> {
        if self.bar_means.len() < 4 {
            return None;
        }
        let mut v = self.bar_means.clone();
        v.sort_by(f32::total_cmp);
        Some(v[v.len() / 2])
    }
}

impl SectionEstimator for HeuristicSectionEstimator {
    fn process(&mut self, f: &AudioFeatures, ts: f64) -> SectionState {
        let bars = self.clock.advance(f, ts);
        let dbars = (bars - self.prev_bars).max(0.0);
        self.prev_bars = bars;
        let dt = self
            .prev_ts
            .map(|p| (ts - p).clamp(0.0, 0.25))
            .unwrap_or(0.0);
        self.prev_ts = Some(ts);

        // Loudness envelope.
        let alpha = (1.0 - (-dt / LOUD_TAU_SECS).exp()) as f32;
        self.loud += (f.loudness_s - self.loud) * alpha;

        // Per-bar aggregation. The trailing median is taken *before* pushing the
        // just-finalized bar — the collapse test compares new against history.
        let bar_now = bars.floor() as i64;
        let mut finalized: Option<(f32, Option<f32>)> = None;
        if bar_now != self.cur_bar && self.bar_hops > 0 {
            let mean = (self.bar_sum / f64::from(self.bar_hops)) as f32;
            finalized = Some((mean, self.trailing_median()));
            self.bar_means.push(mean);
            if self.bar_means.len() > BAR_RING_LEN {
                self.bar_means.remove(0);
            }
            self.bar_sum = 0.0;
            self.bar_hops = 0;
        }
        if bar_now != self.cur_bar {
            self.cur_bar = bar_now;
        }
        self.bar_sum += f64::from(f.loudness_s);
        self.bar_hops += 1;

        // Sustained-condition accumulators.
        if f.buildup < BUILD_EXIT {
            self.build_armed = true;
        }
        if f.buildup >= BUILD_ENTER && self.build_armed {
            self.build_cond_bars += dbars;
        } else if f.buildup < BUILD_ENTER {
            self.build_cond_bars = 0.0;
        }
        if f.buildup < BUILD_EXIT {
            self.build_exit_bars += dbars;
        } else {
            self.build_exit_bars = 0.0;
        }
        if self.loud >= INTRO_LOUD {
            self.intro_loud_bars += dbars;
        } else {
            self.intro_loud_bars = 0.0;
        }
        if self.loud < SILENCE_LOUD {
            self.silence_bars += dbars;
        } else {
            self.silence_bars = 0.0;
        }

        if let Some((mean, _)) = finalized {
            if mean < self.entry_loud - DROP_FADE_DELTA {
                self.faded_bars += 1;
            } else {
                self.faded_bars = 0;
            }
        }

        let age = bars - self.entry_bar;
        let bass = 0.5 * (f.sub_bass + f.bass);

        // Transitions, in priority order. A drop pulse always wins, immediately —
        // it is the one event dwell must never delay.
        if f.drop > 0.5 && self.label != SectionLabel::Drop {
            self.enter(SectionLabel::Drop, f.buildup.max(0.6), bars);
        } else if self.silence_bars >= SILENCE_BARS && self.label != SectionLabel::Intro {
            // Track ended / changeover.
            self.enter(SectionLabel::Intro, 0.3, bars);
        } else {
            // Break entry is shared by Steady / Build / Drop, evaluated on bar close:
            // the freshly finalized bar collapsed hard vs the trailing median, the
            // low end is gone, and it is not silence.
            let break_entry = matches!(
                self.label,
                SectionLabel::Steady | SectionLabel::Build | SectionLabel::Drop
            ) && age >= BREAK_MIN_PREV_AGE_BARS
                && finalized.is_some_and(|(mean, median)| {
                    median.is_some_and(|med| {
                        med - mean >= BREAK_COLLAPSE
                            && bass < BREAK_BASS_CEILING
                            && self.loud > SILENCE_LOUD + 0.02
                    })
                });

            if break_entry {
                let (mean, median) = finalized.expect("checked above");
                let med = median.expect("checked above");
                self.pre_break_median = med;
                let conf = (0.5 + (med - mean - BREAK_COLLAPSE) / 0.2).clamp(0.4, 0.9);
                self.enter(SectionLabel::Break, conf, bars);
            } else {
                match self.label {
                    SectionLabel::Intro => {
                        if self.build_condition() {
                            self.enter_build(f.buildup, bars);
                        } else if self.intro_loud_bars >= INTRO_SUSTAIN_BARS {
                            let conf = (0.4 + (self.loud - INTRO_LOUD) / 0.3).clamp(0.4, 0.9);
                            self.enter(SectionLabel::Steady, conf, bars);
                        } else if age >= INTRO_MAX_BARS && f.downbeat > 0.5 {
                            self.enter(SectionLabel::Steady, 0.4, bars);
                        }
                    }
                    SectionLabel::Steady => {
                        if age >= BUILD_MIN_AGE_BARS && self.build_condition() {
                            self.enter_build(f.buildup, bars);
                        }
                    }
                    SectionLabel::Build => {
                        if age >= FAILED_BUILD_MIN_AGE_BARS
                            && self.build_exit_bars >= BUILD_SUSTAIN_BARS
                        {
                            // The riser deflated without a drop.
                            self.enter(SectionLabel::Steady, 0.5, bars);
                        }
                    }
                    SectionLabel::Drop => {
                        if age >= DROP_MAX_BARS
                            || (age >= DROP_MIN_BARS && self.faded_bars >= DROP_FADE_BARS)
                        {
                            self.enter(SectionLabel::Steady, 0.7, bars);
                        }
                    }
                    SectionLabel::Break => {
                        if self.build_condition() {
                            self.enter_build(f.buildup, bars);
                        } else if finalized.is_some_and(|(mean, _)| {
                            mean >= self.pre_break_median - BREAK_RECOVER_WITHIN
                        }) {
                            self.enter(SectionLabel::Steady, 0.5, bars);
                        } else if age >= BREAK_TIMEOUT_BARS {
                            self.enter(SectionLabel::Steady, 0.3, bars);
                        }
                    }
                    // Never entered by this estimator (see module docs).
                    SectionLabel::Outro => {}
                }
            }
        }

        SectionState {
            label: self.label,
            confidence: self.confidence,
        }
    }

    fn tier(&self) -> &'static str {
        "heuristic-v1"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytemuck::Zeroable;

    /// Drives the estimator through whole bars of synthetic features.
    struct Driver {
        est: HeuristicSectionEstimator,
        ts: f64,
        bar: u32,
        hops_per_bar: u32,
        hop_secs: f64,
        pub states: Vec<SectionState>,
    }

    impl Driver {
        fn new(hops_per_bar: u32) -> Self {
            Self {
                est: HeuristicSectionEstimator::new(),
                ts: 0.0,
                bar: 0,
                hops_per_bar,
                hop_secs: 2.0 / f64::from(hops_per_bar), // 2 s bars
                states: Vec::new(),
            }
        }

        /// Run `n` bars, calling `setup` per hop after the clock fields are set.
        fn bars(&mut self, n: u32, mut setup: impl FnMut(&mut AudioFeatures, u32)) {
            for _ in 0..n {
                for hop in 0..self.hops_per_bar {
                    let mut f = AudioFeatures::zeroed();
                    f.bar_index = self.bar as f32;
                    f.bar_phase = hop as f32 / self.hops_per_bar as f32;
                    f.downbeat = if hop == 0 { 1.0 } else { 0.0 };
                    setup(&mut f, hop);
                    let s = self.est.process(&f, self.ts);
                    self.states.push(s);
                    self.ts += self.hop_secs;
                }
                self.bar += 1;
            }
        }

        fn label(&self) -> SectionLabel {
            self.states.last().expect("ran at least one hop").label
        }
    }

    fn steady_setup(f: &mut AudioFeatures, _hop: u32) {
        f.loudness_s = 0.5;
        f.sub_bass = 0.5;
        f.bass = 0.5;
    }

    #[test]
    fn happy_path_intro_steady_build_drop_steady() {
        let mut d = Driver::new(24);
        d.bars(4, steady_setup);
        assert_eq!(d.label(), SectionLabel::Steady, "intro settles to steady");

        d.bars(3, |f, _| {
            steady_setup(f, 0);
            f.buildup = 0.8;
        });
        assert_eq!(
            d.label(),
            SectionLabel::Build,
            "sustained buildup enters build"
        );

        d.bars(1, |f, hop| {
            steady_setup(f, 0);
            f.buildup = 0.8;
            f.drop = if hop == 0 { 1.0 } else { 0.0 };
        });
        assert_eq!(
            d.label(),
            SectionLabel::Drop,
            "drop pulse transitions immediately"
        );

        d.bars(9, steady_setup);
        assert_eq!(d.label(), SectionLabel::Steady, "drop section ends");

        for s in &d.states {
            assert!((0.0..=1.0).contains(&s.confidence));
            assert_ne!(
                s.label,
                SectionLabel::Outro,
                "live heuristic never emits outro"
            );
        }
    }

    #[test]
    fn buildup_flapping_at_threshold_does_not_enter_build() {
        let mut d = Driver::new(24);
        d.bars(4, steady_setup);
        assert_eq!(d.label(), SectionLabel::Steady);

        // Oscillates around the threshold: dips below 0.60 every half bar reset
        // the sustain accumulator, so Build must not fire.
        d.bars(8, |f, hop| {
            steady_setup(f, 0);
            f.buildup = if hop < 12 { 0.65 } else { 0.55 };
        });
        assert_eq!(d.label(), SectionLabel::Steady);
    }

    #[test]
    fn failed_build_returns_to_steady_without_drop() {
        let mut d = Driver::new(24);
        d.bars(4, steady_setup);
        d.bars(3, |f, _| {
            steady_setup(f, 0);
            f.buildup = 0.8;
        });
        assert_eq!(d.label(), SectionLabel::Build);

        d.bars(5, |f, _| {
            steady_setup(f, 0);
            f.buildup = 0.2;
        });
        assert_eq!(d.label(), SectionLabel::Steady);
        assert!(
            d.states.iter().all(|s| s.label != SectionLabel::Drop),
            "no drop was ever reported"
        );
    }

    #[test]
    fn build_rearm_requires_dip_below_exit_threshold() {
        let mut d = Driver::new(24);
        d.bars(4, steady_setup);
        d.bars(3, |f, _| {
            steady_setup(f, 0);
            f.buildup = 0.8;
        });
        d.bars(1, |f, hop| {
            steady_setup(f, 0);
            f.buildup = 0.8;
            f.drop = if hop == 0 { 1.0 } else { 0.0 };
        });
        assert_eq!(d.label(), SectionLabel::Drop);

        // Buildup stays high (0.65, never below the 0.40 re-arm) right through the
        // drop section; when the drop ends, Build must NOT immediately re-trigger.
        d.bars(9, |f, _| {
            steady_setup(f, 0);
            f.buildup = 0.65;
        });
        assert_eq!(d.label(), SectionLabel::Steady);
    }

    #[test]
    fn loudness_collapse_without_drop_is_a_break_then_recovers() {
        let mut d = Driver::new(24);
        d.bars(8, |f, h| {
            steady_setup(f, h);
            f.loudness_s = 0.6;
        });
        assert_eq!(d.label(), SectionLabel::Steady);

        d.bars(3, |f, _| {
            f.loudness_s = 0.25;
            f.sub_bass = 0.05;
            f.bass = 0.05;
        });
        assert_eq!(d.label(), SectionLabel::Break);

        d.bars(2, |f, h| {
            steady_setup(f, h);
            f.loudness_s = 0.58;
        });
        assert_eq!(d.label(), SectionLabel::Steady);
    }

    #[test]
    fn sustained_silence_returns_to_intro() {
        let mut d = Driver::new(24);
        d.bars(4, steady_setup);
        assert_eq!(d.label(), SectionLabel::Steady);
        // Silence: no beat lock either (bar_index frozen would be more realistic,
        // but the fallback clock covers that case in clock.rs tests).
        d.bars(6, |f, _| {
            f.loudness_s = 0.0;
        });
        assert_eq!(d.label(), SectionLabel::Intro);
    }

    #[test]
    fn drop_pulse_bypasses_dwell() {
        let mut d = Driver::new(24);
        d.bars(3, steady_setup);
        // Half a bar into steady, a drop fires — no dwell gate may delay it.
        d.bars(1, |f, hop| {
            steady_setup(f, 0);
            f.drop = if hop == 12 { 1.0 } else { 0.0 };
        });
        assert_eq!(d.label(), SectionLabel::Drop);
    }

    /// Dwell is measured in bars, not hops: doubling the hop density of the same
    /// bar-timeline must produce the same per-bar label sequence.
    #[test]
    fn dwell_is_bar_denominated() {
        let run = |hops_per_bar: u32| {
            let mut d = Driver::new(hops_per_bar);
            d.bars(4, steady_setup);
            d.bars(3, |f, _| {
                steady_setup(f, 0);
                f.buildup = 0.8;
            });
            d.bars(5, |f, _| {
                steady_setup(f, 0);
                f.buildup = 0.2;
            });
            // Sample the label at each bar boundary.
            (0..12)
                .map(|bar| d.states[(bar * hops_per_bar + hops_per_bar - 1) as usize].label)
                .collect::<Vec<_>>()
        };
        assert_eq!(run(24), run(48));
    }

    #[test]
    fn identical_input_is_deterministic() {
        let run = || {
            let mut d = Driver::new(24);
            d.bars(4, steady_setup);
            d.bars(4, |f, _| {
                steady_setup(f, 0);
                f.buildup = 0.7;
            });
            d.states
                .iter()
                .map(|s| (s.label, s.confidence.to_bits()))
                .collect::<Vec<_>>()
        };
        assert_eq!(run(), run());
    }
}
