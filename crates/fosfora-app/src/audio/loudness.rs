//! EBU R128 / ITU-R BS.1770 loudness (A10 #1461).
//!
//! The only pre-existing level feature is a raw RMS over the last 2048 samples, whose
//! silence gates behave differently across devices and content and carry no perceptual
//! weighting. This stage adds proper loudness: the capture stream is K-weighted (the
//! BS.1770 two-stage pre-filter), then its mean square is integrated over sliding
//! 400 ms (momentary) and 3 s (short-term) windows. `LUFS = -0.691 + 10·log10(ms)`.
//!
//! Outputs fill the reserved shader fields (#1505, CPU-side only, no ABI churn):
//! - `loudness_m`     — momentary loudness, −60..0 LUFS mapped to 0..1
//! - `loudness_s`     — short-term loudness, same mapping
//! - `loudness_trend` — `clamp(M − S)` rising component, a ready-made build/drop hint
//!   consumed by A18 (#1469)
//!
//! Momentary/short-term loudness are *ungated* (BS.1770 gating applies only to the
//! integrated program measurement, which we do not compute). The capture is a mono
//! downmix, so this is single-channel loudness (channel weight 1.0) — an approximation
//! of true program loudness, but device/content-independent and perceptually sane, which
//! is all the visuals need.

/// LUFS mapped to 0.0 at this level.
const LUFS_MIN: f32 = -60.0;
/// LUFS mapped to 1.0 at this level.
const LUFS_MAX: f32 = 0.0;
/// Floor substituted for `-inf` LUFS on (near-)silence so downstream math stays finite.
const SILENCE_LUFS: f32 = -70.0;
/// `M − S` (in LU) that maps `loudness_trend` to 1.0. ~8 LU of momentary-over-short-term
/// excess is a strong riser.
const TREND_RANGE_LU: f32 = 8.0;
/// Momentary loudness below this (LUFS) counts as silence for gating (consumed by the
/// onset detector, A6 #1457). −55 LUFS is well below any musical content.
const SILENCE_GATE_LUFS: f32 = -55.0;

