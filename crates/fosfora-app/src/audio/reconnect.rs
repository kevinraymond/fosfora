//! A9 (#1460): capture-backend watchdog recovery — the off-thread reaper and the reconnect
//! state machine.
//!
//! Both halves are deliberately device-free so they can be unit tested: [`reap`] is generic
//! over its payload, and [`ReconnectState::poll`] is a pure function of (state, clock,
//! [`Health`]) with no atomics and no clock of its own. The parts that must touch a real
//! device live in [`super`].

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// Gaps before attempts 2..=5. Attempt 1 fires immediately on the confirmed death, so the
/// episode spans ~15s in total.
const BACKOFF: [Duration; 4] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
    Duration::from_secs(8),
];

/// Attempts per stall episode before giving up and leaving it to the user.
pub const MAX_ATTEMPTS: u32 = 5;

/// How long a freshly opened backend has to produce its first callback before the attempt is
/// counted as failed. Matches the existing startup check in [`super::AudioSystem::latest_features`].
const PROBE: Duration = Duration::from_secs(5);

/// Dispose of `payload` on a detached thread.
///
/// [`super::CaptureBackend`]'s `Drop` joins its capture thread, which may be blocked in a
/// timeout-less read (`pa_simple_read` on a suspended monitor). On the render thread that is a
/// freeze for as long as the stall lasts — the hazard that kept `poll_health` detection-only
/// before #1460. Here it costs one parked OS thread instead, and only until the read returns:
/// every capture loop re-checks its shutdown flag at the top of each iteration, so the reaper
/// finishes the moment the source produces anything at all. If it never does, we have leaked
/// one thread and one device handle, and the show keeps running.
///
/// Generic over the payload so the one property that matters — *it does not join on the
/// caller* — is testable without an audio device.
pub fn reap<T: Send + 'static>(name: &'static str, payload: T) {
    // The payload can only reach the thread by being moved into the closure, and a failed
    // `spawn` gives the closure back to us to drop — which would run the very join we are
    // escaping, on this thread. Parking it behind an `Arc` lets the failure path leak it
    // instead: an idle thread and a device handle cost far less than a frozen render thread.
    let cell = Arc::new(Mutex::new(Some(payload)));
    let inner = cell.clone();
    let spawned = thread::Builder::new().name(name.into()).spawn(move || {
        let t0 = Instant::now();
        // Recover from a poisoned mutex: nothing else can hold this lock, and dropping the
        // payload is the whole point of the thread.
        drop(inner.lock().unwrap_or_else(|e| e.into_inner()).take());
        log::info!(
            "{name}: released in {:.0}ms",
            t0.elapsed().as_secs_f64() * 1000.0
        );
    });
    if spawned.is_err() {
        log::error!("{name}: spawn failed, leaking payload rather than blocking the caller");
        std::mem::forget(cell);
    }
}

/// One frame's view of capture health, as [`super::AudioSystem::poll_health`] sees it.
#[derive(Debug, Clone, Copy)]
pub struct Health {
    /// The capture thread positively reported its own death (`capture_failed`). Unambiguous
    /// on every backend.
    pub died: bool,
    /// `callback_count` has been frozen past the stall window, having flowed at least once.
    pub frozen: bool,
    /// Any callback has been counted since this backend opened.
    pub any_callbacks: bool,
    /// This backend keeps delivering data (zeros) while nothing is playing, so a frozen count
    /// means the device is gone rather than idle. False for both loopback backends: WASAPI
    /// delivers no packets during silence, and a PulseAudio monitor of a suspended sink stops
    /// delivering — neither is distinguishable from death by a frozen count alone.
    pub silence_delivers_data: bool,
}

impl Health {
    /// The one expression requirement (f) lives in: act on a positive death signal always, but
    /// on a mere freeze only where a freeze cannot just mean "nothing is playing".
    fn is_dead(self) -> bool {
        self.died || (self.frozen && self.silence_delivers_data)
    }
}

