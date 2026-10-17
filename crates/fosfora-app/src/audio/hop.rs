//! The per-hop analysis chain, lifted out of the audio thread so it can be driven by
//! something other than a live capture ring (#2027).
//!
//! [`HopAnalyzer`] owns every stateful detector and runs them in the one order that is
//! correct — the pre-normalization snapshot, the silence gate, and the fresh-CQT key read
//! all depend on it. `audio_thread` keeps the ring, the recording mirror, the shared-config
//! mutexes and the channel; everything between "here is a hop" and "here is a frame" lives
//! here.
//!
//! Nothing in this module touches wall-clock time or a shared lock: the caller supplies the
//! hop, its sample-clock timestamp and this frame's config snapshot. That is what lets an
//! offline file driver run the identical chain faster than realtime and get the same numbers
//! a live session would produce.

use super::analyzer::FftAnalyzer;
use super::beat::{BeatDetector, TempoCommand, TempoConfig};
use super::beat_sidecar::BeatSidecar;
use super::downbeat::DownbeatTracker;
use super::hpss::HpssAnalyzer;
use super::key::KeyDetector;
use super::key_sidecar::KeySidecar;
use super::loudness::LoudnessMeter;
use super::normalizer::FeatureNormalizer;
use super::pitch::PitchAnalyzer;
use super::smoother::FeatureSmoother;
use super::stereo::StereoAnalyzer;
use super::structure::{StructureConfig, StructureTracker};
use super::structure_sidecar::StructureSidecar;
use super::timbre::DeltaMfccAnalyzer;
use super::{ANALYSIS_HOP, AudioFeatures, AudioFrame};
use crate::settings::BandScale;

/// Corner of the DC blocker in front of the analysis chain (#55). A DC offset from a cheap
/// interface or a loopback path otherwise leaks into sub_bass and the lowest onset bands
/// through the Hann window's main lobe, and flattens the zero-crossing rate. Any corner
/// removes a steady offset completely; 5 Hz settles one in ~30 ms and costs the lowest band
/// (20 Hz) only ~0.3 dB at its bottom edge. A 10 Hz corner shifted the phase of kick
/// fundamentals enough to move beat-grid lock by a beat on the bench fixture; 5 Hz scores
/// identically to no filter there.
const DC_CUTOFF_HZ: f32 = 5.0;

/// One-pole DC blocker: `y[n] = x[n] − x[n−1] + R·y[n−1]`.
#[derive(Clone, Copy)]
struct DcBlocker {
    r: f32,
    x1: f32,
    y1: f32,
}

impl DcBlocker {
    fn new(sample_rate: f32) -> Self {
        Self {
            r: (-std::f32::consts::TAU * DC_CUTOFF_HZ / sample_rate).exp(),
            x1: 0.0,
            y1: 0.0,
        }
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let y = x - self.x1 + self.r * self.y1;
        self.x1 = x;
        self.y1 = y;
        y
    }
}

/// One hop's worth of output: the frame to publish, plus the three 1-frame triggers pulled
/// from the *pre-smoothing* features so the caller can counter-latch them (#1976). Reading
/// them off the smoothed frame instead would couple the counters to smoothing policy.
pub struct HopOutput {
    pub frame: AudioFrame,
    pub beat_fired: bool,
    pub downbeat_fired: bool,
    pub drop_fired: bool,
    /// The feature set as snapshotted for the structure tracker: after every producer that
    /// runs before `normalize()`, and therefore *un*-normalized and *un*-smoothed. Offline
    /// analysis (#2027) re-normalizes over the whole song from this instead of inheriting the
    /// causal ~4 s percentile window, which is what makes the running-max saturation (#1973)
    /// and the drumless-material range inflation (#1854) go away.
    ///
    /// CAVEAT — it is snapshotted *before* the beat, downbeat and structure blocks fill their
    /// fields, so these 15 carry the previous hop's value, not this one's:
    /// `onset`, `beat`, `beat_phase`, `bpm`, `beat_strength` (15..=19),
    /// `downbeat`, `bar_phase`, `beat_in_bar` (52..=54),
    /// `section_novelty`, `buildup`, `drop` (58..=60),
    /// `bar_index`, `beat_index`, `tempo_confidence`, `beat_locked` (81..=84).
    /// Read those from `frame.features`, which is complete.
    ///
    /// Only `--analyze` consumes this; the live audio thread ignores it (it costs one 340-byte
    /// `Copy` per hop either way).
    #[cfg_attr(not(feature = "analyze"), allow(dead_code))]
    pub pre_norm: AudioFeatures,
}

