//! `/fosfora/v1/predict/drop` — the episode-gated drop predictor.
//!
//! The v1 formula (build level × phrase proximity × phrase confidence) measured
//! coverage 0.0 over 374 real tracks: phrase confidence is ~0 on real music and
//! the multiplicative product capped below its own 0.5 threshold. This machine
//! replaces it with an explicit commitment decision:
//!
//! - **Idle** (< [`IDLE_CAP`], always below the 0.35 re-arm line): a smoothed
//!   tension readout of `buildup` — telemetry, never a warning.
//! - **Committed** (≥ 0.5): entered when `buildup` has held [`COMMIT_LEVEL`]
//!   for [`COMMIT_BARS`]. Inside an episode the value is a saturating additive
//!   blend of persistence, sub-bass withdrawal, phrase-boundary proximity and
//!   imminence cues (kick gap / hard withdrawal), **ratcheted** so no decaying
//!   input can dent it mid-build — the failure that killed the old slope term
//!   (re-tested in the 2026-08 study: slope still did not earn in).
//! - Episodes end on a drop (collapse to [`DROP_COLLAPSE`], guaranteeing the
//!   bench metric's re-arm), a failed build, or a timeout.
//!
//! Every constant here was calibrated against 374 Harmonix tracks and frozen
//! before implementation (`bench/analyze_predict_drop.py`; record in
//! `bench/out/analysis/predict_drop/frozen_episode.json` and TASKS.md). They
//! are named consts, not config, so the bench numbers stay meaningful. The
//! sub-bass-withdrawal *gate* was ablated out (it barely discriminated); the
//! withdrawal signal remains as in-episode evidence.

use crate::audio::features::AudioFeatures;

use super::clock::BarClock;
use super::phrase::PhraseState;

/// `buildup` level whose sustained presence commits an episode.
const COMMIT_LEVEL: f32 = 0.40;
/// Bars of accumulated time-above-level needed to commit (dips decay 2×).
const COMMIT_BARS: f64 = 1.0;
/// Idle output ceiling — deliberately below `0.5 - 0.15` so a long loud
/// steady section can never block the bench metric's re-arm hysteresis.
const IDLE_CAP: f32 = 0.32;
const IDLE_LO: f32 = 0.30;
const IDLE_HI: f32 = 0.70;
/// A build that sags below this for a sustained bar has failed.
const EXIT_LEVEL: f32 = 0.35;
const EXIT_SUSTAIN_BARS: f64 = 1.0;
/// Safety: no build lasts this long — release rather than stick at 0.5+.
const MAX_EPISODE_BARS: f64 = 24.0;
/// Emitted the hop a drop fires: prediction resolved, start over.
const DROP_COLLAPSE: f32 = 0.15;

// In-episode evidence weights. They sum past 1.0 on purpose (saturating OR):
// any two strong families reach ~0.8; one alone stays in the low 0.6s.
const W_PERSIST: f32 = 0.35;
const W_WITHDRAWAL: f32 = 0.30;
const W_BOUNDARY: f32 = 0.20;
const W_IMMINENT: f32 = 0.35;
/// Bars of episode persistence that saturate the persistence term.
const PERSIST_SATURATION_BARS: f64 = 4.0;
/// Phrase confidence is ~0 on real music (measured); the boundary bonus keeps
/// this floor so the default grid still contributes shape instead of zeroing.
const BOUNDARY_CONF_FLOOR: f32 = 0.4;
const BOUNDARY_EXPONENT: f32 = 1.5;

// Sub-bass withdrawal: current bar's running mean vs the median of the last
// 8 finalized bars (the section estimator's ring pattern).
const WD_RING_LEN: usize = 8;
const WD_MIN_RING: usize = 4;
/// Withdrawal only counts while overall loudness holds up — a collapse of
/// both is a break, not a pre-drop cut.
const WD_LOUDNESS_FLOOR: f32 = 0.35;
/// Withdrawal this deep is itself an imminence cue.
const WD_IMMINENT: f32 = 0.5;

// Kick-gap imminence: the classic pre-drop silence. Mirrors the emitter's
// stem/drums/onset hysteresis so both fire on the same kicks.
const KICK_ENTER: f32 = 0.5;
const KICK_REARM: f32 = 0.3;
const KICK_MIN_ONSETS: u32 = 4;
const KICK_GAP_BARS: f64 = 1.25;
/// Off-grid fallback bar length (same as `clock.rs`).
const FALLBACK_SECS_PER_BAR: f64 = 2.0;

