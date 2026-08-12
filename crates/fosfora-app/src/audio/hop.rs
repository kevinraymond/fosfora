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
    /// fields, so these 13 carry the previous hop's value, not this one's:
    /// `onset`, `beat`, `beat_phase`, `bpm`, `beat_strength` (15..=19),
    /// `downbeat`, `bar_phase`, `beat_in_bar` (52..=54),
    /// `section_novelty`, `buildup`, `drop` (58..=60).
    /// Read those from `frame.features`, which is complete.
    ///
    /// Only `--analyze` consumes this; the live audio thread ignores it (it costs one 324-byte
    /// `Copy` per hop either way).
    #[cfg_attr(not(feature = "analyze"), allow(dead_code))]
    pub pre_norm: AudioFeatures,
}

/// Every stateful detector in the analysis chain, plus the fixed per-hop delta the
/// time-constant smoothers run on.
pub struct HopAnalyzer {
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
    /// `ANALYSIS_HOP / sample_rate` — the attack/release EMAs and the onset decay are
    /// expressed as time constants, so they need the hop duration, not the hop length.
    dt: f32,
}

impl HopAnalyzer {
    pub fn new(sample_rate: f32, band_scale: BandScale, tempo_cfg: TempoConfig) -> Self {
        Self {
            analyzer: FftAnalyzer::new(sample_rate, band_scale),
            normalizer: FeatureNormalizer::new(),
            beat_detector: BeatDetector::new(sample_rate, tempo_cfg),
            key_detector: KeyDetector::new(sample_rate),
            loudness_meter: LoudnessMeter::new(sample_rate),
            downbeat_tracker: DownbeatTracker::new(),
            structure_tracker: StructureTracker::new(sample_rate / ANALYSIS_HOP as f32),
            smoother: FeatureSmoother::new(),
            stereo_analyzer: StereoAnalyzer::new(),
            hpss_analyzer: HpssAnalyzer::new(),
            pitch_analyzer: PitchAnalyzer::new(sample_rate),
            dmfcc_analyzer: DeltaMfccAnalyzer::new(),
            key_sidecar: KeySidecar::from_env(),
            structure_sidecar: StructureSidecar::from_env(),
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

        // Multi-resolution FFT + feature extraction. The analyzer shifts this hop into
        // its 4096-sample window, so consecutive hops overlap 87.5%.
        let mut raw = self.analyzer.analyze(hop);

        // A10 (#1461): perceptual loudness on the fresh hop (each sample once). Fields
        // are Passthrough, so — like the beat block — they survive normalize/smooth
        // unrescaled.
        let loud = self.loudness_meter.process(hop);
        raw.loudness_m = loud.m;
        raw.loudness_s = loud.s;
        raw.loudness_trend = loud.trend;
        // A6 (#1457): the onset detector gates on this perceptual silence flag.
        let loud_silent = self.loudness_meter.is_silent();

        // A13 (#1464): stereo field over the rolling window. Gated inside the analyzer on total
        // stereo energy — NOT the mono `loud_silent` flag, which a fully anti-phase (maximally
        // wide) signal would trip by cancelling to mono silence. The fields are Passthrough, so
        // they survive normalize()/smooth() unrescaled, like the loudness/key blocks below.
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
        let beat_result = self.beat_detector.process(
            self.analyzer.bass_magnitude(),
            self.analyzer.mid_magnitude(),
            self.analyzer.high_magnitude(),
            timestamp,
            loud_silent,
        );
        raw.onset = beat_result.onset_strength;
        raw.beat = beat_result.beat;
        raw.beat_phase = beat_result.beat_phase;
        raw.bpm = beat_result.bpm / crate::audio::features::BPM_NORM; // normalize to 0-1
        raw.beat_strength = beat_result.beat_strength;

        // A12 (#1463): bar/downbeat/meter tracking. Runs every frame (advances bar_phase
        // on the audio clock, integrates flux); heavy scoring gates on a fired beat.
        let db = self.downbeat_tracker.process(
            &beat_result,
            band_flux,
            raw.rms,
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
