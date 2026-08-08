//! Downbeat / bar-phase / meter tracking (A12 #1463).
//!
//! `BeatScheduler` emits per-beat pulses and a per-beat sawtooth but has no concept of a
//! bar. This stage sits after beat detection: at each scheduled beat it snapshots a small
//! "beat-level vector" — per-band spectral flux integrated since the previous beat, the
//! loudness rise, and the chroma-change magnitude (chord changes correlate with downbeats)
//! — into a ring of the last 16 beats. It then scores each candidate meter M ∈ {3, 4} and
//! bar phase p by how much more "downbeat-like" the beats at phase p are than the rest
//! (accent contrast), locks the winner with hysteresis, and falls back to 4/4 on the
//! strongest recent beat when confidence is low.
//!
//! Outputs fill the reserved shader fields (#1505, CPU-side only, no ABI churn):
//! - `downbeat`    — 1.0 on the bar's "one" (a trigger, mirrors `beat`)
//! - `bar_phase`   — 0→1 sawtooth over the bar, advanced on the audio clock between beats
//!   (mirrors how `BeatScheduler::update_phase` advances `beat_phase`). The bar length it
//!   runs on is published alongside it (A8b, #1554) so the render thread can advance the
//!   sawtooth at render rate on the same denominator — see [`DownbeatTracker::bar_clock`].
//! - `beat_in_bar` — beat index within the bar, normalized 0..1
//!
//! DSP downbeat tracking is ~70-80% accurate on 4/4 electronic music and worse elsewhere;
//! the outputs are deliberately the same three fields a future neural tracker (A20 #1471)
//! can drive, so it drops in behind this without touching the ABI.

use std::collections::VecDeque;

use super::beat::BeatResult;

/// Beats retained for meter/phase scoring (four bars of 4/4).
const RING_LEN: usize = 16;
/// Minimum beats in the ring before we trust a meter/phase estimate.
const MIN_BEATS_FOR_LOCK: usize = 8;
/// A challenger (meter, phase) must stay the top candidate for this many consecutive
/// beats before the lock switches — prevents flicker between metrical interpretations.
const HYSTERESIS_BEATS: u32 = 8;
/// Accent-contrast score below which meter is considered ambiguous → default to 4/4.
const CONFIDENCE_THRESHOLD: f32 = 0.08;
/// Weights combining a beat's normalized accent cues into one "downbeat-ness" scalar.
/// Low-band flux (kick on the one) dominates; chroma change (chord change) and loudness
/// rise refine it.
const W_FLUX_LOW: f32 = 0.60;
const W_CHROMA: f32 = 0.25;
const W_LOUD: f32 = 0.15;

/// One beat's accent cues (raw; normalized against the ring at scoring time).
#[derive(Clone, Copy)]
struct BeatEntry {
    /// Monotonic beat index, for `index % meter == phase` grouping.
    global_idx: u64,
    /// Low-band (20-150 Hz) flux integrated since the previous beat.
    flux_low: f32,
    /// L2 chroma distance from the previous beat's chroma (chord-change magnitude).
    chroma_change: f32,
    /// Positive loudness change (rms) since the previous beat.
    loud_rise: f32,
}

/// Per-frame downbeat outputs, copied onto `AudioFeatures` (mirrors `BeatResult`).
pub struct DownbeatResult {
    /// 1.0 on the frame where the bar's "one" is detected, else 0.0 (a trigger).
    pub downbeat: f32,
    /// 0→1 sawtooth over the current bar.
    pub bar_phase: f32,
    /// Beat index within the bar, normalized 0..1.
    pub beat_in_bar: f32,
    /// Seconds per bar — the denominator `bar_phase` is running on, or 0.0 when there is no
    /// bar clock yet. Published (A8b, #1554) so the render thread can advance its own copy
    /// of `bar_phase` at this exact rate rather than re-deriving one from `bpm` and `meter`;
    /// see `DownbeatTracker::bar_clock` and `interp::FeatureInterpolator::advance_bar_phase`.
    pub bar_duration: f64,
    /// 0-based index of the current bar — the count of downbeats fired minus one, held
    /// between downbeats. Steps by exactly 1 the frame `bar_phase` resets, so
    /// `bar_index + bar_phase` is a continuous monotonic bar clock. After a meter/phase
    /// re-lock this is deliberately NOT `beat_index / meter` — it counts "ones" actually
    /// fired, which is what phase-locked visuals want. Exact in f32 to 2^24 bars.
    pub bar_index: f32,
    /// 0-based index of the current beat interval (count of fired beats minus one), held
    /// between beats. Pairs with `beat_phase` the same way `bar_index` pairs with
    /// `bar_phase`.
    pub beat_index: f32,
}

