//! Per-frame audio view for trama modulation sources.
//!
//! Assembled once per frame in `App::update` from the features App already
//! holds (`latest_features` consumes a single-consumer pulse latch — trama
//! must never call it again) plus the raw 64-band mel column. The named
//! features arrive already adaptive-normalized and asymmetrically smoothed
//! by the audio pipeline, so they pass through untouched — a second
//! smoothing stage here would double-smooth. The mel bands are the one raw
//! source, so they get their own attack/release one-pole.

// Wired into the frame loop by the executor-integration commit; the allow
// keeps this commit green under -D warnings until then.
#![allow(dead_code)]

use crate::audio::features::AudioFeatures;

/// Modulation band count: adjacent-pair means of the 64-band mel column.
pub const BAND_COUNT: usize = 32;

/// Mel-band envelope attack, seconds. Matches the fastest named-band attack
/// in the feature schema (`sub_bass` 0.02) so transients stay punchy.
const BAND_ATTACK: f32 = 0.020;
/// Mel-band envelope release, seconds. Matches the onset-hold decay
/// precedent (τ = 0.20) — reads as a VU-meter fall.
const BAND_RELEASE: f32 = 0.200;

/// Audio-feature modulation sources. `Bpm` is the normalized 0..1 field
/// (bpm/300) — correct as a *signal*; anything doing time math must go
/// through `AudioFeatures::raw_bpm` instead (#2054).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioFeature {
    Rms,
    Onset,
    Bass,
    Mid,
    High,
    /// One of the 32 mel-derived bands; indices clamp to 31.
    Band(u8),
    BeatPhase,
    Bpm,
}

/// This frame's modulation-source snapshot: the engine features verbatim,
/// the smoothed 32-band spectrum, and the continuous beat clock.
#[derive(Debug, Default)]
pub struct AudioView {
    features: AudioFeatures,
    bands: [f32; BAND_COUNT],
    beat_clock: f32,
    tempo_locked: bool,
}

impl AudioView {
    /// Fold this frame's state in. `mel` is the raw 64-band column from
    /// `AudioSystem::latest_mel()`; it is empty until the first audio frame
    /// arrives, in which case the bands release toward silence.
    pub fn update(&mut self, dt: f32, features: &AudioFeatures, mel: &[f32]) {
        self.features = *features;
        // beat_index steps by 1 exactly when beat_phase wraps, so the sum is
        // a continuous monotonic clock (features.rs overlay-clock contract).
        self.beat_clock = features.beat_index + features.beat_phase;
        self.tempo_locked = features.beat_period_secs().is_some();

        let have_mel = mel.len() >= BAND_COUNT * 2;
        for (i, band) in self.bands.iter_mut().enumerate() {
            let target = if have_mel {
                (mel[2 * i] + mel[2 * i + 1]) * 0.5
            } else {
                0.0
            };
            // Asymmetric dt-correct one-pole, the audio/smoother.rs template.
            let tau = if target > *band {
                BAND_ATTACK
            } else {
                BAND_RELEASE
            };
            let alpha = 1.0 - (-dt / tau.max(0.001)).exp();
            *band += alpha * (target - *band);
        }
    }

    /// The signal value for a source, in 0..=1.
    pub fn signal(&self, feature: AudioFeature) -> f32 {
        let f = &self.features;
        match feature {
            AudioFeature::Rms => f.rms,
            AudioFeature::Onset => f.onset,
            AudioFeature::Bass => (f.sub_bass + f.bass) * 0.5,
            AudioFeature::Mid => (f.low_mid + f.mid + f.upper_mid) / 3.0,
            AudioFeature::High => (f.presence + f.brilliance) * 0.5,
            AudioFeature::Band(n) => self.bands[(n as usize).min(BAND_COUNT - 1)],
            AudioFeature::BeatPhase => f.beat_phase,
            AudioFeature::Bpm => f.bpm,
        }
    }

    /// Continuous musical time in beats (`beat_index + beat_phase`). Holds
    /// still through silence/pre-lock — the interp PLL freezes the phase, so
    /// BeatSync oscillators freeze with it rather than free-running on a
    /// made-up tempo that would snap when lock lands.
    pub fn beat_clock(&self) -> f32 {
        self.beat_clock
    }

    /// Whether the tempo detector has locked (`bpm > 0`). The inspector uses
    /// this to caption BeatSync rates ("waiting for tempo…").
    pub fn tempo_locked(&self) -> bool {
        self.tempo_locked
    }

