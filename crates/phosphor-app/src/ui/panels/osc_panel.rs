use egui::{Color32, RichText, Ui};

use crate::midi::types::TriggerAction;
use crate::osc::types::OscLearnTarget;
use crate::osc::OscSystem;
use crate::ui::theme::tokens::*;

const OSC_GREEN: Color32 = Color32::from_rgb(0x50, 0xC0, 0x70);

/// Trigger pairs for 2-column grid layout (same pairing as MIDI).
const TRIGGER_PAIRS: &[(TriggerAction, TriggerAction)] = &[
    (TriggerAction::NextEffect, TriggerAction::PrevEffect),
    (TriggerAction::NextPreset, TriggerAction::PrevPreset),
    (TriggerAction::NextLayer, TriggerAction::PrevLayer),
    (TriggerAction::TogglePostProcess, TriggerAction::ToggleOverlay),
];

pub fn draw_osc_panel(ui: &mut Ui, osc: &mut OscSystem) {
    // Enable + activity on one row
    ui.horizontal(|ui| {
        let mut enabled = osc.config.enabled;
        if ui
            .checkbox(&mut enabled, RichText::new("Enable OSC").size(SMALL_SIZE))
            .changed()
        {
            osc.set_enabled(enabled);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // Last address (right-aligned)
            if let Some(ref addr) = osc.last_address {
                let short = abbreviate_address(addr);
                ui.label(
                    RichText::new(short)
                        .size(SMALL_SIZE)
                        .color(DARK_TEXT_SECONDARY),
                );
            }
            // Activity dot
            let color = if osc.is_recently_active() {
                OSC_GREEN
            } else if osc.config.enabled {
                Color32::from_rgb(0x55, 0x55, 0x55)
            } else {
                Color32::from_rgb(0x33, 0x33, 0x33)
            };
            let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
            ui.painter().circle_filled(rect.center(), 4.0, color);
        });
    });

    // RX port
    ui.horizontal(|ui| {
        ui.label(RichText::new("RX").size(SMALL_SIZE));
        let mut port = osc.config.rx_port;
        let resp = ui.add(
            egui::DragValue::new(&mut port)
                .range(1024..=65535)
                .speed(1.0),
        );
        if resp.changed() {
            osc.config.rx_port = port;
            osc.config.save();
            osc.restart_receiver();
        }
    });

    ui.separator();

    // TX: enable + host on one line
    ui.horizontal(|ui| {
        let mut tx_enabled = osc.config.tx_enabled;
        if ui
            .checkbox(&mut tx_enabled, RichText::new("TX").size(SMALL_SIZE))
            .changed()
        {
            osc.set_tx_enabled(tx_enabled);
        }
        ui.label(RichText::new("Host").size(SMALL_SIZE));
        let mut host = osc.config.tx_host.clone();
        let resp = ui.add(
            egui::TextEdit::singleline(&mut host)
                .desired_width(ui.available_width().max(60.0))
                .font(egui::TextStyle::Small),
        );
        if resp.changed() {
            osc.config.tx_host = host;
            osc.config.save();
            if osc.config.tx_enabled {
                osc.sender
                    .configure(&osc.config.tx_host, osc.config.tx_port);
            }
        }
    });

    // TX: port + rate on one line
    ui.horizontal(|ui| {
        ui.add_space(20.0); // indent to align under TX fields
        ui.label(RichText::new("Port").size(SMALL_SIZE));
        let mut port = osc.config.tx_port;
        let resp = ui.add(
            egui::DragValue::new(&mut port)
                .range(1024..=65535)
                .speed(1.0),
        );
        if resp.changed() {
            osc.config.tx_port = port;
            osc.config.save();
            if osc.config.tx_enabled {
                osc.sender
                    .configure(&osc.config.tx_host, osc.config.tx_port);
            }
        }
        ui.label(RichText::new("Rate").size(SMALL_SIZE));
        let mut rate = osc.config.tx_rate_hz;
        let resp = ui.add(
            egui::DragValue::new(&mut rate)
                .range(1..=120)
                .speed(1.0)
                .suffix(" Hz"),
        );
        if resp.changed() {
            osc.config.tx_rate_hz = rate;
            osc.config.save();
        }
    });

    // Learn status (conditional)
    if let Some(ref learn_target) = osc.learn_target {
        ui.separator();
        let label = match learn_target {
            OscLearnTarget::Param(name) => format!("Send OSC for \"{name}\""),
            OscLearnTarget::Trigger(action) => {
                format!("Send OSC for \"{}\"", action.display_name())
            }
        };
        let t = ui.input(|i| i.time) as f32;
        let alpha = ((t * 3.0).sin() * 0.3 + 0.7).clamp(0.4, 1.0);
        let color = Color32::from_rgba_unmultiplied(0xE0, 0xA0, 0x40, (alpha * 255.0) as u8);
        ui.label(RichText::new(label).size(SMALL_SIZE).color(color));
        ui.ctx().request_repaint();
    }

    // Triggers
    ui.separator();
    ui.label(
        RichText::new("TRIGGERS")
            .size(HEADING_SIZE)
            .color(DARK_TEXT_SECONDARY)
            .strong(),
    );

    egui::Grid::new("osc_triggers")
        .num_columns(4)
        .spacing([4.0, 2.0])
        .show(ui, |ui| {
            for (left, right) in TRIGGER_PAIRS {
                ui.label(RichText::new(left.short_name()).size(SMALL_SIZE));
                draw_osc_trigger_badge(ui, osc, *left);
                ui.label(RichText::new(right.short_name()).size(SMALL_SIZE));
                draw_osc_trigger_badge(ui, osc, *right);
                ui.end_row();
            }
        });
}

