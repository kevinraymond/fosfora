use crate::scene::timeline::TimelineInfo;
use crate::scene::types::TransitionType;

/// Info passed from App to the scene panel (avoids borrow conflicts).
#[derive(Debug, Clone)]
pub struct SceneInfo {
    pub scene_store_names: Vec<String>,
    pub current_scene: Option<usize>,
    pub timeline: Option<TimelineInfo>,
    pub preset_names: Vec<String>,
    /// Cues currently in the timeline (name, transition, duration).
    pub cue_list: Vec<CueDisplayInfo>,
}

#[derive(Debug, Clone)]
pub struct CueDisplayInfo {
    pub preset_name: String,
    pub transition: TransitionType,
    pub transition_secs: f32,
    pub hold_secs: Option<f32>,
    /// The cue's own name, when it has one.
    pub label: Option<String>,
    /// The preset's first effect, whose catalog picture stands for the cue.
    pub effect: Option<String>,
    /// How many layers the preset has.
    pub layers: usize,
}
