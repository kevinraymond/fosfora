//! A8 (#1459): render-rate interpolation of the audio feature vector.
//!
//! Analysis runs at a fixed 86.1 Hz hop ([`ANALYSIS_HOP`](super::ANALYSIS_HOP) @ 44.1 kHz)
//! while the render loop polls at the display's refresh rate. On a 144 Hz panel that means
//! ~28% of polls see a byte-identical frame: continuous features go slightly flat, and the
//! two sawtooths — `beat_phase` and `bar_phase`, which only move when a frame lands —
//! visibly stair-step. A dropped frame (the channel is `bounded(4)`, drop-on-full) shows up
//! as a phase pop.
//!
//! This module keeps a short ring of timestamped frames and a playhead running
//! [`TARGET_DELAY_HOPS`] behind the audio sample clock, blends the two frames bracketing
//! it per each slot's [`InterpPolicy`], and advances both sawtooths locally as first-order
//! PLLs locked to the audio thread's phases (A8 #1459 for `beat_phase`, A8b #1554 for
//! `bar_phase`) — see [`advance_phase`].

use std::collections::VecDeque;

use super::ANALYSIS_HOP;
use super::features::AudioFeatures;
use super::schema::{self, InterpPolicy};

/// How far behind the newest audio frame the playhead sits, in analysis hops. 1.5 hops
/// (≈17.4 ms @ 44.1 kHz) keeps the playhead bracketed by retained frames even though the
/// audio thread delivers in bursts: it sleeps 10 ms but a hop is 11.6 ms, so hops land
/// 0/1/2 at a time. One full hop of slack absorbs that.
///
/// At ≤1.0 hop the playhead sits at or past the newest frame and the interpolator degrades
/// to a hold — the stair-step returns. Beyond ~2 hops the added latency buys nothing. The
/// cost is 17 ms on a chain already carrying ~93 ms of 4096-sample analysis window, and it
/// does not delay beat *timing* at all: the phase is advanced locally and the pulse comes
/// from the atomic beat counter, neither of which waits on the playhead.
const TARGET_DELAY_HOPS: f64 = 1.5;

/// Retained frames. A playhead at `newest − 1.5` hops is bracketed by the frames at
/// `newest − 2` and `newest − 1`, so 3 is the minimum; 4 gives a hop of margin and mirrors
/// the channel's `bounded(4)`. The ring always keeps the *newest* 4, so it stays correct
/// at low render rates too (a 15 fps poll drains ~5.7 hops; the retained window still
/// brackets the playhead).
const FRAME_RING_CAP: usize = 4;

/// Exponential time constant pulling the playhead toward `newest − TARGET_DELAY`. Gentle
/// on purpose: free-running by `dt` between arrivals is what makes the output smooth, so
/// the slew must only trim accumulated error. Slew hard and the playhead just tracks
/// arrival times and the stair-step comes straight back.
const PLAYHEAD_SLEW_TAU: f32 = 0.25;
/// Hard cap on playhead correction, as a fraction of real time. Guarantees the playhead
/// never stalls or reverses (`dt − 0.1·dt > 0`), so features can never run backwards.
const PLAYHEAD_MAX_SLEW: f64 = 0.10;
/// Beyond this the error is a genuine discontinuity rather than jitter — snap. Matches the
/// 250 ms stale-decay threshold; 0.25 s of error would take ~2.5 s to slew out at 10%.
const PLAYHEAD_SNAP: f64 = 0.25;

/// Exponential time constant pulling the local beat phase onto the detector's phase.
/// 0.15 s ≈ a fifth of a beat at 120 BPM: honours a re-locked grid within one beat, slow
/// enough that per-hop phase noise doesn't jitter.
const PHASE_RESYNC_TAU: f32 = 0.15;
/// Backstop cap on phase correction per render frame (10% of a cycle). NOT the primary
/// mechanism — with [`PHASE_RESYNC_TAU`] the largest sub-snap correction only exceeds this
/// below ~12 fps. The time constant does the work. (Provably unreachable for the bar
/// sawtooth, whose snap threshold is below it at every musical tempo.)
const PHASE_MAX_STEP: f32 = 0.10;
/// Ceiling on the snap threshold: a wrap-aware phase error beyond a quarter cycle is genuine
/// desync (a tempo octave jump, re-tracking on a new song) — snap rather than crawl there at
/// ≤10%/frame.
///
/// The threshold actually used is `(PHASE_RESYNC_TAU · rate).min(PHASE_SNAP)` — see
/// [`advance_phase`], which derives why a flat quarter-cycle lets the *bar* sawtooth run
/// backwards. This ceiling binds for beats at ≥100 BPM, i.e. it is the A8 behaviour.
const PHASE_SNAP: f32 = 0.25;
/// Below this the detector has no usable tempo (it reports 0 before lock) — hold the local
/// phase rather than free-run it.
const MIN_TRACK_BPM: f32 = 20.0;

/// One retained audio frame, tagged with the sample-clock time it was analyzed at.
struct TimedFrame {
    ts: f64,
    features: AudioFeatures,
    phase_frozen: bool,
    bar_duration: f64,
}

