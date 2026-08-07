//! Ableton Link session sync (cargo feature `link`, workstream B).
//!
//! Wraps [`rusty_link`] (Ableton's official `abl_link` C wrapper — note the
//! GPL licensing comment on the cargo feature). One `LinkSystem` is polled
//! once per frame/wake by whichever thread owns it (render thread in the app,
//! the live loop in `--signal`); Link's own network threads run internally.
//!
//! Tempo flows one way at a time ([`LinkMode`]):
//!
//! - **Follow** — the session tempo pins the beat tracker's prior
//!   ([`TempoConfig`]: centre = session tempo, narrow sigma, auto off) while a
//!   peer is present. This is deliberately a *soft* lock: Fosfora listens to
//!   real audio, so beat **phase** keeps coming from the tracker — Link just
//!   removes the octave/tempo ambiguity. The operator's prior is saved on
//!   first pin and restored when Link disengages; while pinned, operator
//!   tweaks survive until the session tempo actually changes.
//! - **Lead** — Fosfora commits its *detected* BPM to the session, gated on
//!   the estimate holding stable (peers hate a twitchy tempo), on a minimum
//!   delta vs the session, and on a minimum interval between commits.
//!
//! Independent of mode, Link's beat-grid crossings and transport edges feed
//! the scene timeline (`Timeline::feed_beat` / start-stop follow) — see the
//! beat-source selection in `app.rs`.

pub mod types;

pub use types::{LinkConfig, LinkMode};

use std::sync::Mutex;

use rusty_link::{AblLink, SessionState};

use crate::audio::beat::{TempoConfig, TempoControl};

/// Prior width (octaves) while Follow pins the tracker: tight enough to force
/// the octave and snap candidate scoring to the session tempo, loose enough
/// that detection still runs on the audio (estimator floor is 0.05).
const FOLLOW_PRIOR_SIGMA: f32 = 0.08;
/// Follow rewrites the prior only when the session tempo moved this much.
const FOLLOW_MIN_DELTA_BPM: f64 = 0.01;
/// Lead: detected BPM must sit inside this band around a candidate…
const LEAD_STABLE_TOL_BPM: f32 = 0.5;
/// …for this long before it is session-worthy.
const LEAD_STABLE_SECS: f32 = 3.0;
/// Lead: don't bother peers for a change smaller than this.
const LEAD_MIN_DELTA_BPM: f64 = 0.5;
/// Lead: minimum spacing between commits.
const LEAD_MIN_INTERVAL_SECS: f32 = 2.0;
/// Lead sanity range — mirrors the tracker's reportable BPM range
/// (`audio::beat` clamps its prior to the same bounds).
const LEAD_BPM_MIN: f32 = 40.0;
const LEAD_BPM_MAX: f32 = 300.0;
/// Session tempo a fresh `AblLink` starts with before any peer is heard.
const DEFAULT_SESSION_BPM: f64 = 120.0;

/// One poll of the Link session, in consumer terms.
#[derive(Debug, Clone, Copy)]
pub struct LinkTick {
    pub peers: u64,
    /// Session tempo in BPM.
    pub tempo: f64,
    /// Phase within the quantum, `0..quantum` beats (UI readout).
    pub quantum_phase: f64,
    /// Link transport (only meaningful with start/stop sync).
    pub playing: bool,
    /// The session beat grid crossed a whole beat since the last poll.
    pub beat_crossed: bool,
    pub playing_started: bool,
    pub playing_stopped: bool,
}

/// Lead-mode commit gate. Pure dt-driven state so it is testable without a
/// session; `since_commit` starts at infinity so the first stable tempo may
/// commit immediately.
#[derive(Debug)]
struct LeadGate {
    candidate_bpm: f32,
    stable_secs: f32,
    since_commit: f32,
}

impl LeadGate {
    fn new() -> Self {
        Self {
            candidate_bpm: 0.0,
            stable_secs: 0.0,
            since_commit: f32::INFINITY,
        }
    }

    fn feed(&mut self, bpm: f32, dt: f32) {
        self.since_commit += dt;
        if !(LEAD_BPM_MIN..=LEAD_BPM_MAX).contains(&bpm) {
            // 0.0 before tempo lock lands here too.
            self.candidate_bpm = 0.0;
            self.stable_secs = 0.0;
        } else if (bpm - self.candidate_bpm).abs() <= LEAD_STABLE_TOL_BPM {
            self.stable_secs += dt;
        } else {
            self.candidate_bpm = bpm;
            self.stable_secs = 0.0;
        }
    }

    fn should_commit(&self, session_tempo: f64) -> bool {
        self.candidate_bpm > 0.0
            && self.stable_secs >= LEAD_STABLE_SECS
            && self.since_commit >= LEAD_MIN_INTERVAL_SECS
            && (f64::from(self.candidate_bpm) - session_tempo).abs() >= LEAD_MIN_DELTA_BPM
    }

    fn committed(&mut self) {
        self.since_commit = 0.0;
    }
}

