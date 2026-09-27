use egui::{Color32, RichText, Ui};

use crate::bindings::bus::BindingBus;
use crate::bindings::types::BindingTarget;
use crate::midi::MidiSystem;
use crate::midi::types::{LearnTarget, MidiMsgType};
use crate::osc::OscSystem;
use crate::params::{ParamDef, ParamStore, ParamValue};
use crate::ui::panels::osc_panel;
use crate::ui::theme::colors::theme_colors;
use crate::ui::theme::tokens::*;

const MIDI_BLUE: Color32 = Color32::from_rgb(0x60, 0xA0, 0xE0);

/// Draw a compact MIDI mapping badge for a parameter.
fn draw_midi_badge(ui: &mut Ui, midi: &mut MidiSystem, param_name: &str) {
    let is_learning = midi.learn_target == Some(LearnTarget::Param(param_name.to_string()));
    let is_mapped = midi.config.params.contains_key(param_name);

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
            .add(
                egui::Button::new(RichText::new(&label).color(MIDI_BLUE).size(SMALL_SIZE))
                    .min_size(badge_min),
            )
            .on_hover_text("Click to re-learn, right-click to clear");
        if resp.clicked() {
            midi.start_learn(LearnTarget::Param(param_name.to_string()));
        }
        if resp.secondary_clicked() {
            midi.clear_param_mapping(param_name);
        }
    } else {
        if ui
            .add(learn_badge("M"))
            .on_hover_text("MIDI learn")
            .clicked()
        {
            midi.start_learn(LearnTarget::Param(param_name.to_string()));
        }
    }
}

/// An unmapped M or O: a row's MIDI or OSC learn, before it has learned.
pub(crate) fn learn_badge(letter: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(letter).weak().size(SMALL_SIZE))
        .min_size(egui::vec2(16.0, 14.0))
}

fn format_mapping_label(msg_type: MidiMsgType, cc: u8) -> String {
    match msg_type {
        MidiMsgType::Cc => format!("CC{cc}"),
        MidiMsgType::Note => format!("N{cc}"),
    }
}

/// What the workspace's rows need to show and start bindings (#3127): the
/// Classic panel passes none and keeps its rows as they were.
pub struct ParamBinds<'a> {
    pub bus: &'a BindingBus,
    /// The layer these controls belong to, as bindings name it.
    pub layer: usize,
    pub effect: &'a str,
}

impl ParamBinds<'_> {
    fn target(&self, param: &str) -> BindingTarget {
        BindingTarget::Param {
            layer: self.layer,
            effect: self.effect.to_string(),
            param: param.to_string(),
        }
    }

    /// The sources driving `param` on this layer, in the order they were made.
    pub fn sources(&self, param: &str) -> Vec<&str> {
        let target = self.target(param);
        self.bus
            .bindings
            .iter()
            .filter(|b| b.enabled && !b.source.is_empty() && b.target.same_destination(&target))
            .map(|b| b.source.as_str())
            .collect()
    }
}

/// What a row's bind control says: `Bind` when nothing drives the control,
/// else what does, as `◀ Bass`, with a count when more than one does.
pub fn bind_label(sources: &[&str]) -> String {
    let Some(first) = sources.first() else {
        return "Bind".to_string();
    };
    let mut name = crate::ui::panels::binding_helpers::friendly_source_label(first);
    if name.chars().count() > 12 {
        name = name.chars().take(11).collect::<String>() + "\u{2026}";
    }
    match sources.len() {
        1 => format!("\u{25c0} {name}"),
        n => format!("\u{25c0} {name} +{}", n - 1),
    }
}

/// The bind control as drawn: `Bind`, or what drives the control.
fn bind_button(ctx: &egui::Context, sources: &[&str]) -> egui::Button<'static> {
    let label = bind_label(sources);
    let text = if sources.is_empty() {
        RichText::new(label)
            .size(SMALL_SIZE)
            .color(theme_colors(ctx).text_secondary)
    } else {
        // Bound reads by the words and the weight, not a hue.
        RichText::new(label).size(SMALL_SIZE).strong()
    };
    egui::Button::new(text).min_size(egui::vec2(16.0, 14.0))
}