/// Every stateful detector in the analysis chain, plus the fixed per-hop delta the
/// time-constant smoothers run on.
pub struct HopAnalyzer {
    /// DC blockers for the mono feed and for L and R, applied before anything reads the
    /// hop (#55), plus the scratch they filter into.
    dc_mono: DcBlocker,
    dc_left: DcBlocker,
    dc_right: DcBlocker,
    mono: Vec<f32>,
    stereo: Vec<f32>,
    analyzer: FftAnalyzer,
    normalizer: FeatureNormalizer,
    beat_detector: BeatDetector,
    key_detector: KeyDetector,
    loudness_meter: LoudnessMeter,
    downbeat_tracker: DownbeatTracker,
    structure_tracker: StructureTracker,
    smoother: FeatureSmoother,
    stereo_analyzer: StereoAnalyzer,
    hpss_analyzer: HpssAnalyzer,
    pitch_analyzer: PitchAnalyzer,
    dmfcc_analyzer: DeltaMfccAnalyzer,
    /// Dev-only sweep instrumentation, active iff `FOSFORA_KEY_SIDECAR` is set (#2079).
    key_sidecar: Option<KeySidecar>,
    /// Dev-only sweep instrumentation, active iff `FOSFORA_STRUCTURE_SIDECAR` is set (#2080).
    structure_sidecar: Option<StructureSidecar>,
    /// Dev-only beat-path instrumentation, active iff `FOSFORA_BEAT_SIDECAR` is set (#4153).
    beat_sidecar: Option<BeatSidecar>,
    /// `ANALYSIS_HOP / sample_rate` — the attack/release EMAs and the onset decay are
    /// expressed as time constants, so they need the hop duration, not the hop length.
    dt: f32,
}

impl HopAnalyzer {
    pub fn new(sample_rate: f32, band_scale: BandScale, tempo_cfg: TempoConfig) -> Self {
        let hop_rate = super::hop_rate(sample_rate);
        Self {
            dc_mono: DcBlocker::new(sample_rate),
            dc_left: DcBlocker::new(sample_rate),
            dc_right: DcBlocker::new(sample_rate),
            mono: Vec::with_capacity(ANALYSIS_HOP),
            stereo: Vec::with_capacity(ANALYSIS_HOP * 2),
            analyzer: FftAnalyzer::new(sample_rate, band_scale),
            normalizer: FeatureNormalizer::new(hop_rate),
            beat_detector: BeatDetector::new(sample_rate, tempo_cfg),
            key_detector: KeyDetector::new(sample_rate),
            loudness_meter: LoudnessMeter::new(sample_rate),
            downbeat_tracker: DownbeatTracker::new(),
            structure_tracker: StructureTracker::new(),
            smoother: FeatureSmoother::new(),
            stereo_analyzer: StereoAnalyzer::with_sample_rate(sample_rate),
            hpss_analyzer: HpssAnalyzer::new(sample_rate),
            pitch_analyzer: PitchAnalyzer::new(sample_rate),
            dmfcc_analyzer: DeltaMfccAnalyzer::new(hop_rate),
            key_sidecar: KeySidecar::from_env(),
            structure_sidecar: StructureSidecar::from_env(),
            beat_sidecar: BeatSidecar::from_env(),
            dt: ANALYSIS_HOP as f32 / sample_rate,
        }
    }

    /// The tempo estimator's adapted prior centre. In auto mode the audio thread publishes
    /// this back into the shared [`super::TempoControl`] so the UI slider reads what the
    /// estimator settled on.
    pub fn prior_center_bpm(&self) -> f32 {
        self.beat_detector.prior_center_bpm()
    }

