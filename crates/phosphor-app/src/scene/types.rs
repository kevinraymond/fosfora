use std::collections::HashMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::params::ParamValue;

/// How to transition between cues.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TransitionType {
    /// Instant switch (load new preset immediately).
    #[default]
    Cut,
    /// GPU crossfade between outgoing and incoming.
    Dissolve,
    /// Interpolate all params, opacities, and blend modes each frame.
    ParamMorph,
}

impl TransitionType {
    pub const ALL: &[TransitionType] = &[
        TransitionType::Cut,
        TransitionType::Dissolve,
        TransitionType::ParamMorph,
    ];

    pub fn display_name(&self) -> &'static str {
        match self {
            TransitionType::Cut => "Cut",
            TransitionType::Dissolve => "Dissolve",
            TransitionType::ParamMorph => "Morph",
        }
    }
}

impl fmt::Display for TransitionType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.display_name())
    }
}

/// How the timeline advances between cues.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AdvanceMode {
    /// Manual advance only (Space / MIDI / OSC).
    #[default]
    Manual,
    /// Auto-advance after hold_secs timer.
    Timer,
    /// Beat-synced: advance every N beats from MIDI clock.
    BeatSync { beats_per_cue: u32 },
}

impl AdvanceMode {
    pub fn display_name(&self) -> &'static str {
        match self {
            AdvanceMode::Manual => "Manual",
            AdvanceMode::Timer => "Timer",
            AdvanceMode::BeatSync { .. } => "Beat Sync",
        }
    }
}

/// A single cue in a scene — references a preset and describes the transition into it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneCue {
    /// Preset name to load (looked up in PresetStore).
    pub preset_name: String,
    /// How to transition into this cue.
    #[serde(default)]
    pub transition: TransitionType,
    /// Transition duration in seconds (ignored for Cut).
    #[serde(default = "default_transition_secs")]
    pub transition_secs: f32,
    /// How long to hold on this cue before auto-advancing (None = manual advance).
    #[serde(default)]
    pub hold_secs: Option<f32>,
    /// Optional display label for the cue.
    #[serde(default)]
    pub label: Option<String>,
    /// Per-layer param overrides (layer index → param name → value).
    #[serde(default)]
    pub param_overrides: Vec<HashMap<String, ParamValue>>,
    /// Optional transition duration in beats (used with BeatSync; overrides transition_secs).
    #[serde(default)]
    pub transition_beats: Option<u32>,
}

fn default_transition_secs() -> f32 {
    1.0
}

impl SceneCue {
    /// Create a minimal cue referencing a preset.
    pub fn new(preset_name: &str) -> Self {
        Self {
            preset_name: preset_name.to_string(),
            transition: TransitionType::Cut,
            transition_secs: 1.0,
            hold_secs: None,
            label: None,
            param_overrides: Vec::new(),
            transition_beats: None,
        }
    }

    /// Display label, falling back to preset name.
    pub fn display_name(&self) -> &str {
        self.label
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or(&self.preset_name)
    }
}

/// A named set of cues forming a scene.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneSet {
    /// Version for future format changes.
    #[serde(default = "default_version")]
    pub version: u32,
    /// Scene name.
    pub name: String,
    /// Ordered list of cues.
    pub cues: Vec<SceneCue>,
    /// Whether the timeline loops back to cue 0 after the last cue.
    #[serde(default)]
    pub loop_mode: bool,
    /// How the timeline advances.
    #[serde(default)]
    pub advance_mode: AdvanceMode,
}

fn default_version() -> u32 {
    1
}

impl SceneSet {
    pub fn new(name: &str) -> Self {
        Self {
            version: 1,
            name: name.to_string(),
            cues: Vec::new(),
            loop_mode: false,
            advance_mode: AdvanceMode::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transition_type_display_names() {
        assert_eq!(TransitionType::Cut.display_name(), "Cut");
        assert_eq!(TransitionType::Dissolve.display_name(), "Dissolve");
        assert_eq!(TransitionType::ParamMorph.display_name(), "Morph");
    }

    #[test]
    fn transition_type_serde_roundtrip() {
        for t in TransitionType::ALL {
            let json = serde_json::to_string(t).unwrap();
            let t2: TransitionType = serde_json::from_str(&json).unwrap();
            assert_eq!(*t, t2);
        }
    }

    #[test]
    fn scene_cue_defaults() {
        let json = r#"{"preset_name": "Crucible"}"#;
        let cue: SceneCue = serde_json::from_str(json).unwrap();
        assert_eq!(cue.preset_name, "Crucible");
        assert_eq!(cue.transition, TransitionType::Cut);
        assert!((cue.transition_secs - 1.0).abs() < 1e-6);
        assert!(cue.hold_secs.is_none());
        assert!(cue.label.is_none());
        assert!(cue.param_overrides.is_empty());
    }

    #[test]
    fn scene_cue_display_name_with_label() {
        let mut cue = SceneCue::new("Crucible");
        assert_eq!(cue.display_name(), "Crucible");
        cue.label = Some("Intro".to_string());
        assert_eq!(cue.display_name(), "Intro");
        cue.label = Some("".to_string());
        assert_eq!(cue.display_name(), "Crucible");
    }

    #[test]
    fn scene_set_serde_roundtrip() {
        let mut scene = SceneSet::new("My Scene");
        scene.cues.push(SceneCue::new("Crucible"));
        scene.cues.push(SceneCue {
            preset_name: "Spectral Eye".to_string(),
            transition: TransitionType::Dissolve,
            transition_secs: 2.0,
            hold_secs: Some(4.0),
            label: Some("Build".to_string()),
            param_overrides: Vec::new(),
            transition_beats: None,
        });
        scene.loop_mode = true;

        let json = serde_json::to_string_pretty(&scene).unwrap();
        let s2: SceneSet = serde_json::from_str(&json).unwrap();
        assert_eq!(s2.name, "My Scene");
        assert_eq!(s2.cues.len(), 2);
        assert_eq!(s2.cues[1].transition, TransitionType::Dissolve);
        assert!((s2.cues[1].transition_secs - 2.0).abs() < 1e-6);
        assert!(s2.loop_mode);
    }

    #[test]
    fn advance_mode_default_is_manual() {
        assert_eq!(AdvanceMode::default(), AdvanceMode::Manual);
    }

    #[test]
    fn advance_mode_serde_roundtrip() {
        let modes = [
            AdvanceMode::Manual,
            AdvanceMode::Timer,
            AdvanceMode::BeatSync { beats_per_cue: 4 },
        ];
        for mode in &modes {
            let json = serde_json::to_string(mode).unwrap();
            let m2: AdvanceMode = serde_json::from_str(&json).unwrap();
            assert_eq!(*mode, m2);
        }
    }
}
