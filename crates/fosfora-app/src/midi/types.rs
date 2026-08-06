use serde::{Deserialize, Serialize};

/// MIDI message type (CC or Note).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MidiMsgType {
    Cc,
    Note,
}

/// Parsed MIDI message — small enough to Copy through channels.
#[derive(Debug, Clone, Copy)]
pub struct MidiMessage {
    pub msg_type: MidiMsgType,
    pub channel: u8,
    pub number: u8,
    pub value: u8,
}

/// Actions that can be triggered by a MIDI button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TriggerAction {
    NextEffect,
    PrevEffect,
    TogglePostProcess,
    ToggleOverlay,
    NextPreset,
    PrevPreset,
    NextLayer,
    PrevLayer,
    SceneGoNext,
    SceneGoPrev,
    ToggleTimeline,
    /// A7 (#1458): force the detected tempo down/up an octave, and tap tempo — the controls
    /// every hardware tool puts under a thumb, so they need to reach a MIDI pad.
    TempoHalf,
    TempoDouble,
    TempoTap,
}

impl TriggerAction {
    #[allow(dead_code)]
    pub const ALL: &[TriggerAction] = &[
        TriggerAction::NextEffect,
        TriggerAction::PrevEffect,
        TriggerAction::TogglePostProcess,
        TriggerAction::ToggleOverlay,
        TriggerAction::NextPreset,
        TriggerAction::PrevPreset,
        TriggerAction::NextLayer,
        TriggerAction::PrevLayer,
        TriggerAction::SceneGoNext,
        TriggerAction::SceneGoPrev,
        TriggerAction::ToggleTimeline,
        TriggerAction::TempoHalf,
        TriggerAction::TempoDouble,
        TriggerAction::TempoTap,
    ];

    pub fn display_name(&self) -> &'static str {
        match self {
            TriggerAction::NextEffect => "Next Effect",
            TriggerAction::PrevEffect => "Prev Effect",
            TriggerAction::TogglePostProcess => "Toggle Post-Process",
            TriggerAction::ToggleOverlay => "Toggle Overlay",
            TriggerAction::NextPreset => "Next Preset",
            TriggerAction::PrevPreset => "Prev Preset",
            TriggerAction::NextLayer => "Next Layer",
            TriggerAction::PrevLayer => "Prev Layer",
            TriggerAction::SceneGoNext => "Scene Next",
            TriggerAction::SceneGoPrev => "Scene Prev",
            TriggerAction::ToggleTimeline => "Toggle Timeline",
            TriggerAction::TempoHalf => "Tempo Half",
            TriggerAction::TempoDouble => "Tempo Double",
            TriggerAction::TempoTap => "Tap Tempo",
        }
    }

    pub fn short_name(&self) -> &'static str {
        match self {
            TriggerAction::NextEffect => "Next Fx",
            TriggerAction::PrevEffect => "Prev Fx",
            TriggerAction::TogglePostProcess => "Post-Fx",
            TriggerAction::ToggleOverlay => "Overlay",
            TriggerAction::NextPreset => "Next Pre",
            TriggerAction::PrevPreset => "Prev Pre",
            TriggerAction::NextLayer => "Next Lyr",
            TriggerAction::PrevLayer => "Prev Lyr",
            TriggerAction::SceneGoNext => "Scn Next",
            TriggerAction::SceneGoPrev => "Scn Prev",
            TriggerAction::ToggleTimeline => "Timeline",
            TriggerAction::TempoHalf => "Tempo /2",
            TriggerAction::TempoDouble => "Tempo x2",
            TriggerAction::TempoTap => "Tap",
        }
    }
}

/// What we're learning a MIDI mapping for.
#[derive(Debug, Clone, PartialEq)]
pub enum LearnTarget {
    Param(String),
    Trigger(TriggerAction),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trigger_action_all_count() {
        assert_eq!(TriggerAction::ALL.len(), 14);
    }

    #[test]
    fn trigger_action_display_names_non_empty() {
        for action in TriggerAction::ALL {
            assert!(!action.display_name().is_empty());
        }
    }

    #[test]
    fn trigger_action_short_names_non_empty() {
        for action in TriggerAction::ALL {
            assert!(!action.short_name().is_empty());
        }
    }

    #[test]
    fn trigger_action_serde_roundtrip() {
        for action in TriggerAction::ALL {
            let json = serde_json::to_string(action).unwrap();
            let a2: TriggerAction = serde_json::from_str(&json).unwrap();
            assert_eq!(*action, a2);
        }
    }

    #[test]
    fn midi_msg_type_serde_roundtrip() {
        for t in [MidiMsgType::Cc, MidiMsgType::Note] {
            let json = serde_json::to_string(&t).unwrap();
            let t2: MidiMsgType = serde_json::from_str(&json).unwrap();
            assert_eq!(t, t2);
        }
    }
}
