//! A16 (#1467): delta-MFCC timbre dynamics — `timbre_flux` + the `dmfcc.0-12` binding sources.
//!
//! MFCCs describe a frame's static spectral envelope; their *rate of change* captures timbre
//! motion — filter sweeps, vocal entries, evolving pads — that the loudness-driven `flux` misses.
//! Each hop this fits a line to the last [`RING`] MFCC frames and takes the per-coefficient slope
//! (a causal delta-MFCC). The newest frame is one edge of the window, so it adds no latency.
//!
//! - `dmfcc[0..13]` — the 13 slopes, exposed **bindings-only** as `audio.dmfcc.N` (not part of the
//!   [`crate::audio::AudioFeatures`] ABI, to save the uniform budget) — raw bipolar values the
//!   binding graph range-maps.
//! - `timbre_flux` — the L2 norm of the slope over coefficients **1..=12**. MFCC[0] is the
//!   log-energy term, so excluding it keeps `timbre_flux` a pure timbre-*shape* signal orthogonal
//!   to `flux`: a constant-loudness filter sweep fires it, a plain volume change does not. The
//!   caller sets it **before** `normalize()`, so the A2 adaptive normalizer percentile-ranges and
//!   silence-gates it like `flux` (schema policy `Adaptive`).

/// MFCC coefficients per frame (matches [`crate::audio::AudioFeatures::mfcc`]).
const N_MFCC: usize = 13;
/// Frames in the linear-regression window (~58 ms at the 512-sample hop). 5 = librosa's default
/// delta width; here it is a trailing (causal) fit rather than a centred one.
const RING: usize = 5;

/// Delta-MFCC output for one hop.
#[derive(Debug, Clone, Copy)]
pub struct DeltaMfcc {
    /// Per-coefficient slope (delta-MFCC) → bindings-only `audio.dmfcc.0..12`. Zero until the ring
    /// fills and on silence.
    pub dmfcc: [f32; N_MFCC],
    /// L2 norm of `dmfcc[1..=12]` (excludes the log-energy coef 0) — a level-robust timbre-motion
    /// scalar, adaptively normalized downstream.
    pub timbre_flux: f32,
}

impl DeltaMfcc {
    /// Neutral output: no motion. Emitted on silence and before the ring fills.
    pub const NEUTRAL: Self = Self {
        dmfcc: [0.0; N_MFCC],
        timbre_flux: 0.0,
    };
}

/// Rolling causal delta-MFCC over the last [`RING`] MFCC frames.
pub struct DeltaMfccAnalyzer {
    /// Ring of recent MFCC frames.
    ring: [[f32; N_MFCC]; RING],
    /// Next slot to overwrite (the oldest frame).
    pos: usize,
    /// Frames seen so far, capped at `RING`.
    filled: usize,
}

impl DeltaMfccAnalyzer {
    pub fn new() -> Self {
        Self {
            ring: [[0.0; N_MFCC]; RING],
            pos: 0,
            filled: 0,
        }
    }

    /// Push this hop's MFCCs and return the causal delta-MFCC + `timbre_flux`.
    ///
    /// `loud_silent` is the A10 perceptual-silence flag: the ring is cleared and NEUTRAL returned,
    /// so a rest never leaves a stale frame that would spike the slope on the next note. (The
    /// normalizer also silence-gates the Adaptive `timbre_flux`; clearing keeps the Passthrough-style
    /// `dmfcc` sources from chasing the gap too.)
    pub fn process(&mut self, mfcc: &[f32; N_MFCC], loud_silent: bool) -> DeltaMfcc {
        if loud_silent {
            self.pos = 0;
            self.filled = 0;
            return DeltaMfcc::NEUTRAL;
        }

        // Overwrite the oldest slot with the current frame, then advance so `pos` again points at
        // the oldest.
        self.ring[self.pos] = *mfcc;
        self.pos = (self.pos + 1) % RING;
        if self.filled < RING {
            self.filled += 1;
        }

        // A stable slope needs the full window; hold neutral while it fills.
        if self.filled < RING {
            return DeltaMfcc::NEUTRAL;
        }

        // Least-squares slope over unit-spaced frames: slope = Σ (t − t̄)·x_t / Σ (t − t̄)², with
        // t = 0..RING-1 (0 = oldest = `ring[pos]`), t̄ = (RING−1)/2. For RING = 5 the weights are
        // [−2,−1,0,1,2] and the denominator is 10.
        let t_mean = (RING as f32 - 1.0) * 0.5;
        let mut denom = 0.0f32;
        let mut dmfcc = [0.0f32; N_MFCC];
        for k in 0..RING {
            let w = k as f32 - t_mean;
            denom += w * w;
            let frame = &self.ring[(self.pos + k) % RING]; // chronological: k = 0 is oldest
            for c in 0..N_MFCC {
                dmfcc[c] += w * frame[c];
            }
        }
        for d in &mut dmfcc {
            *d /= denom;
        }

        // timbre_flux: L2 over coeffs 1..=12 — exclude the log-energy coef 0 so it tracks
        // timbre shape, not loudness.
        let sq: f32 = dmfcc[1..].iter().map(|d| d * d).sum();
        DeltaMfcc {
            dmfcc,
            timbre_flux: sq.sqrt(),
        }
    }
}

