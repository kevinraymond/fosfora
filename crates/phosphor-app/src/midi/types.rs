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
}

impl TriggerAction {
    pub const ALL: &[TriggerAction] = &[
        TriggerAction::NextEffect,
        TriggerAction::PrevEffect,
        TriggerAction::TogglePostProcess,
        TriggerAction::ToggleOverlay,
        TriggerAction::NextPreset,
        TriggerAction::PrevPreset,
        TriggerAction::NextLayer,
        TriggerAction::PrevLayer,
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
        }
    }
}

/// What we're learning a MIDI mapping for.
#[derive(Debug, Clone, PartialEq)]
pub enum LearnTarget {
    Param(String),
    Trigger(TriggerAction),
}
