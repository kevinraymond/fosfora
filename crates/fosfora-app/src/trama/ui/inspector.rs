//! The trama inspector: the selected node's parameters as the full
//! Parameter/Modulation/Uniform triple.
//!
//! Each parameter is one card: the base-value row on top, and — indented
//! beneath it — the modulation sub-block (source, amount, mode, smoothing,
//! source extras). Every control row leads with the house `R` reset button,
//! which also keeps the label columns aligned. The **ghost indicator**
//! paints the live resolved value over the slider rail — deliberately
//! luminance + shape + text (bright tick, triangle, `→ value` readout),
//! never hue alone. Color/Bool/Point2D params get plain editors; modulation
//! is Float-only in v1.
//!
//! Base-value edits write straight into the `ParamStore` (the executor
//! re-packs every frame — no version bump, no replan). Creating/removing a
//! modulation is deferred out of the borrow and applied through
//! `NodeGraph::set_modulation`, which owns the one-slot-per-param rule.

use egui::RichText;

use crate::params::{ParamDef, ParamValue};
use crate::ui::theme::colors::theme_colors;
use crate::ui::theme::tokens::SMALL_SIZE;
use crate::ui::widgets::card_frame;
use crate::ui::widgets::rows::{ParamRow, checkbox_row, combo_row, custom_row, group_label};

use super::super::audio::{AudioFeature, AudioView};
use super::super::effect::{EffectKind, TramaRegistry};
use super::super::graph::NodeGraph;
use super::super::modulation::{
    BeatDiv, ModMode, ModSource, Modulation, Osc, OscRate, OscShape, ParamMod,
};
use super::super::node::{NodeId, NodeKind};

const DEFAULT_AMOUNT: f32 = 0.5;
const DEFAULT_SMOOTHING: f32 = 0.2;

/// Fresh slot defaults when a source is picked on an unmodulated param:
/// audible depth, gentle slew — something moves the moment you pick it.
fn default_modulation(source: ModSource) -> Modulation {
    Modulation {
        source,
        amount: DEFAULT_AMOUNT,
        mode: ModMode::Add,
        smoothing: DEFAULT_SMOOTHING,
    }
}

/// The house per-control reset affordance (`param_panel.rs` precedent),
/// leading every row so the label columns stay aligned.
fn reset_button(ui: &mut egui::Ui, tip: &str) -> bool {
    ui.small_button(RichText::new("R").size(9.0))
        .on_hover_text(tip)
        .clicked()
}