/// Render-side feature interpolator. Holds `AudioFeatures` only (244 B, `Copy`) — the
/// spectrum/mel array streams keep their existing non-interpolated paths, so nothing here
/// clones a `Box<[f32]>`.
pub struct FeatureInterpolator {
    frames: VecDeque<TimedFrame>,
    /// Playhead on the audio sample clock, seconds. `None` until the first frame seeds it
    /// and after any clock reset, which makes re-seeding explicit rather than a slew
    /// across an arbitrarily large error.
    playhead: Option<f64>,
    local_beat_phase: f32,
    local_bar_phase: f32,
    /// One analysis hop in seconds, from the device's sample rate.
    hop_secs: f64,
}

impl FeatureInterpolator {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            frames: VecDeque::with_capacity(FRAME_RING_CAP),
            playhead: None,
            local_beat_phase: 0.0,
            local_bar_phase: 0.0,
            hop_secs: ANALYSIS_HOP as f64 / sample_rate.max(1) as f64,
        }
    }

    /// Drop all interpolation state. Callers must do this whenever the audio sample clock
    /// restarts (a device switch resets `samples_consumed` to 0, so the next timestamp
    /// jumps *backwards* by the whole session length) or when the held features are being
    /// decayed by the stall path — see [`Self::sample`].
    pub fn reset(&mut self) {
        self.frames.clear();
        self.playhead = None;
        self.local_beat_phase = 0.0;
        self.local_bar_phase = 0.0;
    }

    /// Retain one freshly received frame.
    pub fn push(
        &mut self,
        ts: f64,
        features: &AudioFeatures,
        phase_frozen: bool,
        bar_duration: f64,
    ) {
        // A non-monotonic timestamp means the sample clock restarted under us (a device
        // switch racing the drain) — drop the stale window rather than interpolate across
        // the discontinuity.
        if self.frames.back().is_some_and(|f| ts <= f.ts) {
            self.frames.clear();
            self.playhead = None;
        }
        if self.frames.len() == FRAME_RING_CAP {
            self.frames.pop_front();
        }
        self.frames.push_back(TimedFrame {
            ts,
            features: *features,
            phase_frozen,
            bar_duration,
        });
    }

    /// Advance by one render frame and return the features at the playhead.
    ///
    /// `fallback` is the newest held frame, used when the ring cannot bracket the playhead
    /// — the first two frames after startup, a stall, or a device switch. That path is
    /// exactly the pre-A8 behaviour.
    ///
    /// Note the caller must still apply its beat/downbeat/drop counter latch *after* this:
    /// those pulses are derived from atomics, not from the interpolated vector.
    pub fn sample(&mut self, dt: f32, fallback: Option<&AudioFeatures>) -> Option<AudioFeatures> {
        self.advance_playhead(dt);

        let (mut features, phase_frozen, bar_duration) = self
            .playhead
            .and_then(|p| self.sample_at(p))
            // Nothing bracketing the playhead (startup, a stall, a device switch): serve the
            // caller's held frame. With a frame retained, mirror *its* flags — it is what the
            // fallback stands in for; with the ring empty, default to frozen / no bar clock,
            // so a stalled device can't free-run either local phase.
            .or_else(|| {
                fallback.map(|f| match self.frames.back() {
                    Some(t) => (*f, t.phase_frozen, t.bar_duration),
                    None => (*f, true, 0.0),
                })
            })?;

        // The detector's phase only changes on frame arrival, so at 144 Hz it repeats ~28%
        // of samples. Advance our own copies every render frame instead, resynced to the
        // detector's — the sawtooth's *rate* is what must stay true, and `bpm` /
        // `bar_duration` give it exactly.
        features.beat_phase =
            self.advance_beat_phase(dt, features.beat_phase, features.bpm, phase_frozen);
        features.bar_phase =
            self.advance_bar_phase(dt, features.bar_phase, bar_duration, phase_frozen);
        Some(features)
    }

    /// Free-run the playhead by `dt`, then trim accumulated error toward
    /// `newest − TARGET_DELAY`.
    fn advance_playhead(&mut self, dt: f32) {
        let Some(newest_ts) = self.frames.back().map(|f| f.ts) else {
            return;
        };
        let target = newest_ts - TARGET_DELAY_HOPS * self.hop_secs;
        let Some(p) = self.playhead else {
            self.playhead = Some(target);
            return;
        };
        let advanced = p + dt as f64;
        let err = target - advanced;
        self.playhead = Some(if err.abs() > PLAYHEAD_SNAP {
            target
        } else {
            let corr = err * (1.0 - (-dt / PLAYHEAD_SLEW_TAU).exp()) as f64;
            let cap = PLAYHEAD_MAX_SLEW * dt as f64;
            advanced + corr.clamp(-cap, cap)
        });
    }

    /// Interpolate the retained frames at `p` (sample-clock seconds). `None` if fewer than
    /// two frames are retained.
    ///
    /// Also returns the older bracketing frame's `phase_frozen` and `bar_duration` — the
    /// same frame the Held slots (`beat_phase`, `bar_phase`) come from, so the phases, the
    /// silence gate the detector used, and the rate the tracker's `bar_phase` was computed
    /// on all describe one instant. Taking `bar_duration` from the *newer* frame instead
    /// would pair a fresh bar length with a phase computed on the old one across any
    /// playhead-straddled downbeat.
    fn sample_at(&self, p: f64) -> Option<(AudioFeatures, bool, f64)> {
        if self.frames.len() < 2 {
            return None;
        }
        // Find the pair bracketing `p`. The ring is tiny and ordered, so a linear scan
        // beats anything cleverer. Falls back to the end pairs when `p` lies outside.
        let mut i = 0;
        for w in 0..self.frames.len() - 1 {
            if self.frames[w + 1].ts > p {
                break;
            }
            i = w;
        }
        let (a, b) = (&self.frames[i], &self.frames[i + 1]);
        let span = b.ts - a.ts;
        // Clamp rather than extrapolate: overshooting produces out-of-range values
        // (`rms > 1`), and when the audio thread is late, clamping degrades to the old
        // hold for a few ms instead of popping.
        let alpha = if span <= 0.0 {
            1.0
        } else {
            (((p - a.ts) / span) as f32).clamp(0.0, 1.0)
        };
        Some((
            interp_features(&a.features, &b.features, alpha),
            a.phase_frozen,
            a.bar_duration,
        ))
    }

    /// Advance the local beat phase by `dt` at the detected tempo, then wrap-aware
    /// soft-resync it toward the detector's `audio_phase`.
    ///
    /// The local rate (`bpm/60`) and the detector's phase rate (`1/period`) agree by
    /// construction — `period == 60/bpm` exactly — so a `bpm` that lags during a tempo
    /// change yields a small standing phase offset, never cumulative drift.
    fn advance_beat_phase(
        &mut self,
        dt: f32,
        audio_phase: f32,
        bpm_norm: f32,
        frozen: bool,
    ) -> f32 {
        let bpm = bpm_norm * 300.0; // `bpm` ships normalized to 0..1
        if frozen || bpm < MIN_TRACK_BPM {
            // The detector pins its phase at 0 under silence and reports no tempo before
            // lock. Follow it rather than free-run over a silence.
            self.local_beat_phase = audio_phase;
            return self.local_beat_phase;
        }
        advance_phase(&mut self.local_beat_phase, dt, audio_phase, bpm / 60.0)
    }

    /// Advance the local bar phase by `dt` at `1/bar_duration`, then soft-resync it onto the
    /// tracker's `bar_phase`.
    ///
    /// `bar_duration` is the tracker's own bar-clock denominator, carried on the frame rather
    /// than re-derived here from `bpm` and `meter`. The tracker refreshes it *only on a
    /// downbeat*, in the same branch that zeroes its bar phase, so a frame carrying a new
    /// duration carries `bar_phase == 0` exactly; holding the pair together (see
    /// [`Self::sample_at`]) means the rate and the wrap always change on the same render
    /// frame, and the mid-bar rate change that deferred this task never happens. Re-deriving
    /// the rate from a live `meter` would instead make it *more* current than the phase it
    /// chases — and on a lost tempo lock (`bpm → 0`, `meter` still 4) it would freeze the
    /// local phase while the tracker's kept ramping on its stale duration.
    ///
    /// The `frozen` gate mirrors [`Self::advance_beat_phase`] (#1598). It used to be absent
    /// deliberately, because `DownbeatTracker` had no silence gate of its own and this side
    /// reproduces the audio thread rather than inventing policy it lacks — but the tracker now
    /// pins its `bar_phase` to 0 through a silence, so following it here is what keeps the two
    /// sides agreeing. Note the tracker keeps publishing a live `bar_duration` while pinned, so
    /// the `bar_duration <= 0.0` branch below does *not* cover this case.
    fn advance_bar_phase(
        &mut self,
        dt: f32,
        audio_phase: f32,
        bar_duration: f64,
        frozen: bool,
    ) -> f32 {
        if frozen {
            self.local_bar_phase = audio_phase;
            return self.local_bar_phase;
        }
        if bar_duration <= 0.0 {
            // No bar clock yet (no downbeat locked, or the stall fallback): the tracker pins
            // its bar phase at 0 under exactly this condition, so follow it. Also the guard
            // against `1.0/0.0 = inf` — `(local + dt·inf).fract()` is NaN, which would poison
            // the local phase *permanently*, since every later comparison against it is false.
            self.local_bar_phase = audio_phase;
            return self.local_bar_phase;
        }
        advance_phase(
            &mut self.local_bar_phase,
            dt,
            audio_phase,
            (1.0 / bar_duration) as f32, // reciprocal in f64, narrow once
        )
    }
}

