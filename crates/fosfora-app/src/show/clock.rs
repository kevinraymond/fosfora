//! Wall-clock show time. Never uses the clamped render `dt`.

use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockTransport {
    Idle,
    Running,
    Paused,
}

#[derive(Debug, Clone)]
enum ClockState {
    Idle,
    Running {
        accumulated: Duration,
        resumed_at: Instant,
    },
    Paused {
        accumulated: Duration,
    },
}

#[derive(Debug, Clone)]
pub struct ShowClock {
    state: ClockState,
}

impl Default for ShowClock {
    fn default() -> Self {
        Self::new()
    }
}

impl ShowClock {
    pub fn new() -> Self {
        Self {
            state: ClockState::Idle,
        }
    }

    pub fn transport(&self) -> ClockTransport {
        match self.state {
            ClockState::Idle => ClockTransport::Idle,
            ClockState::Running { .. } => ClockTransport::Running,
            ClockState::Paused { .. } => ClockTransport::Paused,
        }
    }

    /// Testable core: elapsed at an explicit `now`.
    pub fn elapsed_at(&self, now: Instant) -> Duration {
        match self.state {
            ClockState::Idle => Duration::ZERO,
            ClockState::Running {
                accumulated,
                resumed_at,
            } => accumulated + now.saturating_duration_since(resumed_at),
            ClockState::Paused { accumulated } => accumulated,
        }
    }

    pub fn elapsed(&self) -> Duration {
        self.elapsed_at(Instant::now())
    }

    /// START: elapsed → 00:00:00 and running.
    pub fn start(&mut self, now: Instant) {
        self.state = ClockState::Running {
            accumulated: Duration::ZERO,
            resumed_at: now,
        };
    }

    pub fn pause(&mut self, now: Instant) {
        if let ClockState::Running {
            accumulated,
            resumed_at,
        } = self.state
        {
            self.state = ClockState::Paused {
                accumulated: accumulated + now.saturating_duration_since(resumed_at),
            };
        }
    }

    pub fn resume(&mut self, now: Instant) {
        if let ClockState::Paused { accumulated } = self.state {
            self.state = ClockState::Running {
                accumulated,
                resumed_at: now,
            };
        }
    }

    /// RESET: show time → 0; waits for START.
    pub fn reset(&mut self) {
        self.state = ClockState::Idle;
    }

    pub fn seek(&mut self, now: Instant, position: Duration) {
        match self.state {
            ClockState::Running { .. } => {
                self.state = ClockState::Running {
                    accumulated: position,
                    resumed_at: now,
                };
            }
            ClockState::Paused { .. } | ClockState::Idle => {
                self.state = ClockState::Paused {
                    accumulated: position,
                };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_hour_accuracy() {
        let t0 = Instant::now();
        let mut clock = ShowClock::new();
        clock.start(t0);
        let three_h = Duration::from_secs(3 * 3600);
        assert_eq!(clock.elapsed_at(t0 + three_h), three_h);
    }

    #[test]
    fn both_tracks_independent_for_full_3h_from_one_clock() {
        // Phase 3 CHECKPOINT core: both tracks run independently for the
        // full 3h from one ShowClock. Walk every cue boundary of the
        // Hibernation timeline (scene cues at 0/1200/3600/7200/9600,
        // palette cues at 0/3500/6900) plus a mid-transition sample, in
        // milliseconds of test time via `elapsed_at`.
        let t0 = Instant::now();
        let mut clock = ShowClock::new();
        clock.start(t0);
        // (elapsed_s, expected scene idx, expected palette idx)
        let checkpoints = [
            (0, 0, 0),
            (1199, 0, 0),
            (1200, 1, 0),
            (3499, 1, 0),
            (3500, 1, 1),
            (3522, 1, 1), // 37% into the 60s earth→deep-blue blend
            (3600, 2, 1),
            (6899, 2, 1),
            (6900, 2, 2),
            (7200, 3, 2),
            (9600, 4, 2),
            (10799, 4, 2),
        ];
        for (secs, scene_idx, pal_idx) in checkpoints {
            let now = t0 + Duration::from_secs(secs);
            let elapsed = clock.elapsed_at(now);
            assert_eq!(elapsed, Duration::from_secs(secs), "elapsed at {secs}s");
            let e = elapsed.as_secs().min(u64::from(u32::MAX)) as u32;
            assert_eq!(
                crate::show::definition::scheduled_index([0, 1200, 3600, 7200, 9600], e),
                Some(scene_idx),
                "scene index at {secs}s"
            );
            assert_eq!(
                crate::show::definition::scheduled_index([0, 3500, 6900], e),
                Some(pal_idx),
                "palette index at {secs}s"
            );
        }
        // End of show: clock reads exactly 3h.
        assert_eq!(
            clock.elapsed_at(t0 + Duration::from_secs(10800)),
            Duration::from_secs(10800)
        );
    }

    #[test]
    fn pause_ten_minutes_then_resume() {
        let t0 = Instant::now();
        let mut clock = ShowClock::new();
        clock.start(t0);
        let t1 = t0 + Duration::from_secs(90);
        clock.pause(t1);
        clock.resume(t1 + Duration::from_secs(600));
        let t2 = t1 + Duration::from_secs(600) + Duration::from_secs(10);
        assert_eq!(clock.elapsed_at(t2), Duration::from_secs(100));
    }

    #[test]
    fn seek_backward_and_forward() {
        let t0 = Instant::now();
        let mut clock = ShowClock::new();
        clock.start(t0);
        clock.seek(t0 + Duration::from_secs(10), Duration::from_secs(5));
        assert_eq!(
            clock.elapsed_at(t0 + Duration::from_secs(10)),
            Duration::from_secs(5)
        );
        clock.seek(t0 + Duration::from_secs(11), Duration::from_secs(3600));
        assert_eq!(
            clock.elapsed_at(t0 + Duration::from_secs(11)),
            Duration::from_secs(3600)
        );
    }

    #[test]
    fn simulated_stall_advances_exactly_stall_duration() {
        let t0 = Instant::now();
        let mut clock = ShowClock::new();
        clock.start(t0);
        let before = t0 + Duration::from_millis(19_580);
        assert_eq!(clock.elapsed_at(before), Duration::from_millis(19_580));
        let after = before + Duration::from_secs(5);
        assert_eq!(clock.elapsed_at(after), Duration::from_millis(24_580));
    }

    #[test]
    fn reset_returns_to_idle_zero() {
        let t0 = Instant::now();
        let mut clock = ShowClock::new();
        clock.start(t0);
        clock.reset();
        assert_eq!(clock.transport(), ClockTransport::Idle);
        assert_eq!(clock.elapsed_at(t0 + Duration::from_secs(99)), Duration::ZERO);
    }
}