/// A single biquad section in Direct Form I. Coefficients assume `a0 == 1`.
#[derive(Clone, Copy)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl Biquad {
    fn new(b0: f32, b1: f32, b2: f32, a1: f32, a2: f32) -> Self {
        Self {
            b0,
            b1,
            b2,
            a1,
            a2,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        }
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2
            - self.a1 * self.y1
            - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

/// BS.1770 stage 1: high-shelf boost (~+4 dB above ~1.68 kHz), re-derived for `fs`.
/// Coefficients follow the libebur128 formulation (bilinear-transformed at the actual
/// sample rate — Phosphor has no resampler, so device rates vary).
fn k_weight_shelf(fs: f32) -> Biquad {
    let f0 = 1681.974450955533_f64;
    let g_db = 3.999843853973347_f64;
    let q = 0.7071752369554196_f64;
    let k = (std::f64::consts::PI * f0 / fs as f64).tan();
    let vh = 10.0_f64.powf(g_db / 20.0);
    let vb = vh.powf(0.4996667741545416);
    let a0 = 1.0 + k / q + k * k;
    Biquad::new(
        ((vh + vb * k / q + k * k) / a0) as f32,
        (2.0 * (k * k - vh) / a0) as f32,
        ((vh - vb * k / q + k * k) / a0) as f32,
        (2.0 * (k * k - 1.0) / a0) as f32,
        ((1.0 - k / q + k * k) / a0) as f32,
    )
}

/// BS.1770 stage 2: high-pass (~38 Hz), re-derived for `fs`. Numerator is the fixed
/// `[1, -2, 1]` of the libebur128 formulation (unity gain in the passband for a corner
/// this low).
fn k_weight_highpass(fs: f32) -> Biquad {
    let f0 = 38.13547087602444_f64;
    let q = 0.5003270373238773_f64;
    let k = (std::f64::consts::PI * f0 / fs as f64).tan();
    let a0 = 1.0 + k / q + k * k;
    Biquad::new(
        1.0,
        -2.0,
        1.0,
        (2.0 * (k * k - 1.0) / a0) as f32,
        ((1.0 - k / q + k * k) / a0) as f32,
    )
}

/// Per-hop loudness outputs, all mapped to 0..1, copied onto `AudioFeatures`.
pub struct LoudnessResult {
    /// Momentary loudness (400 ms), −60..0 LUFS → 0..1.
    pub m: f32,
    /// Short-term loudness (3 s), −60..0 LUFS → 0..1.
    pub s: f32,
    /// Rising component of `M − S` (in LU), 0..1 — a build/drop hint.
    pub trend: f32,
}

pub struct LoudnessMeter {
    shelf: Biquad,
    highpass: Biquad,
    /// Ring of K-weighted squared samples, long enough for the short-term window.
    sq: Vec<f32>,
    write: usize,
    filled: usize,
    /// Momentary / short-term window lengths (samples).
    n_m: usize,
    n_s: usize,
    /// Running sums over the last `n_m` / `n_s` squared samples.
    sum_m: f64,
    sum_s: f64,
    /// Most recent momentary loudness in LUFS (for the silence gate).
    last_m_lufs: f32,
}

impl LoudnessMeter {
    pub fn new(sample_rate: f32) -> Self {
        let sr = if sample_rate > 0.0 {
            sample_rate
        } else {
            44100.0
        };
        let n_m = (0.400 * sr) as usize;
        let n_s = (3.000 * sr) as usize;
        Self {
            shelf: k_weight_shelf(sr),
            highpass: k_weight_highpass(sr),
            sq: vec![0.0; n_s.max(1)],
            write: 0,
            filled: 0,
            n_m: n_m.max(1),
            n_s: n_s.max(1),
            sum_m: 0.0,
            sum_s: 0.0,
            last_m_lufs: SILENCE_LUFS,
        }
    }

    /// Feed one hop of time-domain samples (each sample exactly once — pass the fresh
    /// block read from the capture ring, not the analyzer's overlapping window). Returns
    /// the loudness at the end of the block.
    pub fn process(&mut self, samples: &[f32]) -> LoudnessResult {
        for &x in samples {
            let w = self.highpass.process(self.shelf.process(x));
            self.push(w * w);
        }
        self.result()
    }

    /// Momentary loudness is below the silence gate (−55 LUFS). Consumed by the onset
    /// detector so all stages gate on the same perceptual threshold (A6 #1457).
    pub fn is_silent(&self) -> bool {
        self.last_m_lufs < SILENCE_GATE_LUFS
    }

    /// Push one K-weighted squared sample, maintaining both sliding-window running sums.
    #[inline]
    fn push(&mut self, sq: f32) {
        // Evict the sample leaving the short-term window (the ring's oldest slot).
        if self.filled == self.n_s {
            self.sum_s -= self.sq[self.write] as f64;
        }
        // Evict the sample leaving the momentary window (written `n_m` pushes ago). Its
        // index differs from `write` because n_m < n_s, so it is safe to read first.
        if self.filled >= self.n_m {
            let leave = (self.write + self.n_s - self.n_m) % self.n_s;
            self.sum_m -= self.sq[leave] as f64;
        }
        self.sq[self.write] = sq;
        self.sum_s += sq as f64;
        self.sum_m += sq as f64;
        self.write = (self.write + 1) % self.n_s;
        if self.filled < self.n_s {
            self.filled += 1;
        }
    }

    fn result(&mut self) -> LoudnessResult {
        let count_m = self.filled.min(self.n_m);
        let count_s = self.filled.min(self.n_s);
        let m_lufs = lufs(self.sum_m, count_m);
        let s_lufs = lufs(self.sum_s, count_s);
        self.last_m_lufs = m_lufs;

        let trend = ((m_lufs - s_lufs).clamp(0.0, TREND_RANGE_LU) / TREND_RANGE_LU).clamp(0.0, 1.0);
        LoudnessResult {
            m: norm_lufs(m_lufs),
            s: norm_lufs(s_lufs),
            trend,
        }
    }
}

/// Mean-square (from a running sum and count) to LUFS, floored on silence.
fn lufs(sum: f64, count: usize) -> f32 {
    if count == 0 {
        return SILENCE_LUFS;
    }
    let ms = sum / count as f64;
    if ms <= 1e-12 {
        return SILENCE_LUFS;
    }
    (-0.691 + 10.0 * ms.log10()) as f32
}

/// Map −60..0 LUFS to 0..1.
fn norm_lufs(lufs: f32) -> f32 {
    ((lufs - LUFS_MIN) / (LUFS_MAX - LUFS_MIN)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feed `secs` seconds of a `freq`-Hz sine at `amp` and return the final result.
    fn drive_sine(sr: f32, freq: f32, amp: f32, secs: f32) -> (LoudnessMeter, LoudnessResult) {
        let mut m = LoudnessMeter::new(sr);
        let n = (secs * sr) as usize;
        let mut buf = Vec::with_capacity(n);
        for i in 0..n {
            let t = i as f32 / sr;
            buf.push(amp * (std::f32::consts::TAU * freq * t).sin());
        }
        // Feed in ~10 ms hops to mimic the audio thread.
        let hop = (0.010 * sr) as usize;
        let mut last = LoudnessResult {
            m: 0.0,
            s: 0.0,
            trend: 0.0,
        };
        for chunk in buf.chunks(hop.max(1)) {
            last = m.process(chunk);
        }
        (m, last)
    }

    #[test]
    fn louder_reads_higher_and_in_range() {
        let (_, quiet) = drive_sine(44100.0, 1000.0, 0.05, 4.0);
        let (_, loud) = drive_sine(44100.0, 1000.0, 0.5, 4.0);
        assert!(
            loud.s > quiet.s,
            "louder sine must read higher loudness ({} vs {})",
            loud.s,
            quiet.s
        );
        // A −6 dBFS sine sits comfortably inside 0..1, not pinned to the rails.
        assert!(
            loud.s > 0.5 && loud.s < 1.0,
            "loud short-term loudness out of expected range: {}",
            loud.s
        );
        assert!(quiet.s > 0.0, "audible sine should read above silence");
    }

    #[test]
    fn silence_reads_zero_and_gates() {
        let mut m = LoudnessMeter::new(48000.0);
        for _ in 0..400 {
            let r = m.process(&[0.0; 480]);
            assert_eq!(r.m, 0.0);
            assert_eq!(r.s, 0.0);
        }
        assert!(m.is_silent(), "pure silence must trip the gate");
    }

    #[test]
    fn momentary_tracks_faster_than_short_term_on_a_rise() {
        // Silence for ~3 s (fills the short-term window low), then a loud tone: the
        // momentary window catches up faster, so M > S and the trend hint fires.
        let sr = 44100.0;
        let mut m = LoudnessMeter::new(sr);
        let hop = (0.010 * sr) as usize;
        for _ in 0..300 {
            m.process(&vec![0.0; hop]);
        }
        let mut phase = 0.0f32;
        let step = std::f32::consts::TAU * 1000.0 / sr;
        let mut buf = vec![0.0f32; hop];
        // ~0.5 s into the tone: momentary (400 ms) has largely filled, short-term (3 s)
        // has not — trend should be positive.
        let mut trend = 0.0;
        for _ in 0..50 {
            for s in &mut buf {
                *s = 0.4 * phase.sin();
                phase += step;
            }
            trend = m.process(&buf).trend;
        }
        assert!(
            trend > 0.0,
            "rising loudness should produce a positive trend"
        );
    }

    #[test]
    fn odd_sample_rate_does_not_panic() {
        // Non-standard device rate: coefficients must still be finite and the meter sane.
        let (_, r) = drive_sine(37913.0, 440.0, 0.3, 4.0);
        assert!(r.s.is_finite() && (0.0..=1.0).contains(&r.s));
    }
}
