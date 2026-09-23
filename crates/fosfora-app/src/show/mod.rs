pub mod clock;
pub mod controller;
pub mod definition;
pub mod pack;
pub mod scheduler;
pub mod validation;

use crate::scene::timeline::TimelineEvent;
use crate::scene::types::{SceneCue, TransitionType};
use crate::show::definition::TransitionKind;

use std::path::PathBuf;
use std::sync::OnceLock;

/// `--show` path captured before the window exists.
pub static SHOW_PACK_ARG: OnceLock<PathBuf> = OnceLock::new();

pub fn parse_show_arg(args: &[String]) -> Option<PathBuf> {
    let i = args.iter().position(|a| a == "--show")?;
    args.get(i + 1)
        .filter(|a| !a.starts_with("--"))
        .map(PathBuf::from)
}

/// Translate a show cue transition into the native scene-timeline types.
/// MVP mapping: cut -> Cut, dissolve -> Dissolve with ms->secs. ParamMorph is
/// deliberately not addressable from a show pack — it returns None so callers
/// fall back to Dissolve rather than silently switching semantics.
pub fn timeline_event_for_cue(
    to_cue: usize,
    kind: TransitionKind,
    duration_ms: u32,
) -> TimelineEvent {
    match kind {
        TransitionKind::Cut => TimelineEvent::LoadCue { cue_index: to_cue },
        TransitionKind::Dissolve | TransitionKind::ParamMorph => {
            TimelineEvent::BeginTransition {
                from_cue: to_cue.saturating_sub(1),
                to_cue,
                transition_type: TransitionType::Dissolve,
                duration: duration_ms as f32 / 1000.0,
            }
        }
    }
}

/// Build a one-cue scene timeline entry per show-pack preset so the native
/// engine can execute show transitions (incl. the dissolve snapshot path)
/// without duplicating renderer logic in the show layer.
pub fn build_timeline_cues(
    preset_ids: &[String],
    default_transition_secs: f32,
) -> Vec<SceneCue> {
    preset_ids
        .iter()
        .map(|id| SceneCue {
            preset_name: id.clone(),
            transition: TransitionType::Dissolve,
            transition_secs: default_transition_secs,
            hold_secs: None,
            label: None,
            param_overrides: Vec::new(),
            transition_beats: None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_show_path() {
        let args = vec![
            "fosfora".into(),
            "--show".into(),
            "/tmp/show.json".into(),
        ];
        assert_eq!(
            parse_show_arg(&args),
            Some(PathBuf::from("/tmp/show.json"))
        );
    }
}