/// The committed tier occupies [0.5, 0.95]: floor + span × evidence.
const COMMITTED_FLOOR: f32 = 0.5;
const COMMITTED_SPAN: f32 = 0.45;

pub struct DropPredictor {
    clock: BarClock,
    prev_bars: f64,

    // Running aggregates for the bar in progress.
    cur_bass: f64,
    cur_loud: f64,
    cur_n: u32,
    // Ring of finalized per-bar means, oldest overwritten first.
    bass_ring: [f32; WD_RING_LEN],
    loud_ring: [f32; WD_RING_LEN],
    ring_len: usize,
    ring_pos: usize,

    // Commit gate + episode state.
    acc_bars: f64,
    committed: bool,
    episode_start_bars: f64,
    episode_peak: f32,
    below_exit_bars: f64,

    // Kick tracking.
    kick_armed: bool,
    last_kick_ts: f64,
    kicks_in_episode: u32,
}

impl DropPredictor {
    pub fn new() -> Self {
        Self {
            clock: BarClock::new(),
            prev_bars: 0.0,
            cur_bass: 0.0,
            cur_loud: 0.0,
            cur_n: 0,
            bass_ring: [0.0; WD_RING_LEN],
            loud_ring: [0.0; WD_RING_LEN],
            ring_len: 0,
            ring_pos: 0,
            acc_bars: 0.0,
            committed: false,
            episode_start_bars: 0.0,
            episode_peak: 0.0,
            below_exit_bars: 0.0,
            kick_armed: true,
            last_kick_ts: f64::NEG_INFINITY,
            kicks_in_episode: 0,
        }
    }

    /// Feed one analysis frame (after the phrase tracker); returns the value
    /// to emit at `/fosfora/v1/predict/drop`.
    pub fn process(&mut self, f: &AudioFeatures, ph: &PhraseState, ts: f64) -> f32 {
        let bars = self.clock.advance(f, ts);
        let dbars = (bars - self.prev_bars).max(0.0);

        // Finalize the previous bar's means when the clock crosses a bar line.
        if bars.floor() > self.prev_bars.floor() && self.cur_n > 0 {
            self.bass_ring[self.ring_pos] = (self.cur_bass / f64::from(self.cur_n)) as f32;
            self.loud_ring[self.ring_pos] = (self.cur_loud / f64::from(self.cur_n)) as f32;
            self.ring_pos = (self.ring_pos + 1) % WD_RING_LEN;
            self.ring_len = (self.ring_len + 1).min(WD_RING_LEN);
            self.cur_bass = 0.0;
            self.cur_loud = 0.0;
            self.cur_n = 0;
        }
        self.prev_bars = bars;
        self.cur_bass += f64::from(f.sub_bass);
        self.cur_loud += f64::from(f.loudness_s);
        self.cur_n += 1;

        // Kick hysteresis (mirrors the emitter's stem/drums/onset detector).
        if f.kick < KICK_REARM {
            self.kick_armed = true;
        } else if self.kick_armed && f.kick >= KICK_ENTER {
            self.kick_armed = false;
            self.last_kick_ts = ts;
            if self.committed {
                self.kicks_in_episode += 1;
            }
        }

        // A drop resolves everything, committed or not.
        if f.drop > 0.5 {
            self.committed = false;
            self.acc_bars = 0.0;
            self.episode_peak = 0.0;
            return DROP_COLLAPSE;
        }

        if !self.committed {
            if f.buildup >= COMMIT_LEVEL {
                self.acc_bars += dbars;
            } else {
                self.acc_bars = (self.acc_bars - 2.0 * dbars).max(0.0);
            }
            if self.acc_bars >= COMMIT_BARS {
                self.committed = true;
                self.episode_start_bars = bars;
                self.episode_peak = 0.0;
                self.below_exit_bars = 0.0;
                self.kicks_in_episode = 0;
            } else {
                return self.idle(f);
            }
        }

        let bars_in = bars - self.episode_start_bars;
        if f.buildup < EXIT_LEVEL {
            self.below_exit_bars += dbars;
        } else {
            self.below_exit_bars = 0.0;
        }
        if self.below_exit_bars >= EXIT_SUSTAIN_BARS || bars_in > MAX_EPISODE_BARS {
            self.committed = false;
            self.acc_bars = 0.0;
            self.episode_peak = 0.0;
            return self.idle(f);
        }

        let e_persist = ((bars_in / PERSIST_SATURATION_BARS).min(1.0)) as f32;
        let e_wd = self.withdrawal();
        let bars_left = ph.beats_left as f32 / 4.0;
        let prox = (1.0 - bars_left / ph.len as f32).clamp(0.0, 1.0);
        let e_boundary = prox.powf(BOUNDARY_EXPONENT)
            * (BOUNDARY_CONF_FLOOR + (1.0 - BOUNDARY_CONF_FLOOR) * ph.len_confidence);
        let bar_secs = f
            .beat_period_secs()
            .map_or(FALLBACK_SECS_PER_BAR, |b| f64::from(b) * 4.0);
        let kick_gap = self.kicks_in_episode >= KICK_MIN_ONSETS
            && ts - self.last_kick_ts >= KICK_GAP_BARS * bar_secs;
        let e_imminent = if kick_gap || e_wd >= WD_IMMINENT {
            1.0
        } else {
            0.0
        };

        let evidence = (W_PERSIST * e_persist
            + W_WITHDRAWAL * e_wd
            + W_BOUNDARY * e_boundary
            + W_IMMINENT * e_imminent)
            .min(1.0);
        let raw = COMMITTED_FLOOR + COMMITTED_SPAN * evidence;
        self.episode_peak = self.episode_peak.max(raw);
        self.episode_peak
    }

