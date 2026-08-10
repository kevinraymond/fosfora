//! Musical key detection via key-profile correlation (A11 #1462, profiles reworked
//! #2079).
//!
//! Maintains a slow (~20 s) rolling mean of the **unnormalized pure-fold** CQT chroma
//! (`CqtChroma`'s `e12`, #2079) and Pearson-correlates it against the 24 major/minor
//! key profiles. Because the input is raw energy, the mean is energy-weighted: a drop
//! frame deposits proportionally more evidence than a noise-floor frame, where the old
//! per-frame-normalized input let every frame vote equally. Pearson is shift/scale
//! invariant, so the unnormalized mean feeds correlation directly. Hysteresis holds the
//! incumbent key unless a challenger beats it by a margin for a sustained interval, so
//! the detected key doesn't flicker between relatives. Outputs feed the reserved
//! `key_*` shader-uniform fields.

/// Major/minor key profiles: the "braw" set — median chroma profiles derived by
/// Faraldo et al. from a 1160-track expert-annotated Beatport corpus (disjoint from
/// the GiantSteps test set; transcribed from Essentia `key.cpp`). Chosen on the
/// GiantSteps tune half (#2079): braw .4840 vs Krumhansl-Kessler .4207, with every
/// candidate that cannot name a major key (edmm's flat major vector) disqualified by
/// the pre-registered major-recall floor. Tonic-relative, degree 0 = tonic.
const PROFILE_MAJOR: [f32; 12] = [
    1.0000, 0.1573, 0.4200, 0.1570, 0.5296, 0.3669, 0.1632, 0.7711, 0.1676, 0.3827, 0.2113, 0.2965,
];
/// Minor sibling of [`PROFILE_MAJOR`] (same derivation).
const PROFILE_MINOR: [f32; 12] = [
    1.0000, 0.2330, 0.3615, 0.3905, 0.2925, 0.3777, 0.1961, 0.7425, 0.2701, 0.2161, 0.4228, 0.2272,
];

