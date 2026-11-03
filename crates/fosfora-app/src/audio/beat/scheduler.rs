//! Stage 3: the phase-locked beat grid oscillator and its phase arbiter.

use std::collections::VecDeque;

use super::ArbVote;
use super::control::MAX_LEAD_PERIOD_FRAC;

// ---------------------------------------------------------------------------
// Stage 3: Beat scheduler — a phase-locked grid oscillator (Q1 rewrite)
// ---------------------------------------------------------------------------
//
// Beats FIRE at predicted grid instants; onsets are evidence that corrects the
// grid's phase, never fire triggers (with one bounded exception while the
// tempo is still unlocked). This inverts the old state machine, whose "a beat
// is an event that happens at the hop where we decide it" assumption produced
// every measured defect at once: firing on every gated onset below the
// confidence gate (n_est/n_ref 1.5-1.7×), a 0.9×period backup free-running
// 11% fast, missed beats fired 80-92 ms late at the hop that noticed them, and
// the grid re-anchored onto whichever late tail onset happened to confirm.
//
// Modes, keyed off the estimator's hysteretic lock (Q3):
// - LOCKED, supported (freerun): fire ON the grid; `beat_time` is the grid
//   instant itself, so n_est/n_ref ≈ 1 and timing error ≈ phase-correction
//   residual by construction. In-window onsets apply a two-gain PI correction.
// - LOCKED, unsupported (muted freerun): after enough unsupported beats the
//   grid keeps advancing silently — a breakdown mutes the strobe instead of
//   spraying it — and the first supported beats resume emission on-grid with
//   no re-acquisition.
// - UNLOCKED with a provisional period (acquisition): fire only where onset
//   and grid agree, at the onset's own time, at most one per 0.6 period.
// - NO TEMPO (bootstrap, or sustained silence): no beats. /onset and the kick
//   stem events keep carrying transient reactivity (user-approved behavior).

/// Onset-to-grid window: ±10% of the period, capped at the bench tolerance.
const PLL_WINDOW_PERIOD_FRAC: f64 = 0.10;
const PLL_WINDOW_MAX_SECS: f64 = 0.07;
/// Bid capture reach on BOTH sides, as a period fraction. Wide, so a
/// desynced grid can still see the true beat (a tight late window measured a
/// self-sustaining −60 ms dead-zone desync on the fixture: real evidence
/// couldn't bid, and 0.005-strength flux ripples kept the grid early).
const PLL_CAPTURE_PERIOD_FRAC: f64 = 0.25;
/// Bid score = √strength × exp(−½(e/σ)²). The root softens loudness wars
/// (a build's crescendo roll is genuinely louder just BEFORE the bar line
/// than on it); the proximity kernel makes far bids need a real strength
/// advantage. σ is in seconds of grid error.
const PLL_BID_PROXIMITY_SIGMA_SECS: f64 = 0.06;
/// Bids below this onset strength are flux ripples, not evidence.
const PLL_MIN_BID_STRENGTH: f32 = 0.05;
/// A locked grid's beat is emitted this far BEFORE its instant (#4474). The grid
/// phase-locks onto onset evidence the hop chain sees late (hop quantization plus the
/// onset detector's own delay), so its instants sit late even on synthetic renders with
/// exact labels (batida house +20 ms, rock +25, techno +19, waltz +29 before this; +3 to
/// +14 after) and ~15-19 ms late against the Harmonix annotations, while a consumer
/// inside TouchDesigner adds up to a 60 fps frame on top. Measured as a pre-registered
/// round (bench/manifests/q4474_lead_frozen.json): 10 ms gives Harmonix beat F .753 ->
/// .758 on tune and .711 -> .717 on the holdout with no track lost, Ballroom flat (its
/// annotations sit earlier; 15 ms gained no more on Harmonix and traded Ballroom tracks
/// at the 70 ms window edge both ways). Only the emission moves: the instant itself
/// settles where it did (support, trim, mute, the arbiter's fold, the per-beat trackers),
/// and an emission ahead of its settle reads the mute state of the last settled instant.
pub(super) const BEAT_LEAD_SECS: f64 = 0.010;

/// Two-gain PI correction (Ellis-style): phase pulls the upcoming beat, the
/// trim nudges the grid rate between tempo updates.
const PLL_PHASE_GAIN: f64 = 0.3;
const PLL_TRIM_GAIN: f64 = 0.02;
const PLL_TRIM_CLAMP: f64 = 0.02;
/// Per-beat trim relaxation. The trim exists to track a sustained rate
/// mismatch; without decay, the corrections of a one-off phase convergence
/// wind it to the clamp and the grid then free-runs off-rate faster than the
/// tight late window can recapture (measured: −20 ms/beat runaway).
const PLL_TRIM_DECAY_SUPPORTED: f64 = 0.85;
const PLL_TRIM_DECAY_UNSUPPORTED: f64 = 0.5;
/// Per-beat support EMA and the mute hysteresis it drives.
const PLL_SUPPORT_ALPHA: f64 = 0.25;
const PLL_MUTE_ENTER_SUPPORT: f64 = 0.2;
const PLL_MUTE_EXIT_SUPPORT: f64 = 0.4;
/// Acquisition-mode minimum spacing, in periods.
const PLL_ACQ_MIN_SPACING: f64 = 0.6;
/// Unlocked and nothing fired for this many periods → the anchor is stale;
/// re-anchor on the next onset instead of letting a dead grid veto agreement.
const PLL_ACQ_STALE_PERIODS: f64 = 4.0;