    fn idle(&self, f: &AudioFeatures) -> f32 {
        IDLE_CAP * ((f.buildup - IDLE_LO) / (IDLE_HI - IDLE_LO)).clamp(0.0, 1.0)
    }

    /// Sub-bass withdrawal of the bar in progress vs the trailing ring median.
    fn withdrawal(&self) -> f32 {
        if self.ring_len < WD_MIN_RING || self.cur_n == 0 {
            return 0.0;
        }
        let med_bass = ring_median(&self.bass_ring[..self.ring_len]);
        let med_loud = ring_median(&self.loud_ring[..self.ring_len]);
        let cur_bass = (self.cur_bass / f64::from(self.cur_n)) as f32;
        let cur_loud = (self.cur_loud / f64::from(self.cur_n)) as f32;
        if cur_loud < WD_LOUDNESS_FLOOR * med_loud {
            return 0.0;
        }
        ((med_bass - cur_bass) / med_bass.max(0.05)).clamp(0.0, 1.0)
    }
}

fn ring_median(v: &[f32]) -> f32 {
    let mut s: Vec<f32> = v.to_vec();
    s.sort_by(|a, b| a.total_cmp(b));
    let n = s.len();
    if n % 2 == 1 {
        s[n / 2]
    } else {
        0.5 * (s[n / 2 - 1] + s[n / 2])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytemuck::Zeroable;

    const HOPS_PER_BAR: u32 = 24;
    const HOP_SECS: f64 = 2.0 / HOPS_PER_BAR as f64;

    struct Driver {
        pr: DropPredictor,
        ts: f64,
        bar: u32,
        pub values: Vec<f32>,
    }

    impl Driver {
        fn new() -> Self {
            Self {
                pr: DropPredictor::new(),
                ts: 0.0,
                bar: 0,
                values: Vec::new(),
            }
        }

        fn phrase() -> PhraseState {
            // The no-evidence default the tracker reports on real music.
            PhraseState {
                bar_in_phrase: 1,
                len: 16,
                len_confidence: 0.0,
                beats_left: 64,
            }
        }

        fn bars(&mut self, n: u32, mut setup: impl FnMut(&mut AudioFeatures, u32, u32)) {
            for _ in 0..n {
                for hop in 0..HOPS_PER_BAR {
                    let mut f = AudioFeatures::zeroed();
                    f.bar_index = self.bar as f32;
                    f.bar_phase = hop as f32 / HOPS_PER_BAR as f32;
                    setup(&mut f, self.bar, hop);
                    let v = self.pr.process(&f, &Self::phrase(), self.ts);
                    self.values.push(v);
                    self.ts += HOP_SECS;
                }
                self.bar += 1;
            }
        }

        fn last(&self) -> f32 {
            *self.values.last().expect("ran at least one hop")
        }
    }

    /// The contract the old formula could never meet: a sustained build commits
    /// past 0.5 with ZERO phrase evidence, and deep withdrawal + persistence
    /// push it past 0.8.
    #[test]
    fn sustained_build_commits_and_reaches_both_tiers() {
        let mut d = Driver::new();
        // Establish a bass baseline (ring needs 4 finalized bars).
        d.bars(6, |f, _, _| {
            f.sub_bass = 0.6;
            f.loudness_s = 0.6;
        });
        // Build: high buildup, bass pulled out, loudness held.
        d.bars(2, |f, _, _| {
            f.buildup = 0.85;
            f.sub_bass = 0.05;
            f.loudness_s = 0.6;
        });
        assert!(d.last() >= 0.5, "committed tier: {}", d.last());
        d.bars(4, |f, _, _| {
            f.buildup = 0.85;
            f.sub_bass = 0.05;
            f.loudness_s = 0.6;
        });
        assert!(d.last() > 0.8, "imminent tier: {}", d.last());
    }

    /// Ratchet: once committed, a wobbling build must never dent the value —
    /// the exact failure that got the old slope term cut.
    #[test]
    fn ratchet_never_decreases_within_episode() {
        let mut d = Driver::new();
        d.bars(6, |f, _, _| {
            f.sub_bass = 0.6;
            f.loudness_s = 0.6;
        });
        d.bars(3, |f, _, _| {
            f.buildup = 0.85;
            f.sub_bass = 0.1;
            f.loudness_s = 0.6;
        });
        let committed_at = d.values.len();
        // Build sags (but stays above EXIT_LEVEL) and bass comes back.
        d.bars(3, |f, _, _| {
            f.buildup = 0.45;
            f.sub_bass = 0.6;
            f.loudness_s = 0.6;
        });
        let vals = &d.values[committed_at..];
        assert!(
            vals.windows(2).all(|w| w[1] >= w[0] - 1e-6),
            "ratchet violated: {vals:?}"
        );
        assert!(d.last() >= 0.5, "still committed: {}", d.last());
    }

    /// A failed build releases below the bench metric's re-arm line (0.35).
    #[test]
    fn failed_build_releases_below_rearm() {
        let mut d = Driver::new();
        d.bars(6, |f, _, _| {
            f.sub_bass = 0.6;
            f.loudness_s = 0.6;
        });
        d.bars(2, |f, _, _| {
            f.buildup = 0.85;
        });
        assert!(d.last() >= 0.5);
        d.bars(2, |f, _, _| {
            f.buildup = 0.1;
        });
        assert!(d.last() < 0.35, "released: {}", d.last());
    }

    /// A drop resolves the prediction immediately.
    #[test]
    fn drop_fire_collapses() {
        let mut d = Driver::new();
        d.bars(6, |f, _, _| {
            f.sub_bass = 0.6;
            f.loudness_s = 0.6;
        });
        d.bars(3, |f, _, _| {
            f.buildup = 0.85;
        });
        assert!(d.last() >= 0.5);
        d.bars(1, |f, _, hop| {
            f.buildup = 0.85;
            if hop == 0 {
                f.drop = 1.0;
            }
        });
        let bar_vals =
            &d.values[d.values.len() - HOPS_PER_BAR as usize..d.values.len() - HOPS_PER_BAR as usize + 1];
        assert!(
            (bar_vals[0] - DROP_COLLAPSE).abs() < 1e-6,
            "collapse on the drop hop: {bar_vals:?}"
        );
        assert!(d.last() < 0.35, "stays re-armable after: {}", d.last());
    }

    /// Quiet music never predicts.
    #[test]
    fn no_build_means_no_prediction() {
        let mut d = Driver::new();
        d.bars(16, |f, _, _| {
            f.sub_bass = 0.5;
            f.loudness_s = 0.4;
        });
        assert!(d.last() < 0.1, "quiet: {}", d.last());
    }

    /// A "build" that never resolves times out: the value must dip below the
    /// bench metric's re-arm line (0.35) before any re-commitment, so an
    /// endless loud section can't wedge the detector above threshold forever.
    #[test]
    fn endless_build_times_out_and_releases_through_rearm() {
        let mut d = Driver::new();
        d.bars(30, |f, _, _| {
            f.buildup = 0.9;
            f.sub_bass = 0.6;
            f.loudness_s = 0.6;
        });
        let tail = &d.values[24 * HOPS_PER_BAR as usize..];
        let dip = tail.iter().cloned().fold(f32::INFINITY, f32::min);
        assert!(dip < 0.35, "release dip through re-arm: {dip}");
        // Idle by construction sits below 0.5 - 0.15 even at max buildup.
        assert!(IDLE_CAP < 0.35);
    }
}
