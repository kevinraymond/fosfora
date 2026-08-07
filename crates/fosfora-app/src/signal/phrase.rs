//! Phrase-grid tracking and drop prediction (the session-addendum differentiator).
//!
//! Detection alone is post-hoc; the operator needs LEAD TIME. This tracker infers
//! the phrase length (8 / 16 / 32 bars, 4/4 assumption) by scoring how well salient
//! events — drops, section changes, buildup onsets, novelty peaks — align with each
//! candidate grid, then fuses build level, build slope and distance-to-boundary
//! into a continuous `/fosfora/v1/predict/drop` confidence that rises *before* the
//! phrase boundary a drop is likely to land on.
//!
//! Off-grid material degrades gracefully: the position keeps being emitted under
//! the best hypothesis, and the confidence value tells the consumer how much to
//! trust it. Signal informs; it never triggers anything.

use crate::audio::features::AudioFeatures;

use super::clock::BarClock;

const LENGTHS: [usize; 3] = [8, 16, 32];
/// Alignment evidence decays with musical distance so the grid can re-lock after
/// a tempo/arrangement change instead of being haunted by the old song.
const DECAY_TAU_BARS: f64 = 64.0;
/// Prefer the longest length whose best-anchor score is within this fraction of
/// the global best — an event every 16 bars also scores perfectly at 8.
const LENGTH_TIE_FRACTION: f64 = 0.8;
/// Aligned evidence needed before confidence saturates. The first event proves
/// nothing (any single event aligns with every grid), so it is discounted.
const EVIDENCE_FLOOR: f64 = 1.0;
const EVIDENCE_SATURATION: f64 = 3.0;

// Internal salience detectors (the emitter feeds drops / section changes in).
const BUILDUP_EVENT_ENTER: f32 = 0.6;
const BUILDUP_EVENT_REARM: f32 = 0.4;
const NOVELTY_EVENT_ENTER: f32 = 0.7;
const NOVELTY_EVENT_REARM: f32 = 0.5;

const W_DROP: f64 = 1.0;
const W_SECTION: f64 = 0.6;
const W_BUILD_ONSET: f64 = 0.4;
const W_NOVELTY_PEAK: f64 = 0.3;

/// 4/4 assumption, per the program addendum.
const BEATS_PER_BAR: f64 = 4.0;

#[derive(Debug, Clone, Copy)]
pub struct PhraseState {
    /// Bar within the phrase, 1-based ("bar 13 of 16").
    pub bar_in_phrase: u32,
    /// Inferred phrase length in bars: 8, 16 or 32.
    pub len: u32,
    /// Confidence in the inferred length/anchor, 0..1.
    pub len_confidence: f32,
    /// Whole beats until the next phrase boundary (4/4 assumption).
    pub beats_left: u32,
    /// Drop-likelihood, 0..1, designed to rise *ahead* of the boundary.
    pub predict_drop: f32,
}

pub struct PhraseTracker {
    clock: BarClock,
    prev_bars: f64,
    /// Per-length anchor scores: `scores[i][b]` is evidence that phrase boundaries
    /// fall on `bar ≡ b (mod LENGTHS[i])`.
    scores: [Vec<f64>; 3],

    buildup_armed: bool,
    novelty_armed: bool,

    /// EMA of `buildup`, τ = 0.5 s — fast enough to register a build within its
    /// first bar, so proximity × level alone gives a clean, monotonic rise (an
    /// explicit slope term was tried and cut: its decay dented the prediction
    /// mid-build, and the harness can reintroduce it with data if it earns back in).
    build_ema: f32,
    prev_ts: Option<f64>,
}

impl PhraseTracker {
    pub fn new() -> Self {
        Self {
            clock: BarClock::new(),
            prev_bars: 0.0,
            scores: [vec![0.0; 8], vec![0.0; 16], vec![0.0; 32]],
            buildup_armed: true,
            novelty_armed: true,
            build_ema: 0.0,
            prev_ts: None,
        }
    }

    /// Record a salient event at the current bar position (drops and section
    /// changes are fed in by the emitter; buildup/novelty events are internal).
    pub fn note_event(&mut self, weight: f64) {
        let bar = self.prev_bars.max(0.0).floor() as usize;
        for (i, len) in LENGTHS.iter().enumerate() {
            self.scores[i][bar % len] += weight;
        }
    }

    /// A confirmed drop: the strongest grid evidence, and the build collapses.
    pub fn on_drop(&mut self) {
        self.note_event(W_DROP);
        self.build_ema *= 0.3;
    }

    pub fn on_section_change(&mut self) {
        self.note_event(W_SECTION);
    }