// ---------------------------------------------------------------------------
// Stage 3b: phase arbiter (#4160)
// ---------------------------------------------------------------------------
//
// The PLL only pulls the grid toward onsets within +-[`PLL_CAPTURE_PERIOD_FRAC`]
// of a period, and the grid anchors on the first gated onset, which can be any
// hit. A grid that anchored on the off-beat therefore stays there: the true beat
// is half a period away. On the bench, 28% of emitted beats sat on the half-beat
// with the tempo right, and the phase was decided at the anchor far more often
// than it drifted later.
//
// The arbiter folds the last bars of events onto the grid and moves the grid half a
// beat when the events clearly and repeatedly sit on the half-beat. Two streams fill
// the window (#4163): KICK events (the kick model rejects bass notes, while in the onset
// function a house off-beat bassline outvotes the beat: onset evidence pulled three
// correct batida house renders onto the off-beat) and the beat activation's peaks
// (`beat_model.rs`), which know the snare and backbeat the kicks do not. Kicks sit on
// the 'and' on some music and vote wrong there; the activation's events are balanced
// on music where a single early kick vote is what sets the grid right; together they
// gained 17 Harmonix holdout tracks and lost none. A 32-beat guard keeps a grid
// the long-run evidence still backs, so a burst of misread events (chiptune
// square-wave bass read as kicks) cannot flip it. The configuration and its
// evidence are in bench/manifests/q4160_phase_frozen.json and board #4160.
//
// #4234 relaxed that guard. #4160 set it on kicks alone (2:1, at most .3 of the window
// on the grid); with both streams in the window, grids that lock on the off-beat at
// acquisition, most of them Ballroom, carry ~1.5-2:1 evidence for the beat there and
// were never moved. At 1.5:1 with a 3-instant streak the arbiter gains on both Ballroom
// halves (+.021 tune / +.016 holdout beat F); on Harmonix it trades syncopated grids,
// where the 'and' outvotes the beat by that much, against off-beat ones it fixes (+.013 /
// -.003). Manifest bench/manifests/q4234_guard_frozen.json.
//
// #4402 added the quarter-beats as candidates. A grid a quarter of a beat off (Latin
// grids that lock on an accent cluster, synthetic techno whose kicks sit .3 of a beat
// from the grid's instants) is outside the PLL's capture window, and the half-beat fold
// reads its events as neither phase, so nothing ever moved it. Where the half-beat rule
// abstains, the fuller quarter bin may move the grid a quarter or three quarters of a
// beat later, under a stricter test than the half-beat's: 3:1 over the on-grid bin in
// the short window and at least 24 events in the 32-beat window, so the first bar after
// lock, where a settling grid reads as a quarter off, cannot vote. Ballroom +.008 /
// +.004 beat F (tune / holdout) with no track lost, Harmonix +.006 / +.002 (4 gained,
// 1 lost: a PLL half-period drag the rule read as a quarter-off grid), the batida techno
// renders .81 -> .90. Manifest bench/manifests/q4402_quarters_frozen.json.

#[derive(Clone, Copy, Debug)]
pub(super) struct PhaseArbiterCfg {
    /// Beats of kick evidence each vote folds onto the grid.
    pub(super) win_beats: f64,
    /// Fewer kicks than this in the window: abstain (kick-sparse music keeps
    /// the PLL's phase untouched).
    pub(super) min_kicks: usize,
    /// The half-beat must hold this many times the on-grid kicks.
    pub(super) ratio: f64,
    /// Consecutive grid instants that must vote to move before the grid moves.
    pub(super) streak: u32,
    /// Stability guard over this many beats (0 = off): the half-beat must also
    /// win by `long_ratio` there, and the on-grid phase may hold at most
    /// `long_share` of that window's kicks.
    pub(super) long_beats: f64,
    pub(super) long_ratio: f64,
    pub(super) long_share: f64,
    /// #4402: also test the quarter-beats (1/4 and 3/4 of a period), consulted only at an
    /// instant where the half-beat rule abstains; `false` is the #4234 arbiter. A grid a
    /// quarter of a beat off sits outside the PLL's capture window with nothing on its
    /// instants, and the half-beat fold reads its events as neither phase.
    pub(super) quarters: bool,
    /// The fuller quarter bin must hold this many times the on-grid events in the short
    /// window (stricter than `ratio`: an accent cluster a quarter after the beat is common,
    /// a grid a quarter off rare), and the long window must hold at least `q_long_min`
    /// events, so the first bar after lock cannot vote the grid away. The long window's
    /// ratio and share, the streak and the bins' tolerance are the half-beat rule's.
    pub(super) q_ratio: f64,
    pub(super) q_long_min: usize,
}

