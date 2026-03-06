use super::types::{AdvanceMode, SceneCue, TransitionType};

/// Runtime playback state of the timeline.
#[derive(Debug, Clone)]
pub enum PlaybackState {
    /// No playback — waiting for user action.
    Idle,
    /// Holding on a cue (counting time before next transition or waiting for manual advance).
    Holding { cue_index: usize, elapsed: f32 },
    /// Transitioning between two cues.
    Transitioning {
        from_cue: usize,
        to_cue: usize,
        progress: f32,
        transition_type: TransitionType,
        duration: f32,
    },
}

/// Events emitted by the timeline each tick.
#[derive(Debug, Clone)]
pub enum TimelineEvent {
    /// Nothing happened.
    None,
    /// Load the preset for this cue (cut or start of transition).
    LoadCue { cue_index: usize },
    /// A transition has begun.
    BeginTransition {
        from_cue: usize,
        to_cue: usize,
        transition_type: TransitionType,
        duration: f32,
    },
    /// Transition progress updated (0.0–1.0).
    TransitionProgress {
        from_cue: usize,
        to_cue: usize,
        progress: f32,
        transition_type: TransitionType,
    },
    /// Transition completed — now holding on to_cue.
    TransitionComplete { cue_index: usize },
}

/// Read-only snapshot for UI (avoids borrow conflicts).
#[derive(Debug, Clone)]
pub struct TimelineInfo {
    pub active: bool,
    pub cue_count: usize,
    pub current_cue: usize,
    pub state: TimelineInfoState,
    pub loop_mode: bool,
    pub advance_mode: AdvanceMode,
}

#[derive(Debug, Clone)]
pub enum TimelineInfoState {
    Idle,
    Holding {
        elapsed: f32,
        hold_secs: Option<f32>,
    },
    Transitioning {
        from: usize,
        to: usize,
        progress: f32,
        transition_type: TransitionType,
    },
}

/// The runtime timeline state machine.
pub struct Timeline {
    pub cues: Vec<SceneCue>,
    pub state: PlaybackState,
    pub loop_mode: bool,
    pub advance_mode: AdvanceMode,
    pub active: bool,
    /// Beat counter for BeatSync mode.
    pub beat_count: u32,
    /// Last beat state (for rising-edge detection).
    last_beat: bool,
}

impl Timeline {
    pub fn new(cues: Vec<SceneCue>, loop_mode: bool, advance_mode: AdvanceMode) -> Self {
        Self {
            cues,
            state: PlaybackState::Idle,
            loop_mode,
            advance_mode,
            active: false,
            beat_count: 0,
            last_beat: false,
        }
    }

    /// Start the timeline at cue 0 (or given index).
    pub fn start(&mut self, cue_index: usize) -> TimelineEvent {
        if self.cues.is_empty() {
            return TimelineEvent::None;
        }
        let idx = cue_index.min(self.cues.len() - 1);
        self.active = true;
        self.beat_count = 0;
        self.state = PlaybackState::Holding {
            cue_index: idx,
            elapsed: 0.0,
        };
        TimelineEvent::LoadCue { cue_index: idx }
    }

    /// Adjust playback state after a cue was removed at `removed`.
    /// Call this right after `self.cues.remove(removed)`.
    pub fn notify_cue_removed(&mut self, removed: usize) {
        if self.cues.is_empty() {
            self.stop();
            return;
        }
        match &mut self.state {
            PlaybackState::Idle => {}
            PlaybackState::Holding { cue_index, .. } => {
                if *cue_index == removed {
                    // The cue we were on got deleted — clamp to valid range
                    *cue_index = removed.min(self.cues.len() - 1);
                } else if *cue_index > removed {
                    *cue_index -= 1;
                }
            }
            PlaybackState::Transitioning {
                from_cue, to_cue, ..
            } => {
                if *from_cue == removed || *to_cue == removed {
                    // Abort transition — hold on the nearest valid cue
                    let fallback = removed.min(self.cues.len() - 1);
                    self.state = PlaybackState::Holding {
                        cue_index: fallback,
                        elapsed: 0.0,
                    };
                } else {
                    if *from_cue > removed {
                        *from_cue -= 1;
                    }
                    if *to_cue > removed {
                        *to_cue -= 1;
                    }
                }
            }
        }
    }

    /// Stop the timeline.
    pub fn stop(&mut self) {
        self.active = false;
        self.state = PlaybackState::Idle;
    }

    /// Advance to next cue (manual trigger).
    pub fn go_next(&mut self) -> TimelineEvent {
        if !self.active || self.cues.is_empty() {
            return TimelineEvent::None;
        }
        let current = self.current_cue_index();
        let next = self.next_cue_index(current);
        match next {
            Some(to) => self.begin_transition(current, to),
            None => {
                self.stop();
                TimelineEvent::None
            }
        }
    }

