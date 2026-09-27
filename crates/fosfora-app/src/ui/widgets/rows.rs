//! Shared "label + control" row widgets.
//!
//! Every panel row is `[label LABEL_WIDTH | control fills | value VALUE_WIDTH]`,
//! so labels, sliders and values line up in one column system across all panels.
//! Panels must not hand-roll `ui.horizontal(label + Slider)` rows — use these.

use std::ops::RangeInclusive;

use egui::{Align2, FontId, Response, RichText, Sense, Ui};

use super::super::theme::colors::theme_colors;
use super::super::theme::tokens::*;
use super::fmt_val;

/// Fixed label column. 315px panel − 2×6 frame margin − 2×8 card padding = 287px
/// usable; 110px fits the longest live label ("Audio dilation", "Speed (gen/s)")
/// at SMALL_SIZE on one line, leaving ~130px of slider + a 46px value cell.
pub const LABEL_WIDTH: f32 = 110.0;
/// Fixed value cell so slider right edges align across rows.
pub const VALUE_WIDTH: f32 = 46.0;

/// Response from a [`ParamRow`] slider/drag.
pub struct RowResponse {
    /// The slider/drag widget itself (for hover, context menus, and overlay
    /// painting — the trama inspector's ghost tick draws over its rect).
    pub response: Response,
    /// Value changed this frame.
    pub changed: bool,
    /// Adjustment finished (`drag_stopped || lost_focus`) — the commit-on-release
    /// contract the audio-tuning rows persist on.
    pub committed: bool,
}

/// Builder for an aligned slider/drag row.
pub struct ParamRow<'a> {
    label: &'a str,
    tooltip: Option<&'a str>,
    label_width: f32,
    logarithmic: bool,
    show_value: bool,
    enabled: bool,
    formatter: Option<fn(f64) -> String>,
    /// Slider step: whole numbers only when set to 1.
    step: Option<f64>,
}

impl<'a> ParamRow<'a> {
    pub fn new(label: &'a str) -> Self {
        Self {
            label,
            tooltip: None,
            label_width: LABEL_WIDTH,
            logarithmic: false,
            show_value: true,
            enabled: true,
            formatter: None,
            step: None,
        }
    }

    pub fn tooltip(mut self, tip: &'a str) -> Self {
        self.tooltip = Some(tip);
        self
    }
    pub fn logarithmic(mut self, on: bool) -> Self {
        self.logarithmic = on;
        self
    }
    #[allow(dead_code)] // part of the row vocabulary; no caller needs it yet
    pub fn hide_value(mut self) -> Self {
        self.show_value = false;
        self
    }

    pub fn enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }

    /// Escape hatch for panels whose column budget differs; prefer the default.
    #[allow(dead_code)] // part of the row vocabulary; no caller needs it yet
    pub fn label_width(mut self, w: f32) -> Self {
        self.label_width = w;
        self
    }
    pub fn formatter(mut self, f: fn(f64) -> String) -> Self {
        self.formatter = Some(f);
        self
    }

    /// A float that only takes whole numbers: the slider steps by 1 and the
    /// value shows no decimals.
    pub fn integer(mut self, on: bool) -> Self {
        if on {
            self.step = Some(1.0);
            self.formatter = Some(|v| format!("{v:.0}"));
        }
        self
    }

    /// `[label | slider fills | value]`. Generic over egui's `Numeric` so one
    /// function serves f32/u32/i32 rows alike.
    pub fn show_slider<T: egui::emath::Numeric>(
        self,
        ui: &mut Ui,
        value: &mut T,
        range: RangeInclusive<T>,
    ) -> RowResponse {
        self.show_impl(ui, value, |ui, this, value| {
            ui.spacing_mut().slider_width = ui.available_width();
            let mut slider = egui::Slider::new(value, range)
                .clamping(egui::SliderClamping::Always)
                .logarithmic(this.logarithmic)
                .show_value(false);
            if let Some(step) = this.step {
                slider = slider.step_by(step);
            }
            ui.add(slider)
        })
    }

    /// Same row geometry with a `DragValue` (ports, counts, hold durations).
    pub fn show_drag<T: egui::emath::Numeric>(
        self,
        ui: &mut Ui,
        value: &mut T,
        range: RangeInclusive<T>,
        speed: f64,
    ) -> RowResponse {
        let mut this = self;
        this.show_value = false; // DragValue renders its own value
        this.show_impl(ui, value, |ui, _this, value| {
            ui.add(egui::DragValue::new(value).range(range).speed(speed))
        })
    }

    fn show_impl<T: egui::emath::Numeric>(
        self,
        ui: &mut Ui,
        value: &mut T,
        add_widget: impl FnOnce(&mut Ui, &Self, &mut T) -> Response,
    ) -> RowResponse {
        let tc = theme_colors(ui.ctx());
        let enabled = self.enabled;
        let row = ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let label_resp = row_label(ui, self.label, self.label_width);
            let widget_resp = ui
                .with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    // Reserve the value cell first (rightmost), paint its text after
                    // the widget runs so the shown value has no one-frame lag.
                    let value_rect = self.show_value.then(|| {
                        ui.allocate_exact_size(
                            egui::vec2(VALUE_WIDTH, MIN_INTERACT_HEIGHT),
                            Sense::hover(),
                        )
                        .0
                    });
                    let resp = ui
                        .add_enabled_ui(enabled, |ui| add_widget(ui, &self, value))
                        .inner;
                    if let Some(rect) = value_rect {
                        let v = value.to_f64();
                        let text = match self.formatter {
                            Some(f) => f(v),
                            None if T::INTEGRAL => format!("{v:.0}"),
                            None => fmt_val(v),
                        };
                        ui.painter().text(
                            rect.right_center(),
                            Align2::RIGHT_CENTER,
                            text,
                            FontId::proportional(SMALL_SIZE),
                            if enabled {
                                tc.text_secondary
                            } else {
                                tc.text_dim
                            },
                        );
                    }
                    resp
                })
                .inner;
            (label_resp, widget_resp)
        });
        let (label_resp, widget_resp) = row.inner;
        if let Some(tip) = self.tooltip {
            label_resp.on_hover_text(tip);
            let _ = widget_resp.clone().on_hover_text(tip);
        }
        RowResponse {
            changed: widget_resp.changed(),
            committed: widget_resp.drag_stopped() || widget_resp.lost_focus(),
            response: widget_resp,
        }
    }
}