/// The windows the #4160 round selected, with the guard and streak #4234 relaxed (from
/// 2.0, .3 and 4; at 1.5 the .4 share is implied by the ratio) and the quarter candidates
/// #4402 added (3:1 and 24 events: of its three arms, the one that lost no track).
pub(super) const PHASE_ARBITER: PhaseArbiterCfg = PhaseArbiterCfg {
    win_beats: 8.0,
    min_kicks: 4,
    ratio: 2.0,
    streak: 3,
    long_beats: 32.0,
    long_ratio: 1.5,
    long_share: 0.4,
    quarters: true,
    q_ratio: 3.0,
    q_long_min: 24,
};

/// Where the arbiter may move the grid, as a fraction of a period LATER.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ArbOffset {
    Half,
    Quarter,
    ThreeQuarter,
}

impl ArbOffset {
    fn periods(self) -> f64 {
        match self {
            Self::Half => 0.5,
            Self::Quarter => 0.25,
            Self::ThreeQuarter => 0.75,
        }
    }
}

/// One window's fold onto the grid: events on the grid, on the half-beat, on the
/// quarter-beats (1/4, 3/4), and all of them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ArbFold {
    on: usize,
    half: usize,
    q1: usize,
    q3: usize,
    total: usize,
}

/// A kick counts toward a phase when it lies within this fraction of a period.
const ARB_PHASE_TOLERANCE: f64 = 0.125;

/// Which events the arbiter folds onto the grid. The #4163 round measured kicks alone (the
/// #4160 configuration), the beat activation's peaks alone, and both streams in one window
/// at four event thresholds; the union at .70 is what ships (bench/manifests/
/// q4163_arbiter_frozen.json: Harmonix holdout beat F .620 -> .654 with 17 tracks gained
/// and none lost, Ballroom .437 -> .445, every guard held).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ArbEvidence {
    /// Kick model events (the #4160 arbiter as shipped).
    Kicks,
    /// Peaks of the beat activation (`beat_model.rs`), each placed `lag_hops` back.
    Activation,
    /// Both streams in one window; an instant where both fire counts twice.
    Union,
}

/// #75: room for the arbiter's longest window (32 beats) of kicks and activation peaks,
/// several per beat, so the window does not grow mid-track (it grew 64 → 128 events about
/// half a minute into a steady 120 BPM kick).
const ARB_EVENTS_CAPACITY: usize = 256;

/// The arbiter's event streams, as the #4163 round selected them.
pub(super) const ARB_EVIDENCE: ArbEvidence = ArbEvidence::Union;

/// The activation's event threshold for the arbiter's window. Higher than the picker
/// threshold the training chose for a standalone picker (.45, F-optimal): the round found
/// the vote wants fewer, surer events (.45 and .60 flipped correct grids on syncopated
/// material; .75 was too sparse to vote where the kicks are wrong).
pub(super) const ARB_ACTIVATION_THRESHOLD: f32 = 0.70;

pub(super) struct BeatScheduler {
    /// Q3: the estimator's hysteretic lock state — selects the firing mode.
    pub(super) tempo_locked: bool,
    bpm: f64,
    pub(super) period: f64,
    tempo_confidence: f64,

    /// The next grid instant on the sample clock; `None` until the first onset
    /// anchors a grid, and again after silence drops it.
    pub(super) next_beat_time: Option<f64>,
    /// Multiplicative grid-rate trim from the PI loop, clamped ±[`PLL_TRIM_CLAMP`].
    period_trim: f64,
    /// Time of the most recently fired (or silently passed) grid beat.
    pub(super) last_beat_time: f64,
    /// #293: the locked phase last published and the `last_beat_time` it was measured
    /// from, so a correction that pushes the upcoming instant later cannot move the phase
    /// back within that interval.
    phase_floor: Option<(f64, f64)>,
    /// #4473: the grid instant whose beat was already emitted `BEAT_LEAD_SECS` ahead of
    /// its settle, and whether it was emitted (muted instants pass silently).
    lead_emitted: Option<(f64, bool)>,
    /// #4473: `Some(emitted beat time)` on the hop an emitted beat's instant settles (a
    /// locked instant passing, or an acquisition / anchor fire, which settles as it fires).
    /// The per-beat trackers (downbeat, structure) key on this rather than on the early
    /// emission, so their evidence windows end where they did before the lead.
    pub(super) settled: Option<f64>,
    /// Time of the last EMITTED beat (grid or acquisition).
    last_fired_time: f64,
    /// Was any in-window onset seen since the last grid beat?
    onset_support_pending: bool,
    /// Best EARLY-side bid (score, error) for the upcoming grid instant —
    /// held until the fire so a weak far hit cannot pre-empt the actual
    /// beat-carrying hit behind it.
    slot_early_best: Option<(f64, f64)>,
    /// The just-fired slot, still collecting late-side bids until its
    /// deadline: (deadline, best bid so far from either side).
    closing_slot: Option<(f64, Option<(f64, f64)>)>,
    /// EMA of per-beat onset support; drives mute hysteresis.
    beat_support: f64,
    /// Running count of grid instants passed — INCLUDING muted ones — so the
    /// downbeat tracker's bar rotation stays phase-coherent through mutes and
    /// unlocked gaps (counting only fired beats slips the modulo every gap).
    pub(super) grid_beat_count: u64,
    muted: bool,
    pub(super) beat_strength: f64,