    /// Go to previous cue (manual trigger).
    pub fn go_prev(&mut self) -> TimelineEvent {
        if !self.active || self.cues.is_empty() {
            return TimelineEvent::None;
        }
        let current = self.current_cue_index();
        if current == 0 {
            if self.loop_mode {
                let to = self.cues.len() - 1;
                return self.begin_transition(current, to);
            }
            return TimelineEvent::None;
        }
        self.begin_transition(current, current - 1)
    }

    /// Jump directly to a specific cue.
    pub fn go_to_cue(&mut self, index: usize) -> TimelineEvent {
        if index >= self.cues.len() {
            return TimelineEvent::None;
        }
        if !self.active {
            return self.start(index);
        }
        let current = self.current_cue_index();
        if current == index {
            return TimelineEvent::None;
        }
        self.begin_transition(current, index)
    }

    /// Called every frame. Returns a timeline event.
    pub fn tick(&mut self, dt: f32) -> TimelineEvent {
        if !self.active || self.cues.is_empty() {
            return TimelineEvent::None;
        }

        match self.state.clone() {
            PlaybackState::Idle => TimelineEvent::None,
            PlaybackState::Holding { cue_index, elapsed } => {
                let new_elapsed = elapsed + dt;
                self.state = PlaybackState::Holding {
                    cue_index,
                    elapsed: new_elapsed,
                };

                // Check for auto-advance (Timer mode)
                if self.advance_mode == AdvanceMode::Timer {
                    if let Some(hold) = self.cues[cue_index].hold_secs {
                        if new_elapsed >= hold {
                            if let Some(next) = self.next_cue_index(cue_index) {
                                return self.begin_transition(cue_index, next);
                            } else {
                                self.stop();
                            }
                        }
                    }
                }

                TimelineEvent::None
            }
            PlaybackState::Transitioning {
                from_cue,
                to_cue,
                progress,
                transition_type,
                duration,
            } => {
                let new_progress = if duration > 0.0 {
                    (progress + dt / duration).min(1.0)
                } else {
                    1.0
                };

                if new_progress >= 1.0 {
                    // Transition complete
                    self.state = PlaybackState::Holding {
                        cue_index: to_cue,
                        elapsed: 0.0,
                    };
                    TimelineEvent::TransitionComplete { cue_index: to_cue }
                } else {
                    self.state = PlaybackState::Transitioning {
                        from_cue,
                        to_cue,
                        progress: new_progress,
                        transition_type,
                        duration,
                    };
                    TimelineEvent::TransitionProgress {
                        from_cue,
                        to_cue,
                        progress: new_progress,
                        transition_type,
                    }
                }
            }
        }
    }

    /// Feed a beat signal (true on beat). Used in BeatSync mode.
    pub fn feed_beat(&mut self, beat: bool) -> TimelineEvent {
        if !self.active {
            return TimelineEvent::None;
        }
        // Rising-edge detection
        let rising = beat && !self.last_beat;
        self.last_beat = beat;

        if !rising {
            return TimelineEvent::None;
        }

        if let AdvanceMode::BeatSync { beats_per_cue } = self.advance_mode {
            self.beat_count += 1;
            if self.beat_count >= beats_per_cue {
                self.beat_count = 0;
                return self.go_next();
            }
        }

        TimelineEvent::None
    }

    /// Get a read-only snapshot for UI.
    pub fn info(&self) -> TimelineInfo {
        let current = self.current_cue_index();
        let state = match &self.state {
            PlaybackState::Idle => TimelineInfoState::Idle,
            PlaybackState::Holding { elapsed, cue_index } => {
                let hold = self.cues.get(*cue_index).and_then(|c| c.hold_secs);
                TimelineInfoState::Holding {
                    elapsed: *elapsed,
                    hold_secs: hold,
                }
            }
            PlaybackState::Transitioning {
                from_cue,
                to_cue,
                progress,
                transition_type,
                ..
            } => TimelineInfoState::Transitioning {
                from: *from_cue,
                to: *to_cue,
                progress: *progress,
                transition_type: *transition_type,
            },
        };

        TimelineInfo {
            active: self.active,
            cue_count: self.cues.len(),
            current_cue: current,
            state,
            loop_mode: self.loop_mode,
            advance_mode: self.advance_mode,
        }
    }