pub struct DownbeatTracker {
    ring: VecDeque<BeatEntry>,
    /// Band flux accumulated across frames since the previous beat.
    flux_accum: [f32; 3],
    /// rms and chroma captured at the previous beat, for delta cues.
    prev_rms: f32,
    prev_chroma: [f32; 12],
    have_prev: bool,
    /// Monotonic beat counter (advances once per fired beat).
    beat_idx: u64,
    /// Monotonic downbeat counter (advances once per fired "one"). See
    /// `DownbeatResult::bar_index` for the emitted semantics.
    bar_idx: u64,
    /// Locked meter (3 or 4) and which beat of the bar is the "one".
    meter: usize,
    phase: usize,
    /// Hysteresis: the (meter, phase) currently accruing challenge time and its count.
    candidate: Option<(usize, usize)>,
    candidate_count: u32,
    /// Wall-clock (audio-thread timestamp) of the last detected downbeat, and the bar
    /// duration estimate, for advancing `bar_phase` continuously between beats. Both are
    /// written only on a detected downbeat, in the same branch — so a refreshed
    /// `bar_duration` always coincides with `bar_phase == 0`, which is what lets the render
    /// thread share the rate without ever seeing it change mid-bar (A8b, #1554).
    last_downbeat_time: f64,
    bar_duration: f64,
    /// Held between beats (discrete per-beat value emitted every frame).
    cur_beat_in_bar: f32,
}

impl DownbeatTracker {
    pub fn new() -> Self {
        Self {
            ring: VecDeque::with_capacity(RING_LEN),
            flux_accum: [0.0; 3],
            prev_rms: 0.0,
            prev_chroma: [0.0; 12],
            have_prev: false,
            beat_idx: 0,
            bar_idx: 0,
            meter: 4,
            phase: 0,
            candidate: None,
            candidate_count: 0,
            last_downbeat_time: 0.0,
            bar_duration: 0.0,
            cur_beat_in_bar: 0.0,
        }
    }

    /// Called every audio frame. `band_flux` is `Analyzer::band_flux_3()` (low/mid/high),
    /// `rms` the current amplitude, `chroma` the pre-normalization pitch-class vector, and
    /// `timestamp` the audio-thread clock (seconds). Heavy scoring runs only on a fired beat.
    ///
    /// `loud_silent` is the A10 perceptual silence gate, mirroring `BeatDetector::process`
    /// (#1598). Without it this tracker had no silence gate at all: beats stop firing through
    /// a silence, so `last_downbeat_time`/`bar_duration` freeze while `bar_clock` — a pure
    /// function of the audio clock — keeps ramping. The result was an incoherent pairing where
    /// `beat_phase` sat pinned at 0 while `bar_phase` swept the full 0→1 (verified live: over
    /// 361 silent samples `beat_phase` held one value, `bar_phase` swept at cv 0.035).
    pub fn process(
        &mut self,
        beat: &BeatResult,
        band_flux: [f32; 3],
        rms: f32,
        chroma: &[f32; 12],
        timestamp: f64,
        loud_silent: bool,
    ) -> DownbeatResult {
        // Integrate flux across every frame in the current beat interval.
        for i in 0..3 {
            self.flux_accum[i] += band_flux[i];
        }

        let mut downbeat = 0.0;

        if beat.beat > 0.5 {
            downbeat = self.on_beat(beat, rms, chroma, timestamp);
        }

        // Pin the phase through silence but keep publishing the *rate* (#1598). Returning the
        // `(0.0, 0.0)` sentinel here instead would need no render-side change at all — but it
        // would overload a sentinel that means "no downbeat ever locked", and on the way *out*
        // of a silence `bar_duration` would jump 0 → live while `bar_phase` resumed mid-bar,
        // breaking the invariant on `last_downbeat_time` above that A8b's shared rate rests on.
        let (bar_phase, bar_duration) = if loud_silent {
            (0.0, self.bar_duration)
        } else {
            self.bar_clock(timestamp)
        };

        DownbeatResult {
            downbeat,
            bar_phase,
            beat_in_bar: self.cur_beat_in_bar,
            bar_duration,
            // saturating_sub: "no bar/beat yet" and "inside the first one" both read 0,
            // matching the phases (both sit at 0 before their clock starts).
            bar_index: self.bar_idx.saturating_sub(1) as f32,
            beat_index: self.beat_idx.saturating_sub(1) as f32,
        }
    }

