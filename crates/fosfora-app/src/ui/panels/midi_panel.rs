use egui::{Color32, RichText, Ui};

use crate::midi::MidiSystem;
use crate::midi::types::{LearnTarget, MidiMsgType, TriggerAction};
use crate::ui::theme::tokens::*;

const MIDI_BLUE: Color32 = Color32::from_rgb(0x60, 0xA0, 0xE0);

pub fn draw_midi_panel(ui: &mut Ui, midi: &mut MidiSystem) {
    // Enable checkbox
    let mut enabled = midi.config.enabled;
    if ui
        .checkbox(&mut enabled, RichText::new("Enable MIDI").size(SMALL_SIZE))
        .changed()
    {
        midi.set_enabled(enabled);
    }

    // Port selector
    let current_label = midi.connected_port().unwrap_or("Not connected").to_string();

    egui::ComboBox::from_id_salt("midi_port")
        .selected_text(RichText::new(&current_label).size(SMALL_SIZE))
        .width((ui.available_width() - 8.0).max(1.0))
        .show_ui(ui, |ui| {
            if ui
                .selectable_label(midi.connected_port().is_none(), "Not connected")
                .clicked()
            {
                midi.disconnect();
            }
            let mut connect_port = None;
            for port in &midi.available_ports {
                let is_selected = midi.connected_port() == Some(port.as_str());
                if ui.selectable_label(is_selected, port).clicked() && !is_selected {
                    connect_port = Some(port.clone());
                }
            }
            if let Some(port) = connect_port {
                midi.connect(&port);
            }
            if midi.available_ports.is_empty() {
                ui.label(
                    RichText::new("No MIDI ports")
                        .weak()
                        .italics()
                        .size(SMALL_SIZE),
                );
            }
        });

    // Learn status (conditional)
    if let Some(ref learn_target) = midi.learn_target {
        let label = match learn_target {
            LearnTarget::Param(name) => format!("Move knob for \"{name}\""),
            LearnTarget::Trigger(action) => format!("Press btn for \"{}\"", action.display_name()),
        };
        let t = ui.input(|i| i.time) as f32;
        let alpha = ((t * 3.0).sin() * 0.3 + 0.7).clamp(0.4, 1.0);
        let color = Color32::from_rgba_unmultiplied(0xE0, 0xA0, 0x40, (alpha * 255.0) as u8);
        ui.label(RichText::new(label).size(SMALL_SIZE).color(color));
        ui.ctx().request_repaint();
    }
}

/// One MIDI mapping badge for `action` — unmapped "M" / learning ".." / mapped
/// "CCn"/"Nn". Shared with the unified Triggers table.
pub(crate) fn draw_trigger_badge(ui: &mut Ui, midi: &mut MidiSystem, action: TriggerAction) {
    let is_learning = midi.learn_target == Some(LearnTarget::Trigger(action));
    let is_mapped = midi.config.triggers.contains_key(&action);

    if is_learning {
        let t = ui.input(|i| i.time) as f32;
        let alpha = ((t * 4.0).sin() * 0.3 + 0.7).clamp(0.4, 1.0);
        let color = Color32::from_rgba_unmultiplied(0xE0, 0xA0, 0x40, (alpha * 255.0) as u8);
        if ui
            .button(RichText::new("..").color(color).size(SMALL_SIZE))
            .on_hover_text("Cancel")
            .clicked()
        {
            midi.cancel_learn();
        }
        ui.ctx().request_repaint();
    } else if is_mapped {
        let mapping = &midi.config.triggers[&action];
        let label = match mapping.msg_type {
            MidiMsgType::Cc => format!("CC{}", mapping.cc),
            MidiMsgType::Note => format!("N{}", mapping.cc),
        };
        let resp = ui
            .button(RichText::new(&label).color(MIDI_BLUE).size(SMALL_SIZE))
            .on_hover_text("Click to re-learn, right-click to clear");
        if resp.clicked() {
            midi.start_learn(LearnTarget::Trigger(action));
        }
        if resp.secondary_clicked() {
            midi.clear_trigger_mapping(action);
        }
    } else {
        if ui
            .button(RichText::new("M").weak().size(SMALL_SIZE))
            .on_hover_text("MIDI learn")
            .clicked()
        {
            midi.start_learn(LearnTarget::Trigger(action));
        }
    }
}