    /// Best (length, anchor, confidence) under the tie rule.
    fn best(&self) -> (usize, usize, f32) {
        let mut per_len: Vec<(usize, usize, f64, f64)> = Vec::with_capacity(3);
        for (i, len) in LENGTHS.iter().enumerate() {
            let mut best_a = 0;
            let mut s1 = 0.0f64;
            let mut s2 = 0.0f64;
            for (a, &s) in self.scores[i].iter().enumerate() {
                if s > s1 {
                    s2 = s1;
                    s1 = s;
                    best_a = a;
                } else if s > s2 {
                    s2 = s;
                }
            }
            per_len.push((*len, best_a, s1, s2));
        }
        let global_best = per_len.iter().map(|t| t.2).fold(0.0, f64::max);
        if global_best < EVIDENCE_FLOOR {
            // Not enough evidence to beat the prior: hold the EDM default at zero
            // confidence rather than flapping the announced length on scraps.
            return (16, 0, 0.0);
        }
        // Longest length still carrying most of the best score wins the tie.
        let (len, anchor, s1, s2) = per_len
            .iter()
            .rev()
            .find(|t| t.2 >= global_best * LENGTH_TIE_FRACTION)
            .copied()
            .expect("global_best came from this list");
        let sharpness = ((s1 - s2) / (s1 + 1e-9)).clamp(0.0, 1.0);
        let evidence =
            ((s1 - EVIDENCE_FLOOR) / (EVIDENCE_SATURATION - EVIDENCE_FLOOR)).clamp(0.0, 1.0);
        (len, anchor, (sharpness * evidence) as f32)
    }

