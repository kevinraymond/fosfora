use super::definition::{scheduled_index, SceneCue, TransitionKind};

/// Edge-triggered scene scheduler. Manual NEXT/PREV changes the active visual
/// without moving `last_processed_scheduled_index`.
#[derive(Debug, Clone, Default)]
pub struct SceneScheduler {
    pub last_processed_scheduled_index: Option<usize>,
    pub active_preset_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledSceneAction {
    pub preset_id: String,
    pub kind: TransitionKind,
    pub duration_ms: u32,
    /// True when this came from SEEK (always snap/cut).
    pub snap: bool,
}

impl SceneScheduler {
    pub fn scheduled_index(cues: &[SceneCue], elapsed_secs: u32) -> Option<usize> {
        scheduled_index(cues.iter().map(|c| c.at_secs), elapsed_secs)
    }

    /// Only a change of scheduled index triggers an automatic action.
    pub fn tick(&mut self, cues: &[SceneCue], elapsed_secs: u32) -> Option<ScheduledSceneAction> {
        let idx = Self::scheduled_index(cues, elapsed_secs);
        if idx == self.last_processed_scheduled_index {
            return None;
        }
        self.last_processed_scheduled_index = idx;
        let cue = idx.and_then(|i| cues.get(i))?;
        if self.active_preset_id.as_deref() == Some(cue.preset_id.as_str()) {
            return None;
        }
        self.active_preset_id = Some(cue.preset_id.clone());
        Some(ScheduledSceneAction {
            preset_id: cue.preset_id.clone(),
            kind: cue.transition.kind,
            duration_ms: cue.transition.duration_ms,
            snap: false,
        })
    }

    /// Manual override: change the look, not the scheduled index.
    pub fn manual_set_preset(&mut self, preset_id: String) {
        self.active_preset_id = Some(preset_id);
    }

    pub fn seek_snap(&mut self, cues: &[SceneCue], elapsed_secs: u32) -> Option<ScheduledSceneAction> {
        let idx = Self::scheduled_index(cues, elapsed_secs);
        self.last_processed_scheduled_index = idx;
        let cue = idx.and_then(|i| cues.get(i))?;
        self.active_preset_id = Some(cue.preset_id.clone());
        Some(ScheduledSceneAction {
            preset_id: cue.preset_id.clone(),
            kind: TransitionKind::Cut,
            duration_ms: 0,
            snap: true,
        })
    }

    pub fn start_cut(&mut self, cues: &[SceneCue]) -> Option<ScheduledSceneAction> {
        self.last_processed_scheduled_index = None;
        self.active_preset_id = None;
        self.tick(cues, 0).map(|mut action| {
            action.kind = TransitionKind::Cut;
            action.duration_ms = 0;
            action.snap = true;
            action
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::show::definition::SceneTransition;

    fn cue(at: u32, id: &str) -> SceneCue {
        SceneCue {
            at_secs: at,
            preset_id: id.into(),
            transition: SceneTransition {
                kind: TransitionKind::Dissolve,
                duration_ms: 8000,
            },
        }
    }

    fn hibernation_like() -> Vec<SceneCue> {
        vec![
            SceneCue {
                at_secs: 0,
                preset_id: "A".into(),
                transition: SceneTransition {
                    kind: TransitionKind::Cut,
                    duration_ms: 0,
                },
            },
            cue(20 * 60, "B"),
            cue(60 * 60, "C"),
        ]
    }

    #[test]
    fn exact_boundary_at_twenty_minutes() {
        let cues = hibernation_like();
        let mut s = SceneScheduler::default();
        let first = s.tick(&cues, 0).unwrap();
        assert_eq!(first.preset_id, "A");
        assert!(s.tick(&cues, 20 * 60 - 1).is_none());
        let at_b = s.tick(&cues, 20 * 60).unwrap();
        assert_eq!(at_b.preset_id, "B");
        assert_eq!(at_b.duration_ms, 8000);
    }

    #[test]
    fn manual_next_does_not_move_index() {
        let cues = hibernation_like();
        let mut s = SceneScheduler::default();
        s.tick(&cues, 0);
        s.manual_set_preset("B".into());
        // 15:00 still index 0
        assert!(s.tick(&cues, 15 * 60).is_none());
        // 20:00 index 1, already showing B → no-op
        assert!(s.tick(&cues, 20 * 60).is_none());
        let to_c = s.tick(&cues, 60 * 60).unwrap();
        assert_eq!(to_c.preset_id, "C");
    }

    #[test]
    fn stall_jump_resolves_to_latest() {
        let cues = hibernation_like();
        let mut s = SceneScheduler::default();
        s.tick(&cues, 0);
        let jumped = s.tick(&cues, 60 * 60 + 3).unwrap();
        assert_eq!(jumped.preset_id, "C");
    }

    #[test]
    fn index_advance_noop_when_already_active() {
        let cues = hibernation_like();
        let mut s = SceneScheduler::default();
        s.tick(&cues, 0);
        s.manual_set_preset("B".into());
        assert!(s.tick(&cues, 20 * 60).is_none());
    }

    #[test]
    fn transitions_do_not_alter_schedule_length() {
        let cues = hibernation_like();
        assert_eq!(SceneScheduler::scheduled_index(&cues, 20 * 60 - 1), Some(0));
        assert_eq!(SceneScheduler::scheduled_index(&cues, 20 * 60), Some(1));
        // 8s dissolve does not shift the next cue
        assert_eq!(
            SceneScheduler::scheduled_index(&cues, 20 * 60 + 8),
            Some(1)
        );
    }
}
