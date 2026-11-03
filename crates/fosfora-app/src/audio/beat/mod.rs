//! 3-stage beat detection pipeline: OnsetDetector → TempoEstimator → BeatScheduler.
//! Ported from easey-glyph's Python implementation.

use super::beat_model::{BeatActivation, PeakPicker, RawEvidence};
use super::features::AudioFeatures;
use super::kick_model::N_FRAME;

mod buffer;
mod control;
mod kalman;
mod onset;
mod scheduler;
mod tempo;

pub use self::control::{
    BPM_MAX, BPM_MIN, SYNC_OFFSET_MAX_MS, SYNC_OFFSET_MIN_MS, TempoCommand, TempoConfig,
    TempoControl, TempoPreset,
};
pub use self::onset::ONSET_FLUX_NAMES;
use self::onset::OnsetDetector;
pub(in crate::audio) use self::scheduler::PHASE_BELOW_ONE;
use self::scheduler::{ARB_ACTIVATION_THRESHOLD, ARB_EVIDENCE, BeatScheduler, PHASE_ARBITER};
pub use self::tempo::TempoDebug;
use self::tempo::TempoEstimator;

// ---------------------------------------------------------------------------
// Main BeatDetector facade
// ---------------------------------------------------------------------------

/// Result from beat detection for one frame.
#[derive(Clone, Copy, Debug)]
pub struct BeatResult {
    pub onset_strength: f32,
    /// #4163: P(a beat landed `BeatActivation::lag_hops()` hops ago), from the tree
    /// ensemble over this hop's raw evidence. Evidence for the grid, not a beat.
    pub beat_activation: f32,
    pub beat: f32,
    pub beat_phase: f32,
    pub bpm: f32,
    pub beat_strength: f32,
    /// The beat's event time on the sample clock, seconds. Only meaningful when
    /// `beat > 0.5`. The PLL scheduler fires grid beats at their predicted
    /// instants, so this sits at or (by up to one hop + one correction step)
    /// before the hop timestamp that emits it.
    pub beat_time: f64,
    /// #4473: `Some(that beat's emitted time)` on the hop a beat's instant settles — the
    /// beat the scheduler emitted `BEAT_LEAD_SECS` earlier, or an acquisition fire on its
    /// own hop. The per-beat trackers (downbeat, structure) key on this, not on `beat`,
    /// so their evidence windows and bar clock end where they did before the lead.
    pub settled_beat_time: Option<f64>,
    /// The scheduler's grid index of this beat — counts every grid instant,
    /// including MUTED ones, so bar rotation downstream stays phase-coherent
    /// through breakdown mutes and unlocked gaps. Meaningful when `beat > 0.5`.
    pub beat_index: u64,
    /// How much recent tempo evidence agrees with the tracked tempo, 0..1: the estimator's
    /// lock support EMA (#81). Not `beat_strength`, which is onset strength.
    pub tempo_confidence: f32,
    /// Whether the estimator holds a tempo lock (support crossed the enter threshold and
    /// has not fallen below the exit one) — the grid is worth trusting (#81).
    pub beat_locked: bool,
}

/// One phase arbiter vote at a grid instant, for the dev-only beat sidecar (#4234):
/// the (on the grid, on the half-beat, all) event counts of both windows, whatever the
/// vote read (the shipped rule only folds the long window once the short one votes).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArbVote {
    pub now: f64,
    pub period: f64,
    pub short: (usize, usize, usize),
    pub long: (usize, usize, usize),
    /// #4402: the (1/4, 3/4) bins of both windows (zero unless the quarters are on).
    pub quarter: (usize, usize),
    pub lquarter: (usize, usize),
    /// Whether this instant voted to move, the streak after it, whether the grid moved and
    /// by how many periods (0.0 when it did not).
    pub vote: bool,
    pub streak: u32,
    pub shift: bool,
    pub offset: f64,
}