    /// Seconds per beat at the detected tempo; `None` pre-lock. Time math
    /// only (Drift slew periods) — signals use the normalized `Bpm`.
    pub fn beat_period_secs(&self) -> Option<f32> {
        self.features.beat_period_secs()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mel64(f: impl Fn(usize) -> f32) -> Vec<f32> {
        (0..64).map(f).collect()
    }

    /// dt far above both taus → alpha ≈ 1, bands snap to their targets.
    const SNAP_DT: f32 = 10.0;

    #[test]
    fn bands_are_adjacent_pair_means_of_64_mel() {
        let mut view = AudioView::default();
        let mel = mel64(|i| i as f32);
        view.update(SNAP_DT, &AudioFeatures::default(), &mel);
        for (i, band) in view.bands.iter().enumerate() {
            let expected = (2 * i) as f32 + 0.5;
            assert!((band - expected).abs() < 1e-3, "band {i}: {band}");
        }
    }

    #[test]
    fn bands_empty_mel_stays_zero() {
        let mut view = AudioView::default();
        view.update(SNAP_DT, &AudioFeatures::default(), &[]);
        assert!(view.bands.iter().all(|b| *b == 0.0));
        // A short column (device mid-switch) is treated the same as empty.
        view.update(SNAP_DT, &AudioFeatures::default(), &[1.0; 10]);
        assert!(view.bands.iter().all(|b| *b == 0.0));
    }

    #[test]
    fn band_attack_faster_than_release() {
        let features = AudioFeatures::default();
        let mut view = AudioView::default();
        let loud = mel64(|_| 1.0);
        view.update(0.02, &features, &loud);
        let after_attack = view.bands[0];
        // One attack-tau step lands near 1 - 1/e.
        assert!(after_attack > 0.5, "attack too slow: {after_attack}");

        view.update(SNAP_DT, &features, &loud); // settle at 1.0
        view.update(0.02, &features, &mel64(|_| 0.0));
        let after_release = view.bands[0];
        // Same dt on the way down barely moves (release tau is 10× longer).
        assert!(after_release > 0.85, "release too fast: {after_release}");
    }

    #[test]
    fn band_smoothing_is_dt_correct() {
        let features = AudioFeatures::default();
        let loud = mel64(|_| 1.0);
        let mut two_steps = AudioView::default();
        two_steps.update(0.008, &features, &loud);
        two_steps.update(0.008, &features, &loud);
        let mut one_step = AudioView::default();
        one_step.update(0.016, &features, &loud);
        assert!(
            (two_steps.bands[0] - one_step.bands[0]).abs() < 1e-5,
            "{} vs {}",
            two_steps.bands[0],
            one_step.bands[0]
        );
    }

    #[test]
    fn signal_bass_mid_high_are_named_band_means() {
        let mut view = AudioView::default();
        let features = AudioFeatures {
            sub_bass: 0.2,
            bass: 0.4,
            low_mid: 0.3,
            mid: 0.6,
            upper_mid: 0.9,
            presence: 0.5,
            brilliance: 0.7,
            ..Default::default()
        };
        view.update(0.016, &features, &[]);
        assert!((view.signal(AudioFeature::Bass) - 0.3).abs() < 1e-6);
        assert!((view.signal(AudioFeature::Mid) - 0.6).abs() < 1e-6);
        assert!((view.signal(AudioFeature::High) - 0.6).abs() < 1e-6);
    }

    #[test]
    fn signal_band_index_clamps_to_31() {
        let mut view = AudioView::default();
        let mel = mel64(|i| if i >= 62 { 1.0 } else { 0.0 });
        view.update(SNAP_DT, &AudioFeatures::default(), &mel);
        assert!((view.signal(AudioFeature::Band(31)) - 1.0).abs() < 1e-3);
        assert_eq!(
            view.signal(AudioFeature::Band(200)),
            view.signal(AudioFeature::Band(31))
        );
    }

    #[test]
    fn signal_passthrough_rms_onset_beat_phase_bpm() {
        let mut view = AudioView::default();
        let features = AudioFeatures {
            rms: 0.42,
            onset: 0.9,
            beat_phase: 0.25,
            bpm: 0.4,
            ..Default::default()
        };
        view.update(0.016, &features, &[]);
        assert_eq!(view.signal(AudioFeature::Rms), 0.42);
        assert_eq!(view.signal(AudioFeature::Onset), 0.9);
        assert_eq!(view.signal(AudioFeature::BeatPhase), 0.25);
        assert_eq!(view.signal(AudioFeature::Bpm), 0.4);
    }

    #[test]
    fn beat_clock_is_index_plus_phase_and_lock_tracks_bpm() {
        let mut view = AudioView::default();
        let features = AudioFeatures {
            beat_index: 17.0,
            beat_phase: 0.75,
            bpm: 0.0,
            ..Default::default()
        };
        view.update(0.016, &features, &[]);
        assert!((view.beat_clock() - 17.75).abs() < 1e-6);
        assert!(!view.tempo_locked(), "bpm 0 must read unlocked");

        let locked = AudioFeatures {
            bpm: 120.0 / 300.0,
            ..features
        };
        view.update(0.016, &locked, &[]);
        assert!(view.tempo_locked());
    }
}
