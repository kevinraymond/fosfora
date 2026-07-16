//! Musical key detection via Krumhansl-Kessler profile correlation (A11 #1462).
//!
//! Maintains a slow (~12 s) rolling mean of the CQT chroma vector and Pearson-correlates
//! it against the 24 major/minor key profiles. Hysteresis holds the incumbent key unless a
//! challenger beats it by a margin for a sustained interval, so the detected key doesn't
//! flicker between relatives. Outputs feed the reserved `key_*` shader-uniform fields.

/// Krumhansl-Kessler major key profile (tonic-relative, degree 0 = tonic).
const KK_MAJOR: [f32; 12] = [
    6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88,
];
/// Krumhansl-Kessler minor key profile (tonic-relative).
const KK_MINOR: [f32; 12] = [
    6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17,
];

/// Rolling-mean time constant (seconds). Key is a global, slowly-varying property.
const MEAN_TAU: f32 = 12.0;
/// A challenger must beat the incumbent's correlation by this much…
const SWITCH_MARGIN: f32 = 0.05;
/// …sustained for this long (seconds) before the detected key switches.
const SWITCH_TIME: f32 = 3.0;
/// Minimum chroma-mean variance before correlation is trusted (else: silence/atonal).
const MIN_VARIANCE: f32 = 1e-4;

pub struct KeyResult {
    /// Tonic pitch class / 11 (same encoding as `dominant_chroma`), 0 = C.
    pub key_class: f32,
    /// 1.0 for a minor key, 0.0 for major.
    pub is_minor: f32,
    /// Winning Pearson correlation, clamped to 0..1.
    pub confidence: f32,
}

pub struct KeyDetector {
    /// 24 tonic-rotated profiles: 0..11 major (tonic = index), 12..23 minor (tonic = index−12).
    profiles: [[f32; 12]; 24],
    chroma_mean: [f32; 12],
    started: bool,
    current: usize,       // winning profile index 0..24
    challenger: usize,    // profile currently accruing challenge time
    challenger_time: f32, // seconds the challenger has led by the margin
    confidence: f32,
}

impl KeyDetector {
    pub fn new(_sample_rate: f32) -> Self {
        let mut profiles = [[0.0f32; 12]; 24];
        for tonic in 0..12 {
            for pc in 0..12 {
                let deg = (pc + 12 - tonic) % 12;
                profiles[tonic][pc] = KK_MAJOR[deg];
                profiles[tonic + 12][pc] = KK_MINOR[deg];
            }
        }
        Self {
            profiles,
            chroma_mean: [0.0; 12],
            started: false,
            current: 0,
            challenger: 0,
            challenger_time: 0.0,
            confidence: 0.0,
        }
    }

    /// Fold one chroma frame into the rolling mean and update the detected key.
    /// `chroma` should be the raw (pre-normalization) CQT chroma; `dt` is seconds
    /// since the previous call.
    pub fn process(&mut self, chroma: &[f32; 12], dt: f32) -> KeyResult {
        let alpha = 1.0 - (-dt / MEAN_TAU).exp();
        for i in 0..12 {
            self.chroma_mean[i] += alpha * (chroma[i] - self.chroma_mean[i]);
        }

        // Too little tonal variance (silence / broadband noise): hold key, decay confidence.
        if variance(&self.chroma_mean) < MIN_VARIANCE {
            self.confidence *= 0.99;
            return self.result();
        }

        // Correlate the mean against all 24 key profiles.
        let mut best = 0usize;
        let mut best_corr = f32::MIN;
        let mut corr = [0.0f32; 24];
        for (k, profile) in self.profiles.iter().enumerate() {
            let c = pearson(&self.chroma_mean, profile);
            corr[k] = c;
            if c > best_corr {
                best_corr = c;
                best = k;
            }
        }

        if !self.started {
            // Warm-up: adopt the best key directly until an incumbent is established.
            self.current = best;
            self.started = true;
        } else if best != self.current {
            // Accrue challenge time only while the same challenger keeps leading by the margin.
            if best == self.challenger && corr[best] > corr[self.current] + SWITCH_MARGIN {
                self.challenger_time += dt;
            } else {
                self.challenger = best;
                self.challenger_time = 0.0;
            }
            if self.challenger_time >= SWITCH_TIME {
                self.current = best;
                self.challenger_time = 0.0;
            }
        } else {
            // Incumbent still wins outright — no pending challenge.
            self.challenger_time = 0.0;
        }

        self.confidence = corr[self.current].clamp(0.0, 1.0);
        self.result()
    }