/// One hop's view of the detector's internals for the dev-only beat sidecar
/// (#4153). `events` are the state transitions since the previous call, in
/// processing order (estimator first, then scheduler), as (name, extra) where
/// `extra` is the onset strength for the fires that carry one and NaN otherwise.
pub struct BeatDebug {
    /// Whether the tempo estimator ran its autocorrelation this hop.
    pub ran: bool,
    pub tempo: TempoDebug,
    pub mode: &'static str,
    /// Fraction of the last 8 grid instants with an in-window onset / a kick.
    pub on_sup: f64,
    pub kick_sup: f64,
    pub grid_n: u64,
    pub events: Vec<(&'static str, f64)>,
    /// #4234: the events pushed into the arbiter's window this hop ("kick" / "act", time),
    /// and its votes.
    pub arb_events: Vec<(&'static str, f64)>,
    pub arb_votes: Vec<ArbVote>,
}

/// 3-stage beat detection pipeline.
pub struct BeatDetector {
    onset_detector: OnsetDetector,
    tempo_estimator: TempoEstimator,
    beat_scheduler: BeatScheduler,

    // Onset hold+decay
    held_onset: f32,
    onset_decay_tau: f32,
    last_timestamp: f64,

    // Onset cooldown
    onset_cooldown: f64,
    last_onset_time: f64,

    /// #4160: this hop's kick envelope (0.5 = a kick) and the event rule's state.
    kick_now: f32,
    kick_armed: bool,
    last_kick_time: f64,

    /// #4163: the beat activation and the hop's raw evidence it reads, set by
    /// [`BeatDetector::set_hop_evidence`] before each [`BeatDetector::process`].
    activation: BeatActivation,
    /// #4163: the activation's causal peak picker (the training's rule); an event sits
    /// `act_lag` hops back from the hop that fires it.
    act_picker: PeakPicker,
    act_lag: f64,
    hop_secs: f64,
    hop_kick_frame: [f32; N_FRAME],
    hop_kick_probability: f32,
    hop_pre_norm: AudioFeatures,
}

impl BeatDetector {
    pub fn new(sample_rate: f32, tempo: TempoConfig) -> Self {
        // A5 (#1456): exact frame rate from the fixed analysis hop (sr / ANALYSIS_HOP),
        // e.g. ~86.1 Hz @ 44.1 kHz. Replaces the old hardcoded ~100 Hz approximation that
        // the tempo estimator then had to correct at runtime.
        let frame_rate = sample_rate as f64 / super::ANALYSIS_HOP as f64;

        let history_size = (0.5 * frame_rate) as usize; // ~0.5s
        let long_term_size = (4.0 * frame_rate) as usize; // ~4s

        // #4153: collect sidecar events only when something will read them.
        let dbg = std::env::var_os("FOSFORA_BEAT_SIDECAR").is_some();
        let mut tempo_estimator = TempoEstimator::new(8.0, frame_rate, tempo);
        tempo_estimator.dbg = dbg;
        let mut beat_scheduler = BeatScheduler::new();
        beat_scheduler.dbg = dbg;
        beat_scheduler.arb_evidence = ARB_EVIDENCE;
        beat_scheduler.arbiter = Some(PHASE_ARBITER);
        let picker = BeatActivation::picker();

        Self {
            onset_detector: OnsetDetector::new(sample_rate, history_size, long_term_size),
            tempo_estimator,
            beat_scheduler,
            held_onset: 0.0,
            onset_decay_tau: 0.20,
            last_timestamp: 0.0,
            onset_cooldown: 0.05,
            last_onset_time: 0.0,
            kick_now: 0.0,
            kick_armed: true,
            last_kick_time: f64::NEG_INFINITY,
            activation: BeatActivation::new(),
            act_picker: PeakPicker::with_threshold(picker, ARB_ACTIVATION_THRESHOLD),
            act_lag: f64::from(BeatActivation::lag_hops()),
            hop_secs: 1.0 / frame_rate,
            hop_kick_frame: [0.0; N_FRAME],
            hop_kick_probability: 0.0,
            hop_pre_norm: AudioFeatures::default(),
        }
    }

    /// The kick model's envelope for the hop about to be processed (#4160).
    pub fn set_kick(&mut self, kick: f32) {
        self.kick_now = kick;
    }

    /// The raw evidence of the hop about to be processed, for the beat activation
    /// (#4163): the kick model's frame and probability and the pre-normalization
    /// snapshot. The onset flux and the silence flag come from `process` itself.
    pub fn set_hop_evidence(
        &mut self,
        kick_frame: &[f32; N_FRAME],
        kick_probability: f32,
        pre_norm: &AudioFeatures,
    ) {
        self.hop_kick_frame = *kick_frame;
        self.hop_kick_probability = kick_probability;
        self.hop_pre_norm = *pre_norm;
    }

    /// Apply live tempo config (A7 #1458), snapshotted from the shared `TempoControl`.
    pub fn set_tempo_config(&mut self, config: TempoConfig) {
        self.tempo_estimator.set_config(config);
    }

    /// Apply a one-shot tempo override (A7 #1458).
    pub fn apply_tempo_command(&mut self, cmd: TempoCommand) {
        match cmd {
            TempoCommand::ShiftOctave(dir) => self.tempo_estimator.shift_octave(dir),
            TempoCommand::Tap(bpm) => self.tempo_estimator.tap(bpm),
        }
    }

    /// #80: emit locked beats this much earlier than the measured [`BEAT_LEAD_SECS`], seconds
    /// (a positive A/V sync offset). Negative and non-finite values read as 0: holding the
    /// output back is the caller's job, since everything is held, not just the grid.
    pub fn set_sync_lead(&mut self, secs: f64) {
        self.beat_scheduler.user_lead = if secs.is_finite() { secs.max(0.0) } else { 0.0 };
    }

    /// #80: the sync lead in effect and the beat period it applies to, both seconds — `None`
    /// with no lead set or no grid to lead. `beat_phase` and `bar_phase` advance by this lead
    /// so they wrap with the early beat.
    pub fn sync_lead(&self) -> Option<(f64, f64)> {
        let bs = &self.beat_scheduler;
        if bs.next_beat_time.is_none() || bs.period <= 0.0 {
            return None;
        }
        let period = bs.grid_period();
        let lead = bs.user_lead(period);
        (lead > 0.0).then_some((lead, period))
    }

    /// Prior centre in BPM — published back to the shared config in auto mode (A7 #1458).
    pub fn prior_center_bpm(&self) -> f32 {
        self.tempo_estimator.prior_center_bpm()
    }

    /// #4153: the internals of the hop just processed, draining the pending
    /// events. Only meaningful when `FOSFORA_BEAT_SIDECAR` was set at
    /// construction; the per-instant fractions read 0 otherwise.
    pub fn debug_state(&mut self) -> BeatDebug {
        let mut events = std::mem::take(&mut self.tempo_estimator.dbg_events);
        events.append(&mut self.beat_scheduler.dbg_events);
        BeatDebug {
            ran: self.tempo_estimator.ran_this_hop,
            tempo: self.tempo_estimator.last_debug,
            mode: self.beat_scheduler.dbg_mode(),
            on_sup: self.beat_scheduler.dbg_onset_support(),
            kick_sup: self.beat_scheduler.dbg_kick_support(),
            grid_n: self.beat_scheduler.grid_beat_count,
            events,
            arb_events: std::mem::take(&mut self.beat_scheduler.dbg_arb_events),
            arb_votes: std::mem::take(&mut self.beat_scheduler.dbg_arb_votes),
        }
    }

    /// This hop's SuperFlux onset flux as the onset detector computed it, before hold,
    /// threshold and cooldown: combined, then the low / mid / high partition means
    /// (#4163's hop sidecar).
    pub fn raw_onset_flux(&self) -> [f64; 4] {
        self.onset_detector.last_flux
    }

    /// Process one frame of audio data.
    ///
    /// Arguments:
    /// - bass_spectrum: magnitude spectrum from 4096-pt FFT (num_bins)
    /// - mid_spectrum: magnitude spectrum from 1024-pt FFT
    /// - high_spectrum: magnitude spectrum from 512-pt FFT
    /// - timestamp: current time in seconds
    /// - loud_silent: perceptual silence gate from the A10 loudness meter (#1457). Gates
    ///   the phase freeze; took over from an `rms < 1e-4` test that misfired on loud audio
    ///   (see the freeze site below).
    pub fn process(
        &mut self,
        bass_spectrum: &[f32],
        mid_spectrum: &[f32],
        high_spectrum: &[f32],
        timestamp: f64,
        loud_silent: bool,
    ) -> BeatResult {
        let dt = if self.last_timestamp > 0.0 {
            (timestamp - self.last_timestamp).max(0.0)
        } else {
            0.0
        };
        self.last_timestamp = timestamp;

        // Stage 1: Onset detection
        let (is_onset, onset_strength, combined_flux) =
            self.onset_detector
                .process(bass_spectrum, mid_spectrum, high_spectrum, loud_silent);

        // #4163: the beat activation over this hop's raw evidence, every hop (silent or
        // not, so its history stays aligned).
        let beat_activation = self.activation.process(&RawEvidence {
            kick_frame: &self.hop_kick_frame,
            kick_probability: self.hop_kick_probability,
            onset_flux: self.onset_detector.last_flux,
            pre_norm: &self.hop_pre_norm,
            loud_silent,
        });
        // Its causal peak picker, the one the training chose, as an EVENT for the
        // arbiter's window (#4163), placed where the beat was: `act_lag` hops back.
        if self.act_picker.push(beat_activation) {
            self.beat_scheduler.act_event = Some(timestamp - self.act_lag * self.hop_secs);
            self.beat_scheduler
                .dbg_event("act_fire", f64::from(beat_activation));
        }

        // Apply onset cooldown
        let mut onset_gated = is_onset;
        if is_onset && (timestamp - self.last_onset_time) < self.onset_cooldown {
            onset_gated = false;
        }
        if onset_gated {
            self.last_onset_time = timestamp;
        }

        // Stage 2: Tempo estimation. #4250: the flux seeds, the activation chooses the
        // period (see `TempoEstimator::update`).
        let (bpm, confidence, period_s) = self
            .tempo_estimator
            .update(combined_flux, f64::from(beat_activation));

        // #4160: a kick EVENT, by the `/stem/drums/onset` rule.
        if self.kick_now < 0.3 {
            self.kick_armed = true;
        }
        if self.kick_armed && self.kick_now >= 0.5 && timestamp - self.last_kick_time >= 0.10 {
            self.kick_armed = false;
            self.last_kick_time = timestamp;
            self.beat_scheduler.kick_event = true;
        }

        // Stage 3: Beat scheduling
        self.beat_scheduler
            .update_tempo(bpm, period_s, confidence, self.tempo_estimator.locked);
        let (is_beat, beat_time, beat_phase, _smoothed_bpm) = self.beat_scheduler.process(
            onset_gated,
            onset_strength,
            timestamp,
            self.onset_detector.is_sustained_silence(),
        );

        // Onset hold+decay (instant attack, exponential release)
        if onset_strength > self.held_onset {
            self.held_onset = onset_strength;
        } else if dt > 0.0 {
            self.held_onset *= (-dt as f32 / self.onset_decay_tau).exp();
        }

        // Freeze phase at 0 during silence.
        //
        // Gate on the A10 perceptual flag, not on `rms`: by this point `rms` has been
        // through the adaptive normalizer, which maps it to `(v − P5) / (P95 − P5)` and so
        // floors it at exactly 0.0 whenever the signal touches the bottom of its recent
        // range. On any rhythmic material that is the trough between every hit, on
        // perfectly loud audio — which used to punch a spurious 1-hop `beat_phase` dropout
        // to 0 several times a beat (found while verifying A8 #1459).
        let phase = if loud_silent { 0.0 } else { beat_phase as f32 };

        BeatResult {
            settled_beat_time: self.beat_scheduler.settled,
            onset_strength: self.held_onset,
            beat_activation,
            beat: if is_beat { 1.0 } else { 0.0 },
            beat_phase: phase,
            // Q2b (F3): the wire carries the held tempo, not the raw filter —
            // see `TempoEstimator::published_bpm`.
            bpm: self.tempo_estimator.published_bpm() as f32,
            beat_strength: if is_beat {
                self.beat_scheduler.beat_strength as f32
            } else {
                0.0
            },
            beat_time: if is_beat { beat_time } else { timestamp },
            beat_index: self.beat_scheduler.grid_beat_count,
            tempo_confidence: self.tempo_estimator.support.clamp(0.0, 1.0) as f32,
            beat_locked: self.tempo_estimator.locked,
        }
    }
}

#[cfg(test)]
mod tests;