/// What [`ReconnectState::poll`] wants the caller to do this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconnectAction {
    Idle,
    /// Reap the current backend and reopen the same target. `attempt` is 1-based.
    Reopen {
        attempt: u32,
    },
    /// Attempts exhausted — report once, then stay quiet until something changes.
    GiveUp,
}

#[derive(Debug, Clone, Copy)]
enum Phase {
    Healthy,
    /// A reopen is in flight on the worker thread.
    Opening,
    /// The new backend is open; waiting until `until` for it to prove it delivers data.
    Probing {
        until: Instant,
    },
    /// The attempt failed; the next one fires at `at`.
    Waiting {
        at: Instant,
    },
    /// [`MAX_ATTEMPTS`] spent. Cleared by a recovered callback or a manual switch.
    Exhausted,
}

/// Backoff and attempt bookkeeping for one stall episode.
///
/// Owned by [`super::AudioSystem`] and deliberately *not* swapped by `adopt` — like
/// `band_scale`, it belongs to the system rather than to any one backend.
pub struct ReconnectState {
    enabled: bool,
    phase: Phase,
    attempt: u32,
    /// Set when [`Self::fail`] exhausts the episode, drained by the next [`Self::poll`] so
    /// `GiveUp` surfaces exactly once regardless of which call site failed the attempt.
    giveup_pending: bool,
}

