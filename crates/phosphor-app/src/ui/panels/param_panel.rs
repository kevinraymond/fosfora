use egui::{Color32, RichText, Ui};

use crate::midi::types::{LearnTarget, MidiMsgType};
use crate::midi::MidiSystem;
use crate::osc::OscSystem;
use crate::params::{ParamDef, ParamStore, ParamValue};
use crate::ui::panels::osc_panel;
use crate::ui::theme::tokens::*;

const MIDI_BLUE: Color32 = Color32::from_rgb(0x60, 0xA0, 0xE0);

/// Draw a compact MIDI mapping badge for a parameter.
fn draw_midi_badge(ui: &mut Ui, midi: &mut MidiSystem, param_name: &str) {
    let is_learning = midi.learn_target == Some(LearnTarget::Param(param_name.to_string()));
    let is_mapped = midi.config.params.contains_key(param_name);

    if is_learning {
        let t = ui.input(|i| i.time) as f32;
        let alpha = ((t * 4.0).sin() * 0.3 + 0.7).clamp(0.4, 1.0);
        let color = Color32::from_rgba_unmultiplied(0xE0, 0xA0, 0x40, (alpha * 255.0) as u8);
        if ui
            .button(RichText::new("..").color(color).size(SMALL_SIZE))
            .on_hover_text("Cancel MIDI learn")
            .clicked()
        {
            midi.cancel_learn();
        }
        ui.ctx().request_repaint();
    } else if is_mapped {
        let mapping = &midi.config.params[param_name];
        let label = format_mapping_label(mapping.msg_type, mapping.cc);
        let resp = ui
            .button(RichText::new(&label).color(MIDI_BLUE).size(SMALL_SIZE))
            .on_hover_text("Click to re-learn, right-click to clear");
        if resp.clicked() {
            midi.start_learn(LearnTarget::Param(param_name.to_string()));
        }
        if resp.secondary_clicked() {
            midi.clear_param_mapping(param_name);
        }
    } else {
        if ui
            .button(RichText::new("M").weak().size(SMALL_SIZE))
            .on_hover_text("MIDI learn")
            .clicked()
        {
            midi.start_learn(LearnTarget::Param(param_name.to_string()));
        }
    }
}

fn format_mapping_label(msg_type: MidiMsgType, cc: u8) -> String {
    match msg_type {
        MidiMsgType::Cc => format!("CC{cc}"),
        MidiMsgType::Note => format!("N{cc}"),
    }
}

/// Format a float value compactly: no trailing zeros, max 2 decimal places.
fn fmt_val(v: f32) -> String {
    if v == v.round() {
        format!("{:.0}", v)
    } else if (v * 10.0).round() == v * 10.0 {
        format!("{:.1}", v)
    } else {
        format!("{:.2}", v)
    }
}

pub fn draw_param_panel(ui: &mut Ui, store: &mut ParamStore, midi: &mut MidiSystem, osc: &mut OscSystem) {
    if store.defs.is_empty() {
        ui.label(RichText::new("No parameters").size(SMALL_SIZE).color(DARK_TEXT_SECONDARY));
        return;
    }

    let defs = store.defs.clone();

    for def in &defs {
        match def {
            ParamDef::Float {
                name,
                min,
                max,
                ..
            } => {
                let current = match store.get(name) {
                    Some(ParamValue::Float(v)) => *v,
                    _ => *min,
                };
                let mut val = current;

                // Row 1: name (left) ... value + OSC/MIDI badges (right)
                ui.horizontal(|ui| {
                    ui.label(RichText::new(name).size(SMALL_SIZE).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        osc_panel::draw_osc_badge(ui, osc, name);
                        draw_midi_badge(ui, midi, name);
                        ui.label(
                            RichText::new(fmt_val(val))
                                .size(SMALL_SIZE)
                                .color(DARK_TEXT_SECONDARY),
                        );
                    });
                });

                // Row 2: min ... slider ... max
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    ui.label(
                        RichText::new(fmt_val(*min))
                            .size(MONO_SIZE)
                            .color(DARK_TEXT_SECONDARY),
                    );
                    let slider = egui::Slider::new(&mut val, *min..=*max)
                        .clamping(egui::SliderClamping::Always)
                        .show_value(false);
                    ui.add(slider);
                    ui.label(
                        RichText::new(fmt_val(*max))
                            .size(MONO_SIZE)
                            .color(DARK_TEXT_SECONDARY),
                    );
                });

                if val != current {
                    store.set(name, ParamValue::Float(val));
                }
            }
            ParamDef::Color { name, .. } => {
                let current = match store.get(name) {
                    Some(ParamValue::Color(c)) => *c,
                    _ => [1.0, 1.0, 1.0, 1.0],
                };
                let mut color = current;

                ui.horizontal(|ui| {
                    ui.label(RichText::new(name).size(SMALL_SIZE));
                    ui.color_edit_button_rgba_unmultiplied(&mut color);
                    if ui.small_button("R").on_hover_text("Reset").clicked() {
                        store.reset(name);
                        return;
                    }
                });

                if color != current {
                    store.set(name, ParamValue::Color(color));
                }
            }
            ParamDef::Bool { name, .. } => {
                let current = match store.get(name) {
                    Some(ParamValue::Bool(b)) => *b,
                    _ => false,
                };
                let mut val = current;

                ui.horizontal(|ui| {
                    ui.checkbox(&mut val, RichText::new(name).size(SMALL_SIZE));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        osc_panel::draw_osc_badge(ui, osc, name);
                        draw_midi_badge(ui, midi, name);
                    });
                });

                if val != current {
                    store.set(name, ParamValue::Bool(val));
                }
            }
            ParamDef::Point2D {
                name, min, max, ..
            } => {
                let current = match store.get(name) {
                    Some(ParamValue::Point2D(p)) => *p,
                    _ => *min,
                };
                let mut val = current;

                ui.horizontal(|ui| {
                    ui.label(RichText::new(name).size(SMALL_SIZE).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!("{}, {}", fmt_val(val[0]), fmt_val(val[1])))
                                .size(SMALL_SIZE)
                                .color(DARK_TEXT_SECONDARY),
                        );
                    });
                });
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    ui.label(RichText::new("X").size(MONO_SIZE).color(DARK_TEXT_SECONDARY));
                    ui.add(egui::Slider::new(&mut val[0], min[0]..=max[0]).show_value(false));
                });
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    ui.label(RichText::new("Y").size(MONO_SIZE).color(DARK_TEXT_SECONDARY));
                    ui.add(egui::Slider::new(&mut val[1], min[1]..=max[1]).show_value(false));
                });

                if val != current {
                    store.set(name, ParamValue::Point2D(val));
                }
            }
        }
        ui.add_space(2.0);
    }

    ui.add_space(4.0);
    if ui
        .button(RichText::new("Reset All").size(SMALL_SIZE))
        .clicked()
    {
        store.reset_all();
    }
}