/// Rolling-mean time constant (seconds). Key is a global, slowly-varying property.
/// 20 s over 12 s is worth ~+.015 on the tune half; 30 s ties 20 within the sweep's
/// noise floor and loses live key-change responsiveness, so 20 it is (#2079).
const MEAN_TAU: f32 = 20.0;
/// A challenger must beat the incumbent's correlation by this much…
const SWITCH_MARGIN: f32 = 0.05;
/// …sustained for this long (seconds) before the detected key switches.
const SWITCH_TIME: f32 = 3.0;
/// Total-energy floor for the rolling mean. Under sustained silence the mean decays
/// toward zero — a pure scale change Pearson cannot see — so correlation is only
/// trusted while the mean still carries real energy (#2079).
const ENERGY_FLOOR: f32 = 1e-9;
/// Minimum variance of the mean's *shape* (mean rescaled to sum 1) before correlation
/// is trusted — a flat shape is broadband noise / atonal material. Scale-invariant,
/// replacing the old absolute-variance gate that was calibrated to unit-peak input.
const SHAPE_MIN_VAR: f32 = 2e-4;

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
                profiles[tonic][pc] = PROFILE_MAJOR[deg];
                profiles[tonic + 12][pc] = PROFILE_MINOR[deg];
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
    /// `e12` is the **unnormalized pure-fold** energy chroma (`CqtChroma`'s key
    /// output, #2079) — never a per-frame-normalized vector, or loud and silent
    /// frames vote equally again. `dt` is seconds since the previous call.
    pub fn process(&mut self, e12: &[f32; 12], dt: f32) -> KeyResult {
        let alpha = 1.0 - (-dt / MEAN_TAU).exp();
        for i in 0..12 {
            self.chroma_mean[i] += alpha * (e12[i] - self.chroma_mean[i]);
        }

        // Cold or decayed-to-silence mean: hold key, decay confidence. Silence now
        // stops *voting* instead of voting flat.
        let total: f32 = self.chroma_mean.iter().sum();
        if total < ENERGY_FLOOR {
            self.confidence *= 0.99;
            return self.result();
        }
        // Flat shape (broadband noise / atonal): hold key, decay confidence.
        let mut shape = [0.0f32; 12];
        for (s, m) in shape.iter_mut().zip(&self.chroma_mean) {
            *s = m / total;
        }
        if variance(&shape) < SHAPE_MIN_VAR {
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

    /// A chroma vector for a triad on `tonic` with light diatonic support — musical
    /// ground truth rather than the detector's own profiles, so these tests survive
    /// profile swaps (#2079; the old `profile_chroma` helper fed the detector its own
    /// rotated profile, which passed by construction for ANY profile set).
    fn triad_chroma(tonic: usize, minor: bool) -> [f32; 12] {
        let mut c = [0.05f32; 12];
        c[tonic] = 1.0;
        c[(tonic + if minor { 3 } else { 4 }) % 12] = 0.75;
        c[(tonic + 7) % 12] = 0.85;
        // Light scale-tone support: 2nd, 4th, 6th and 7th degrees. Minor uses the
        // HARMONIC-minor leading tone — same reason the bench fixture's cadence
        // raises it: it pins the tonic against the relative-major reading.
        for d in if minor { [2, 5, 8, 11] } else { [2, 5, 9, 11] } {
            c[(tonic + d) % 12] += 0.15;
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
        let r = settle(&mut det, &triad_chroma(0, false), 60.0);
        assert_eq!(r.key_class, 0.0, "expected C tonic");
        assert_eq!(r.is_minor, 0.0);
        assert!(r.confidence > 0.8, "confidence {}", r.confidence);
    }

    #[test]
    fn detects_a_minor() {
        let mut det = KeyDetector::new(48_000.0);
        let r = settle(&mut det, &triad_chroma(9, true), 60.0);
        assert!((r.key_class - 9.0 / 11.0).abs() < 1e-6, "expected A tonic");
        assert_eq!(r.is_minor, 1.0);
        assert!(r.confidence > 0.8, "confidence {}", r.confidence);
    }

    #[test]
    fn flat_chroma_has_low_confidence() {
        let mut det = KeyDetector::new(48_000.0);
        let r = settle(&mut det, &[0.5f32; 12], 30.0);
        assert!(r.confidence < 0.2, "confidence {}", r.confidence);
    }

    /// #2079: the rolling mean is energy-weighted — near-silent frames (a noise
    /// floor the old per-frame L-∞ normalization blew up to unit peak) must not
    /// dilute loud tonal evidence. Reintroducing any per-frame normalization of
    /// the detector's input makes this fail: the wrong-key quiet frames would
    /// again vote as loudly as the music.
    #[test]
    fn quiet_noise_frames_do_not_dilute_loud_key() {
        let mut det = KeyDetector::new(48_000.0);
        // Loud C major alternating with a −60 dB frame shaped like the maximally
        // distant key (F# major) at unit peak — the old pathology's worst case.
        let mut loud = triad_chroma(0, false);
        for v in &mut loud {
            *v *= 10.0;
        }
        let mut quiet = triad_chroma(6, false);
        let peak = quiet.iter().cloned().fold(0.0f32, f32::max);
        for v in &mut quiet {
            *v = *v / peak * 1e-3;
        }

        let mut r = det.process(&loud, 0.01);
        for i in 0..12_000 {
            r = det.process(if i % 2 == 0 { &loud } else { &quiet }, 0.01);
        }
        assert_eq!(r.key_class, 0.0, "expected C tonic");
        assert_eq!(r.is_minor, 0.0);
        assert!(r.confidence > 0.85, "confidence {}", r.confidence);
    }

    /// Sustained silence must decay confidence and hold the key (energy-floor
    /// gate) — under energy weighting the mean decays as a pure scale change
    /// Pearson can't see, so without the floor the detector would stay confident
    /// on nothing forever.
    #[test]
    fn silence_decays_confidence_and_holds_key() {
        let mut det = KeyDetector::new(48_000.0);
        settle(&mut det, &triad_chroma(9, true), 60.0);
        let zero = [0.0f32; 12];
        let mut r = det.process(&zero, 0.01);
        // The mean reaches ENERGY_FLOOR after ~22 time constants (~440 s at τ = 20 s);
        // run past that so the decay path actually engages.
        for _ in 0..60_000 {
            r = det.process(&zero, 0.01);
        }
        assert!(
            (r.key_class - 9.0 / 11.0).abs() < 1e-6,
            "key must hold through silence"
        );
        assert!(
            r.confidence < 0.2,
            "confidence should decay: {}",
            r.confidence
        );
        assert!(r.confidence.is_finite());
    }

    #[test]
    fn hysteresis_holds_then_switches() {
        let mut det = KeyDetector::new(48_000.0);
        // Establish C major.
        settle(&mut det, &triad_chroma(0, false), 60.0);
        // Feed F#-major (maximally distant) briefly — should still read C major.
        let fs = triad_chroma(6, false);
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
