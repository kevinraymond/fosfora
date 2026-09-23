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