pub fn draw_inspector(
    ui: &mut egui::Ui,
    graph: &mut NodeGraph,
    registry: &TramaRegistry,
    view: &AudioView,
    selected: Option<NodeId>,
) {
    let tc = theme_colors(ui.ctx());
    let dim = |text: &str| RichText::new(text).size(SMALL_SIZE).color(tc.text_dim);

    let Some(id) = selected else {
        ui.label(dim("Click a node to edit its parameters."));
        ui.add_space(2.0);
        ui.label(dim(
            "Every parameter here can also be driven by an oscillator or \
             the music — the \"mod\" row under each slider.",
        ));
        return;
    };
    let Some(node) = graph.node(id) else {
        ui.label(dim("Selection no longer exists."));
        return;
    };

    // Header: what is selected.
    match &node.kind {
        NodeKind::Output => {
            ui.label(RichText::new("Output").color(tc.text_primary));
            ui.label(dim("The output node has no parameters."));
            return;
        }
        NodeKind::Source { effect } | NodeKind::Effect { effect } => match registry.get(effect) {
            Some(def) => {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&def.name).color(tc.text_primary));
                    ui.label(dim(match def.kind {
                        EffectKind::Source => "source",
                        EffectKind::Effect => "effect",
                    }));
                });
            }
            None => {
                ui.label(RichText::new(format!("{} (missing effect)", effect.0)).color(tc.warning));
            }
        },
    }
    if node.params.defs.is_empty() {
        ui.label(dim("No parameters."));
        return;
    }
    ui.separator();

    // Creating/removing a slot goes through `set_modulation` after the node
    // borrow ends; in-place config tweaks go straight through `mods`.
    let mut slot_action: Option<(String, Option<Modulation>)> = None;
    {
        let node = graph.params_mut(id).expect("checked above");
        let node_id = node.id;
        let (defs, values, changed) = node.params.split_borrow();
        let mods: &mut Vec<ParamMod> = node.mods;

        for def in defs {
            card_frame(ui).show(ui, |ui| match def {
                ParamDef::Float {
                    name,
                    default,
                    min,
                    max,
                } => {
                    let current = match values.get(name) {
                        Some(ParamValue::Float(v)) => *v,
                        _ => *default,
                    };
                    let mut val = current;
                    let tip = format!("{min:.2} – {max:.2} · default {default:.2}");
                    let (reset, row) = ui
                        .horizontal(|ui| {
                            let reset = reset_button(ui, "Reset to default");
                            let row = ParamRow::new(name).tooltip(&tip).show_slider(
                                ui,
                                &mut val,
                                *min..=*max,
                            );
                            (reset, row)
                        })
                        .inner;
                    if reset {
                        val = *default;
                    }
                    if val != current {
                        values.insert(name.clone(), ParamValue::Float(val));
                        *changed = true;
                    }
                    let slot = mods.iter_mut().find(|m| m.param == *name);
                    if let Some(m) = &slot {
                        if m.state.slot.is_some() {
                            draw_ghost(
                                ui,
                                &row.response.rect,
                                ghost_fraction(*min, *max, m.state.resolved),
                            );
                        }
                    }
                    ui.indent(("trama-mod-indent", node_id.0, name.as_str()), |ui| {
                        draw_mod_editor(ui, node_id, name, slot, view, &mut slot_action);
                    });
                }
                ParamDef::Color { name, default } => {
                    let current = match values.get(name) {
                        Some(ParamValue::Color(c)) => *c,
                        _ => *default,
                    };
                    let mut rgba = egui::Rgba::from_rgba_unmultiplied(
                        current[0], current[1], current[2], current[3],
                    );
                    let (reset, resp) = ui
                        .horizontal(|ui| {
                            let reset = reset_button(ui, "Reset to default");
                            let resp = custom_row(ui, name, None, |ui| {
                                egui::color_picker::color_edit_button_rgba(
                                    ui,
                                    &mut rgba,
                                    egui::color_picker::Alpha::OnlyBlend,
                                )
                            });
                            (reset, resp)
                        })
                        .inner;
                    if reset {
                        values.insert(name.clone(), ParamValue::Color(*default));
                        *changed = true;
                    } else if resp.changed() {
                        values.insert(name.clone(), ParamValue::Color(rgba.to_rgba_unmultiplied()));
                        *changed = true;
                    }
                }
                ParamDef::Bool { name, default } => {
                    let current = match values.get(name) {
                        Some(ParamValue::Bool(b)) => *b,
                        _ => *default,
                    };
                    let mut val = current;
                    let (reset, toggled) = ui
                        .horizontal(|ui| {
                            let reset = reset_button(ui, "Reset to default");
                            let toggled = checkbox_row(ui, &mut val, name, None).changed();
                            (reset, toggled)
                        })
                        .inner;
                    if reset {
                        values.insert(name.clone(), ParamValue::Bool(*default));
                        *changed = true;
                    } else if toggled {
                        values.insert(name.clone(), ParamValue::Bool(val));
                        *changed = true;
                    }
                }
                ParamDef::Point2D {
                    name,
                    default,
                    min,
                    max,
                } => {
                    let current = match values.get(name) {
                        Some(ParamValue::Point2D(p)) => *p,
                        _ => *default,
                    };
                    let mut val = current;
                    for (axis, label) in ["x", "y"].iter().enumerate() {
                        let tip = format!(
                            "{:.2} – {:.2} · default {:.2}",
                            min[axis], max[axis], default[axis]
                        );
                        let reset = ui
                            .horizontal(|ui| {
                                let reset = reset_button(ui, "Reset to default");
                                ParamRow::new(&format!("{name} {label}"))
                                    .tooltip(&tip)
                                    .show_slider(ui, &mut val[axis], min[axis]..=max[axis]);
                                reset
                            })
                            .inner;
                        if reset {
                            val[axis] = default[axis];
                        }
                    }
                    if val != current {
                        values.insert(name.clone(), ParamValue::Point2D(val));
                        *changed = true;
                    }
                }
            });
        }
    }
    if let Some((param, config)) = slot_action {
        let _ = graph.set_modulation(id, &param, config);
    }
}