    /// #4160 phase arbiter; `None` disables it (tests).
    pub(super) arbiter: Option<PhaseArbiterCfg>,
    /// #4163: which event streams feed the arbiter's window.
    pub(super) arb_evidence: ArbEvidence,
    /// #4163: an activation peak for this hop, placed at its beat time (lag removed);
    /// taken by `process` like `kick_event`.
    pub(super) act_event: Option<f64>,
    /// Event times within the arbiter's longest window (kicks, activation peaks or
    /// both, per `arb_evidence`), kept in time order.
    pub(super) arb_kicks: VecDeque<f64>,
    /// #4474: the lead a locked beat is emitted with; [`BEAT_LEAD_SECS`], 0 in the tests
    /// that place beats on their instants.
    pub(super) beat_lead: f64,
    /// #80: the user's sync lead on top of `beat_lead`, seconds (0 unless the sync offset is
    /// positive); capped per period by [`Self::user_lead`].
    pub(super) user_lead: f64,
    /// Consecutive grid instants that voted to move the grid, and by which offset; a vote
    /// for another offset restarts it.
    arb_streak: u32,
    arb_streak_offset: Option<ArbOffset>,
    /// Set by [`BeatDetector::process`] for the hop being processed.
    pub(super) kick_event: bool,
    /// Grid phase shifts the arbiter made (diagnostics).
    pub(super) phase_shifts: u64,