impl ReconnectState {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            phase: Phase::Healthy,
            attempt: 0,
            giveup_pending: false,
        }
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        // Turning it off mid-episode abandons that episode rather than freezing it.
        self.reset();
    }

    /// Whether auto-reconnect is on. [`Self::poll`] gates on this itself, but A9b's
    /// default-sink trigger (#1617) reaches [`Self::note_reopen_started`] without going
    /// through `poll`, so it has to ask.
    ///
    /// A9b's only caller is Linux-gated, so this is dead code on other platforms until the
    /// WASAPI default-device follow-up (finding #1616) gives it a second caller.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Begin an episode that `poll` did not initiate — A9b's default-sink change (#1617),
    /// which is neither a stall nor a death and so has no [`Health`] signal to poll on.
    ///
    /// Mirrors the `Healthy` + `is_dead` arm of [`Self::poll`]. Two things carry over from it
    /// deliberately: `Phase::Opening` is what stops the caller re-firing the reopen on every
    /// rendered frame (`poll_health` runs at 60-144+Hz), and `attempt = 1` puts the new sink
    /// on the same backoff ladder — so a sink we cannot open retries rather than giving up on
    /// the first failure.
    ///
    /// Dead code off Linux for the same reason as [`Self::is_enabled`].
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn note_reopen_started(&mut self) {
        self.attempt = 1;
        self.phase = Phase::Opening;
        self.giveup_pending = false;
    }

    /// Callbacks advanced — the backend is alive. No-op while a reopen is in flight, so a late
    /// callback from the outgoing backend cannot cancel it.
    pub fn note_healthy(&mut self) {
        if matches!(self.phase, Phase::Opening) {
            return;
        }
        self.phase = Phase::Healthy;
        self.attempt = 0;
    }

    /// Forget the current episode — a manual device switch supersedes it.
    pub fn reset(&mut self) {
        self.phase = Phase::Healthy;
        self.attempt = 0;
        self.giveup_pending = false;
    }

    /// An in-flight reopen landed. `opened` is whether `open_backend` returned `Ok` — which is
    /// necessary but not sufficient, hence the probation window (see [`Phase::Probing`]).
    pub fn note_attempt(&mut self, now: Instant, opened: bool) {
        if opened {
            self.phase = Phase::Probing { until: now + PROBE };
        } else {
            self.fail(now);
        }
    }

    pub fn poll(&mut self, now: Instant, h: Health) -> ReconnectAction {
        if !self.enabled {
            return ReconnectAction::Idle;
        }
        if std::mem::take(&mut self.giveup_pending) {
            return ReconnectAction::GiveUp;
        }
        match self.phase {
            Phase::Healthy => {
                if h.is_dead() {
                    self.attempt = 1;
                    self.phase = Phase::Opening;
                    ReconnectAction::Reopen { attempt: 1 }
                } else {
                    ReconnectAction::Idle
                }
            }
            // The worker owns it; `note_attempt` moves us on. Without this phase the reopen
            // would re-fire every rendered frame — `poll_health` runs at 60-144+Hz.
            Phase::Opening => ReconnectAction::Idle,
            Phase::Probing { until } => {
                if h.died {
                    // Opened and died inside probation. Consuming an attempt (rather than
                    // starting a fresh episode) is what stops a backend that dies on open
                    // from becoming a hot reopen loop.
                    self.fail(now);
                    ReconnectAction::Idle
                } else if now >= until {
                    // A backend that stays silent by design cannot prove itself this way, so
                    // give it the benefit of the doubt; `died` is its real signal.
                    if h.any_callbacks || !h.silence_delivers_data {
                        self.phase = Phase::Healthy;
                        self.attempt = 0;
                    } else {
                        self.fail(now);
                    }
                    ReconnectAction::Idle
                } else {
                    ReconnectAction::Idle
                }
            }
            Phase::Waiting { at } => {
                if now >= at {
                    self.attempt += 1;
                    self.phase = Phase::Opening;
                    ReconnectAction::Reopen {
                        attempt: self.attempt,
                    }
                } else {
                    ReconnectAction::Idle
                }
            }
            Phase::Exhausted => ReconnectAction::Idle,
        }
    }

    fn fail(&mut self, now: Instant) {
        if self.attempt >= MAX_ATTEMPTS {
            self.phase = Phase::Exhausted;
            self.giveup_pending = true;
        } else {
            // `attempt` is 1-based and capped at MAX_ATTEMPTS above, so this indexes 0..=3.
            self.phase = Phase::Waiting {
                at: now + BACKOFF[(self.attempt - 1) as usize],
            };
        }
    }

    /// 1-based attempt number for the status bar; 0 when healthy.
    pub fn attempt(&self) -> u32 {
        self.attempt
    }

    pub fn is_reconnecting(&self) -> bool {
        matches!(
            self.phase,
            Phase::Opening | Phase::Probing { .. } | Phase::Waiting { .. }
        )
    }

    pub fn is_exhausted(&self) -> bool {
        matches!(self.phase, Phase::Exhausted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// Healthy on a backend that delivers through silence (cpal).
    fn alive() -> Health {
        Health {
            died: false,
            frozen: false,
            any_callbacks: true,
            silence_delivers_data: true,
        }
    }

    fn dead() -> Health {
        Health {
            died: true,
            ..alive()
        }
    }

    /// A frozen callback count on a backend that delivers through silence — unambiguous death.
    fn frozen_cpal() -> Health {
        Health {
            frozen: true,
            ..alive()
        }
    }

    /// A frozen callback count on a loopback backend — could just be silence.
    fn frozen_loopback() -> Health {
        Health {
            frozen: true,
            silence_delivers_data: false,
            ..alive()
        }
    }

    /// When attempts 1..=5 fire, relative to the episode start, *given each one fails the
    /// instant it opens*. A backoff gap starts when an attempt fails, not when it fired, so
    /// these cumulative times are the best case; a slow open pushes everything later.
    const FIRE_SECS: [u64; 5] = [0, 1, 3, 7, 15];

    /// Drive an episode through all five failed attempts. Returns when the 5th one fired —
    /// at which point `giveup_pending` is set and the next `poll` reports it.
    fn exhaust(s: &mut ReconnectState, t0: Instant) -> Instant {
        assert_eq!(s.poll(t0, dead()), ReconnectAction::Reopen { attempt: 1 });
        for i in 1..FIRE_SECS.len() {
            s.note_attempt(t0 + Duration::from_secs(FIRE_SECS[i - 1]), false);
            s.poll(t0 + Duration::from_secs(FIRE_SECS[i]), alive());
        }
        let last = t0 + Duration::from_secs(FIRE_SECS[4]);
        s.note_attempt(last, false);
        last
    }

    #[test]
    fn reap_returns_before_a_slow_drop_finishes() {
        struct SlowDrop(Arc<AtomicBool>);
        impl Drop for SlowDrop {
            fn drop(&mut self) {
                // Stands in for a capture thread blocked in a timeout-less pa_simple_read.
                thread::sleep(Duration::from_millis(300));
                self.0.store(true, Ordering::Release);
            }
        }

        let done = Arc::new(AtomicBool::new(false));
        let t0 = Instant::now();
        reap("test-reaper", SlowDrop(done.clone()));
        assert!(
            t0.elapsed() < Duration::from_millis(50),
            "reap must not join on the caller"
        );
        thread::sleep(Duration::from_millis(500));
        assert!(
            done.load(Ordering::Acquire),
            "the reaper must still run the drop"
        );
    }

    #[test]
    fn disabled_never_reconnects() {
        let mut s = ReconnectState::new(false);
        assert_eq!(s.poll(Instant::now(), dead()), ReconnectAction::Idle);
    }

    #[test]
    fn death_flag_reconnects_immediately() {
        let mut s = ReconnectState::new(true);
        assert_eq!(
            s.poll(Instant::now(), dead()),
            ReconnectAction::Reopen { attempt: 1 }
        );
    }

    #[test]
    fn frozen_reconnects_when_silence_delivers_data() {
        let mut s = ReconnectState::new(true);
        assert_eq!(
            s.poll(Instant::now(), frozen_cpal()),
            ReconnectAction::Reopen { attempt: 1 }
        );
    }

    /// Requirement (f). A future "simplification" of `Health::is_dead` to `died || frozen` is
    /// the exact hazard this guards: it would reopen the device on every quiet passage.
    #[test]
    fn loopback_silence_does_not_reconnect() {
        let mut s = ReconnectState::new(true);
        assert_eq!(
            s.poll(Instant::now(), frozen_loopback()),
            ReconnectAction::Idle
        );
    }

    #[test]
    fn opening_does_not_re_fire() {
        let mut s = ReconnectState::new(true);
        let t = Instant::now();
        assert_eq!(s.poll(t, dead()), ReconnectAction::Reopen { attempt: 1 });
        for i in 1..10 {
            let later = t + Duration::from_millis(i * 8);
            assert_eq!(s.poll(later, dead()), ReconnectAction::Idle);
        }
    }

    #[test]
    fn failed_opens_back_off_1_2_4_8() {
        let mut s = ReconnectState::new(true);
        let t0 = Instant::now();
        assert_eq!(s.poll(t0, dead()), ReconnectAction::Reopen { attempt: 1 });

        for i in 1..FIRE_SECS.len() {
            let attempt = (i + 1) as u32;
            let fire = FIRE_SECS[i];
            // The previous attempt fails the instant it opened; its gap starts there.
            s.note_attempt(t0 + Duration::from_secs(FIRE_SECS[i - 1]), false);
            assert_eq!(
                s.poll(t0 + Duration::from_millis(fire * 1000 - 1), alive()),
                ReconnectAction::Idle,
                "attempt {attempt} must not fire early"
            );
            assert_eq!(
                s.poll(t0 + Duration::from_secs(fire), alive()),
                ReconnectAction::Reopen { attempt },
                "attempt {attempt} should fire at t0+{fire}s"
            );
        }
    }

    #[test]
    fn gives_up_after_five_attempts() {
        let mut s = ReconnectState::new(true);
        let t0 = Instant::now();
        let t = exhaust(&mut s, t0);
        assert_eq!(s.poll(t, alive()), ReconnectAction::GiveUp);
        assert!(s.is_exhausted());
        for i in 1..5 {
            assert_eq!(
                s.poll(t + Duration::from_secs(i), dead()),
                ReconnectAction::Idle,
                "GiveUp must be reported exactly once"
            );
        }
    }

    #[test]
    fn probe_succeeds_on_first_callback() {
        let mut s = ReconnectState::new(true);
        let t0 = Instant::now();
        s.poll(t0, dead());
        s.note_attempt(t0, true);
        assert!(s.is_reconnecting());
        s.note_healthy();
        assert!(!s.is_reconnecting());
        assert_eq!(s.attempt(), 0);
    }

    #[test]
    fn probe_timeout_fails_attempt_when_no_callbacks() {
        let mut s = ReconnectState::new(true);
        let t0 = Instant::now();
        s.poll(t0, dead());
        s.note_attempt(t0, true);
        let silent = Health {
            any_callbacks: false,
            ..alive()
        };
        assert_eq!(s.poll(t0 + PROBE, silent), ReconnectAction::Idle);
        // Probation failed, so we are now waiting out the first backoff gap.
        assert_eq!(
            s.poll(t0 + PROBE + Duration::from_secs(1), silent),
            ReconnectAction::Reopen { attempt: 2 }
        );
    }

    /// A loopback backend legitimately delivers nothing while nothing plays — failing it for
    /// that would burn all five attempts on a device that is perfectly fine.
    #[test]
    fn probe_trusts_loopback_backend_without_callbacks() {
        let mut s = ReconnectState::new(true);
        let t0 = Instant::now();
        s.poll(t0, frozen_loopback().with_died());
        s.note_attempt(t0, true);
        let silent_loopback = Health {
            died: false,
            frozen: false,
            any_callbacks: false,
            silence_delivers_data: false,
        };
        assert_eq!(s.poll(t0 + PROBE, silent_loopback), ReconnectAction::Idle);
        assert!(!s.is_reconnecting(), "probation should pass");
    }

    #[test]
    fn death_during_probe_consumes_an_attempt() {
        let mut s = ReconnectState::new(true);
        let t0 = Instant::now();
        s.poll(t0, dead());
        s.note_attempt(t0, true);
        // Opens, then dies 1ms later — without consuming an attempt this is a hot loop.
        assert_eq!(
            s.poll(t0 + Duration::from_millis(1), dead()),
            ReconnectAction::Idle
        );
        assert_eq!(
            s.poll(t0 + Duration::from_millis(1001), dead()),
            ReconnectAction::Reopen { attempt: 2 }
        );
    }

    #[test]
    fn manual_switch_resets_episode() {
        let mut s = ReconnectState::new(true);
        let t0 = Instant::now();
        s.poll(t0, dead());
        s.note_attempt(t0, false);
        s.reset();
        assert_eq!(
            s.poll(t0 + Duration::from_secs(1), dead()),
            ReconnectAction::Reopen { attempt: 1 },
            "a manual switch starts a fresh episode"
        );
    }

    #[test]
    fn recovery_clears_exhausted() {
        let mut s = ReconnectState::new(true);
        let t0 = Instant::now();
        let t = exhaust(&mut s, t0);
        assert_eq!(s.poll(t, alive()), ReconnectAction::GiveUp);
        s.note_healthy();
        assert!(!s.is_exhausted());
        assert_eq!(
            s.poll(t + Duration::from_secs(1), dead()),
            ReconnectAction::Reopen { attempt: 1 },
            "a device that comes back on its own starts a fresh episode if it dies again"
        );
    }

    #[test]
    fn set_enabled_false_cancels_in_flight_episode() {
        let mut s = ReconnectState::new(true);
        let t0 = Instant::now();
        s.poll(t0, dead());
        s.set_enabled(false);
        assert!(!s.is_reconnecting());
        assert_eq!(s.poll(t0, dead()), ReconnectAction::Idle);
    }

    #[test]
    fn attempt_number_is_reported() {
        let mut s = ReconnectState::new(true);
        let t0 = Instant::now();
        s.poll(t0, dead());
        assert_eq!(s.attempt(), 1);
        s.note_attempt(t0, false);
        s.poll(t0 + Duration::from_secs(1), alive());
        assert_eq!(s.attempt(), 2);
    }

    impl Health {
        /// Test helper: the loopback death path is `died`, never `frozen`.
        fn with_died(self) -> Self {
            Self { died: true, ..self }
        }
    }
}