/// The modulation editor under one Float row: source picker, and when a
/// slot is active, its amount/mode/smoothing plus source-specific extras.
fn draw_mod_editor(
    ui: &mut egui::Ui,
    node: NodeId,
    param: &str,
    slot: Option<&mut ParamMod>,
    view: &AudioView,
    slot_action: &mut Option<(String, Option<Modulation>)>,
) {
    let tc = theme_colors(ui.ctx());
    let salt = format!("trama-mod-{}-{param}", node.0);
    let selected_text = slot
        .as_ref()
        .map_or_else(|| "off".to_string(), |m| source_label(&m.config.source));

    let current_source = slot.as_ref().map(|m| m.config.source);
    let clear = ui
        .horizontal(|ui| {
            let clear = reset_button(ui, "Remove modulation");
            combo_row(
                ui,
                &salt,
                "mod",
                Some("Drive this parameter from an oscillator or the music"),
                &selected_text,
                |ui| {
                    if ui
                        .selectable_label(current_source.is_none(), "Off")
                        .clicked()
                    {
                        *slot_action = Some((param.to_string(), None));
                    }
                    group_label(ui, "oscillator");
                    for shape in OscShape::ALL {
                        let is_current = matches!(
                            current_source,
                            Some(ModSource::Oscillator(o)) if o.shape == shape
                        );
                        if ui
                            .selectable_label(is_current, format!("~ {}", shape_label(shape)))
                            .clicked()
                        {
                            *slot_action = Some((
                                param.to_string(),
                                Some(picked_source(
                                    current_source,
                                    ModSource::Oscillator(Osc {
                                        shape,
                                        rate: current_osc_rate(current_source),
                                        phase: current_osc_phase(current_source),
                                    }),
                                )),
                            ));
                        }
                    }
                    group_label(ui, "audio");
                    for feature in AUDIO_SOURCES {
                        let is_current = matches!(
                            current_source,
                            Some(ModSource::Audio(f)) if f == feature
                        );
                        if ui
                            .selectable_label(is_current, format!("≈ {}", feature_label(feature)))
                            .clicked()
                        {
                            *slot_action = Some((
                                param.to_string(),
                                Some(picked_source(current_source, ModSource::Audio(feature))),
                            ));
                        }
                    }
                },
            );
            clear
        })
        .inner;
    if clear && current_source.is_some() {
        *slot_action = Some((param.to_string(), None));
    }

    let Some(m) = slot else { return };

    let mut amount = m.config.amount;
    let (reset, row) = ui
        .horizontal(|ui| {
            let reset = reset_button(ui, "Reset depth");
            let row = ParamRow::new("amount")
                .tooltip("Depth: how far the signal swings this parameter; negative inverts")
                .show_slider(ui, &mut amount, -1.0..=1.0);
            (reset, row)
        })
        .inner;
    if reset {
        m.config.amount = DEFAULT_AMOUNT;
    } else if row.changed {
        m.config.amount = amount;
    }

    ui.horizontal(|ui| {
        if reset_button(ui, "Reset to Add") {
            m.config.mode = ModMode::Add;
        }
        custom_row(
            ui,
            "mode",
            Some("How the signal combines with the slider value"),
            |ui| {
                ui.selectable_value(&mut m.config.mode, ModMode::Add, "Add")
                    .on_hover_text("Offset the slider by the signal");
                ui.selectable_value(&mut m.config.mode, ModMode::Multiply, "Mult")
                    .on_hover_text("Scale the slider by the signal");
                ui.selectable_value(&mut m.config.mode, ModMode::Replace, "Repl")
                    .on_hover_text("Crossfade the slider toward the signal");
            },
        );
    });

    let mut smoothing = m.config.smoothing;
    let (reset, row) = ui
        .horizontal(|ui| {
            let reset = reset_button(ui, "Reset smoothing");
            let row = ParamRow::new("smooth")
                .tooltip("Slew toward the modulated value (0 = instant)")
                .show_slider(ui, &mut smoothing, 0.0..=1.0);
            (reset, row)
        })
        .inner;
    if reset {
        m.config.smoothing = DEFAULT_SMOOTHING;
    } else if row.changed {
        m.config.smoothing = smoothing;
    }

    match &mut m.config.source {
        ModSource::Oscillator(osc) => {
            ui.horizontal(|ui| {
                if reset_button(ui, "Reset to 1 Hz") {
                    osc.rate = OscRate::Hz(1.0);
                }
                custom_row(
                    ui,
                    "rate",
                    Some("Free-running (Hz) or locked to the detected beat"),
                    |ui| {
                        let is_hz = matches!(osc.rate, OscRate::Hz(_));
                        if ui.selectable_label(is_hz, "Hz").clicked() && !is_hz {
                            osc.rate = OscRate::Hz(1.0);
                        }
                        if ui.selectable_label(!is_hz, "Beat").clicked() && is_hz {
                            osc.rate = OscRate::BeatSync(BeatDiv::Bar);
                        }
                    },
                );
            });
            match &mut osc.rate {
                OscRate::Hz(hz) => {
                    let reset = ui
                        .horizontal(|ui| {
                            let reset = reset_button(ui, "Reset to 1 Hz");
                            ParamRow::new("freq")
                                .tooltip("Oscillator speed, cycles per second")
                                .logarithmic(true)
                                .formatter(|v| format!("{v:.2}"))
                                .show_slider(ui, hz, 0.01..=20.0);
                            reset
                        })
                        .inner;
                    if reset {
                        *hz = 1.0;
                    }
                }
                OscRate::BeatSync(div) => {
                    ui.horizontal(|ui| {
                        if reset_button(ui, "Reset to 1 bar") {
                            *div = BeatDiv::Bar;
                        }
                        combo_row(
                            ui,
                            &format!("{salt}-div"),
                            "cycle",
                            Some("Cycle length in musical time (4/4 assumed)"),
                            div.label(),
                            |ui| {
                                for candidate in BeatDiv::ALL {
                                    ui.selectable_value(div, candidate, candidate.label());
                                }
                            },
                        );
                    });
                    if !view.tempo_locked() {
                        ui.label(
                            RichText::new("waiting for tempo — oscillator holds until beat lock")
                                .size(SMALL_SIZE)
                                .color(tc.text_dim),
                        );
                    }
                }
            }
            let mut phase = osc.phase;
            let (reset, row) = ui
                .horizontal(|ui| {
                    let reset = reset_button(ui, "Reset phase");
                    let row = ParamRow::new("phase")
                        .tooltip("Cycle offset, in cycles")
                        .show_slider(ui, &mut phase, 0.0..=1.0);
                    (reset, row)
                })
                .inner;
            if reset {
                osc.phase = 0.0;
            } else if row.changed {
                osc.phase = phase;
            }
        }
        ModSource::Audio(AudioFeature::Band(n)) => {
            let reset = ui
                .horizontal(|ui| {
                    let reset = reset_button(ui, "Reset to band 0");
                    ParamRow::new("band")
                        .tooltip("Which of the 32 spectrum bands drives this (0 = lowest)")
                        .show_drag(ui, n, 0..=31, 0.1);
                    reset
                })
                .inner;
            if reset {
                *n = 0;
            }
        }
        ModSource::Audio(_) => {}
    }

    // Live resolved value — the textual half of the ghost indicator.
    custom_row(
        ui,
        "live",
        Some("The value reaching the shader — also the bright tick on the slider"),
        |ui| {
            ui.label(
                RichText::new(format!("→ {:.3}", m.state.resolved))
                    .monospace()
                    .size(SMALL_SIZE)
                    .color(tc.text_primary),
            );
        },
    );
}