/// The prior that Follow writes while pinned to a session tempo.
fn follow_prior(session_tempo: f64) -> TempoConfig {
    TempoConfig {
        prior_center_bpm: session_tempo.clamp(f64::from(LEAD_BPM_MIN), f64::from(LEAD_BPM_MAX))
            as f32,
        prior_sigma: FOLLOW_PRIOR_SIGMA,
        auto_prior: false,
    }
}

fn crossed(prev: Option<i64>, now: i64) -> bool {
    prev.is_some_and(|p| now > p)
}

pub struct LinkSystem {
    pub config: LinkConfig,
    link: AblLink,
    state: SessionState,
    last_whole_beat: Option<i64>,
    was_playing: bool,
    /// The operator's prior, saved when Follow first pins and restored when
    /// Link disengages (disable, mode change, last peer gone).
    saved_prior: Option<TempoConfig>,
    last_follow_tempo: f64,
    lead: LeadGate,
    last_tick: Option<LinkTick>,
}

impl LinkSystem {
    pub fn new(config: LinkConfig) -> Self {
        let link = AblLink::new(DEFAULT_SESSION_BPM);
        link.enable_start_stop_sync(config.start_stop_sync);
        link.enable(config.enabled);
        Self {
            config,
            link,
            state: SessionState::new(),
            last_whole_beat: None,
            was_playing: false,
            saved_prior: None,
            last_follow_tempo: 0.0,
            lead: LeadGate::new(),
            last_tick: None,
        }
    }

    /// One step: poll the session, then route tempo per the configured mode.
    /// `detected_bpm` is the tracker's raw BPM (0.0 before lock), `dt` the
    /// caller's frame/wake delta in seconds. Returns `None` while disabled.
    pub fn drive(
        &mut self,
        tempo_ctl: &Mutex<TempoControl>,
        detected_bpm: f32,
        dt: f32,
    ) -> Option<LinkTick> {
        if !self.config.enabled {
            self.restore_prior(tempo_ctl);
            self.last_tick = None;
            return None;
        }
        let tick = self.poll();
        match self.config.mode {
            LinkMode::Follow => {
                // A session tempo is only authoritative while someone else is
                // actually on it; alone, it's just our own stale default.
                if tick.peers > 0 {
                    self.apply_follow(tempo_ctl, tick.tempo);
                } else {
                    self.restore_prior(tempo_ctl);
                }
            }
            LinkMode::Lead => {
                self.restore_prior(tempo_ctl);
                self.lead.feed(detected_bpm, dt);
                if self.lead.should_commit(tick.tempo) {
                    let at = self.link.clock_micros();
                    self.state.set_tempo(f64::from(self.lead.candidate_bpm), at);
                    self.link.commit_app_session_state(&self.state);
                    self.lead.committed();
                }
            }
        }
        self.last_tick = Some(tick);
        Some(tick)
    }

    /// Latest tick from `drive` (UI readout); `None` while disabled.
    pub fn last_tick(&self) -> Option<LinkTick> {
        self.last_tick
    }

    pub fn set_enabled(&mut self, on: bool) {
        if on == self.config.enabled {
            return;
        }
        self.config.enabled = on;
        self.config.save();
        self.link.enable(on);
        if !on {
            self.last_whole_beat = None;
            self.was_playing = false;
            self.lead = LeadGate::new();
            // The pinned prior is restored on the next drive() call.
        }
    }

    pub fn set_mode(&mut self, mode: LinkMode) {
        if mode == self.config.mode {
            return;
        }
        self.config.mode = mode;
        self.config.save();
        self.lead = LeadGate::new();
    }

    pub fn set_quantum(&mut self, quantum: f64) {
        self.config.quantum = quantum;
        self.config.quantum = self.config.quantum_clamped();
        self.config.save();
        // Takes effect on the next poll; nothing to push to Link — quantum is
        // a per-app view of the shared timeline.
        self.last_whole_beat = None;
    }

    pub fn set_start_stop_sync(&mut self, on: bool) {
        if on == self.config.start_stop_sync {
            return;
        }
        self.config.start_stop_sync = on;
        self.config.save();
        self.link.enable_start_stop_sync(on);
    }

    fn poll(&mut self) -> LinkTick {
        let now = self.link.clock_micros();
        self.link.capture_app_session_state(&mut self.state);
        let quantum = self.config.quantum_clamped();
        let beat = self.state.beat_at_time(now, quantum);
        let whole = beat.floor() as i64;
        let beat_crossed = crossed(self.last_whole_beat, whole);
        self.last_whole_beat = Some(whole);
        let playing = self.state.is_playing();
        let playing_started = playing && !self.was_playing;
        let playing_stopped = !playing && self.was_playing;
        self.was_playing = playing;
        LinkTick {
            peers: self.link.num_peers(),
            tempo: self.state.tempo(),
            quantum_phase: self.state.phase_at_time(now, quantum),
            playing,
            beat_crossed,
            playing_started,
            playing_stopped,
        }
    }