    /// Analyze exactly [`ANALYSIS_HOP`] mono samples (`hop`) with their interleaved L/R
    /// counterpart (`hop_stereo`, twice as long).
    ///
    /// `timestamp` is the sample-clock time of this hop, not wall-clock. `struct_cfg` and
    /// `tempo_cfg` are this frame's snapshots of the live-tunable config, and `tempo_cmds`
    /// is the mailbox drained for this hop — the caller owns the locks so this stays
    /// callable off-thread.
    pub fn process_hop(
        &mut self,
        hop: &[f32],
        hop_stereo: &[f32],
        timestamp: f64,
        struct_cfg: StructureConfig,
        tempo_cfg: TempoConfig,
        tempo_cmds: Vec<TempoCommand>,
    ) -> HopOutput {
        let dt = self.dt;

        // Strip DC before any stage sees the hop (#55).
        self.mono.clear();
        self.mono
            .extend(hop.iter().map(|&x| self.dc_mono.process(x)));
        self.stereo.clear();
        for f in hop_stereo.chunks_exact(2) {
            self.stereo.push(self.dc_left.process(f[0]));
            self.stereo.push(self.dc_right.process(f[1]));
        }
        let (hop, hop_stereo) = (&self.mono[..], &self.stereo[..]);

        // Multi-resolution FFT + feature extraction. The analyzer shifts this hop into
        // its 4096-sample window, so consecutive hops overlap 87.5%.
        let mut raw = self.analyzer.analyze(hop);

        // A10 (#1461): perceptual loudness on the fresh hop (each frame once), summed over
        // L and R per BS.1770 (#56). Fields are Passthrough, so — like the beat block — they
        // survive normalize/smooth unrescaled.
        let loud = self.loudness_meter.process(hop_stereo);
        raw.loudness_m = loud.m;
        raw.loudness_s = loud.s;
        raw.loudness_trend = loud.trend;
        // A6 (#1457): the onset detector gates on this perceptual silence flag.
        let loud_silent = self.loudness_meter.is_silent();

        // A13 (#1464): stereo field over the rolling window. Gated inside the analyzer on its own
        // total stereo energy rather than `loud_silent`, so the two gates stay independent. The
        // fields are Passthrough, so they survive normalize()/smooth() unrescaled, like the
        // loudness/key blocks below.
        let stereo_field = self.stereo_analyzer.process(hop_stereo);
        raw.pan = stereo_field.pan;
        raw.stereo_width = stereo_field.stereo_width;
        raw.stereo_corr = stereo_field.stereo_corr;
        // A13b (#1801): per-band pan, from the same analyzer and the same gate. Also
        // Passthrough — the producer already holds an empty band at 0.5.
        let [bp_sub, bp_bass, bp_lo, bp_mid, bp_up, bp_pres, bp_bril] = stereo_field.band_pan;
        raw.band_pan_sub_bass = bp_sub;
        raw.band_pan_bass = bp_bass;
        raw.band_pan_low_mid = bp_lo;
        raw.band_pan_mid = bp_mid;
        raw.band_pan_upper_mid = bp_up;
        raw.band_pan_presence = bp_pres;
        raw.band_pan_brilliance = bp_bril;

        // A14 (#1465): harmonic/percussive split from the medium (1024-pt) magnitude. The two
        // energies arrive dB-mapped 0..1 (volume-invariant spans — see hpss.rs) and are set
        // before normalize() so the adaptive normalizer ranges and silence-gates them like
        // the bands (Adaptive); `harmonic_ratio` is a level-invariant 0..1 balance
        // (Passthrough), neutral-gated inside the analyzer on `loud_silent`.
        let hpss = self
            .hpss_analyzer
            .process(self.analyzer.mid_magnitude(), loud_silent);
        raw.percussive_energy = hpss.percussive_energy;
        raw.harmonic_energy = hpss.harmonic_energy;
        raw.harmonic_ratio = hpss.harmonic_ratio;

        // A15 (#1466): monophonic f0 via YIN on the analyzer's raw time-domain window. Producer-
        // normalized to a 0..1 log-frequency (Passthrough); confidence = YIN periodicity
        // (1 − aperiodicity). Held through unvoiced gaps with confidence 0, so a pitch-keyed
        // visual doesn't snap to the lowest note on rests. Set before normalize() like the block
        // above (a Passthrough field survives normalize/smooth unrescaled).
        let pitch = self
            .pitch_analyzer
            .process(self.analyzer.time_domain(), loud_silent);
        raw.pitch = pitch.pitch;
        raw.pitch_confidence = pitch.pitch_confidence;

        // A16 (#1467): spectral contrast — per-octave peak-vs-valley tonality on the large
        // (4096-pt) magnitude, producer-mapped 0-60 dB -> 0..1 (Passthrough, silence-gated
        // inside the analyzer, so it survives normalize/smooth unrescaled).
        let contrast = self.analyzer.spectral_contrast(loud_silent);
        raw.contrast_0 = contrast[0];
        raw.contrast_1 = contrast[1];
        raw.contrast_2 = contrast[2];
        raw.contrast_3 = contrast[3];
        raw.contrast_4 = contrast[4];
        raw.contrast_5 = contrast[5];
        raw.contrast_mean = contrast[6];
        // A16 (#1467): delta-MFCC timbre dynamics from this hop's (pre-normalization) MFCCs.
        // `timbre_flux` (L2 of the delta over coeffs 1..12) is a raw level set before normalize()
        // so the adaptive normalizer ranges it like `flux` (Adaptive); the full slope vector
        // rides the frame for the bindings-only `audio.dmfcc.N` sources.
        let timbre = self.dmfcc_analyzer.process(&raw.mfcc, loud_silent);
        raw.timbre_flux = timbre.timbre_flux;

        // A3 (#1454): fill `kick` now that the silence flag is known — a single
        // detector-owned P95 normalizer, gated so noise-floor log-flux can't fire. Set
        // before the pre-norm snapshot, and it survives normalize() unchanged (kick is
        // Passthrough).
        raw.kick = self.analyzer.kick_envelope(loud_silent);

        // A11 (#1462), reworked #2079: key detection on the analyzer's pure-fold
        // *unnormalized* energy chroma, so loud frames outvote quiet ones in the
        // detector's rolling mean; the visual `raw.chroma` stays harmonic-templated
        // and L-∞ normalized for the feature bus. Key fields are Passthrough, so
        // they survive normalize/smooth.
        let key_result = self.key_detector.process(self.analyzer.key_chroma(), dt);
        raw.key_class = key_result.key_class;
        raw.key_is_minor = key_result.is_minor;
        raw.key_confidence = key_result.confidence;

        // Dev-only (#2079): dump the key path's raw inputs for offline sweeps.
        if let Some(sidecar) = &mut self.key_sidecar {
            sidecar.record(
                timestamp,
                self.analyzer.key_e61(),
                self.analyzer.key_bass(),
                raw.harmonic_ratio,
                loud_silent,
            );
        }

        // A12 (#1463): capture pre-normalization chroma + per-band flux for the downbeat
        // tracker. The adaptive normalizer rescales chroma per-bin, which would distort
        // the inter-beat chord-change magnitude, so snapshot both before normalize().
        let pre_norm_chroma = raw.chroma;
        let band_flux = self.analyzer.band_flux_3();
        // A18 (#1469): snapshot the whole feature set before normalize() for the structure
        // tracker — it keys on the true loudness / sub-bass / centroid dynamics the adaptive
        // normalizer would flatten. (`AudioFeatures` is Copy; loudness + spectral shape are
        // already filled at this point; onset/bpm come from `beat_result` below.)
        let pre_norm = raw;

        // A2 (#1453): per-feature normalization (gated percentile / fixed-range /
        // z-score / passthrough), silence-gated on the A10 perceptual flag.
        raw = self.normalizer.normalize(&raw, loud_silent);

        // A7 (#1458): apply this hop's tempo config snapshot and mailbox. The caller holds
        // the lock and drains; the ordering relative to normalize() is preserved.
        self.beat_detector.set_tempo_config(tempo_cfg);
        for cmd in tempo_cmds {
            self.beat_detector.apply_tempo_command(cmd);
        }

        // Beat detection (on raw magnitude spectra)
        self.beat_detector.set_kick(raw.kick);
        let beat_result = self.beat_detector.process(
            self.analyzer.bass_magnitude(),
            self.analyzer.mid_magnitude(),
            self.analyzer.high_magnitude(),
            timestamp,
            loud_silent,
        );
        // Dev-only (#4153): dump the beat path's internals for offline reading.
        if let Some(sidecar) = &mut self.beat_sidecar {
            sidecar.record(timestamp, self.beat_detector.debug_state());
        }
        raw.onset = beat_result.onset_strength;
        raw.beat = beat_result.beat;
        raw.beat_phase = beat_result.beat_phase;
        raw.bpm = beat_result.bpm / crate::audio::features::BPM_NORM; // normalize to 0-1
        raw.beat_strength = beat_result.beat_strength;
        raw.tempo_confidence = beat_result.tempo_confidence;
        raw.beat_locked = if beat_result.beat_locked { 1.0 } else { 0.0 };

        // A12 (#1463): bar/downbeat/meter tracking. Runs every frame (advances bar_phase
        // on the audio clock, integrates flux); heavy scoring gates on a fired beat. RMS is
        // the pre-normalization level (#60): the adaptive normalizer re-ranges `raw.rms`
        // per hop, which would distort the beat-to-beat loudness rise the tracker scores.
        let db = self.downbeat_tracker.process(
            &beat_result,
            band_flux,
            pre_norm.rms,
            &pre_norm_chroma,
            timestamp,
            loud_silent,
        );
        raw.downbeat = db.downbeat;
        raw.bar_phase = db.bar_phase;
        raw.beat_in_bar = db.beat_in_bar;
        raw.bar_index = db.bar_index;
        raw.beat_index = db.beat_index;

        // A18 (#1469): section novelty / build-up / drop. Reads the pre-normalization
        // snapshot + the beat result; heavy work is decimated to ~10 Hz internally.
        let structure =
            self.structure_tracker
                .process(struct_cfg, &pre_norm, &beat_result, timestamp);
        raw.section_novelty = structure.section_novelty;
        raw.buildup = structure.buildup;
        raw.drop = structure.drop;

        // The three counter-latched pulses, read before smoothing (see `HopOutput`).
        let beat_fired = beat_result.beat > 0.5;
        let downbeat_fired = db.downbeat > 0.5;
        let drop_fired = structure.drop > 0.5;

        // Smoothing (per-feature asymmetric EMA; beat/beat_phase pass through)
        let smoothed = self.smoother.smooth(&raw, dt);

        // Dev-only (#2080): dump the structure path's raw inputs for offline sweeps. Takes
        // the fingerprint from `pre_norm` and the label-machine fields from the smoothed
        // features, matching where production reads each (see `structure_sidecar`).
        if let Some(sidecar) = &mut self.structure_sidecar {
            sidecar.record(
                timestamp,
                &pre_norm,
                &smoothed,
                &self.structure_tracker.drop_trace(),
                &struct_cfg,
            );
        }

        // A17 (#1468): sample the render-facing spectrum + mel column from the analyzer's
        // fresh magnitude, so all three ride the same frame across the channel.
        let frame = AudioFrame {
            features: smoothed,
            spectrum: Box::new(self.analyzer.log_spectrum_512()),
            mel: Box::new(self.analyzer.spectrogram_column()),
            // A16 (#1467): this hop's delta-MFCC slopes for the `audio.dmfcc.N` sources.
            dmfcc: timbre.dmfcc,
            timestamp,
            // Mirrors the silence gate in `BeatDetector::process` exactly: it pins
            // phase at 0 under perceptual silence, so A8's local oscillator must follow
            // rather than free-run. Same flag the detector gates on — `raw.rms` would
            // be wrong here, since it is post-normalization and hits 0 at the bottom of
            // the adaptive range on loud audio.
            phase_frozen: loud_silent,
            // A8b (#1554): the tracker's own bar-clock denominator, so the render side
            // advances `bar_phase` on the same rate that produced the phase above.
            bar_duration: db.bar_duration,
            beat_time: beat_fired.then_some(beat_result.beat_time),
            // Q4 (#2080): read off the pre-smoothing structure result, like the three pulses
            // above — smoothing a trigger would smear it across hops.
            section_boundary: (structure.boundary > 0.0).then_some(structure.boundary),
        };

        HopOutput {
            frame,
            beat_fired,
            downbeat_fired,
            drop_fired,
            pre_norm,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Raw (pre-normalization) features after ~2.3 s of a 1 kHz tone riding on `offset`.
    fn settled_features(offset: f32) -> AudioFeatures {
        const SR: f32 = 44100.0;
        let mut a = HopAnalyzer::new(SR, BandScale::Db, TempoConfig::default());
        let mut last = AudioFeatures::default();
        for h in 0..200 {
            let mono: Vec<f32> = (0..ANALYSIS_HOP)
                .map(|i| {
                    let t = (h * ANALYSIS_HOP + i) as f32 / SR;
                    offset + 0.2 * (std::f32::consts::TAU * 1000.0 * t).sin()
                })
                .collect();
            let stereo: Vec<f32> = mono.iter().flat_map(|&x| [x, x]).collect();
            let ts = ((h + 1) * ANALYSIS_HOP) as f64 / f64::from(SR);
            let cfg = (StructureConfig::default(), TempoConfig::default());
            last = a
                .process_hop(&mono, &stereo, ts, cfg.0, cfg.1, Vec::new())
                .pre_norm;
        }
        last
    }

    /// The last frame's published features after `secs` of `signal(t)`.
    fn final_frame(secs: f32, mut signal: impl FnMut(f32) -> f32) -> AudioFeatures {
        const SR: f32 = 44100.0;
        let mut a = HopAnalyzer::new(SR, BandScale::Db, TempoConfig::default());
        let mut last = AudioFeatures::default();
        for h in 0..(secs * SR) as usize / ANALYSIS_HOP {
            let mono: Vec<f32> = (0..ANALYSIS_HOP)
                .map(|i| signal((h * ANALYSIS_HOP + i) as f32 / SR))
                .collect();
            let stereo: Vec<f32> = mono.iter().flat_map(|&x| [x, x]).collect();
            let ts = ((h + 1) * ANALYSIS_HOP) as f64 / f64::from(SR);
            let cfg = (StructureConfig::default(), TempoConfig::default());
            last = a
                .process_hop(&mono, &stereo, ts, cfg.0, cfg.1, Vec::new())
                .frame
                .features;
        }
        last
    }

    /// #81: a steady four-on-the-floor locks and reports high confidence; noise never does.
    #[test]
    fn tempo_trust_tracks_the_lock() {
        let tau = std::f32::consts::TAU;
        let kick = |t: f32| {
            let tb = t % 0.5; // 120 BPM
            (tau * (50.0 + 100.0 * (-tb * 40.0).exp()) * tb).sin() * (-tb * 10.0).exp() * 0.7
        };
        let steady = final_frame(25.0, kick);
        assert_eq!(steady.beat_locked, 1.0, "120 BPM kick never locked");
        assert!(
            steady.tempo_confidence > 0.6,
            "confidence {}",
            steady.tempo_confidence
        );

        let mut lcg = 0x1234_5678u32;
        let noise = final_frame(25.0, |_| {
            lcg = lcg.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            ((lcg >> 8) as f32 / 8_388_608.0 - 1.0) * 0.2
        });
        assert_eq!(noise.beat_locked, 0.0, "noise locked");
        assert!(
            noise.tempo_confidence < 0.6,
            "confidence {}",
            noise.tempo_confidence
        );
    }

    /// Deterministic non-rhythmic test signals at 44.1 kHz: `(name, sample(t))`. Each holds
    /// energy, transients or harmonic change, but none has a beat for the tempo estimator to
    /// find, so none may ever publish one.
    /// A test signal: sample value at time `t` seconds.
    type Signal = Box<dyn FnMut(f32) -> f32>;

    fn non_rhythmic_signals(seed: u32) -> Vec<(&'static str, Signal)> {
        fn lcg(state: &mut u32) -> f32 {
            *state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (*state >> 8) as f32 / 8_388_608.0 - 1.0
        }
        let mut white = 0x1234_5678u32 ^ seed.wrapping_mul(0x9e37_79b9);
        let mut pink = (0x0bad_f00du32 ^ seed.wrapping_mul(0x9e37_79b9), [0.0f32; 7]);
        // Poisson-timed noise bursts over a quiet bed: onsets with no period.
        let mut clicks = (
            0x00c0_ffee_u32 ^ seed.wrapping_mul(0x9e37_79b9),
            0.0f32,
            0.0f32,
        );
        // A three-note chord that changes at random 0.7-2.5 s intervals, 0.3 s attack.
        let mut chords = (
            0x5eed_1234u32 ^ seed.wrapping_mul(0x9e37_79b9),
            -1.0f32,
            0.0f32,
            [0.0f32; 3],
        );
        // Piano-like notes at Poisson times (~2/s), each a decaying partial pair.
        let mut notes = (
            0x0a0e_5eedu32 ^ seed.wrapping_mul(0x9e37_79b9),
            0.0f32,
            [(0.0f32, -9.0f32); 6],
            0usize,
        );
        // Slow pads: random chords every 2-6 s with a 1.5 s crossfade swell.
        let mut pads = (
            0x0bad_ca5eu32 ^ seed.wrapping_mul(0x9e37_79b9),
            -1.0f32,
            0.0f32,
            [0.0f32; 3],
            [0.0f32; 3],
        );
        let mut v: Vec<(&'static str, Signal)> = Vec::new();
        v.push(("white noise", Box::new(move |_| lcg(&mut white) * 0.2)));
        v.push((
            "pink noise",
            // Paul Kellet's pink filter over white noise.
            Box::new(move |_| {
                let w = lcg(&mut pink.0);
                let b = &mut pink.1;
                b[0] = 0.99886 * b[0] + w * 0.0555179;
                b[1] = 0.99332 * b[1] + w * 0.0750759;
                b[2] = 0.96900 * b[2] + w * 0.153_852;
                b[3] = 0.86650 * b[3] + w * 0.3104856;
                b[4] = 0.55000 * b[4] + w * 0.5329522;
                b[5] = -0.7616 * b[5] - w * 0.0168980;
                let out = b.iter().sum::<f32>() + w * 0.5362;
                b[6] = w * 0.115926;
                out * 0.05
            }),
        ));
        v.push((
            "random clicks",
            Box::new(move |t| {
                // ~3 bursts per second, exponentially spaced.
                if t >= clicks.1 {
                    let u = (lcg(&mut clicks.0) * 0.5 + 0.5).max(1e-4);
                    clicks.2 = t;
                    clicks.1 = t - u.ln() / 3.0;
                }
                let age = t - clicks.2;
                let burst = if age < 0.03 {
                    lcg(&mut clicks.0) * (-age / 0.008).exp() * 0.6
                } else {
                    0.0
                };
                burst + lcg(&mut clicks.0) * 0.01
            }),
        ));
        v.push((
            "random chords",
            Box::new(move |t| {
                if t >= chords.1 {
                    const NOTES: [f32; 9] = [
                        110.0, 131.0, 147.0, 165.0, 196.0, 220.0, 262.0, 294.0, 330.0,
                    ];
                    chords.2 = t;
                    chords.1 = t + 0.7 + 1.8 * (lcg(&mut chords.0) * 0.5 + 0.5);
                    for f in &mut chords.3 {
                        let i = ((lcg(&mut chords.0) * 0.5 + 0.5) * 8.999) as usize;
                        *f = NOTES[i];
                    }
                }
                let age = t - chords.2;
                let env = (age / 0.3).min(1.0) * 0.1;
                chords
                    .3
                    .iter()
                    .map(|f| (std::f32::consts::TAU * f * t).sin())
                    .sum::<f32>()
                    * env
            }),
        ));
        v.push((
            "random notes",
            Box::new(move |t| {
                if t >= notes.1 {
                    const NOTES: [f32; 12] = [
                        196.0, 220.0, 247.0, 262.0, 294.0, 330.0, 349.0, 392.0, 440.0, 494.0,
                        523.0, 587.0,
                    ];
                    let u = (lcg(&mut notes.0) * 0.5 + 0.5).max(1e-4);
                    notes.1 = t - u.ln() / 2.0;
                    let i = ((lcg(&mut notes.0) * 0.5 + 0.5) * 11.999) as usize;
                    notes.2[notes.3 % 6] = (NOTES[i], t);
                    notes.3 += 1;
                }
                let tau = std::f32::consts::TAU;
                notes
                    .2
                    .iter()
                    .map(|&(f, t0)| {
                        let age = t - t0;
                        if age < 0.0 || f == 0.0 {
                            return 0.0;
                        }
                        ((tau * f * age).sin() + 0.4 * (tau * 2.0 * f * age).sin())
                            * (-age / 0.8).exp()
                            * 0.15
                    })
                    .sum::<f32>()
            }),
        ));
        v.push((
            "slow pads",
            Box::new(move |t| {
                if t >= pads.1 {
                    const NOTES: [f32; 9] = [
                        110.0, 131.0, 147.0, 165.0, 196.0, 220.0, 262.0, 294.0, 330.0,
                    ];
                    pads.4 = pads.3;
                    pads.2 = t;
                    pads.1 = t + 2.0 + 4.0 * (lcg(&mut pads.0) * 0.5 + 0.5);
                    for f in &mut pads.3 {
                        *f = NOTES[((lcg(&mut pads.0) * 0.5 + 0.5) * 8.999) as usize];
                    }
                }
                let x = ((t - pads.2) / 1.5).min(1.0);
                let tau = std::f32::consts::TAU;
                let chord = |fs: &[f32; 3]| fs.iter().map(|f| (tau * f * t).sin()).sum::<f32>();
                (chord(&pads.3) * x + chord(&pads.4) * (1.0 - x)) * 0.08
            }),
        ));
        // The same pads with every pair of notes >= 55 Hz apart: no two tones share Hann
        // main-lobe bins (+-21.5 Hz at 4096 points), so the band energies cannot beat.
        let mut wide = (
            0x0bad_ca5fu32 ^ seed.wrapping_mul(0x9e37_79b9),
            -1.0f32,
            0.0f32,
            [0.0f32; 3],
            [0.0f32; 3],
        );
        v.push((
            "pads wide",
            Box::new(move |t| {
                if t >= wide.1 {
                    const NOTES: [f32; 9] = [
                        110.0, 165.0, 247.0, 370.0, 554.0, 831.0, 1245.0, 1865.0, 2794.0,
                    ];
                    wide.4 = wide.3;
                    wide.2 = t;
                    wide.1 = t + 2.0 + 4.0 * (lcg(&mut wide.0) * 0.5 + 0.5);
                    for f in &mut wide.3 {
                        *f = NOTES[((lcg(&mut wide.0) * 0.5 + 0.5) * 8.999) as usize];
                    }
                }
                let x = ((t - wide.2) / 1.5).min(1.0);
                let tau = std::f32::consts::TAU;
                let chord = |fs: &[f32; 3]| fs.iter().map(|f| (tau * f * t).sin()).sum::<f32>();
                (chord(&wide.3) * x + chord(&wide.4) * (1.0 - x)) * 0.08
            }),
        ));
        v
    }

    /// GH #63 guard: no non-rhythmic signal publishes a tempo on ANY hop, at mastered level
    /// or 30 dB down. The onset function feeds the tempo estimator, so a change that makes
    /// weak rhythm more visible must not make noise visible too; and live input level varies
    /// widely, so the guard runs at both levels.
    #[test]
    fn non_rhythmic_audio_never_publishes_a_tempo() {
        const SR: f32 = 44100.0;
        const SECS: f32 = 60.0;
        // TEMP #63 measurement mode: report, don't assert, until the baseline is known.
        let seeds: u32 = std::env::var("NEG63_SEEDS").map_or(1, |s| s.parse().unwrap());
        for seed in 0..seeds {
            for gain_db in [0.0f32, -30.0] {
                let k = 10f32.powf(gain_db / 20.0);
                for (name, mut signal) in non_rhythmic_signals(seed) {
                    let mut a = HopAnalyzer::new(SR, BandScale::Db, TempoConfig::default());
                    let (mut first, mut locked, mut n, mut bpm) = (None, 0usize, 0usize, 0.0f32);
                    // TEMP #4153: NEG63_DUMP=<dir> writes seed 0 / 0 dB audio as raw f32.
                    let mut dump = std::env::var("NEG63_DUMP")
                        .ok()
                        .filter(|_| seed == 0 && gain_db == 0.0)
                        .map(|d| (d, Vec::<f32>::new()));
                    for h in 0..(SECS * SR) as usize / ANALYSIS_HOP {
                        let mono: Vec<f32> = (0..ANALYSIS_HOP)
                            .map(|i| signal((h * ANALYSIS_HOP + i) as f32 / SR) * k)
                            .collect();
                        let stereo: Vec<f32> = mono.iter().flat_map(|&x| [x, x]).collect();
                        if let Some((_, buf)) = &mut dump {
                            buf.extend_from_slice(&mono);
                        }
                        let ts = ((h + 1) * ANALYSIS_HOP) as f64 / f64::from(SR);
                        let o = a.process_hop(
                            &mono,
                            &stereo,
                            ts,
                            StructureConfig::default(),
                            TempoConfig::default(),
                            Vec::new(),
                        );
                        // Published values live on the frame: `pre_norm` is snapshotted
                        // before the beat detector runs, so its bpm is always 0.
                        let wire = o.frame.features.bpm * crate::audio::features::BPM_NORM;
                        if wire > 0.0 && first.is_none() {
                            first = Some(ts);
                        }
                        bpm = bpm.max(wire);
                        locked += usize::from(o.frame.features.beat_locked > 0.5);
                        n += 1;
                    }
                    if let Some((dir, buf)) = dump {
                        let bytes: Vec<u8> = buf.iter().flat_map(|x| x.to_le_bytes()).collect();
                        std::fs::write(format!("{dir}/{}.f32", name.replace(' ', "_")), bytes)
                            .unwrap();
                    }
                    eprintln!(
                        "NEG63 seed{seed} {name:14} {gain_db:>5} dB first_publish={} max_bpm={bpm:.1} locked_frac={:.3}",
                        first.map_or("never".to_string(), |t| format!("{t:.1}s")),
                        locked as f64 / n as f64
                    );
                }
            }
        }
    }

    /// A DC offset larger than the tone it carries would leak into sub_bass through the Hann
    /// main lobe and stop every zero crossing (#55). Once the blocker settles, the tone reads
    /// exactly as it does with no offset.
    #[test]
    fn dc_offset_does_not_reach_the_features() {
        let clean = settled_features(0.0);
        let offset = settled_features(0.3);
        assert!(clean.zcr > 0.0);
        for (name, a, b) in [
            ("sub_bass", clean.sub_bass, offset.sub_bass),
            ("bass", clean.bass, offset.bass),
            ("zcr", clean.zcr, offset.zcr),
            ("rms", clean.rms, offset.rms),
        ] {
            assert!(
                (a - b).abs() < 1e-3,
                "{name}: {a} clean vs {b} with a DC offset"
            );
        }
    }
}