/// Picking a new source on an existing slot keeps its depth settings —
/// only the source swaps; a fresh param starts from the defaults.
fn picked_source(current: Option<ModSource>, source: ModSource) -> Modulation {
    match current {
        Some(_) => Modulation {
            source,
            ..default_modulation(source)
        },
        None => default_modulation(source),
    }
}

fn current_osc_rate(source: Option<ModSource>) -> OscRate {
    match source {
        Some(ModSource::Oscillator(o)) => o.rate,
        _ => OscRate::Hz(1.0),
    }
}

fn current_osc_phase(source: Option<ModSource>) -> f32 {
    match source {
        Some(ModSource::Oscillator(o)) => o.phase,
        _ => 0.0,
    }
}

/// Where the resolved value sits inside the param's range, 0..=1.
fn ghost_fraction(min: f32, max: f32, resolved: f32) -> f32 {
    if max > min {
        ((resolved - min) / (max - min)).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// The graphical ghost: a bright full-height tick with a dark outline and a
/// triangle riding the top edge — luminance + shape, readable without color
/// vision and with a knob sitting on top of it.
fn draw_ghost(ui: &egui::Ui, rect: &egui::Rect, fraction: f32) {
    let tc = theme_colors(ui.ctx());
    let x = rect.left() + fraction * rect.width();
    let painter = ui.painter();
    painter.line_segment(
        [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
        egui::Stroke::new(4.0, tc.canvas),
    );
    painter.line_segment(
        [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
        egui::Stroke::new(2.0, tc.text_primary),
    );
    painter.add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(x - 3.5, rect.top()),
            egui::pos2(x + 3.5, rect.top()),
            egui::pos2(x, rect.top() + 5.0),
        ],
        tc.text_primary,
        egui::Stroke::new(1.0, tc.canvas),
    ));
}

/// Non-Band audio sources for the picker; `Band` enters via its own row.
const AUDIO_SOURCES: [AudioFeature; 8] = [
    AudioFeature::Rms,
    AudioFeature::Onset,
    AudioFeature::Bass,
    AudioFeature::Mid,
    AudioFeature::High,
    AudioFeature::Band(0),
    AudioFeature::BeatPhase,
    AudioFeature::Bpm,
];

fn shape_label(shape: OscShape) -> &'static str {
    match shape {
        OscShape::Sine => "sine",
        OscShape::Saw => "saw",
        OscShape::Square => "square",
        OscShape::Triangle => "triangle",
        OscShape::SampleHold => "s&h",
        OscShape::Drift => "drift",
    }
}