    fn apply_follow(&mut self, tempo_ctl: &Mutex<TempoControl>, session_tempo: f64) {
        let first = self.saved_prior.is_none();
        if !first && (session_tempo - self.last_follow_tempo).abs() < FOLLOW_MIN_DELTA_BPM {
            return; // Unchanged — leave any operator tweaks alone.
        }
        let mut ctl = tempo_ctl.lock().unwrap_or_else(|e| e.into_inner());
        if first {
            self.saved_prior = Some(ctl.config);
        }
        ctl.config = follow_prior(session_tempo);
        self.last_follow_tempo = session_tempo;
    }

    fn restore_prior(&mut self, tempo_ctl: &Mutex<TempoControl>) {
        if let Some(saved) = self.saved_prior.take() {
            tempo_ctl.lock().unwrap_or_else(|e| e.into_inner()).config = saved;
            self.last_follow_tempo = 0.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctl() -> Mutex<TempoControl> {
        Mutex::new(TempoControl::new(TempoConfig::default()))
    }

    /// Constructing (FFI) with `enabled: false` never joins the network and
    /// never touches the tracker's prior.
    #[test]
    fn disabled_system_is_inert() {
        let ctl = ctl();
        let mut sys = LinkSystem::new(LinkConfig::default());
        assert!(sys.drive(&ctl, 120.0, 0.016).is_none());
        assert!(sys.last_tick().is_none());
        assert_eq!(ctl.lock().unwrap().config, TempoConfig::default());
    }

    #[test]
    fn follow_prior_pins_center_narrow_sigma_auto_off() {
        let p = follow_prior(174.0);
        assert_eq!(p.prior_center_bpm, 174.0);
        assert_eq!(p.prior_sigma, FOLLOW_PRIOR_SIGMA);
        assert!(!p.auto_prior);
        // Out-of-range session tempos clamp to what the tracker can report.
        assert_eq!(follow_prior(20.0).prior_center_bpm, 40.0);
        assert_eq!(follow_prior(999.0).prior_center_bpm, 300.0);
    }

    #[test]
    fn follow_pins_then_restores_and_respects_operator_tweaks() {
        let ctl = ctl();
        let mut sys = LinkSystem::new(LinkConfig::default());

        sys.apply_follow(&ctl, 128.0);
        assert_eq!(ctl.lock().unwrap().config.prior_center_bpm, 128.0);

        // Session tempo unchanged → an operator tweak survives.
        ctl.lock().unwrap().config.prior_sigma = 0.5;
        sys.apply_follow(&ctl, 128.0);
        assert_eq!(ctl.lock().unwrap().config.prior_sigma, 0.5);

        // Session tempo moved → re-pinned.
        sys.apply_follow(&ctl, 174.0);
        let pinned = ctl.lock().unwrap().config;
        assert_eq!(pinned.prior_center_bpm, 174.0);
        assert_eq!(pinned.prior_sigma, FOLLOW_PRIOR_SIGMA);

        // Disengage → the pre-Link prior comes back exactly.
        sys.restore_prior(&ctl);
        assert_eq!(ctl.lock().unwrap().config, TempoConfig::default());
        // Idempotent.
        sys.restore_prior(&ctl);
        assert_eq!(ctl.lock().unwrap().config, TempoConfig::default());
    }

    #[test]
    fn lead_gate_requires_stability() {
        let mut g = LeadGate::new();
        // 2 s stable at 120 — not yet.
        for _ in 0..20 {
            g.feed(120.0, 0.1);
        }
        assert!(!g.should_commit(128.0));
        // 1.5 s more — stable long enough now.
        for _ in 0..15 {
            g.feed(120.05, 0.1);
        }
        assert!(g.should_commit(128.0));
        // A tempo jump resets the clock.
        g.feed(140.0, 0.1);
        assert!(!g.should_commit(128.0));
    }

    #[test]
    fn lead_gate_rejects_unlocked_and_out_of_range() {
        let mut g = LeadGate::new();
        for _ in 0..100 {
            g.feed(0.0, 0.1); // tracker not locked
        }
        assert!(!g.should_commit(128.0));
        for _ in 0..100 {
            g.feed(350.0, 0.1);
        }
        assert!(!g.should_commit(128.0));
    }

    #[test]
    fn lead_gate_delta_and_rate_limits() {
        let mut g = LeadGate::new();
        for _ in 0..40 {
            g.feed(120.0, 0.1);
        }
        // Too close to the session tempo — don't bother the peers.
        assert!(!g.should_commit(120.2));
        assert!(g.should_commit(122.0));
        g.committed();
        // Still stable, but inside the commit interval.
        for _ in 0..10 {
            g.feed(120.0, 0.1);
        }
        assert!(!g.should_commit(122.0));
        for _ in 0..15 {
            g.feed(120.0, 0.1);
        }
        assert!(g.should_commit(122.0));
    }

    #[test]
    fn beat_crossing_needs_a_previous_poll() {
        assert!(!crossed(None, 5));
        assert!(!crossed(Some(5), 5));
        assert!(crossed(Some(5), 6));
        // Grid can jump backwards (requantize/peer join) — that is not a beat.
        assert!(!crossed(Some(5), 4));
    }
}
