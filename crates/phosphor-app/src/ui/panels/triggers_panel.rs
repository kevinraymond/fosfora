//! Unified trigger-mapping table: one row per action, a MIDI column and an
//! OSC column. Replaces the two per-protocol trigger grids that repeated the
//! same ten actions (and their layout code) once per protocol.

use egui::{RichText, Ui};

use crate::midi::MidiSystem;
use crate::midi::types::TriggerAction;
use crate::osc::OscSystem;
use crate::ui::panels::{midi_panel, osc_panel};
use crate::ui::theme::colors::theme_colors;
use crate::ui::theme::tokens::*;

/// The single source of truth for the mappable actions (was duplicated as a
/// `TRIGGER_PAIRS` const in both `midi_panel` and `osc_panel`).
const TRIGGER_ACTIONS: &[TriggerAction] = &[
    TriggerAction::NextEffect,
    TriggerAction::PrevEffect,
    TriggerAction::NextPreset,
    TriggerAction::PrevPreset,
    TriggerAction::NextLayer,
    TriggerAction::PrevLayer,
    TriggerAction::TogglePostProcess,
    TriggerAction::ToggleOverlay,
    TriggerAction::SceneGoNext,
    TriggerAction::SceneGoPrev,
];

pub fn draw_triggers_table(ui: &mut Ui, midi: &mut MidiSystem, osc: &mut OscSystem) {
    let tc = theme_colors(ui.ctx());

    let label_w = 64.0;
    let badge_w = ((ui.available_width() - label_w - 12.0) / 2.0).max(40.0);

    egui::Grid::new("triggers_table")
        .num_columns(3)
        .min_col_width(0.0)
        .spacing([4.0, 3.0])
        .show(ui, |ui| {
            // Header row
            ui.add_sized([label_w, 12.0], egui::Label::new(""));
            for proto in ["MIDI", "OSC"] {
                ui.add_sized(
                    [badge_w, 12.0],
                    egui::Label::new(RichText::new(proto).size(8.0).color(tc.text_secondary)),
                );
            }
            ui.end_row();

            for &action in TRIGGER_ACTIONS {
                ui.add_sized(
                    [label_w, MIN_INTERACT_HEIGHT],
                    egui::Label::new(
                        RichText::new(action.short_name())
                            .size(SMALL_SIZE)
                            .color(tc.text_secondary),
                    ),
                );
                ui.add_sized([badge_w, MIN_INTERACT_HEIGHT], |ui: &mut Ui| {
                    ui.horizontal(|ui| midi_panel::draw_trigger_badge(ui, midi, action))
                        .response
                });
                ui.add_sized([badge_w, MIN_INTERACT_HEIGHT], |ui: &mut Ui| {
                    ui.horizontal(|ui| osc_panel::draw_osc_trigger_badge(ui, osc, action))
                        .response
                });
                ui.end_row();
            }
        });
}