/// Advance `local` by `dt` at `rate_hz` cycles/sec, then wrap-aware soft-resync it onto the
/// detector's `audio_phase`. A first-order PLL, shared by the beat and bar sawtooths — they
/// differ only in which state they carry and where the rate comes from (`bpm/60` for the
/// beat, `1/bar_duration` for the bar).
///
/// `rate_hz` must be finite and > 0; each caller gates on its own "no usable clock yet"
/// signal first (see [`MIN_TRACK_BPM`] and [`FeatureInterpolator::advance_bar_phase`]),
/// because a rate of 0 freezes the oscillator and an infinite one poisons it with NaN —
/// permanently, since every later comparison against a NaN is false.
fn advance_phase(local: &mut f32, dt: f32, audio_phase: f32, rate_hz: f32) -> f32 {
    *local = (*local + dt * rate_hz).fract();

    // Wrap-aware shortest path: local 0.98 against audio 0.02 is +0.04 forward, not
    // −0.96 backwards through the middle of the cycle.
    let mut err = audio_phase - *local;
    if err > 0.5 {
        err -= 1.0;
    } else if err < -0.5 {
        err += 1.0;
    }

    // Snap threshold, as a fraction of one cycle. Solving the loop (`ė = −e/τ` against a
    // constant advance) gives `dp/dt = rate + (err/τ)·e^(−t/τ)`, so a soft correction
    // outruns the advance — running the sawtooth *backwards* — exactly when
    // `|err| > τ·rate`; and since `1 − e^(−x) ≤ x`, that continuous bound is conservative at
    // every `dt`. Snapping there makes "the phase only ever rises or wraps" a property of
    // the loop rather than of the tempo. [`PHASE_SNAP`] caps it: past a quarter cycle the
    // error is a genuine desync (a tempo octave jump, re-tracking on a new song, the tracker
    // moving which beat is the "one") and crawling there at ≤10%/frame would drag a visibly
    // wrong phase across several cycles.
    //
    // A beat at 120 BPM has τ·rate = 0.30, so the cap binds and this is exactly the A8
    // behaviour; below 100 BPM it tightens, closing a reversal band A8 left open. A 2 s bar
    // has τ·rate = 0.075 — it snaps past ~7% of a bar, far more drift than a Kalman-smoothed
    // `bpm` leaves in one bar and far less than the ≥0.25 a re-assigned "one" produces.
    // Sharing the flat 0.25 would instead sweep the bar phase *backwards* for ~180 ms and
    // delay the visual "one" by a quarter bar.
    let snap = (PHASE_RESYNC_TAU * rate_hz).min(PHASE_SNAP);

    *local = if err.abs() > snap {
        audio_phase
    } else {
        let corr =
            (err * (1.0 - (-dt / PHASE_RESYNC_TAU).exp())).clamp(-PHASE_MAX_STEP, PHASE_MAX_STEP);
        (*local + corr).rem_euclid(1.0)
    };
    *local
}