    /// #4153 sidecar: the last 8 grid instants as (time, had in-window onset
    /// support), the state transitions since the last drain (name, extra).
    /// Both only maintained while `dbg` is set.
    pub(super) dbg: bool,
    dbg_grid: VecDeque<(f64, bool)>,
    pub(super) dbg_events: Vec<(&'static str, f64)>,
    /// #4234 sidecar: the arbiter's window pushes and votes since the last drain.
    pub(super) dbg_arb_events: Vec<(&'static str, f64)>,
    pub(super) dbg_arb_votes: Vec<ArbVote>,
}

/// #4153: how many grid instants the sidecar's support fractions cover.
const DBG_GRID_LEN: usize = 8;

impl BeatScheduler {
    pub(super) fn new() -> Self {
        Self {
            tempo_locked: false,
            bpm: 0.0,
            period: 0.0,
            tempo_confidence: 0.0,
            next_beat_time: None,
            lead_emitted: None,
            settled: None,
            period_trim: 0.0,
            last_beat_time: 0.0,
            phase_floor: None,
            last_fired_time: 0.0,
            onset_support_pending: false,
            slot_early_best: None,
            closing_slot: None,
            beat_support: 0.0,
            grid_beat_count: 0,
            muted: false,
            beat_strength: 0.0,
            arbiter: Some(PHASE_ARBITER),
            arb_evidence: ArbEvidence::Kicks,
            act_event: None,
            arb_kicks: VecDeque::with_capacity(ARB_EVENTS_CAPACITY),
            beat_lead: BEAT_LEAD_SECS,
            user_lead: 0.0,
            arb_streak: 0,
            arb_streak_offset: None,
            kick_event: false,
            phase_shifts: 0,
            dbg: false,
            dbg_grid: VecDeque::new(),
            dbg_events: Vec::new(),
            dbg_arb_events: Vec::new(),
            dbg_arb_votes: Vec::new(),
        }
    }

    /// #4153 sidecar event; a no-op unless `dbg` is set.
    pub(super) fn dbg_event(&mut self, name: &'static str, extra: f64) {
        if self.dbg {
            self.dbg_events.push((name, extra));
        }
    }

    /// #4153: note a grid instant passing (every `grid_beat_count += 1`), with
    /// whether an in-window onset backed it. A no-op unless `dbg` is set.
    fn dbg_grid_instant(&mut self, time: f64, supported: bool) {
        if !self.dbg {
            return;
        }
        if self.dbg_grid.len() == DBG_GRID_LEN {
            self.dbg_grid.pop_front();
        }
        self.dbg_grid.push_back((time, supported));
    }

    /// #4153: fraction of the tracked grid instants that had onset support.
    pub(super) fn dbg_onset_support(&self) -> f64 {
        if self.dbg_grid.is_empty() {
            return 0.0;
        }
        let n = self.dbg_grid.iter().filter(|&&(_, s)| s).count();
        n as f64 / self.dbg_grid.len() as f64
    }

    /// #4153: fraction of the tracked grid instants with a kick event within
    /// [`ARB_PHASE_TOLERANCE`] of a period, from the arbiter's kick window.
    pub(super) fn dbg_kick_support(&self) -> f64 {
        let period = self.grid_period();
        if self.dbg_grid.is_empty() || period <= 0.0 {
            return 0.0;
        }
        let tol = ARB_PHASE_TOLERANCE * period;
        let n = self
            .dbg_grid
            .iter()
            .filter(|&&(t, _)| self.arb_kicks.iter().any(|&k| (k - t).abs() <= tol))
            .count();
        n as f64 / self.dbg_grid.len() as f64
    }

    /// #4153: which firing path the scheduler is on.
    pub(super) fn dbg_mode(&self) -> &'static str {
        if self.next_beat_time.is_none() || self.period <= 0.0 {
            "none"
        } else if !self.tempo_locked {
            "acq"
        } else if self.muted {
            "muted"
        } else {
            "locked"
        }
    }

    /// #4160: fold the events of the window ending at the grid instant `now` onto the
    /// grid. Returns the offset the grid moves LATER by (`None` = stays): half a beat once
    /// the half-beat has won `streak` instants in a row (and the stability guard agrees);
    /// with `quarters`, a quarter or three quarters once a quarter-beat has won `streak`
    /// instants where the half-beat rule abstained (#4402).
    fn arbitrate(&mut self, cfg: PhaseArbiterCfg, now: f64, period: f64) -> Option<ArbOffset> {
        let keep = cfg.win_beats.max(cfg.long_beats) * period;
        while self.arb_kicks.front().is_some_and(|&t| t < now - keep) {
            self.arb_kicks.pop_front();
        }
        let fold = |beats: f64| {
            let mut f = ArbFold::default();
            for &t in self
                .arb_kicks
                .iter()
                .filter(|&&t| t >= now - beats * period)
            {
                f.total += 1;
                let x = ((t - now) / period).rem_euclid(1.0);
                if x.min(1.0 - x) <= ARB_PHASE_TOLERANCE {
                    f.on += 1;
                }
                if (x - 0.5).abs() <= ARB_PHASE_TOLERANCE {
                    f.half += 1;
                }
                if cfg.quarters {
                    if (x - 0.25).abs() <= ARB_PHASE_TOLERANCE {
                        f.q1 += 1;
                    }
                    if (x - 0.75).abs() <= ARB_PHASE_TOLERANCE {
                        f.q3 += 1;
                    }
                }
            }
            f
        };
        let s = fold(cfg.win_beats);
        let l = fold(cfg.long_beats);
        // The shipped half-beat rule, unchanged.
        let mut offset = ArbOffset::Half;
        let mut vote = s.half > s.on
            && s.total >= cfg.min_kicks
            && s.half as f64 >= cfg.ratio * s.on.max(1) as f64;
        if vote && cfg.long_beats > 0.0 {
            vote = l.on as f64 <= cfg.long_share * l.total as f64
                && l.half as f64 >= cfg.long_ratio * l.on.max(1) as f64;
        }
        // #4402: where it abstains, the fuller quarter bin challenges (ties to 1/4).
        if !vote && cfg.quarters {
            let (k, sq, lq) = if s.q3 > s.q1 {
                (ArbOffset::ThreeQuarter, s.q3, l.q3)
            } else {
                (ArbOffset::Quarter, s.q1, l.q1)
            };
            let mut v = sq > s.on
                && s.total >= cfg.min_kicks
                && sq as f64 >= cfg.q_ratio * s.on.max(1) as f64;
            if v && cfg.long_beats > 0.0 {
                v = l.on as f64 <= cfg.long_share * l.total as f64
                    && lq as f64 >= cfg.long_ratio * l.on.max(1) as f64
                    && l.total >= cfg.q_long_min;
            }
            if v {
                vote = true;
                offset = k;
            }
        }
        let mut shift = None;
        if vote {
            self.arb_streak = if self.arb_streak_offset == Some(offset) {
                self.arb_streak + 1
            } else {
                1
            };
            self.arb_streak_offset = Some(offset);
            if self.arb_streak >= cfg.streak {
                shift = Some(offset);
            }
        }
        if !vote || shift.is_some() {
            self.arb_streak = 0;
            self.arb_streak_offset = None;
        }
        if self.dbg {
            self.dbg_arb_votes.push(ArbVote {
                now,
                period,
                short: (s.on, s.half, s.total),
                long: (l.on, l.half, l.total),
                quarter: (s.q1, s.q3),
                lquarter: (l.q1, l.q3),
                vote,
                streak: self.arb_streak,
                shift: shift.is_some(),
                offset: shift.map_or(0.0, ArbOffset::periods),
            });
        }
        shift
    }

    pub(super) fn update_tempo(&mut self, bpm: f64, period: f64, confidence: f64, locked: bool) {
        self.tempo_locked = locked;
        self.bpm = bpm;
        self.period = period;
        self.tempo_confidence = confidence;
    }

    pub(super) fn grid_period(&self) -> f64 {
        self.period * (1.0 + self.period_trim)
    }

    /// Two-gain PI step: e > 0 means the music runs late vs the grid.
    fn apply_phase_correction(&mut self, next_beat_time: &mut f64, e: f64, period: f64) {
        *next_beat_time += PLL_PHASE_GAIN * e;
        self.period_trim =
            (self.period_trim + PLL_TRIM_GAIN * e / period).clamp(-PLL_TRIM_CLAMP, PLL_TRIM_CLAMP);
    }

    /// #80: the user's sync lead at this period, capped so the whole emission lead stays
    /// under [`MAX_LEAD_PERIOD_FRAC`] of a period.
    pub(super) fn user_lead(&self, period: f64) -> f64 {
        self.user_lead
            .min(MAX_LEAD_PERIOD_FRAC * period - self.beat_lead)
            .max(0.0)
    }

    fn window(&self) -> f64 {
        (self.grid_period() * PLL_WINDOW_PERIOD_FRAC).min(PLL_WINDOW_MAX_SECS)
    }

    /// Main beat decision. Returns (is_beat, beat_time, beat_phase, bpm) —
    /// `beat_time` is the beat's emitted instant (the grid instant less
    /// [`BEAT_LEAD_SECS`] while locked), at or before `timestamp`.
    pub(super) fn process(
        &mut self,
        is_onset: bool,
        onset_strength: f32,
        timestamp: f64,
        is_silence: bool,
    ) -> (bool, f64, f64, f64) {
        // Sustained silence: mute and drop the grid — re-entry re-acquires
        // fresh instead of trusting a phase that free-ran through the gap.
        let kick_event = std::mem::take(&mut self.kick_event);
        let act_event = self.act_event.take();
        self.settled = None;
        if is_silence {
            self.next_beat_time = None;
            self.lead_emitted = None;
            self.period_trim = 0.0;
            self.beat_support = 0.0;
            self.muted = false;
            self.slot_early_best = None;
            self.closing_slot = None;
            self.arb_kicks.clear();
            self.arb_streak = 0;
            self.arb_streak_offset = None;
            return (false, 0.0, 0.0, self.bpm);
        }

        if self.period <= 0.0 {
            // Bootstrap: no tempo hypothesis yet — no grid, no beats.
            return (false, 0.0, 0.0, self.bpm);
        }
        let period = self.grid_period();
        if self.arbiter.is_some() {
            if kick_event && self.arb_evidence != ArbEvidence::Activation {
                self.arb_kicks.push_back(timestamp);
                if self.dbg {
                    self.dbg_arb_events.push(("kick", timestamp));
                }
            }
            if let Some(t) = act_event.filter(|_| self.arb_evidence != ArbEvidence::Kicks) {
                // Placed `lag` before this hop, so it may precede a kick pushed above
                // at this hop: keep the window ordered for the fold's trim.
                let pos = self
                    .arb_kicks
                    .iter()
                    .rposition(|&x| x <= t)
                    .map_or(0, |p| p + 1);
                self.arb_kicks.insert(pos, t);
                if self.dbg {
                    self.dbg_arb_events.push(("act", t));
                }
            }
        }

        // No grid anchored yet: anchor on the first gated onset. Locked or
        // not, the first beat needs an event to phase against.
        let Some(mut next_beat_time) = self.next_beat_time.take() else {
            if is_onset {
                self.next_beat_time = Some(self.anchor(onset_strength, timestamp, period));
                self.dbg_event("first_anchor", f64::NAN);
                self.settled = Some(timestamp);
                return (true, timestamp, 0.0, self.bpm);
            }
            return (false, 0.0, 0.0, self.bpm);
        };
        let out = self.process_anchored(
            &mut next_beat_time,
            is_onset,
            onset_strength,
            timestamp,
            period,
        );
        self.next_beat_time = Some(next_beat_time);
        out
    }

    /// Start a fresh grid on this onset, which fires as its first beat.
    /// Returns the new grid's next instant.
    fn anchor(&mut self, onset_strength: f32, timestamp: f64, period: f64) -> f64 {
        self.beat_strength = f64::from(onset_strength);
        self.lead_emitted = None;
        self.last_beat_time = timestamp;
        self.last_fired_time = timestamp;
        self.dbg_grid_instant(timestamp, self.onset_support_pending);
        self.onset_support_pending = false;
        self.grid_beat_count += 1;
        timestamp + period
    }

    /// [`Self::process`] once a grid exists. `next_beat_time` is the anchored
    /// grid instant, taken out of `self` for the duration and written back by
    /// the caller.
    fn process_anchored(
        &mut self,
        next_beat_time: &mut f64,
        is_onset: bool,
        onset_strength: f32,
        timestamp: f64,
        period: f64,
    ) -> (bool, f64, f64, f64) {
        // Onset evidence: measure against the NEAREST grid instant (the
        // upcoming one or the one just passed) and BID to correct that slot.
        // One correction per slot, decided at the slot's deadline by the best
        // bid. Bid score = √strength × proximity — three measured failure
        // modes shaped this: first-captured-wins let roll hits drag the grid
        // fast; strength-alone let a build's crescendo (louder just before
        // the bar line than on it) walk the grid early; and a tight late
        // window created a dead zone where a desynced grid could no longer
        // see the true beat at all. SUPPORT (mute hysteresis) stays tight:
        // only onsets inside the ±window count as "this beat had backing".
        if is_onset && onset_strength >= PLL_MIN_BID_STRENGTH {
            let prev = *next_beat_time - period;
            let e_next = timestamp - *next_beat_time;
            let e_prev = timestamp - prev;
            let nearest_is_next = e_next.abs() <= e_prev.abs();
            let e = if nearest_is_next { e_next } else { e_prev };
            let capture = period * PLL_CAPTURE_PERIOD_FRAC;
            if e.abs() <= capture {
                if e.abs() <= self.window() {
                    self.beat_strength = f64::from(onset_strength);
                    self.onset_support_pending = true;
                }
                let prox = (-0.5 * (e / PLL_BID_PROXIMITY_SIGMA_SECS).powi(2)).exp();
                let score = f64::from(onset_strength).sqrt() * prox;
                if nearest_is_next && e < 0.0 {
                    // Early side of the upcoming instant: hold until its fire.
                    if self.slot_early_best.is_none_or(|(b, _)| score > b) {
                        self.slot_early_best = Some((score, e));
                    }
                } else if !nearest_is_next && e >= 0.0 {
                    // Late side of the just-fired slot: bid into its window.
                    if let Some((_, best)) = &mut self.closing_slot {
                        if best.is_none_or(|(b, _)| score > b) {
                            *best = Some((score, e));
                        }
                    }
                }
            }
        }

        let mut fired = false;
        let mut beat_time = 0.0;

        if self.tempo_locked {
            // #4474 / #80: how far ahead of its instant this grid's beat is emitted.
            let lead = self.beat_lead + self.user_lead(period);
            // Settle an expired decision window: its closest attack corrects
            // the grid now, in time to shape the very next instant.
            if let Some((deadline, best)) = self.closing_slot {
                if timestamp > deadline {
                    if let Some((_, e)) = best {
                        self.apply_phase_correction(next_beat_time, e, period);
                    }
                    self.closing_slot = None;
                }
            }
            // Degenerate transitional slot: around a mode flip, a pending
            // negative correction can leave the next grid instant at or
            // before the beat just emitted (measured once in 374 tracks:
            // a −1.6 ms inversion after an acquisition fire). That instant
            // IS the beat already fired — it passes silently below so
            // /beat stays strictly monotonic.
            // #4473: the beat is EMITTED `BEAT_LEAD_SECS` before its instant, once per
            // instant, on the mute state of the last settled instant; the instant itself
            // settles below exactly as before. Never twice within half a period, whatever
            // moved the instant in between; an instant already emitted is not degenerate.
            let already = self
                .lead_emitted
                .is_some_and(|(t, _)| (t - *next_beat_time).abs() < 0.5 * period);
            let degenerate = !already && *next_beat_time <= self.last_fired_time + 0.3 * period;
            let settles_now = *next_beat_time <= timestamp;
            if !already && !degenerate && !settles_now && *next_beat_time - lead <= timestamp {
                self.lead_emitted = Some((*next_beat_time, !self.muted));
                if !self.muted {
                    fired = true;
                    beat_time = *next_beat_time - lead;
                    self.last_fired_time = *next_beat_time;
                }
            }
            // Grid instants due this hop settle (or pass silently while muted).
            if *next_beat_time <= timestamp {
                if degenerate {
                    *next_beat_time += period;
                    self.lead_emitted = None;
                    let phase = self.locked_phase(timestamp, *next_beat_time);
                    return (false, 0.0, phase, self.bpm);
                }
                let supported = self.onset_support_pending;
                self.beat_support +=
                    PLL_SUPPORT_ALPHA * (f64::from(supported as u8) - self.beat_support);
                self.onset_support_pending = false;
                self.period_trim *= if supported {
                    PLL_TRIM_DECAY_SUPPORTED
                } else {
                    PLL_TRIM_DECAY_UNSUPPORTED
                };
                // Open the fired slot's decision window: early-side evidence
                // already collected bids against late-side attacks still to
                // come; the closest wins when the window expires. (A pending
                // unresolved slot is settled first — periods are ≫ windows, so
                // it has necessarily expired by now.)
                if let Some((_, Some((_, e)))) = self.closing_slot.take() {
                    self.apply_phase_correction(next_beat_time, e, period);
                }
                self.closing_slot = Some((
                    *next_beat_time + period * PLL_CAPTURE_PERIOD_FRAC,
                    self.slot_early_best.take(),
                ));
                if self.muted {
                    if self.beat_support > PLL_MUTE_EXIT_SUPPORT {
                        self.muted = false;
                    }
                } else if self.beat_support < PLL_MUTE_ENTER_SUPPORT {
                    self.muted = true;
                }

                let instant = *next_beat_time;
                let emitted = match self.lead_emitted.take() {
                    Some((t, e)) if (t - instant).abs() < 0.5 * period => e,
                    _ => {
                        // Not emitted ahead (no lead, or the lead window fell inside
                        // this hop): emit now, on the mute state just settled, exactly
                        // as before the lead.
                        if !self.muted {
                            fired = true;
                            beat_time = instant - lead;
                            self.last_fired_time = instant;
                        }
                        !self.muted
                    }
                };
                self.last_beat_time = instant;
                *next_beat_time += period;
                self.grid_beat_count += 1;
                self.dbg_grid_instant(instant, supported);
                if let Some(cfg) = self.arbiter {
                    if let Some(shift) = self.arbitrate(cfg, instant, period) {
                        let shift = shift.periods();
                        // Move the grid LATER by the offset (a long gap reads
                        // better than a double beat): half a beat, or a quarter
                        // / three quarters with #4402's candidates; the PLL's
                        // pending bids and trim belong to the old phase.
                        *next_beat_time += shift * period;
                        self.period_trim = 0.0;
                        self.closing_slot = None;
                        self.slot_early_best = None;
                        self.onset_support_pending = false;
                        self.phase_shifts += 1;
                        self.dbg_event("arb_shift", shift);
                    }
                }
                if !supported {
                    self.beat_strength = 0.5;
                }
                if emitted {
                    self.settled = Some(instant - lead);
                }
            }
        } else {
            self.lead_emitted = None;
            // Acquisition: evidence-gated fires only — an in-window onset, far
            // enough from the previous fire. The onset is the better clock
            // while unlocked, so the beat carries the onset's own time.
            if is_onset
                && self.onset_support_pending
                && timestamp - self.last_fired_time >= PLL_ACQ_MIN_SPACING * period
            {
                fired = true;
                beat_time = timestamp;
                self.settled = Some(timestamp);
                self.last_fired_time = timestamp;
                self.last_beat_time = timestamp;
                self.grid_beat_count += 1;
                self.dbg_grid_instant(timestamp, self.onset_support_pending);
                self.dbg_event("acq_fire", f64::from(onset_strength));
                // Fold the grid onto this fire so phase reads from it.
                while *next_beat_time <= timestamp {
                    *next_beat_time += period;
                }
            } else if is_onset
                && !self.onset_support_pending
                && timestamp - self.last_fired_time >= PLL_ACQ_STALE_PERIODS * period
            {
                // Stale anchor: nothing has agreed for several periods —
                // re-anchor on this onset rather than let a dead grid veto
                // every candidate forever.
                *next_beat_time = self.anchor(onset_strength, timestamp, period);
                self.dbg_event("stale_reanchor", f64::from(onset_strength));
                self.settled = Some(timestamp);
                return (true, timestamp, 0.0, self.bpm);
            }
            // Advance the provisional grid past due instants (no emission —
            // unsupported unlocked beats are exactly the old over-firing).
            while *next_beat_time <= timestamp {
                self.last_beat_time = *next_beat_time;
                self.dbg_grid_instant(*next_beat_time, self.onset_support_pending);
                *next_beat_time += period;
                self.onset_support_pending = false;
                self.grid_beat_count += 1;
            }
            self.onset_support_pending = is_onset && self.onset_support_pending;
        }

        let phase = if self.tempo_locked {
            self.locked_phase(timestamp, *next_beat_time)
        } else {
            self.phase_floor = None;
            ((timestamp - self.last_beat_time) / period).rem_euclid(1.0)
        };
        (fired, beat_time, phase, self.bpm)
    }

    /// #293: the locked beat phase, the fraction of the way from the last settled instant to
    /// `next`, the upcoming one. It reaches 1 only as that instant settles — the hop the
    /// downbeat tracker's `beat_index` steps — however far a correction or an arbiter shift
    /// moved the instant from `last + period`; measured from the last instant alone, it
    /// wrapped a hop early and `beat_index + beat_phase` dipped by a beat. Within one interval
    /// it never moves back, even when a correction pushes `next` later.
    fn locked_phase(&mut self, timestamp: f64, next: f64) -> f64 {
        let last = self.last_beat_time;
        let span = next - last;
        let raw = if span > 0.0 {
            (timestamp - last) / span
        } else {
            0.0
        };
        let floor = match self.phase_floor {
            Some((from, p)) if from.to_bits() == last.to_bits() => p,
            _ => 0.0,
        };
        let phase = raw.clamp(floor, PHASE_BELOW_ONE);
        self.phase_floor = Some((last, phase));
        phase
    }
}

/// #293: the largest phase below 1 that survives the cast to `f32` (`1 - f32::EPSILON`).
pub(in crate::audio) const PHASE_BELOW_ONE: f64 = 1.0 - f32::EPSILON as f64;