fn draw_osc_trigger_badge(ui: &mut Ui, osc: &mut OscSystem, action: TriggerAction) {
    let is_learning = osc.learn_target == Some(OscLearnTarget::Trigger(action));
    let is_mapped = osc.config.triggers.contains_key(&action);

    if is_learning {
        let t = ui.input(|i| i.time) as f32;
        let alpha = ((t * 4.0).sin() * 0.3 + 0.7).clamp(0.4, 1.0);
        let color = Color32::from_rgba_unmultiplied(0xE0, 0xA0, 0x40, (alpha * 255.0) as u8);
        if ui
            .button(RichText::new("..").color(color).size(SMALL_SIZE))
            .on_hover_text("Cancel")
            .clicked()
        {
            osc.cancel_learn();
        }
        ui.ctx().request_repaint();
    } else if is_mapped {
        let mapping = &osc.config.triggers[&action];
        let label = abbreviate_address(&mapping.address);
        let resp = ui
            .button(RichText::new(&label).color(OSC_GREEN).size(SMALL_SIZE))
            .on_hover_text(format!(
                "{}\nClick to re-learn, right-click to clear",
                mapping.address
            ));
        if resp.clicked() {
            osc.start_learn(OscLearnTarget::Trigger(action));
        }
        if resp.secondary_clicked() {
            osc.clear_trigger_mapping(action);
        }
    } else {
        if ui
            .button(RichText::new("O").weak().size(SMALL_SIZE))
            .on_hover_text("OSC learn")
            .clicked()
        {
            osc.start_learn(OscLearnTarget::Trigger(action));
        }
    }
}

/// Draw a compact OSC mapping badge for a parameter.
pub fn draw_osc_badge(ui: &mut Ui, osc: &mut OscSystem, param_name: &str) {
    let is_learning = osc.learn_target == Some(OscLearnTarget::Param(param_name.to_string()));
    let is_mapped = osc.config.params.contains_key(param_name);

    if is_learning {
        let t = ui.input(|i| i.time) as f32;
        let alpha = ((t * 4.0).sin() * 0.3 + 0.7).clamp(0.4, 1.0);
        let color = Color32::from_rgba_unmultiplied(0xE0, 0xA0, 0x40, (alpha * 255.0) as u8);
        if ui
            .button(RichText::new("..").color(color).size(SMALL_SIZE))
            .on_hover_text("Cancel OSC learn")
            .clicked()
        {
            osc.cancel_learn();
        }
        ui.ctx().request_repaint();
    } else if is_mapped {
        let mapping = &osc.config.params[param_name];
        let label = abbreviate_address(&mapping.address);
        let resp = ui
            .button(RichText::new(&label).color(OSC_GREEN).size(SMALL_SIZE))
            .on_hover_text(format!(
                "{}\nClick to re-learn, right-click to clear",
                mapping.address
            ));
        if resp.clicked() {
            osc.start_learn(OscLearnTarget::Param(param_name.to_string()));
        }
        if resp.secondary_clicked() {
            osc.clear_param_mapping(param_name);
        }
    } else {
        if ui
            .button(RichText::new("O").weak().size(SMALL_SIZE))
            .on_hover_text("OSC learn")
            .clicked()
        {
            osc.start_learn(OscLearnTarget::Param(param_name.to_string()));
        }
    }
}

/// Abbreviate long OSC addresses for badge display.
fn abbreviate_address(addr: &str) -> String {
    // Show last two path components, max ~12 chars
    let parts: Vec<&str> = addr.rsplitn(3, '/').collect();
    if parts.len() >= 2 {
        let short = format!("/{}/{}", parts[1], parts[0]);
        if short.len() <= 14 {
            return short;
        }
    }
    if addr.len() <= 14 {
        addr.to_string()
    } else {
        format!("..{}", &addr[addr.len() - 12..])
    }
}
