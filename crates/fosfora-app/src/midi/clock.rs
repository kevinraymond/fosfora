use std::collections::VecDeque;

/// Parses MIDI system realtime messages to derive external BPM.
/// Accumulates 0xF8 timing clock ticks (24 per quarter note).
pub struct MidiClock {
    /// Whether external transport is playing (Start/Continue received).
    playing: bool,
    /// Tick counter within current beat (0-23).
    tick_count: u32,
    /// Total beat count since start.
    beat_count: u32,
    /// Recent tick intervals for BPM calculation.
    tick_intervals: VecDeque<f64>,
    /// Timestamp of last tick.
    last_tick_time: Option<std::time::Instant>,
    /// Derived external BPM.
    bpm: f64,
    /// Phase within current beat (0.0-1.0).
    phase: f32,
}

const TICKS_PER_BEAT: u32 = 24;
/// Keep this many tick intervals for averaging.
const MAX_INTERVALS: usize = 48;

impl MidiClock {
    pub fn new() -> Self {
        Self {
            playing: false,
            tick_count: 0,
            beat_count: 0,
            tick_intervals: VecDeque::with_capacity(MAX_INTERVALS),
            last_tick_time: None,
            bpm: 0.0,
            phase: 0.0,
        }
    }

    /// Process a MIDI system realtime byte.
    /// Returns true if a beat boundary was crossed.
    pub fn process_byte(&mut self, status: u8) -> bool {
        match status {
            0xF8 => self.on_tick(),
            0xFA => {
                // Start
                self.playing = true;
                self.tick_count = 0;
                self.beat_count = 0;
                self.tick_intervals.clear();
                self.last_tick_time = None;
                self.phase = 0.0;
                false
            }
            0xFB => {
                // Continue
                self.playing = true;
                false
            }
            0xFC => {
                // Stop
                self.playing = false;
                false
            }
            _ => false,
        }
    }

    fn on_tick(&mut self) -> bool {
        let now = std::time::Instant::now();
        let mut beat_crossed = false;

        // Record interval
        if let Some(last) = self.last_tick_time {
            let interval = now.duration_since(last).as_secs_f64();
            if interval > 0.0 && interval < 0.5 {
                // Reject intervals > 500ms (< 5 BPM at 24 ppqn)
                self.tick_intervals.push_back(interval);
                if self.tick_intervals.len() > MAX_INTERVALS {
                    self.tick_intervals.pop_front();
                }
                self.update_bpm();
            }
        }
        self.last_tick_time = Some(now);

        // Count ticks
        self.tick_count += 1;
        self.phase = self.tick_count as f32 / TICKS_PER_BEAT as f32;

        if self.tick_count >= TICKS_PER_BEAT {
            self.tick_count = 0;
            self.beat_count += 1;
            self.phase = 0.0;
            beat_crossed = true;
        }

        beat_crossed
    }

    fn update_bpm(&mut self) {
        if self.tick_intervals.len() < 4 {
            return;
        }
        let sum: f64 = self.tick_intervals.iter().sum();
        let avg = sum / self.tick_intervals.len() as f64;
        // BPM = 60 / (avg_tick_interval * ticks_per_beat)
        let beat_duration = avg * TICKS_PER_BEAT as f64;
        if beat_duration > 0.0 {
            self.bpm = 60.0 / beat_duration;
        }
    }

    /// Whether external transport is currently playing.
    pub fn playing(&self) -> bool {
        self.playing
    }

    /// Total beats counted since Start.
    #[allow(dead_code)]
    pub fn beat_count(&self) -> u32 {
        self.beat_count
    }

    /// Derived BPM from clock ticks (0 if not enough data).
    #[allow(dead_code)]
    pub fn bpm(&self) -> f64 {
        self.bpm
    }

    /// Phase within current beat (0.0 at beat start, ~1.0 just before next).
    #[allow(dead_code)]
    pub fn beat_phase(&self) -> f32 {
        self.phase
    }

    /// Whether we've received enough ticks to report BPM.
    #[allow(dead_code)]
    pub fn has_bpm(&self) -> bool {
        self.tick_intervals.len() >= 4
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_start_stop() {
        let mut clock = MidiClock::new();
        assert!(!clock.playing());
        clock.process_byte(0xFA); // Start
        assert!(clock.playing());
        clock.process_byte(0xFC); // Stop
        assert!(!clock.playing());
        clock.process_byte(0xFB); // Continue
        assert!(clock.playing());
    }

    #[test]
    fn clock_start_resets_counters() {
        let mut clock = MidiClock::new();
        clock.beat_count = 10;
        clock.tick_count = 5;
        clock.process_byte(0xFA);
        assert_eq!(clock.beat_count, 0);
        assert_eq!(clock.tick_count, 0);
    }

    #[test]
    fn clock_tick_counts_beats() {
        let mut clock = MidiClock::new();
        clock.process_byte(0xFA);

        // 24 ticks = 1 beat
        for i in 0..23 {
            let beat = clock.process_byte(0xF8);
            assert!(!beat, "tick {} should not be a beat", i);
        }
        let beat = clock.process_byte(0xF8);
        assert!(beat, "24th tick should be a beat");
        assert_eq!(clock.beat_count(), 1);
    }

    #[test]
    fn clock_phase_progresses() {
        let mut clock = MidiClock::new();
        clock.process_byte(0xFA);

        // After 12 ticks, phase should be ~0.5
        for _ in 0..12 {
            clock.process_byte(0xF8);
        }
        assert!((clock.beat_phase() - 0.5).abs() < 1e-6);
    }
}