fn feature_label(feature: AudioFeature) -> String {
    match feature {
        AudioFeature::Rms => "rms".into(),
        AudioFeature::Onset => "onset".into(),
        AudioFeature::Bass => "bass".into(),
        AudioFeature::Mid => "mid".into(),
        AudioFeature::High => "high".into(),
        AudioFeature::Band(n) => format!("band {n}"),
        AudioFeature::BeatPhase => "beat phase".into(),
        AudioFeature::Bpm => "bpm".into(),
    }
}

fn source_label(source: &ModSource) -> String {
    match source {
        ModSource::Oscillator(o) => format!("~ {}", shape_label(o.shape)),
        ModSource::Audio(f) => format!("≈ {}", feature_label(*f)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ghost_fraction_clamps_and_maps() {
        assert_eq!(ghost_fraction(0.0, 1.0, 0.25), 0.25);
        assert_eq!(ghost_fraction(0.0, 4.0, 2.0), 0.5);
        assert_eq!(ghost_fraction(0.0, 1.0, -3.0), 0.0);
        assert_eq!(ghost_fraction(0.0, 1.0, 9.0), 1.0);
        assert_eq!(ghost_fraction(1.0, 1.0, 1.0), 0.0, "degenerate span");
    }

    /// Exhaustive label coverage: adding a variant breaks these matches at
    /// compile time, so the picker can never silently miss a source.
    #[test]
    fn mod_source_labels_cover_all_variants() {
        for shape in OscShape::ALL {
            assert!(!shape_label(shape).is_empty());
        }
        let all_features = [
            AudioFeature::Rms,
            AudioFeature::Onset,
            AudioFeature::Bass,
            AudioFeature::Mid,
            AudioFeature::High,
            AudioFeature::Band(7),
            AudioFeature::BeatPhase,
            AudioFeature::Bpm,
        ];
        let labels: Vec<String> = all_features.iter().map(|f| feature_label(*f)).collect();
        for label in &labels {
            assert!(!label.is_empty());
        }
        let unique: std::collections::HashSet<&String> = labels.iter().collect();
        assert_eq!(unique.len(), labels.len(), "labels must be distinct");
    }
}