    pub fn process(&mut self, f: &AudioFeatures, ts: f64) -> PhraseState {
        let bars = self.clock.advance(f, ts);
        let dbars = (bars - self.prev_bars).max(0.0);
        self.prev_bars = bars;
        let dt = self
            .prev_ts
            .map(|p| (ts - p).clamp(0.0, 0.25))
            .unwrap_or(0.0);
        self.prev_ts = Some(ts);

        if dbars > 0.0 {
            let k = (-dbars / DECAY_TAU_BARS).exp();
            for lane in &mut self.scores {
                for s in lane.iter_mut() {
                    *s *= k;
                }
            }
        }

        // Internal salience: buildup onset and novelty peak, both with re-arm
        // hysteresis so a plateau is one event, not a stream of them.
        if f.buildup < BUILDUP_EVENT_REARM {
            self.buildup_armed = true;
        }
        if self.buildup_armed && f.buildup >= BUILDUP_EVENT_ENTER {
            self.buildup_armed = false;
            self.note_event(W_BUILD_ONSET);
        }
        if f.section_novelty < NOVELTY_EVENT_REARM {
            self.novelty_armed = true;
        }
        if self.novelty_armed && f.section_novelty >= NOVELTY_EVENT_ENTER {
            self.novelty_armed = false;
            self.note_event(W_NOVELTY_PEAK);
        }

        // Build envelope.
        let alpha = (1.0 - (-dt / 0.5_f64).exp()) as f32;
        self.build_ema += (f.buildup - self.build_ema) * alpha;

        // Position under the best hypothesis.
        let (len, anchor, len_confidence) = self.best();
        let pos = (bars - anchor as f64).rem_euclid(len as f64);
        let bars_left = (len as f64) - pos;
        let bar_in_phrase = (pos.floor() as u32) + 1;
        let beats_left = (bars_left * BEATS_PER_BAR).ceil().max(1.0) as u32;

        // predict/drop: how hard the music is building × how close the likely
        // landing point is. Phrase confidence scales how much the boundary term
        // is allowed to contribute.
        let build_score = ((self.build_ema - 0.3) / 0.6).clamp(0.0, 1.0);
        let proximity = (1.0 - (bars_left / len as f64) as f32).clamp(0.0, 1.0);
        let boundary = proximity.powf(1.5) * (0.5 + 0.5 * len_confidence);
        let predict_drop = (build_score * (0.35 + 0.65 * boundary)).clamp(0.0, 1.0);

        PhraseState {
            bar_in_phrase,
            len: len as u32,
            len_confidence,
            beats_left,
            predict_drop,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytemuck::Zeroable;

    const HOPS_PER_BAR: u32 = 24;
    const HOP_SECS: f64 = 2.0 / HOPS_PER_BAR as f64;

    struct Driver {
        tr: PhraseTracker,
        ts: f64,
        bar: u32,
        pub states: Vec<PhraseState>,
    }

    impl Driver {
        fn new() -> Self {
            Self {
                tr: PhraseTracker::new(),
                ts: 0.0,
                bar: 0,
                states: Vec::new(),
            }
        }

        fn bars(&mut self, n: u32, mut setup: impl FnMut(&mut AudioFeatures, u32, u32)) {
            for _ in 0..n {
                for hop in 0..HOPS_PER_BAR {
                    let mut f = AudioFeatures::zeroed();
                    f.bar_index = self.bar as f32;
                    f.bar_phase = hop as f32 / HOPS_PER_BAR as f32;
                    setup(&mut f, self.bar, hop);
                    let s = self.tr.process(&f, self.ts);
                    self.states.push(s);
                    self.ts += HOP_SECS;
                }
                self.bar += 1;
            }
        }

        fn last(&self) -> PhraseState {
            *self.states.last().expect("ran at least one hop")
        }
    }

    /// Drops landing every 16 bars lock a 16-bar grid with usable confidence,
    /// and the reported position cycles with it.
    #[test]
    fn sixteen_bar_drops_lock_a_sixteen_bar_grid() {
        // Drops every 16 bars, fed through the same API the emitter uses.
        let mut d = Driver::new();
        for _ in 0..3 {
            d.bars(16, |_, _, _| {});
            d.tr.on_drop();
        }
        d.bars(4, |_, _, _| {});
        let s = d.last();
        assert_eq!(s.len, 16, "locked length");
        assert!(s.len_confidence > 0.3, "confidence {}", s.len_confidence);
        // 4 bars past a boundary at bar 48: position is bar 5 of 16.
        assert_eq!(s.bar_in_phrase, 5);
        assert_eq!(s.beats_left, 48, "12 bars to go × 4 beats");
    }

    /// Arrhythmic events must not fake a confident grid.
    #[test]
    fn off_grid_events_stay_low_confidence() {
        let mut d = Driver::new();
        for gap in [7u32, 6, 9, 5, 8] {
            d.bars(gap, |_, _, _| {});
            d.tr.note_event(1.0);
        }
        d.bars(2, |_, _, _| {});
        let s = d.last();
        assert!(
            s.len_confidence < 0.3,
            "off-grid confidence {} should stay low",
            s.len_confidence
        );
    }

    /// The product claim: during a build, prediction RISES as the phrase boundary
    /// approaches — that rise is the operator's lead time.
    #[test]
    fn predict_drop_rises_toward_the_boundary_during_a_build() {
        let mut d = Driver::new();
        // Lock a 16-bar grid first. The drops land on the tracker's bar clock at
        // 15, 31, 47, so phrase boundaries sit at clock ≡ 15 (mod 16) — next at 63.
        for _ in 0..3 {
            d.bars(16, |_, _, _| {});
            d.tr.on_drop();
        }
        // Quiet until clock 58, then a 4-bar build approaching the boundary at 63
        // (positions 12→15 of the phrase, no wrap inside the sampled window).
        d.bars(11, |_, _, _| {});
        d.states.clear();
        d.bars(4, |f, _, _| {
            f.buildup = 0.85;
        });
        let per_bar: Vec<f32> = (0..4)
            .map(|b| d.states[(b * HOPS_PER_BAR + HOPS_PER_BAR - 1) as usize].predict_drop)
            .collect();
        assert!(
            per_bar.windows(2).all(|w| w[1] >= w[0] - 1e-6),
            "prediction should be non-decreasing toward the boundary: {per_bar:?}"
        );
        assert!(
            per_bar[3] > per_bar[0] + 0.1,
            "prediction should rise materially: {per_bar:?}"
        );
        assert!(per_bar[3] > 0.5, "high build near boundary: {per_bar:?}");
    }

    /// No build, no prediction — proximity alone must not cry wolf.
    #[test]
    fn no_build_means_no_prediction() {
        let mut d = Driver::new();
        for _ in 0..3 {
            d.bars(16, |_, _, _| {});
            d.tr.on_drop();
        }
        d.bars(15, |_, _, _| {});
        assert!(
            d.last().predict_drop < 0.1,
            "quiet music near a boundary: {}",
            d.last().predict_drop
        );
    }

    #[test]
    fn no_evidence_defaults_to_sixteen_with_zero_confidence() {
        let mut d = Driver::new();
        d.bars(3, |_, _, _| {});
        let s = d.last();
        assert_eq!(s.len, 16);
        assert!(s.len_confidence < 0.05);
        // The bar clock counts bar *starts*: after driving three bars it reads 2.x,
        // i.e. we are inside the third bar of the default 16-bar phrase.
        assert_eq!(s.bar_in_phrase, 3);
    }
}