/// A control's row as the inspector draws it: the name, then the slider
/// filling the width, the value, and `badges` at the right edge, laid out
/// right to left (so the first one drawn is rightmost).
fn float_row(
    ui: &mut Ui,
    name: &str,
    val: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    badges: impl FnOnce(&mut Ui),
) {
    let tc = theme_colors(ui.ctx());
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.label(
            RichText::new(name)
                .size(SMALL_SIZE)
                .color(tc.text_secondary),
        );
        // Right-to-left: badges rightmost, then value, slider fills the rest
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            badges(ui);
            ui.label(
                RichText::new(fmt_val(*val))
                    .size(SMALL_SIZE)
                    .color(tc.text_secondary),
            );
            ui.spacing_mut().slider_width = ui.available_width();
            ui.add(
                egui::Slider::new(val, range)
                    .clamping(egui::SliderClamping::Always)
                    .show_value(false),
            );
        });
    });
}

/// A row following the bass, drawn by the rows' own code: the Bindings
/// tour's picture of what a bound control looks like. Nothing on it does
/// anything.
pub fn draw_example_row(ui: &mut Ui) {
    let mut val = 0.62;
    float_row(ui, "speed", &mut val, 0.0..=1.0, |ui| {
        let _ = ui.add(learn_badge("O"));
        let _ = ui.add(learn_badge("M"));
        let _ = ui.add(bind_button(ui.ctx(), &["audio.band.1"]));
    });
}

/// A row's bind control. A click asks for the matrix with this control as
/// the target (or its binding open, if it has one): the request is
/// `bind_param`, (layer, name), read with the matrix. Off during a tour,
/// like every other way into another view.
fn draw_bind_control(ui: &mut Ui, binds: &ParamBinds<'_>, param: &str) {
    let sources = binds.sources(param);
    let touring = crate::ui::tour::is_running(ui.ctx());
    let hover = if sources.is_empty() {
        "Make this control follow a source: opens Bindings with it picked".to_string()
    } else {
        let all: Vec<String> = sources
            .iter()
            .map(|s| crate::ui::panels::binding_helpers::friendly_source_label(s))
            .collect();
        format!("Follows {}. Click to open its binding.", all.join(", "))
    };
    if ui
        .add_enabled(!touring, bind_button(ui.ctx(), &sources))
        .on_hover_text(hover)
        .on_disabled_hover_text(crate::ui::tour::NOT_DURING)
        .clicked()
    {
        let request = (binds.layer, param.to_string());
        ui.ctx()
            .data_mut(|d| d.insert_temp(egui::Id::new("bind_param"), request));
    }
}

/// Shared compact float formatting, f32-flavored for this panel's values.
fn fmt_val(v: f32) -> String {
    crate::ui::widgets::fmt_val(f64::from(v))
}