    /// The bar clock at `timestamp`: the 0→1 sawtooth (0 at the last downbeat, wrapping every
    /// bar on the audio clock) and the bar length it is running on.
    ///
    /// Returned as a pair, from one gate, because the render thread advances its own copy of
    /// the phase at `1/bar_duration` (A8b, #1554) — publishing them separately would let the
    /// two threads disagree about whether the clock is live at all. A `bar_duration` of 0 is
    /// the "no bar clock yet" sentinel, so `bar_duration > 0.0` ⟺ `bar_phase` is a live ramp
    /// holds by construction rather than by the reader re-deriving this gate.
    fn bar_clock(&self, timestamp: f64) -> (f32, f64) {
        if self.last_downbeat_time <= 0.0 || self.bar_duration <= 0.0 {
            return (0.0, 0.0);
        }
        (
            (((timestamp - self.last_downbeat_time) / self.bar_duration).rem_euclid(1.0)) as f32,
            self.bar_duration,
        )
    }

    /// Snapshot the beat vector, re-score meter/phase, and decide if this beat is the "one".
    fn on_beat(&mut self, beat: &BeatResult, rms: f32, chroma: &[f32; 12], timestamp: f64) -> f32 {
        // Q1: index on the scheduler's GRID (which counts muted instants too),
        // not on fired-beat count — a gap that skips fires must not slip the
        // bar rotation's modulo. Fall back to the local counter pre-grid.
        let idx = if beat.beat_index > 0 {
            beat.beat_index - 1
        } else {
            self.beat_idx
        };

        // Beat-level cues (0 on the very first beat, before we have a reference).
        let (chroma_change, loud_rise) = if self.have_prev {
            (
                chroma_distance(chroma, &self.prev_chroma),
                (rms - self.prev_rms).max(0.0),
            )
        } else {
            (0.0, 0.0)
        };

        // Q1: ALL cues attribute one beat BACK. The PLL fires a beat AT its
        // instant, so the beat's own attack — its flux, its loudness step, its
        // chord's (attack-delayed) chroma move — lands in the interval that
        // ends at the NEXT fire. The old scheduler fired 80-155 ms late, which
        // put a beat's attack before its own fire and made forward attribution
        // accidentally correct; on time, forward attribution rotated every
        // fixture downbeat exactly one beat late.
        if let Some(last) = self.ring.back_mut() {
            last.flux_low = self.flux_accum[0];
            last.chroma_change = chroma_change;
            last.loud_rise = loud_rise;
        }

        // Reset per-beat accumulators / references for the next interval.
        self.flux_accum = [0.0; 3];
        self.prev_rms = rms;
        self.prev_chroma = *chroma;
        self.have_prev = true;

        // Score on complete entries, then open this beat's (still-empty) one.
        self.update_meter_phase();
        if self.ring.len() == RING_LEN {
            self.ring.pop_front();
        }
        self.ring.push_back(BeatEntry {
            global_idx: idx,
            flux_low: 0.0,
            chroma_change: 0.0,
            loud_rise: 0.0,
        });

        // This beat's position within the bar and whether it is the downbeat.
        let pos = (idx as i64 - self.phase as i64).rem_euclid(self.meter as i64) as usize;
        self.cur_beat_in_bar = pos as f32 / self.meter as f32;
        let is_downbeat = pos == 0;

        if is_downbeat {
            self.bar_idx += 1;
            self.last_downbeat_time = timestamp;
            if beat.bpm > 1.0 {
                self.bar_duration = (60.0 / beat.bpm as f64) * self.meter as f64;
            }
        }

        self.beat_idx += 1;
        if is_downbeat { 1.0 } else { 0.0 }
    }