impl Default for DeltaMfccAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame where coefficient `c` holds `val` and the rest are zero.
    fn only(c: usize, val: f32) -> [f32; N_MFCC] {
        let mut f = [0.0f32; N_MFCC];
        f[c] = val;
        f
    }

    #[test]
    fn under_full_window_is_neutral() {
        let mut a = DeltaMfccAnalyzer::new();
        // Four pushes: fewer than RING (5) frames, so no slope yet.
        for i in 0..RING - 1 {
            let out = a.process(&only(1, i as f32), false);
            assert_eq!(out.dmfcc, [0.0; N_MFCC]);
            assert_eq!(out.timbre_flux, 0.0);
        }
    }

    #[test]
    fn linear_ramp_recovers_slope() {
        // Coefficient 3 rises by exactly 0.5 per frame → slope 0.5; others flat → 0.
        let mut a = DeltaMfccAnalyzer::new();
        let mut out = DeltaMfcc::NEUTRAL;
        for i in 0..RING {
            out = a.process(&only(3, 0.5 * i as f32), false);
        }
        assert!((out.dmfcc[3] - 0.5).abs() < 1e-5, "slope {}", out.dmfcc[3]);
        for (c, &d) in out.dmfcc.iter().enumerate() {
            if c != 3 {
                assert!(d.abs() < 1e-6, "coef {c} should be flat, got {d}");
            }
        }
        assert!(
            (out.timbre_flux - 0.5).abs() < 1e-5,
            "flux {}",
            out.timbre_flux
        );
    }

    #[test]
    fn steady_input_has_zero_delta() {
        let mut a = DeltaMfccAnalyzer::new();
        let frame = only(2, 1.0);
        let mut out = DeltaMfcc::NEUTRAL;
        for _ in 0..RING {
            out = a.process(&frame, false);
        }
        assert_eq!(out.dmfcc, [0.0; N_MFCC]);
        assert_eq!(out.timbre_flux, 0.0);
    }

    #[test]
    fn timbre_flux_excludes_coef_zero() {
        // A ramp *only* in the log-energy coefficient (0): dmfcc[0] moves, but timbre_flux stays 0.
        let mut a = DeltaMfccAnalyzer::new();
        let mut out = DeltaMfcc::NEUTRAL;
        for i in 0..RING {
            out = a.process(&only(0, 2.0 * i as f32), false);
        }
        assert!(out.dmfcc[0].abs() > 1.0, "coef 0 slope {}", out.dmfcc[0]);
        assert_eq!(out.timbre_flux, 0.0, "flux must ignore coef 0");
    }

    #[test]
    fn silence_clears_ring_and_is_neutral() {
        // Fill the ring, then a silent frame resets it; the next loud frame must not spike a slope
        // across the gap (ring re-fills from empty → neutral until full again).
        let mut a = DeltaMfccAnalyzer::new();
        for i in 0..RING {
            a.process(&only(1, i as f32), false);
        }
        let silent = a.process(&only(1, 100.0), true);
        assert_eq!(silent.timbre_flux, 0.0);
        assert_eq!(silent.dmfcc, [0.0; N_MFCC]);
        // The next loud frame is now frame 1 of a fresh window → still neutral (no cross-gap spike).
        assert_eq!(a.process(&only(1, 0.0), false).timbre_flux, 0.0);
    }

    #[test]
    fn all_zeros_no_nan() {
        let mut a = DeltaMfccAnalyzer::new();
        let mut out = DeltaMfcc::NEUTRAL;
        for _ in 0..RING {
            out = a.process(&[0.0; N_MFCC], false);
        }
        assert!(out.timbre_flux.is_finite());
        assert!(out.dmfcc.iter().all(|d| d.is_finite()));
    }
}
