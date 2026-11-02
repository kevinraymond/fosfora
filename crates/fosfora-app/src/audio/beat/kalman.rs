//! Kalman filter for BPM tracking in log2-BPM space.

use super::tempo::{LOCK_INNOVATION_GATE_LOG2, LOCK_INNOVATION_R_MULT};

// ---------------------------------------------------------------------------
// Kalman filter for BPM tracking in log2-BPM space
// ---------------------------------------------------------------------------

pub(super) struct KalmanBpm {
    state: f64,         // log2(BPM)
    variance: f64,      // estimation uncertainty
    q: f64,             // process noise
    r: f64,             // measurement noise
    diverge_count: u32, // leaky count of recent divergent frames
    pub(super) initialized: bool,
}

impl KalmanBpm {
    pub(super) fn new() -> Self {
        Self {
            state: 0.0,
            variance: 1.0,
            q: 0.001,
            r: 0.1,
            diverge_count: 0,
            initialized: false,
        }
    }

    /// Jump the state to `bpm` with high certainty (A7 #1458: tap tempo).
    pub(super) fn force(&mut self, bpm: f64) {
        if bpm <= 0.0 {
            return;
        }
        self.state = bpm.log2();
        self.variance = 0.01;
        self.diverge_count = 0;
        self.initialized = true;
    }

    /// Shift the filtered state by `direction` octaves (A7 #1458).
    pub(super) fn shift_octave(&mut self, direction: i32) {
        if !self.initialized {
            return;
        }
        self.state += direction as f64;
    }

    /// Update with a raw BPM measurement and confidence. Returns filtered BPM.
    /// `locked` engages the innovation gate — see the update step below.
    /// `anchored` (Q2b) hardens the divergence path: the hard reset needs ~3 s
    /// of sustained deviation instead of ~1 s, and process noise stays low so
    /// transition mush cannot glide the state off the anchor — an ACF window
    /// flushing between metrical levels produces exactly such mush, and it
    /// must not read as "a genuinely different song".
    pub(super) fn update(
        &mut self,
        raw_bpm: f64,
        confidence: f64,
        locked: bool,
        anchored: bool,
    ) -> f64 {
        if raw_bpm <= 0.0 {
            return if self.initialized {
                2.0f64.powf(self.state)
            } else {
                0.0
            };
        }

        if !self.initialized {
            self.state = raw_bpm.log2();
            self.variance = 1.0;
            self.initialized = true;
            return raw_bpm;
        }

        let current_bpm = 2.0f64.powf(self.state);

        // Q2: no octave-snap preprocessing here anymore. Octave decisions live in
        // `compute_tempo`'s candidate scoring (continuity weight), where they are
        // made on graded evidence — the old snap-then-count-to-30 loop reset on any
        // single non-snapping frame, which is how tracks got permanently stuck on
        // the wrong octave.
        let measurement = raw_bpm.log2();

        // Divergence detection: sustained large deviation -> hard reset. The count
        // leaks instead of zeroing so one in-range frame cannot indefinitely
        // postpone a genuine tempo change.
        let bpm_deviation = (raw_bpm - current_bpm).abs() / current_bpm.max(1.0);
        if bpm_deviation > 0.10 {
            self.diverge_count += 1;
        } else {
            self.diverge_count = self.diverge_count.saturating_sub(1);
        }

        let reset_limit = if anchored { 45 } else { 15 };
        if self.diverge_count >= reset_limit {
            log::debug!(
                "Kalman hard reset: {:.1} -> {:.1} BPM (diverged for {} frames)",
                current_bpm,
                raw_bpm,
                self.diverge_count
            );
            self.state = raw_bpm.log2();
            self.variance = 1.0;
            self.diverge_count = 0;
            return raw_bpm;
        }

        // Adaptive noise: R = f(confidence), Q = f(stability)
        self.r = 0.01 + (1.0 - confidence) * 0.5;
        // Escalate process noise only once divergence is SUSTAINED — a single
        // divergent frame with q = 0.1 would let 2-3 noisy frames drag the
        // state most of the way to a false octave. While anchored, never: the
        // hard reset (above) is the only unrelated-change path, so the state
        // cannot be dragged off the anchor by mush.
        self.q = if self.diverge_count >= 5 && !anchored {
            0.1
        } else {
            0.001
        };

        // Kalman predict (constant model: state unchanged)
        self.variance += self.q;

        // Kalman update
        let innovation = measurement - self.state;
        // Q3 lock dynamics: once the estimator is locked, a strongly-disagreeing
        // measurement is far more likely a transient (breakdown, fill, half-bar
        // of percussion dropout) than a tempo change, so its gain is cut instead
        // of letting a ~2 s wobble drag the state out of the ±4% band — measured
        // as up to 10 excursions per GiantSteps track, each resetting the
        // trailing-lock clock. A GENUINE change still escapes: the divergence
        // counter above accumulates on the raw deviation regardless of gain and
        // hard-resets after ~1 s of sustained disagreement.
        let r = if locked && innovation.abs() > LOCK_INNOVATION_GATE_LOG2 {
            self.r * LOCK_INNOVATION_R_MULT
        } else {
            self.r
        };
        let s = self.variance + r;
        let k = self.variance / s;
        self.state += k * innovation;
        self.variance *= 1.0 - k;

        2.0f64.powf(self.state)
    }
}