    /// Get current cue index regardless of state.
    pub fn current_cue_index(&self) -> usize {
        match &self.state {
            PlaybackState::Idle => 0,
            PlaybackState::Holding { cue_index, .. } => *cue_index,
            PlaybackState::Transitioning { to_cue, .. } => *to_cue,
        }
    }

    /// Get the next cue index, respecting loop mode.
    fn next_cue_index(&self, current: usize) -> Option<usize> {
        if current + 1 < self.cues.len() {
            Some(current + 1)
        } else if self.loop_mode {
            Some(0)
        } else {
            None
        }
    }

    /// Begin a transition from one cue to another.
    fn begin_transition(&mut self, from: usize, to: usize) -> TimelineEvent {
        let cue = &self.cues[to];
        let transition_type = cue.transition;
        let duration = cue.transition_secs;

        match transition_type {
            TransitionType::Cut => {
                // Instant switch
                self.state = PlaybackState::Holding {
                    cue_index: to,
                    elapsed: 0.0,
                };
                self.beat_count = 0;
                TimelineEvent::LoadCue { cue_index: to }
            }
            TransitionType::Dissolve | TransitionType::ParamMorph => {
                self.state = PlaybackState::Transitioning {
                    from_cue: from,
                    to_cue: to,
                    progress: 0.0,
                    transition_type,
                    duration,
                };
                self.beat_count = 0;
                TimelineEvent::BeginTransition {
                    from_cue: from,
                    to_cue: to,
                    transition_type,
                    duration,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::types::SceneCue;

    fn make_cues() -> Vec<SceneCue> {
        vec![
            SceneCue::new("Preset A"),
            SceneCue {
                preset_name: "Preset B".to_string(),
                transition: TransitionType::Dissolve,
                transition_secs: 2.0,
                hold_secs: Some(3.0),
                label: None,
                param_overrides: Vec::new(),
                transition_beats: None,
            },
            SceneCue {
                preset_name: "Preset C".to_string(),
                transition: TransitionType::ParamMorph,
                transition_secs: 1.5,
                hold_secs: None,
                label: None,
                param_overrides: Vec::new(),
                transition_beats: None,
            },
        ]
    }

    #[test]
    fn timeline_start_emits_load_cue() {
        let mut tl = Timeline::new(make_cues(), false, AdvanceMode::Manual);
        let event = tl.start(0);
        assert!(matches!(event, TimelineEvent::LoadCue { cue_index: 0 }));
        assert!(tl.active);
        assert_eq!(tl.current_cue_index(), 0);
    }

    #[test]
    fn timeline_start_clamps_index() {
        let mut tl = Timeline::new(make_cues(), false, AdvanceMode::Manual);
        let event = tl.start(99);
        assert!(matches!(event, TimelineEvent::LoadCue { cue_index: 2 }));
    }

    #[test]
    fn timeline_go_next_with_cut() {
        let cues = vec![
            SceneCue::new("A"),
            SceneCue::new("B"), // Cut transition
        ];
        let mut tl = Timeline::new(cues, false, AdvanceMode::Manual);
        tl.start(0);
        let event = tl.go_next();
        // Cue B has Cut transition → immediate load
        assert!(matches!(event, TimelineEvent::LoadCue { cue_index: 1 }));
        assert_eq!(tl.current_cue_index(), 1);
    }

    #[test]
    fn timeline_go_next_with_dissolve() {
        let mut tl = Timeline::new(make_cues(), false, AdvanceMode::Manual);
        tl.start(0);
        let event = tl.go_next();
        // Cue B has Dissolve 2.0s
        assert!(matches!(
            event,
            TimelineEvent::BeginTransition {
                from_cue: 0,
                to_cue: 1,
                transition_type: TransitionType::Dissolve,
                duration,
            } if (duration - 2.0).abs() < 1e-6
        ));
    }

    #[test]
    fn timeline_transition_progress() {
        let mut tl = Timeline::new(make_cues(), false, AdvanceMode::Manual);
        tl.start(0);
        tl.go_next(); // starts dissolve from 0→1, 2.0s

        // Tick 1 second → progress ~0.5
        let event = tl.tick(1.0);
        assert!(matches!(
            event,
            TimelineEvent::TransitionProgress { progress, .. } if (progress - 0.5).abs() < 1e-6
        ));

        // Tick another 1.5 second → completes (clamped to 1.0)
        let event = tl.tick(1.5);
        assert!(matches!(
            event,
            TimelineEvent::TransitionComplete { cue_index: 1 }
        ));
        assert_eq!(tl.current_cue_index(), 1);
    }

    #[test]
    fn timeline_no_loop_stops_at_end() {
        let cues = vec![SceneCue::new("A"), SceneCue::new("B")];
        let mut tl = Timeline::new(cues, false, AdvanceMode::Manual);
        tl.start(0);
        tl.go_next(); // → cue 1
        let event = tl.go_next(); // no more cues
        assert!(matches!(event, TimelineEvent::None));
        assert!(!tl.active);
    }

    #[test]
    fn timeline_loop_wraps_around() {
        let cues = vec![SceneCue::new("A"), SceneCue::new("B")];
        let mut tl = Timeline::new(cues, true, AdvanceMode::Manual);
        tl.start(0);
        tl.go_next(); // → cue 1
        let event = tl.go_next(); // loop → cue 0
        assert!(matches!(event, TimelineEvent::LoadCue { cue_index: 0 }));
        assert!(tl.active);
    }

    #[test]
    fn timeline_go_prev() {
        let mut tl = Timeline::new(make_cues(), false, AdvanceMode::Manual);
        tl.start(1);
        let event = tl.go_prev();
        // Going back to cue 0 uses cue 0's transition (Cut)
        assert!(matches!(event, TimelineEvent::LoadCue { cue_index: 0 }));
    }

    #[test]
    fn timeline_go_prev_at_start_no_loop() {
        let mut tl = Timeline::new(make_cues(), false, AdvanceMode::Manual);
        tl.start(0);
        let event = tl.go_prev();
        assert!(matches!(event, TimelineEvent::None));
    }

    #[test]
    fn timeline_go_to_cue() {
        let mut tl = Timeline::new(make_cues(), false, AdvanceMode::Manual);
        tl.start(0);
        let event = tl.go_to_cue(2);
        // Cue 2 has ParamMorph 1.5s
        assert!(matches!(
            event,
            TimelineEvent::BeginTransition {
                from_cue: 0,
                to_cue: 2,
                transition_type: TransitionType::ParamMorph,
                ..
            }
        ));
    }

    #[test]
    fn timeline_timer_auto_advance() {
        let mut tl = Timeline::new(make_cues(), false, AdvanceMode::Timer);
        tl.start(0);
        // Cue 0 has no hold_secs → stays forever
        let event = tl.tick(10.0);
        assert!(matches!(event, TimelineEvent::None));

        // Jump to cue 1 (hold_secs = 3.0)
        tl.go_to_cue(1);
        // Partial dissolve transition (2s duration)
        let event = tl.tick(1.0); // progress = 0.5
        assert!(matches!(event, TimelineEvent::TransitionProgress { .. }));
        // Complete the dissolve transition
        let event = tl.tick(1.5); // progress = 0.5 + 1.5/2.0 = 1.25 → clamped to 1.0
        assert!(matches!(event, TimelineEvent::TransitionComplete { .. }));
        // Now holding on cue 1 with elapsed = 0.0
        let event = tl.tick(3.0);
        // Should auto-advance to cue 2
        assert!(matches!(
            event,
            TimelineEvent::BeginTransition { to_cue: 2, .. }
        ));
    }

    #[test]
    fn timeline_beat_sync_advances() {
        let cues = vec![SceneCue::new("A"), SceneCue::new("B"), SceneCue::new("C")];
        let mut tl = Timeline::new(cues, false, AdvanceMode::BeatSync { beats_per_cue: 4 });
        tl.start(0);

        // Feed 3 beats — not enough
        for _ in 0..3 {
            let ev = tl.feed_beat(true);
            assert!(matches!(ev, TimelineEvent::None));
            tl.feed_beat(false); // release
        }

        // 4th beat → advance
        let ev = tl.feed_beat(true);
        assert!(matches!(ev, TimelineEvent::LoadCue { cue_index: 1 }));
    }

    #[test]
    fn timeline_info_snapshot() {
        let mut tl = Timeline::new(make_cues(), true, AdvanceMode::Manual);
        tl.start(0);
        let info = tl.info();
        assert!(info.active);
        assert_eq!(info.cue_count, 3);
        assert_eq!(info.current_cue, 0);
        assert!(info.loop_mode);
        assert!(matches!(info.state, TimelineInfoState::Holding { .. }));
    }

    #[test]
    fn timeline_stop() {
        let mut tl = Timeline::new(make_cues(), false, AdvanceMode::Manual);
        tl.start(0);
        assert!(tl.active);
        tl.stop();
        assert!(!tl.active);
        assert!(matches!(tl.state, PlaybackState::Idle));
    }

    #[test]
    fn timeline_empty_cues() {
        let mut tl = Timeline::new(vec![], false, AdvanceMode::Manual);
        let event = tl.start(0);
        assert!(matches!(event, TimelineEvent::None));
        let event = tl.tick(1.0);
        assert!(matches!(event, TimelineEvent::None));
    }
}