/// Label column + arbitrary control filling the remainder — the escape hatch
/// (TextEdit rows, button pairs) and the primitive `combo_row` builds on.
pub fn custom_row<R>(
    ui: &mut Ui,
    label: &str,
    tooltip: Option<&str>,
    add_control: impl FnOnce(&mut Ui) -> R,
) -> R {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let label_resp = row_label(ui, label, LABEL_WIDTH);
        if let Some(tip) = tooltip {
            label_resp.on_hover_text(tip);
        }
        add_control(ui)
    })
    .inner
}

/// Label column + full-width `ComboBox`. The popup body is supplied by the
/// caller so existing `selectable_label` bodies (with their side effects) move
/// in unchanged. Returns the body's result while the popup is open.
pub fn combo_row<R>(
    ui: &mut Ui,
    id_salt: &str,
    label: &str,
    tooltip: Option<&str>,
    selected_text: &str,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> Option<R> {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let label_resp = row_label(ui, label, LABEL_WIDTH);
        if let Some(tip) = tooltip {
            label_resp.on_hover_text(tip);
        }
        egui::ComboBox::from_id_salt(id_salt)
            .selected_text(RichText::new(selected_text).size(SMALL_SIZE))
            .width(ui.available_width() - 4.0)
            .show_ui(ui, add_contents)
            .inner
    })
    .inner
}

/// Label in the shared column + checkbox. Returns the checkbox response.
pub fn checkbox_row(ui: &mut Ui, value: &mut bool, label: &str, tooltip: Option<&str>) -> Response {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let label_resp = row_label(ui, label, LABEL_WIDTH);
        let resp = ui.checkbox(value, "");
        if let Some(tip) = tooltip {
            label_resp.on_hover_text(tip);
            resp.clone().on_hover_text(tip);
        }
        resp
    })
    .inner
}

/// Tiny uppercase group heading inside a section ("BUILD-UP", "DROP", …).
pub fn group_label(ui: &mut Ui, text: &str) {
    let tc = theme_colors(ui.ctx());
    ui.add_space(2.0);
    ui.label(
        RichText::new(text.to_uppercase())
            .size(SMALL_SIZE)
            .color(tc.text_secondary)
            .strong(),
    );
}

/// The fixed-width, truncating label cell every row shares.
fn row_label(ui: &mut Ui, label: &str, width: f32) -> Response {
    let tc = theme_colors(ui.ctx());
    ui.allocate_ui_with_layout(
        egui::vec2(width, MIN_INTERACT_HEIGHT),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.set_min_width(width);
            ui.add(
                egui::Label::new(
                    RichText::new(label)
                        .size(SMALL_SIZE)
                        .color(tc.text_secondary),
                )
                .truncate(),
            )
        },
    )
    .inner
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One frame of a row holding `value`, with `events`. Returns the texts
    /// painted and where the slider was.
    fn frame(
        ctx: &egui::Context,
        value: &mut f32,
        integer: bool,
        events: Vec<egui::Event>,
    ) -> (Vec<String>, egui::Rect) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(400.0, 100.0),
            )),
            events,
            ..Default::default()
        };
        let mut slider = egui::Rect::NOTHING;
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let r =
                    ParamRow::new("segments")
                        .integer(integer)
                        .show_slider(ui, value, 2.0..=24.0);
                slider = r.response.rect;
            });
        });
        let texts = out
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Text(t) => Some(t.galley.text().to_string()),
                _ => None,
            })
            .collect();
        (texts, slider)
    }

    // Kaleidoscope's segments (#3128): a whole-number parameter shows no
    // decimals, and a click on its slider lands on a whole number.
    #[test]
    fn an_integer_row_shows_and_takes_whole_numbers() {
        // 6.4, as an older preset or a modulation can leave it: the shader
        // rounds it to 6, and the row says 6.
        let ctx = egui::Context::default();
        let mut v = 6.4_f32;
        let (texts, _) = frame(&ctx, &mut v, true, vec![]);
        assert!(texts.iter().any(|t| t == "6"), "{texts:?}");
        let mut plain = 6.4_f32;
        let (texts, _) = frame(&egui::Context::default(), &mut plain, false, vec![]);
        assert!(
            texts.iter().any(|t| t == "6.40"),
            "a plain float keeps decimals: {texts:?}"
        );

        let (_, slider) = frame(&ctx, &mut v, true, vec![]);
        let at = egui::pos2(slider.left() + slider.width() * 0.43, slider.center().y);
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        frame(
            &ctx,
            &mut v,
            true,
            vec![egui::Event::PointerMoved(at), button(true)],
        );
        frame(&ctx, &mut v, true, vec![button(false)]);
        assert!((v - 6.4).abs() > 0.5, "the click moved it: {v}");
        assert_eq!(v, v.round(), "{v} is not whole");
    }
}