    fn result(&self) -> KeyResult {
        let tonic = (self.current % 12) as f32;
        KeyResult {
            key_class: tonic / 11.0,
            is_minor: if self.current >= 12 { 1.0 } else { 0.0 },
            confidence: self.confidence,
        }
    }
}

fn variance(v: &[f32; 12]) -> f32 {
    let mean = v.iter().sum::<f32>() / 12.0;
    v.iter().map(|x| (x - mean) * (x - mean)).sum::<f32>() / 12.0
}

/// Pearson correlation of two 12-vectors; 0 when either has (near-)zero variance.
fn pearson(a: &[f32; 12], b: &[f32; 12]) -> f32 {
    let ma = a.iter().sum::<f32>() / 12.0;
    let mb = b.iter().sum::<f32>() / 12.0;
    let (mut num, mut da, mut db) = (0.0f32, 0.0f32, 0.0f32);
    for i in 0..12 {
        let xa = a[i] - ma;
        let xb = b[i] - mb;
        num += xa * xb;
        da += xa * xa;
        db += xb * xb;
    }
    let den = (da * db).sqrt();
    if den > 1e-9 { num / den } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A chroma vector matching a key profile rotated to `tonic` (0 = C).
    fn profile_chroma(tonic: usize, minor: bool) -> [f32; 12] {
        let base = if minor { KK_MINOR } else { KK_MAJOR };
        let mut c = [0.0f32; 12];
        for (pc, cv) in c.iter_mut().enumerate() {
            *cv = base[(pc + 12 - tonic) % 12];
        }
        c
    }

    fn settle(det: &mut KeyDetector, chroma: &[f32; 12], secs: f32) -> KeyResult {
        let mut r = det.process(chroma, 0.01);
        let frames = (secs / 0.01) as usize;
        for _ in 0..frames {
            r = det.process(chroma, 0.01);
        }
        r
    }

    #[test]
    fn detects_c_major() {
        let mut det = KeyDetector::new(48_000.0);
        let r = settle(&mut det, &profile_chroma(0, false), 60.0);
        assert_eq!(r.key_class, 0.0, "expected C tonic");
        assert_eq!(r.is_minor, 0.0);
        assert!(r.confidence > 0.9, "confidence {}", r.confidence);
    }

    #[test]
    fn detects_a_minor() {
        let mut det = KeyDetector::new(48_000.0);
        let r = settle(&mut det, &profile_chroma(9, true), 60.0);
        assert!((r.key_class - 9.0 / 11.0).abs() < 1e-6, "expected A tonic");
        assert_eq!(r.is_minor, 1.0);
        assert!(r.confidence > 0.9, "confidence {}", r.confidence);
    }

    #[test]
    fn flat_chroma_has_low_confidence() {
        let mut det = KeyDetector::new(48_000.0);
        let r = settle(&mut det, &[0.5f32; 12], 30.0);
        assert!(r.confidence < 0.2, "confidence {}", r.confidence);
    }

    #[test]
    fn hysteresis_holds_then_switches() {
        let mut det = KeyDetector::new(48_000.0);
        // Establish C major.
        settle(&mut det, &profile_chroma(0, false), 60.0);
        // Feed F#-major (maximally distant) briefly — should still read C major.
        let fs = profile_chroma(6, false);
        let brief = det.process(&fs, 0.01);
        assert_eq!(brief.key_class, 0.0, "flipped too early");
        // Sustained exposure eventually switches to F# major.
        let switched = settle(&mut det, &fs, 60.0);
        assert!(
            (switched.key_class - 6.0 / 11.0).abs() < 1e-6,
            "expected F# tonic, got {}",
            switched.key_class
        );
    }
}