    /// Score every (meter, phase) by accent contrast and update the lock with hysteresis.
    fn update_meter_phase(&mut self) {
        if self.ring.len() < MIN_BEATS_FOR_LOCK {
            return;
        }

        // Normalize each cue against the ring so the weights are meaningful, then reduce to
        // one "downbeat-ness" scalar per beat.
        let max_flux = self.ring.iter().map(|e| e.flux_low).fold(1e-6, f32::max);
        let max_chroma = self
            .ring
            .iter()
            .map(|e| e.chroma_change)
            .fold(1e-6, f32::max);
        let max_loud = self.ring.iter().map(|e| e.loud_rise).fold(1e-6, f32::max);
        let scored: Vec<(u64, f32)> = self
            .ring
            .iter()
            .map(|e| {
                let s = W_FLUX_LOW * (e.flux_low / max_flux)
                    + W_CHROMA * (e.chroma_change / max_chroma)
                    + W_LOUD * (e.loud_rise / max_loud);
                (e.global_idx, s)
            })
            .collect();

        // Best (meter, phase) by contrast = mean score at phase p minus mean score off it.
        let mut best = (self.meter, self.phase, f32::MIN);
        for &m in &[3usize, 4usize] {
            for p in 0..m {
                if let Some(c) = contrast(&scored, m, p) {
                    if c > best.2 {
                        best = (m, p, c);
                    }
                }
            }
        }
        let (best_m, best_p, best_score) = best;

        if best_score < CONFIDENCE_THRESHOLD {
            // Meter ambiguous: default to 4/4 aligned to the strongest recent beat so the
            // downbeat still lands somewhere musical.
            self.meter = 4;
            self.phase = strongest_phase(&scored, 4);
            self.candidate = None;
            self.candidate_count = 0;
            return;
        }

        if (best_m, best_p) == (self.meter, self.phase) {
            self.candidate = None;
            self.candidate_count = 0;
        } else if self.candidate == Some((best_m, best_p)) {
            self.candidate_count += 1;
            if self.candidate_count >= HYSTERESIS_BEATS {
                self.meter = best_m;
                self.phase = best_p;
                self.candidate = None;
                self.candidate_count = 0;
            }
        } else {
            self.candidate = Some((best_m, best_p));
            self.candidate_count = 1;
        }
    }

    #[cfg(test)]
    fn meter(&self) -> usize {
        self.meter
    }
}

impl Default for DownbeatTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// L2 distance between two chroma vectors (chord-change magnitude).
fn chroma_distance(a: &[f32; 12], b: &[f32; 12]) -> f32 {
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| (x - y) * (x - y))
        .sum::<f32>()
        .sqrt()
}

/// Mean downbeat-ness of beats at `index % m == p` minus the mean of the rest. Higher means
/// phase `p` is more consistently accented — a stronger candidate for the bar's "one".
/// Returns None if either group is empty (can't compare).
fn contrast(scored: &[(u64, f32)], m: usize, p: usize) -> Option<f32> {
    let (mut in_sum, mut in_n, mut out_sum, mut out_n) = (0.0f32, 0u32, 0.0f32, 0u32);
    for &(idx, s) in scored {
        if idx as usize % m == p {
            in_sum += s;
            in_n += 1;
        } else {
            out_sum += s;
            out_n += 1;
        }
    }
    if in_n == 0 || out_n == 0 {
        return None;
    }
    Some(in_sum / in_n as f32 - out_sum / out_n as f32)
}