pub fn draw_param_panel(
    ui: &mut Ui,
    store: &mut ParamStore,
    midi: &mut MidiSystem,
    osc: &mut OscSystem,
    binds: Option<&ParamBinds<'_>>,
) {
    let tc = theme_colors(ui.ctx());

    if store.defs.is_empty() {
        ui.label(
            RichText::new("No parameters")
                .size(SMALL_SIZE)
                .color(tc.text_secondary),
        );
        return;
    }

    let defs = store.defs.clone();

    for (i, def) in defs.iter().enumerate() {
        let row_top = ui.cursor().top();
        match def {
            ParamDef::Float { name, min, max, .. } => {
                let current = match store.get(name) {
                    Some(ParamValue::Float(v)) => *v,
                    _ => *min,
                };
                let mut val = current;

                float_row(ui, name, &mut val, *min..=*max, |ui| {
                    osc_panel::draw_osc_badge(ui, osc, name);
                    draw_midi_badge(ui, midi, name);
                    if let Some(b) = binds {
                        draw_bind_control(ui, b, name);
                    }
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
                        if let Some(b) = binds {
                            draw_bind_control(ui, b, name);
                        }
                    });
                });

                if val != current {
                    store.set(name, ParamValue::Bool(val));
                }
            }
            ParamDef::Point2D { name, min, max, .. } => {
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
                                .color(tc.text_secondary),
                        );
                    });
                });
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    ui.label(RichText::new("X").size(MONO_SIZE).color(tc.text_secondary));
                    ui.add(egui::Slider::new(&mut val[0], min[0]..=max[0]).show_value(false));
                });
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    ui.label(RichText::new("Y").size(MONO_SIZE).color(tc.text_secondary));
                    ui.add(egui::Slider::new(&mut val[1], min[1]..=max[1]).show_value(false));
                });

                if val != current {
                    store.set(name, ParamValue::Point2D(val));
                }
            }
        }
        if i == 0 && binds.is_some() {
            // The first row: where the Bindings tour shows Bind, M and O.
            let row =
                egui::Rect::from_x_y_ranges(ui.max_rect().x_range(), row_top..=ui.cursor().top());
            crate::ui::tour::anchor(ui, crate::ui::tour::Anchor::ParamRow, row);
        }
        ui.add_space(2.0);
    }

    ui.add_space(4.0);
    if ui
        .add(
            egui::Button::new(RichText::new("Reset All").size(SMALL_SIZE))
                .min_size(egui::vec2(0.0, 20.0)),
        )
        .clicked()
    {
        store.reset_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bindings::types::BindingScope;
    use crate::ui::panels::binding_matrix::{ArmedEnd, ScopeTab};
    use crate::ui::shell_harness::{PARAMS, ShellHarness};

    fn target(layer: usize, param: &str) -> BindingTarget {
        BindingTarget::Param {
            layer,
            effect: "Effect 0".into(),
            param: param.into(),
        }
    }

    /// The harness in Build with its effect layer selected.
    fn on_the_effect_layer() -> ShellHarness {
        let mut h = ShellHarness::new(egui::vec2(1400.0, 900.0));
        h.active_layer = ShellHarness::EFFECT_LAYER;
        h.settle(3);
        h
    }

    #[test]
    fn the_label_names_the_source_and_counts_the_rest() {
        assert_eq!(bind_label(&[]), "Bind");
        assert_eq!(bind_label(&["audio.kick"]), "\u{25c0} Kick");
        assert_eq!(bind_label(&["audio.kick", "audio.rms"]), "\u{25c0} Kick +1");
        let long = bind_label(&["osc./a/very/long/address/indeed"]);
        assert!(long.chars().count() <= 14, "{long}");
    }

    // The mockup's marker is a glyph Inter may not have; egui's fallbacks
    // must, or every bound row shows a box.
    #[test]
    fn the_app_fonts_draw_the_marker() {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::ui::overlay::font_definitions());
        let _ = ctx.run(Default::default(), |_| {});
        let id = egui::FontId::proportional(SMALL_SIZE);
        assert!(ctx.fonts_mut(|f| f.has_glyph(&id, '\u{25c0}')));
    }

    // Bind picks the control as the target: one click on a source then
    // finishes the binding, the way the tour teaches it.
    #[test]
    fn bind_opens_the_matrix_with_the_control_picked() {
        let mut h = on_the_effect_layer();
        h.matrix.scope_tab = ScopeTab::Global;
        let binds = h.text_rects("Bind");
        assert_eq!(binds.len(), PARAMS.len(), "a Bind on each row");
        h.click(binds[1].center());
        h.settle(2);
        assert!(h.matrix.open);
        assert_eq!(
            h.matrix.armed,
            Some(ArmedEnd::Target(target(
                ShellHarness::EFFECT_LAYER,
                PARAMS[1]
            )))
        );
        assert!(
            h.matrix.scope_tab == ScopeTab::Effect,
            "a layer's control binds into the preset"
        );
    }

    // A bound row names its source, and Bind on it opens that binding
    // rather than starting another. A binding on another layer's control
    // of the same name is not this row's.
    #[test]
    fn a_bound_row_names_its_source_and_opens_its_card() {
        let mut h = on_the_effect_layer();
        let id = h.bindings.add_binding(
            "audio.kick".into(),
            target(ShellHarness::EFFECT_LAYER, PARAMS[0]),
            BindingScope::Preset,
        );
        h.bindings.add_binding(
            "audio.rms".into(),
            target(0, PARAMS[1]),
            BindingScope::Preset,
        );
        h.settle(2);
        let chip = h
            .text_rect("\u{25c0} Kick")
            .expect("the bound row says Kick");
        assert!(
            h.text_rect("\u{25c0} RMS").is_none(),
            "another layer's binding"
        );
        h.click(chip.center());
        h.settle(2);
        assert!(h.matrix.open);
        assert_eq!(h.matrix.expanded_binding_id.as_deref(), Some(id.as_str()));
        assert_eq!(h.matrix.armed, None);
    }
}
