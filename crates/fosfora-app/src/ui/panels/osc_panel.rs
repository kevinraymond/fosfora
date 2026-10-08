use egui::{Color32, RichText, Ui};

use crate::osc::OscSystem;
use crate::osc::types::OscLearnTarget;
use crate::ui::theme::tokens::*;

const OSC_GREEN: Color32 = Color32::from_rgb(0x50, 0xC0, 0x70);

/// Draw a compact OSC mapping badge for a parameter.
pub fn draw_osc_badge(ui: &mut Ui, osc: &mut OscSystem, param_name: &str) {
    let is_learning = osc.learn_target == Some(OscLearnTarget::Param(param_name.to_string()));
    let is_mapped = osc.config.params.contains_key(param_name);
    let badge_min = egui::vec2(16.0, 14.0);

    if is_learning {
        let t = ui.input(|i| i.time) as f32;
        let alpha = ((t * 4.0).sin() * 0.3 + 0.7).clamp(0.4, 1.0);
        let color = Color32::from_rgba_unmultiplied(0xE0, 0xA0, 0x40, (alpha * 255.0) as u8);
        if ui
            .add(
                egui::Button::new(RichText::new("..").color(color).size(SMALL_SIZE))
                    .min_size(badge_min),
            )
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
            .add(
                egui::Button::new(RichText::new(&label).color(OSC_GREEN).size(SMALL_SIZE))
                    .min_size(badge_min),
            )
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
            .add(crate::ui::panels::param_panel::learn_badge("O"))
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
    let count = addr.chars().count();
    if count <= 14 {
        addr.to_string()
    } else {
        // Keep the tail — the discriminating part of an OSC address (char-safe).
        let tail: String = addr.chars().skip(count - 12).collect();
        format!("..{tail}")
    }
}