/// Phase (0..m) with the highest mean downbeat-ness — the strongest recent beat.
fn strongest_phase(scored: &[(u64, f32)], m: usize) -> usize {
    let mut best = (0usize, f32::MIN);
    for p in 0..m {
        let (mut sum, mut n) = (0.0f32, 0u32);
        for &(idx, s) in scored {
            if idx as usize % m == p {
                sum += s;
                n += 1;
            }
        }
        if n > 0 {
            let mean = sum / n as f32;
            if mean > best.1 {
                best = (p, mean);
            }
        }
    }
    best.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn beat_result(bpm: f32) -> BeatResult {
        BeatResult {
            onset_strength: 0.0,
            beat: 1.0,
            beat_phase: 0.0,
            bpm,
            beat_strength: 1.0,
            beat_time: 0.0,
            beat_index: 0,
        }
    }

    /// What `BeatDetector` emits through a silence: no beat fires, but the tempo estimate
    /// holds (`bpm` is a `Hold` feature, not a level). This is the input shape that made
    /// `bar_phase` free-run before #1598 — nothing fires, so nothing refreshes the bar
    /// anchor, yet the audio clock keeps ticking underneath `bar_clock`.
    fn silent_beat(bpm: f32) -> BeatResult {
        BeatResult {
            onset_strength: 0.0,
            beat: 0.0,
            beat_phase: 0.0,
            bpm,
            beat_strength: 0.0,
            beat_time: 0.0,
            beat_index: 0,
        }
    }

    /// Drive `count` beats at `meter`, accenting the "one" (idx % meter == 0) with strong
    /// low-band flux + a chord change + a loudness rise. Returns the downbeat pattern
    /// (1.0/0.0) for each beat in order.
    fn run_meter(tracker: &mut DownbeatTracker, meter: usize, count: usize, bpm: f32) -> Vec<f32> {
        let period = 60.0 / bpm as f64;
        let mut out = Vec::new();
        for i in 0..count {
            // The interval arriving at fire i carries the PREVIOUS beat's
            // content — the PLL fires at the beat instant, before its attack.
            let cue_beat = i.wrapping_sub(1);
            let accent = i > 0 && cue_beat.is_multiple_of(meter);
            let flux = if accent {
                [1.0, 0.2, 0.1]
            } else {
                [0.15, 0.3, 0.2]
            };
            let rms = if accent { 0.9 } else { 0.5 };
            // Chord changes land on the downbeat.
            let chroma = if (cue_beat / meter).is_multiple_of(2) {
                [1.0, 0.0, 0.5, 0.0, 0.7, 0.0, 0.0, 0.6, 0.0, 0.0, 0.0, 0.0]
            } else {
                [0.0, 0.6, 0.0, 0.8, 0.0, 0.5, 0.0, 0.0, 1.0, 0.0, 0.4, 0.0]
            };
            let chroma = if accent {
                chroma
            } else {
                // Non-downbeats reuse the current chord (small change).
                [0.5, 0.3, 0.3, 0.4, 0.35, 0.25, 0.1, 0.3, 0.5, 0.1, 0.2, 0.1]
            };
            let t = i as f64 * period;
            let r = tracker.process(&beat_result(bpm), flux, rms, &chroma, t, false);
            out.push(r.downbeat);
        }
        out
    }

    #[test]
    fn locks_4_4_and_fires_downbeat_on_the_one() {
        let mut t = DownbeatTracker::new();
        run_meter(&mut t, 4, 32, 120.0);
        assert_eq!(t.meter(), 4, "should lock 4/4");
        // After locking, downbeats fire on phase == the accented beat, spaced 4 apart.
        let tail = run_meter(&mut t, 4, 16, 120.0);
        let fired: Vec<usize> = tail
            .iter()
            .enumerate()
            .filter(|(_, d)| **d > 0.5)
            .map(|(i, _)| i)
            .collect();
        assert!(!fired.is_empty(), "downbeats should fire");
        for w in fired.windows(2) {
            assert_eq!(w[1] - w[0], 4, "downbeats spaced one bar (4 beats) apart");
        }
    }

    /// v4 overlay clock: `beat_index` steps by exactly 1 per fired beat, `bar_index`
    /// steps by exactly 1 on the fired "one" (spaced `meter` beats apart) and holds
    /// everywhere else — including on the between-beat frames where no beat fires.
    #[test]
    fn counters_step_on_fire_and_hold_between() {
        let mut t = DownbeatTracker::new();
        run_meter(&mut t, 4, 32, 120.0);
        assert_eq!(t.meter(), 4, "should lock 4/4");

        let period = 60.0 / 120.0f64;
        let t0 = 32.0 * period;
        let chroma = [0.5; 12];
        // Call 32's interval carries beat 31 (≡ 3 mod 4): not accented.
        let mut prev = t.process(
            &beat_result(120.0),
            [0.15, 0.3, 0.2],
            0.5,
            &chroma,
            t0,
            false,
        );
        let mut bar_steps = Vec::new();
        for i in 1..16 {
            let ts = t0 + i as f64 * period;
            // A between-beat frame first: both counters must hold.
            let held = t.process(
                &silent_beat(120.0),
                [0.1, 0.1, 0.1],
                0.5,
                &chroma,
                ts - period / 2.0,
                false,
            );
            assert_eq!(
                held.beat_index, prev.beat_index,
                "beat_index held between beats"
            );
            assert_eq!(
                held.bar_index, prev.bar_index,
                "bar_index held between beats"
            );

            // Interval arriving at call i carries beat (32 + i − 1): accents
            // (beats ≡ 0 mod 4) therefore land in calls with i % 4 == 1.
            let accent = i % 4 == 1;
            let flux = if accent {
                [1.0, 0.2, 0.1]
            } else {
                [0.15, 0.3, 0.2]
            };
            let r = t.process(
                &beat_result(120.0),
                flux,
                if accent { 0.9 } else { 0.5 },
                &chroma,
                ts,
                false,
            );
            assert_eq!(
                r.beat_index,
                prev.beat_index + 1.0,
                "beat_index steps by exactly 1 per fired beat"
            );
            let bar_step = r.bar_index - prev.bar_index;
            assert!(
                bar_step == 0.0 || bar_step == 1.0,
                "bar_index steps by 0 or 1, got {bar_step}"
            );
            assert_eq!(
                bar_step == 1.0,
                r.downbeat > 0.5,
                "bar_index steps exactly when the downbeat fires"
            );
            if bar_step == 1.0 {
                bar_steps.push(i);
            }
            prev = r;
        }
        assert!(
            bar_steps.len() >= 3,
            "expected several bars, got {bar_steps:?}"
        );
        for w in bar_steps.windows(2) {
            assert_eq!(
                w[1] - w[0],
                4,
                "bar_index steps spaced one bar (4 beats) apart"
            );
        }
    }

    #[test]
    fn locks_3_4_waltz() {
        let mut t = DownbeatTracker::new();
        run_meter(&mut t, 3, 33, 150.0);
        assert_eq!(t.meter(), 3, "should lock 3/4");
        let tail = run_meter(&mut t, 3, 12, 150.0);
        let fired: Vec<usize> = tail
            .iter()
            .enumerate()
            .filter(|(_, d)| **d > 0.5)
            .map(|(i, _)| i)
            .collect();
        for w in fired.windows(2) {
            assert_eq!(w[1] - w[0], 3, "downbeats spaced one bar (3 beats) apart");
        }
    }

    #[test]
    fn ambiguous_input_defaults_to_4_4_without_panic() {
        let mut t = DownbeatTracker::new();
        // Flat, featureless beats: no accent structure to lock onto.
        for i in 0..40 {
            let chroma = [0.4; 12];
            let r = t.process(
                &beat_result(128.0),
                [0.3, 0.3, 0.3],
                0.5,
                &chroma,
                i as f64 * 0.47,
                false,
            );
            assert!(r.bar_phase >= 0.0 && r.bar_phase < 1.0);
            assert!(r.beat_in_bar >= 0.0 && r.beat_in_bar < 1.0);
            assert!(
                r.bar_duration >= 0.0,
                "bar_duration is a length or the 0 sentinel"
            );
        }
        assert_eq!(t.meter(), 4, "ambiguous input falls back to 4/4");
    }

    #[test]
    fn bar_phase_sawtooths_and_resets_on_downbeat() {
        let mut t = DownbeatTracker::new();
        // Establish a lock and a bar clock.
        run_meter(&mut t, 4, 32, 120.0);
        let period = 60.0 / 120.0;
        // Sample bar_phase across one bar between beats: should rise monotonically toward 1.
        let base = 33.0 * period; // start after the driven beats
        // Fire a downbeat to seed last_downbeat_time cleanly.
        let seed = t.process(
            &beat_result(120.0),
            [1.0, 0.2, 0.1],
            0.9,
            &[0.4; 12],
            base,
            false,
        );
        // The bar clock the render thread will advance on (A8b #1554): 4 beats at 120 BPM.
        assert!(
            (seed.bar_duration - 2.0).abs() < 1e-9,
            "4/4 at 120 BPM is a 2 s bar, got {}",
            seed.bar_duration
        );
        let mut last = seed.bar_phase;
        let mut rose = false;
        for k in 1..8 {
            let bp = t
                .process(
                    &beat_result(120.0),
                    [0.0, 0.0, 0.0],
                    0.5,
                    &[0.4; 12],
                    base + k as f64 * period * 0.1,
                    false,
                )
                .bar_phase;
            if bp > last {
                rose = true;
            }
            last = bp;
        }
        assert!(
            rose,
            "bar_phase should advance between beats on the audio clock"
        );
        assert!(last < 1.0, "bar_phase stays in [0,1)");
    }

    /// #1598: the silence gate `BeatDetector` always had. Beats stop firing through a silence,
    /// so `bar_clock` — a pure function of the audio clock — would otherwise keep ramping, and
    /// `bar_phase` would sweep the full 0→1 while `beat_phase` sat pinned at 0.
    ///
    /// `bar_duration` must stay live while pinned: the render side shares that rate (A8b
    /// #1554), and dropping it to the 0 sentinel would mean "no downbeat ever locked" and would
    /// make it jump back to live mid-bar on the way out of the silence.
    #[test]
    fn bar_phase_pins_to_zero_under_silence() {
        let mut t = DownbeatTracker::new();
        run_meter(&mut t, 4, 32, 120.0);
        let period = 60.0 / 120.0;
        let base = 33.0 * period;
        // Seed a clean bar clock, then go silent mid-bar — the point in the sawtooth where a
        // free-running phase is most obviously wrong.
        let seed = t.process(
            &beat_result(120.0),
            [1.0, 0.2, 0.1],
            0.9,
            &[0.4; 12],
            base,
            false,
        );
        assert!(
            (seed.bar_duration - 2.0).abs() < 1e-9,
            "4/4 at 120 BPM = 2 s"
        );

        // A silence long enough to sweep more than a whole 2 s bar, so a free-running phase
        // could not hide inside one.
        for k in 1..40 {
            let r = t.process(
                &silent_beat(120.0),
                [0.0, 0.0, 0.0],
                0.0,
                &[0.0; 12],
                base + k as f64 * period * 0.1,
                true,
            );
            assert_eq!(r.bar_phase, 0.0, "bar_phase pins through silence");
            assert!(
                (r.bar_duration - 2.0).abs() < 1e-9,
                "the bar rate stays published while pinned, got {}",
                r.bar_duration
            );
        }

        // Audio returns. The bar clock resumes on the tracker's own timeline — 2.5 s past a
        // downbeat is a quarter of the way into the next bar — rather than restarting from 0.
        let back = t.process(
            &silent_beat(120.0),
            [0.0, 0.0, 0.0],
            0.5,
            &[0.4; 12],
            base + 2.5,
            false,
        );
        assert!(
            (back.bar_phase - 0.25).abs() < 1e-6,
            "the bar clock resumes where the audio timeline says, got {}",
            back.bar_phase
        );
    }

    #[test]
    fn chroma_distance_zero_when_identical() {
        let a = [0.3; 12];
        assert!(chroma_distance(&a, &a) < 1e-6);
    }
}