/// Blend `a` -> `b` by `alpha` per each slot's [`InterpPolicy`]: continuous quantities
/// lerp, while triggers, wrapping phases and categorical indices zero-order-hold from `a`
/// — the *older* frame, so the whole vector stays temporally consistent at the playhead
/// rather than mixing two instants.
fn interp_features(a: &AudioFeatures, b: &AudioFeatures, alpha: f32) -> AudioFeatures {
    let mut out = *a;
    let (av, bv) = (a.as_slice(), b.as_slice());
    for (i, v) in out.as_slice_mut().iter_mut().enumerate() {
        match schema::FEATURES[i].interp {
            InterpPolicy::Lerp => *v = av[i] + (bv[i] - av[i]) * alpha,
            InterpPolicy::Hold => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 44_100;
    /// The detector's real hop clock: 512 / 44100 ≈ 11.6 ms (86.1 Hz).
    ///
    /// Feed test frames at exactly this rate, never at a round 100 Hz. A BPM harness that
    /// used `dt = 0.01` made every target land 13.9% low, and ±15% tolerance bands hid it
    /// for months (#1516).
    const HOP: f64 = ANALYSIS_HOP as f64 / SR as f64;
    /// One 144 Hz render frame — the display rate this whole feature exists for.
    const RENDER_DT: f32 = 1.0 / 144.0;

    fn feats(rms: f32, beat_phase: f32, bpm: f32) -> AudioFeatures {
        AudioFeatures {
            rms,
            beat_phase,
            bpm: bpm / 300.0, // ships normalized
            ..Default::default()
        }
    }

    /// A frame carrying only a bar sawtooth. `bpm` stays 0, so the beat PLL holds and these
    /// tests exercise the bar path in isolation.
    fn bar_feats(bar_phase: f32) -> AudioFeatures {
        AudioFeatures {
            rms: 0.5,
            bar_phase,
            ..Default::default()
        }
    }

    /// One bar at 120 BPM in 4/4 — the tempo every other test in this file uses.
    const BAR_2S: f64 = 2.0;

    /// Drive `n` render frames' worth of polls, feeding audio frames at the 86.1 Hz hop
    /// clock as their timestamps come due. `f(elapsed)` supplies each audio frame, and
    /// `bar_duration` the bar clock they carry (0.0 = no bar clock, for beat-only tests).
    fn run(
        interp: &mut FeatureInterpolator,
        n: usize,
        bar_duration: f64,
        mut f: impl FnMut(f64) -> AudioFeatures,
    ) -> Vec<AudioFeatures> {
        let mut out = Vec::with_capacity(n);
        let mut clock = 0.0f64;
        let mut next_hop = 0.0f64;
        for _ in 0..n {
            while next_hop <= clock {
                interp.push(next_hop, &f(next_hop), false, bar_duration);
                next_hop += HOP;
            }
            if let Some(s) = interp.sample(RENDER_DT, None) {
                out.push(s);
            }
            clock += RENDER_DT as f64;
        }
        out
    }

    #[test]
    fn lerps_continuous_features() {
        let mut it = FeatureInterpolator::new(SR);
        it.push(0.0, &feats(0.0, 0.0, 120.0), false, 0.0);
        it.push(HOP, &feats(1.0, 0.0, 120.0), false, 0.0);
        // Sample the midpoint of the only bracketing pair directly.
        let (f, _, _) = it
            .sample_at(HOP / 2.0)
            .expect("two frames bracket the midpoint");
        assert!(
            (f.rms - 0.5).abs() < 1e-5,
            "rms at alpha=0.5, got {}",
            f.rms
        );
    }

    #[test]
    fn never_extrapolates_past_the_ring() {
        let mut it = FeatureInterpolator::new(SR);
        it.push(0.0, &feats(0.0, 0.0, 120.0), false, 0.0);
        it.push(HOP, &feats(1.0, 0.0, 120.0), false, 0.0);
        // Well past the newest frame: must clamp to it, not extrapolate to rms > 1.
        let (f, _, _) = it.sample_at(HOP * 5.0).expect("clamps to the last pair");
        assert!(
            (f.rms - 1.0).abs() < 1e-5,
            "rms must clamp to 1.0, got {}",
            f.rms
        );
        // Well before the oldest: must clamp to it, not run negative.
        let (f, _, _) = it.sample_at(-HOP * 5.0).expect("clamps to the first pair");
        assert!(
            (f.rms - 0.0).abs() < 1e-5,
            "rms must clamp to 0.0, got {}",
            f.rms
        );
    }

    #[test]
    fn holds_pulses_and_indices() {
        // A 1-frame trigger, an argmax pitch-class index, and a categorical key.
        let a = AudioFeatures {
            dominant_chroma: 0.0, // C
            key_class: 0.0,
            ..Default::default()
        };
        let b = AudioFeatures {
            beat: 1.0,
            downbeat: 1.0,
            drop: 1.0,
            dominant_chroma: 2.0 / 11.0, // D — lerping would read C# in between
            key_class: 5.0 / 11.0,
            key_is_minor: 1.0,
            beat_in_bar: 1.0 / 3.0,
            ..Default::default()
        };

        let mid = interp_features(&a, &b, 0.5);
        assert_eq!(mid.beat, 0.0, "a trigger must never read fractional");
        assert_eq!(mid.downbeat, 0.0);
        assert_eq!(mid.drop, 0.0);
        assert_eq!(mid.dominant_chroma, 0.0, "argmax index must hold");
        assert_eq!(mid.key_class, 0.0);
        assert_eq!(mid.key_is_minor, 0.0);
        assert_eq!(mid.beat_in_bar, 0.0);
    }

    #[test]
    fn holds_phase_across_the_wrap() {
        let a = feats(0.5, 0.98, 120.0);
        let b = feats(0.5, 0.02, 120.0);
        // Lerping the wrap would sweep backwards through ~0.5 — the middle of the beat.
        let mid = interp_features(&a, &b, 0.5);
        assert_eq!(mid.beat_phase, 0.98, "a wrapping phase must hold, not lerp");
    }

    #[test]
    fn beat_phase_has_no_stair_step_at_144hz() {
        let mut it = FeatureInterpolator::new(SR);
        // A detector phase that only moves on the 86.1 Hz hop clock — the stair-step source.
        let out = run(&mut it, 600, 0.0, |t| {
            feats(0.5, ((t * 2.0) % 1.0) as f32, 120.0)
        });
        // Skip the seeding frames, then require every consecutive pair to differ.
        let tail = &out[out.len() - 400..];
        let dupes = tail
            .windows(2)
            .filter(|w| w[0].beat_phase == w[1].beat_phase)
            .count();
        assert_eq!(
            dupes, 0,
            "beat_phase repeated on {dupes} consecutive render frames"
        );
    }

    #[test]
    fn beat_phase_rate_locks_without_drift() {
        let mut it = FeatureInterpolator::new(SR);
        // 120 BPM = 2 beats/sec. 10 s of 144 Hz polls => expect ~20 wraps.
        let secs = 10.0;
        let n = (secs * 144.0) as usize;
        let out = run(&mut it, n, 0.0, |t| {
            feats(0.5, ((t * 2.0) % 1.0) as f32, 120.0)
        });
        let wraps = out
            .windows(2)
            .filter(|w| w[1].beat_phase < w[0].beat_phase)
            .count();
        assert!(
            (wraps as i32 - 20).abs() <= 1,
            "expected ~20 wraps in {secs}s at 120 BPM, got {wraps}"
        );
        // Between wraps the phase must climb, never sit still or reverse.
        for w in out.windows(2) {
            let d = w[1].beat_phase - w[0].beat_phase;
            assert!(
                !(-0.5..=0.0).contains(&d),
                "phase must rise or wrap, got delta {d}"
            );
        }
    }

    #[test]
    fn beat_phase_resyncs_forward_across_the_wrap() {
        let mut it = FeatureInterpolator::new(SR);
        it.local_beat_phase = 0.98;
        // Detector says 0.02 — just past the wrap. Shortest path is +0.04 forward.
        let p = it.advance_beat_phase(1.0 / 1000.0, 0.02, 120.0 / 300.0, false);
        assert!(
            p >= 0.98 || p <= 0.06,
            "must correct forward across the wrap, landed at {p}"
        );
    }

    #[test]
    fn beat_phase_snaps_on_genuine_desync() {
        let mut it = FeatureInterpolator::new(SR);
        it.local_beat_phase = 0.1;
        // A half-beat error is past PHASE_SNAP — crawling there would drag a visibly wrong
        // phase across several beats.
        let p = it.advance_beat_phase(RENDER_DT, 0.6, 120.0 / 300.0, false);
        assert!((p - 0.6).abs() < 1e-5, "must snap to 0.6, landed at {p}");
    }

    #[test]
    fn beat_phase_frozen_on_silence() {
        let mut it = FeatureInterpolator::new(SR);
        it.local_beat_phase = 0.4;
        // The detector pins phase at 0 under silence; the local oscillator must follow
        // rather than free-run through a quiet passage.
        for _ in 0..100 {
            it.advance_beat_phase(RENDER_DT, 0.0, 120.0 / 300.0, true);
        }
        assert_eq!(
            it.local_beat_phase, 0.0,
            "local phase must follow the freeze"
        );
    }

    #[test]
    fn beat_phase_holds_before_tempo_lock() {
        let mut it = FeatureInterpolator::new(SR);
        // The detector reports bpm 0 before it locks — must not free-run at 0 BPM.
        for _ in 0..100 {
            it.advance_beat_phase(RENDER_DT, 0.0, 0.0, false);
        }
        assert_eq!(it.local_beat_phase, 0.0);
    }

    #[test]
    fn playhead_never_reverses_and_slew_is_bounded() {
        let mut it = FeatureInterpolator::new(SR);
        let mut clock = 0.0f64;
        let mut next_hop = 0.0f64;
        let mut last = None;
        for _ in 0..600 {
            while next_hop <= clock {
                it.push(next_hop, &feats(0.5, 0.0, 120.0), false, 0.0);
                next_hop += HOP;
            }
            it.sample(RENDER_DT, None);
            if let (Some(prev), Some(now)) = (last, it.playhead) {
                let step: f64 = now - prev;
                assert!(step > 0.0, "playhead reversed: {prev} -> {now}");
                // Free-run dt, trimmed by at most ±10% — never more than 1.1·dt.
                assert!(
                    step <= RENDER_DT as f64 * (1.0 + PLAYHEAD_MAX_SLEW) + 1e-9,
                    "playhead slewed {step} > 1.1·dt"
                );
            }
            last = it.playhead;
            clock += RENDER_DT as f64;
        }
    }

    #[test]
    fn playhead_tracks_the_target_delay() {
        let mut it = FeatureInterpolator::new(SR);
        let mut clock = 0.0f64;
        let mut next_hop = 0.0f64;
        for _ in 0..600 {
            while next_hop <= clock {
                it.push(next_hop, &feats(0.5, 0.0, 120.0), false, 0.0);
                next_hop += HOP;
            }
            it.sample(RENDER_DT, None);
            clock += RENDER_DT as f64;
        }
        // After settling, the playhead should sit ~1.5 hops behind the newest frame — the
        // invariant that keeps alpha inside [0,1] without clamping.
        let newest = it.frames.back().expect("frames retained").ts;
        let lag = (newest - it.playhead.expect("playhead seeded")) / HOP;
        assert!(
            (lag - TARGET_DELAY_HOPS).abs() < 0.6,
            "playhead should lag ~{TARGET_DELAY_HOPS} hops, lagged {lag}"
        );
    }

    #[test]
    fn clock_reset_reseeds_instead_of_slewing() {
        let mut it = FeatureInterpolator::new(SR);
        // Build up a session at t≈100s...
        for i in 0..4 {
            it.push(100.0 + i as f64 * HOP, &feats(0.5, 0.0, 120.0), false, 0.0);
        }
        it.sample(RENDER_DT, None);
        assert!(it.playhead.expect("seeded") > 99.0);
        // ...then a device switch restarts `samples_consumed` at 0.
        it.push(0.0, &feats(0.5, 0.0, 120.0), false, 0.0);
        assert!(
            it.playhead.is_none(),
            "backwards clock must drop the playhead"
        );
        assert_eq!(it.frames.len(), 1, "stale window must be dropped");
        it.push(HOP, &feats(0.5, 0.0, 120.0), false, 0.0);
        it.sample(RENDER_DT, None);
        let p = it.playhead.expect("re-seeded on the new clock");
        assert!(p < 1.0, "playhead must re-seed near the new clock, got {p}");
    }

    #[test]
    fn falls_back_to_held_features_before_the_ring_fills() {
        let mut it = FeatureInterpolator::new(SR);
        let held = feats(0.7, 0.0, 120.0);
        // Nothing pushed yet: must serve the caller's held frame, not None.
        let f = it.sample(RENDER_DT, Some(&held)).expect("falls back");
        assert!((f.rms - 0.7).abs() < 1e-6);
        // And with nothing held either, there's nothing to serve.
        let mut it2 = FeatureInterpolator::new(SR);
        assert!(it2.sample(RENDER_DT, None).is_none());
    }

    /// The invariant `AudioSystem`'s stall path depends on: after a reset, `sample` serves
    /// the caller's fallback *verbatim* rather than anything retained.
    ///
    /// The stall path decays `self.latest` (the fallback) but cannot reach into the ring.
    /// If a stalled device left the ring live, the last loud frame would keep being served
    /// at alpha=1 and the decay would be silently defeated — visuals frozen loud on a dead
    /// device. That wiring lives in `AudioSystem::latest_features`, which needs a real
    /// capture pipeline to construct; this pins the half that can be tested in isolation,
    /// and the E2E (kill the null sink, watch features decay) covers the wiring.
    #[test]
    fn reset_makes_sample_serve_the_fallback() {
        let mut it = FeatureInterpolator::new(SR);
        for i in 0..4 {
            it.push(i as f64 * HOP, &feats(1.0, 0.0, 120.0), false, 0.0);
        }
        it.sample(RENDER_DT, None);
        it.reset();

        // Stand in for a decayed `self.latest`: the ring still "remembers" rms 1.0.
        let decayed = feats(0.02, 0.0, 120.0);
        let f = it
            .sample(RENDER_DT, Some(&decayed))
            .expect("serves the fallback");
        assert!(
            (f.rms - 0.02).abs() < 1e-6,
            "must serve the decayed fallback, not the retained loud frame (got rms {})",
            f.rms
        );
    }

    #[test]
    fn reset_clears_everything() {
        let mut it = FeatureInterpolator::new(SR);
        for i in 0..4 {
            it.push(i as f64 * HOP, &feats(0.5, 0.3, 120.0), false, 0.0);
        }
        it.sample(RENDER_DT, None);
        it.reset();
        assert!(it.frames.is_empty());
        assert!(it.playhead.is_none());
        assert_eq!(it.local_beat_phase, 0.0);
        assert_eq!(it.local_bar_phase, 0.0);
    }

    // ---- A8b (#1554): the locally-advanced bar phase -------------------------------------

    #[test]
    fn bar_phase_has_no_stair_step_at_144hz() {
        let mut it = FeatureInterpolator::new(SR);
        // A tracker bar_phase that only moves on the 86.1 Hz hop clock — the stair-step source.
        let out = run(&mut it, 600, BAR_2S, |t| {
            bar_feats(((t / BAR_2S) % 1.0) as f32)
        });
        let tail = &out[out.len() - 400..];
        let dupes = tail
            .windows(2)
            .filter(|w| w[0].bar_phase == w[1].bar_phase)
            .count();
        assert_eq!(
            dupes, 0,
            "bar_phase repeated on {dupes} consecutive render frames"
        );
    }

    #[test]
    fn bar_phase_rate_locks_without_drift() {
        let mut it = FeatureInterpolator::new(SR);
        // A 2 s bar over 10 s of 144 Hz polls => expect ~5 wraps.
        let secs = 10.0;
        let n = (secs * 144.0) as usize;
        let out = run(&mut it, n, BAR_2S, |t| {
            bar_feats(((t / BAR_2S) % 1.0) as f32)
        });
        let wraps = out
            .windows(2)
            .filter(|w| w[1].bar_phase < w[0].bar_phase)
            .count();
        assert!(
            (wraps as i32 - 5).abs() <= 1,
            "expected ~5 wraps in {secs}s of 2 s bars, got {wraps}"
        );
        // Between wraps the phase must climb, never sit still or reverse.
        for w in out.windows(2) {
            let d = w[1].bar_phase - w[0].bar_phase;
            assert!(
                !(-0.5..=0.0).contains(&d),
                "bar_phase must rise or wrap, got delta {d}"
            );
        }
    }

    /// The test that pins the rate-derived snap threshold (see [`advance_phase`]).
    ///
    /// The tracker re-assigning which beat is the "one" lands a downbeat ~0.2 of a bar from
    /// where the local oscillator sat. That error is *under* the flat `PHASE_SNAP` of 0.25,
    /// so a shared quarter-cycle threshold takes the gentle path — where, at a bar's 0.5 Hz,
    /// the correction outruns the advance and sweeps `bar_phase` **backwards** for ~180 ms
    /// before converging a quarter-bar later. `τ·rate` (0.075 here) snaps instead.
    #[test]
    fn bar_phase_snaps_rather_than_sweeping_backwards() {
        let mut it = FeatureInterpolator::new(SR);
        it.local_bar_phase = 0.25;
        let p = it.advance_bar_phase(RENDER_DT, 0.05, BAR_2S, false);
        assert!(
            (p - 0.05).abs() < 1e-5,
            "must snap onto the tracker at 0.05, landed at {p} (a flat PHASE_SNAP crawls \
             backwards from 0.25 instead)"
        );
    }

    #[test]
    fn bar_phase_holds_before_the_first_downbeat() {
        let mut it = FeatureInterpolator::new(SR);
        // No downbeat has landed: the tracker reports bar_phase 0 and no bar clock. Must not
        // free-run — and must not divide by zero (a NaN would fail this `assert_eq!`, and
        // would poison the local phase permanently).
        for _ in 0..100 {
            it.advance_bar_phase(RENDER_DT, 0.0, 0.0, false);
        }
        assert_eq!(it.local_bar_phase, 0.0);

        // The gate is "follow the tracker", not "freeze at 0".
        let p = it.advance_bar_phase(RENDER_DT, 0.37, 0.0, false);
        assert!((p - 0.37).abs() < 1e-6, "must follow the tracker, got {p}");
    }

    /// The bar-side half of [`reset_makes_sample_serve_the_fallback`]: an emptied ring must
    /// supply *no bar clock*, so the local phase follows the stall decay instead of
    /// free-running a dead device's last bar around and around.
    #[test]
    fn bar_phase_follows_the_stall_fallback() {
        let mut it = FeatureInterpolator::new(SR);
        for i in 0..4 {
            it.push(i as f64 * HOP, &bar_feats(0.5), false, BAR_2S);
        }
        it.sample(RENDER_DT, None);
        it.reset();

        let decayed = bar_feats(0.02);
        let f = it
            .sample(RENDER_DT, Some(&decayed))
            .expect("serves the fallback");
        assert!(
            (f.bar_phase - 0.02).abs() < 1e-6,
            "must follow the decayed fallback, not free-run from 0.5 (got {})",
            f.bar_phase
        );
    }

    /// Both phases pin together through a silence (#1598). `DownbeatTracker` gained the
    /// silence gate `BeatDetector` always had, so it now publishes `bar_phase == 0` with its
    /// bar *rate* still live, and this side follows rather than sweeping on into the quiet.
    ///
    /// This test asserted the opposite before #1598 — a smooth bar sweep against a pinned
    /// `beat_phase`, which is precisely the incoherent pairing the task was filed for.
    ///
    /// The live `BAR_2S` is what makes this a real test of the `frozen` gate: the
    /// `bar_duration <= 0.0` branch cannot cover this case, so without the gate the PLL would
    /// creep up at 0.5 Hz and snap back each time it drifted past the threshold.
    #[test]
    fn bar_phase_pins_to_zero_through_silence() {
        let mut it = FeatureInterpolator::new(SR);
        let mut clock = 0.0f64;
        let mut next_hop = 0.0f64;
        let mut out = Vec::new();
        for _ in 0..600 {
            while next_hop <= clock {
                // Silence: the detector pins beat_phase at 0 and flags the freeze, and the
                // tracker now pins bar_phase alongside it while still publishing the rate.
                let mut f = bar_feats(0.0);
                f.beat_phase = 0.0;
                it.push(next_hop, &f, true, BAR_2S);
                next_hop += HOP;
            }
            if let Some(s) = it.sample(RENDER_DT, None) {
                out.push(s);
            }
            clock += RENDER_DT as f64;
        }
        let tail = &out[out.len() - 400..];
        assert!(
            tail.iter().all(|f| f.beat_phase == 0.0),
            "beat_phase must follow the detector's freeze"
        );
        assert!(
            tail.iter().all(|f| f.bar_phase == 0.0),
            "bar_phase must follow the tracker's freeze, not free-run on the bar clock"
        );
    }

    /// The invariant the whole design rests on: `bar_duration` is held from the *older*
    /// bracketing frame — the same one the Held `bar_phase` comes from — so the rate and the
    /// phase always describe one instant. The tracker only ever rewrites `bar_duration` in
    /// the same branch that zeroes `bar_phase`, so one playhead crossing swaps both at once
    /// and the render never sees a mid-bar rate change either.
    #[test]
    fn bar_duration_holds_from_the_older_bracketing_frame() {
        let mut it = FeatureInterpolator::new(SR);
        // A 4/4 → 3/4 flip at 120 BPM: the downbeat at `b` resets the phase and re-sizes the
        // bar in the same frame.
        it.push(0.0, &bar_feats(0.97), false, BAR_2S);
        it.push(HOP, &bar_feats(0.0), false, 1.5);

        let (f, _, bar_duration) = it.sample_at(HOP / 2.0).expect("two frames bracket it");
        assert_eq!(f.bar_phase, 0.97, "the wrapping sawtooth holds from `a`");
        assert_eq!(
            bar_duration, BAR_2S,
            "the rate must hold from `a` too — pairing 1.5 with a phase computed at 2.0 \
             would advance the local oscillator on a bar it isn't on"
        );
    }

    #[test]
    fn meter_flip_keeps_the_bar_sawtooth_continuous() {
        let mut it = FeatureInterpolator::new(SR);
        let mut clock = 0.0f64;
        let mut next_hop = 0.0f64;
        // Four 2 s bars, then a 4/4 → 3/4 flip: the "one" lands early and every later bar is
        // 1.5 s. `t0` is the flip instant on the audio clock.
        let t0 = 8.0f64;
        let mut out = Vec::new();
        for _ in 0..(14.0 * 144.0) as usize {
            while next_hop <= clock {
                let phase = if next_hop < t0 {
                    (next_hop / BAR_2S) % 1.0
                } else {
                    ((next_hop - t0) / 1.5) % 1.0
                };
                let dur = if next_hop < t0 { BAR_2S } else { 1.5 };
                it.push(next_hop, &bar_feats(phase as f32), false, dur);
                next_hop += HOP;
            }
            if let Some(s) = it.sample(RENDER_DT, None) {
                out.push((clock, s.bar_phase));
            }
            clock += RENDER_DT as f64;
        }
        for &(t, p) in &out {
            assert!(
                (0.0..1.0).contains(&p),
                "bar_phase left [0,1) at t={t}: {p}"
            );
        }
        // After the flip has settled, the sawtooth must run on the *new* 1.5 s bar.
        let tail: Vec<f32> = out
            .iter()
            .filter(|(t, _)| *t > 9.0)
            .map(|(_, p)| *p)
            .collect();
        let wraps = tail.windows(2).filter(|w| w[1] < w[0]).count();
        let expect = ((14.0 - 9.0) / 1.5) as i32;
        assert!(
            (wraps as i32 - expect).abs() <= 1,
            "after the flip expected ~{expect} wraps on 1.5 s bars, got {wraps}"
        );
    }
}
