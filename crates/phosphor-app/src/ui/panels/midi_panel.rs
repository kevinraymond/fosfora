use egui::{Color32, RichText, Ui};

use crate::midi::MidiSystem;

pub fn draw_midi_panel(ui: &mut Ui, midi: &mut MidiSystem) {
    // Port selector
    let current_label = midi
        .connected_port()
        .unwrap_or("Not connected")
        .to_string();

    egui::ComboBox::from_id_salt("midi_port")
        .selected_text(&current_label)
        .width((ui.available_width() - 8.0).max(1.0))
        .show_ui(ui, |ui| {
            // Disconnect option
            if ui
                .selectable_label(midi.connected_port().is_none(), "Not connected")
                .clicked()
            {
                midi.disconnect();
            }

            // Available ports
            for port in &midi.available_ports.clone() {
                let is_selected = midi.connected_port() == Some(port.as_str());
                if ui.selectable_label(is_selected, port).clicked() && !is_selected {
                    midi.connect(port);
                }
            }

            if midi.available_ports.is_empty() {
                ui.label(RichText::new("No MIDI ports found").weak().italics());
            }
        });

    ui.add_space(4.0);

    // Activity indicator + last message
    ui.horizontal(|ui| {
        // Activity dot
        let color = if midi.is_recently_active() {
            Color32::from_rgb(0x50, 0xC0, 0x70) // green
        } else if midi.connected_port().is_some() {
            Color32::from_rgb(0x60, 0x60, 0x60) // dim grey
        } else {
            Color32::from_rgb(0x40, 0x40, 0x40) // dark grey
        };
        let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
        ui.painter().circle_filled(rect.center(), 5.0, color);

        // Last message display
        if let Some(msg) = midi.last_message {
            ui.label(
                RichText::new(format!(
                    "{:?} #{} val:{} ch:{}",
                    msg.msg_type, msg.number, msg.value, msg.channel
                ))
                .small()
                .color(Color32::from_rgb(0xA0, 0xA0, 0xA0)),
            );
        } else if midi.connected_port().is_some() {
            ui.label(RichText::new("Waiting for input...").small().weak());
        } else {
            ui.label(RichText::new("No device").small().weak());
        }
    });

    // Learn status (only shown when active)
    if let Some(ref target) = midi.learn_target {
        let label = match target {
            crate::midi::types::LearnTarget::Param(name) => format!("Move a knob for \"{}\"", name),
            crate::midi::types::LearnTarget::Trigger(action) => {
                format!("Press a button for \"{}\"", action.display_name())
            }
        };
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let t = ui.input(|i| i.time) as f32;
            let alpha = ((t * 3.0).sin() * 0.3 + 0.7).clamp(0.4, 1.0);
            let color = Color32::from_rgba_unmultiplied(0xE0, 0xA0, 0x40, (alpha * 255.0) as u8);
            ui.colored_label(color, label);
        });
        ui.ctx().request_repaint();
    }
}
