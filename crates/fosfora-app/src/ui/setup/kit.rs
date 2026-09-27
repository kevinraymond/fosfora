//! The pieces every Setup page is built from (#3237): a block per device or
//! stream, with its state in words and a mark beside its name, an On/Off
//! switch, and form rows with one label column so the fields line up down
//! the page.

use egui::{Align, Layout, Margin, RichText, Sense, Stroke, Ui, Vec2};

use crate::ui::theme::colors::theme_colors;
use crate::ui::widgets::{Mark, paint_mark_at};

/// The label column of a form row.
pub const LABEL_W: f32 = 170.0;
/// Between the label column and the field.
const LABEL_GAP: f32 = 20.0;
/// A row's height at least: a field's.
pub const ROW_H: f32 = 30.0;
/// The widest a page's content grows: past it, rows read as far apart.
pub const PAGE_MAX_W: f32 = 900.0;

pub const LABEL_SIZE: f32 = 14.0;
pub const HELP_SIZE: f32 = 13.0;
const STATUS_SIZE: f32 = 13.0;
const BLOCK_TITLE_SIZE: f32 = 17.0;
const GROUP_TITLE_SIZE: f32 = 19.0;
const PAGE_TITLE_SIZE: f32 = 26.0;

/// What a device or stream is doing: a mark, and the same in words.
#[derive(Clone, Debug, PartialEq)]
pub struct Status {
    pub mark: Mark,
    pub words: String,
}

impl Status {
    pub fn new(mark: Mark, words: impl Into<String>) -> Self {
        Self {
            mark,
            words: words.into(),
        }
    }

    pub fn off() -> Self {
        Self::new(Mark::Off, "Off")
    }
}

/// The page's title and what it is for.
pub fn page_heading(ui: &mut Ui, title: &str, desc: &str) {
    let tc = theme_colors(ui.ctx());
    ui.label(RichText::new(title).size(PAGE_TITLE_SIZE).strong());
    ui.add_space(2.0);
    ui.label(
        RichText::new(desc)
            .size(LABEL_SIZE)
            .color(tc.text_secondary),
    );
    ui.add_space(18.0);
}

/// A heading over a run of blocks or rows, with an optional note after it.
pub fn group_title(ui: &mut Ui, title: &str, note: Option<&str>) {
    let tc = theme_colors(ui.ctx());
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).size(GROUP_TITLE_SIZE).strong());
        if let Some(note) = note {
            ui.add_space(8.0);
            ui.label(RichText::new(note).size(HELP_SIZE).color(tc.text_secondary));
        }
    });
    ui.add_space(8.0);
}

/// A mark and words, as the head of a block shows them: on one line,
/// centered on the head's row.
pub fn status(ui: &mut Ui, s: &Status) {
    ui.horizontal(|ui| status_parts(ui, s, false));
}

/// [`status`] for a long message (a driver's error): the words wrap under
/// themselves rather than run past the block.
pub fn status_wrapped(ui: &mut Ui, s: &Status) {
    ui.horizontal_top(|ui| status_parts(ui, s, true));
}

fn status_parts(ui: &mut Ui, s: &Status, wrap: bool) {
    let tc = theme_colors(ui.ctx());
    ui.spacing_mut().item_spacing.x = 7.0;
    let line = STATUS_SIZE + 4.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(14.0, line), Sense::hover());
    paint_mark_at(ui.painter(), &tc, rect.center(), s.mark, 1.4);
    let text = RichText::new(&s.words).size(STATUS_SIZE);
    let text = if s.mark == Mark::Active {
        text.strong().color(tc.text_primary)
    } else {
        text.color(tc.text_secondary)
    };
    let label = egui::Label::new(text);
    ui.add(if wrap { label.wrap() } else { label.extend() });
}

/// A page's state for the list, from the states of its parts: how many are
/// switched on (anything but a dash), and how many of those are waiting.
/// Counting only the working ones read as "1 on" beside four switches at On.
pub fn summarize(parts: &[Status]) -> Status {
    let on = parts.iter().filter(|s| s.mark != Mark::Off).count();
    let waiting = parts.iter().filter(|s| s.mark == Mark::Idle).count();
    if parts.iter().any(|s| s.mark == Mark::Fault) {
        Status::new(Mark::Fault, "Needs a look")
    } else if on == 0 {
        Status::off()
    } else if waiting == 0 {
        Status::new(Mark::Active, format!("{on} on"))
    } else {
        Status::new(
            if waiting < on {
                Mark::Active
            } else {
                Mark::Idle
            },
            format!("{on} on, {waiting} waiting"),
        )
    }
}

/// One device or stream: its name and state on top, with `head_end` at the
/// right (a switch, a button), and its settings under them. Returns what
/// `head_end` returns, so a switch's click can be acted on after the body,
/// which may borrow the same device.
pub fn block<H>(
    ui: &mut Ui,
    title: &str,
    state: Option<&Status>,
    head_end: impl FnOnce(&mut Ui) -> H,
    body: impl FnOnce(&mut Ui),
) -> H {
    let tc = theme_colors(ui.ctx());
    let head = egui::Frame::new()
        .fill(tc.card_bg)
        .stroke(Stroke::new(1.0_f32, tc.card_border))
        .corner_radius(8.0)
        .inner_margin(Margin::symmetric(20, 16))
        .show(ui, |ui| {
            let mut head = None;
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.set_min_height(ROW_H + 2.0);
                ui.label(RichText::new(title).size(BLOCK_TITLE_SIZE).strong());
                if let Some(s) = state {
                    ui.add_space(10.0);
                    status(ui, s);
                }
                head = Some(
                    ui.with_layout(Layout::right_to_left(Align::Center), head_end)
                        .inner,
                );
            });
            ui.add_space(10.0);
            body(ui);
            head.expect("the head is drawn")
        })
        .inner;
    ui.add_space(14.0);
    head
}

/// A form row: the label in its column, the field beside it. The field's
/// closure may add help under the field with [`help`].
pub fn row<R>(ui: &mut Ui, label: &str, field: impl FnOnce(&mut Ui) -> R) -> R {
    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(
            Vec2::new(LABEL_W, ROW_H),
            Layout::left_to_right(Align::Center),
            |ui| {
                ui.set_min_size(Vec2::new(LABEL_W, ROW_H));
                ui.add(egui::Label::new(RichText::new(label).size(LABEL_SIZE).strong()).wrap());
            },
        );
        ui.add_space(LABEL_GAP - ui.spacing().item_spacing.x);
        ui.vertical(|ui| {
            ui.set_max_width(ui.available_width());
            ui.allocate_ui_with_layout(
                Vec2::new(ui.available_width(), ROW_H),
                Layout::left_to_right(Align::Center),
                |ui| {
                    ui.set_min_height(ROW_H);
                    field(ui)
                },
            )
            .inner
        })
        .inner
    })
    .inner
}

/// A form row whose field is taller than one line: checkboxes, a
/// paragraph, a stack of rows.
pub fn tall_row<R>(ui: &mut Ui, label: &str, field: impl FnOnce(&mut Ui) -> R) -> R {
    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(
            Vec2::new(LABEL_W, ROW_H),
            Layout::left_to_right(Align::Center),
            |ui| {
                ui.set_min_size(Vec2::new(LABEL_W, ROW_H));
                ui.add(egui::Label::new(RichText::new(label).size(LABEL_SIZE).strong()).wrap());
            },
        );
        ui.add_space(LABEL_GAP - ui.spacing().item_spacing.x);
        ui.vertical(|ui| {
            ui.set_max_width(ui.available_width());
            ui.add_space(5.0);
            field(ui)
        })
        .inner
    })
    .inner
}

/// A line of explanation, under a field or a block's head.
pub fn help(ui: &mut Ui, text: &str) {
    let tc = theme_colors(ui.ctx());
    ui.add(egui::Label::new(RichText::new(text).size(HELP_SIZE).color(tc.text_secondary)).wrap());
}

/// Help that lines up with the fields: indented past the label column.
pub fn row_help(ui: &mut Ui, text: &str) {
    ui.horizontal_top(|ui| {
        ui.add_space(LABEL_W + LABEL_GAP - ui.spacing().item_spacing.x);
        ui.vertical(|ui| {
            ui.set_max_width(ui.available_width());
            help(ui, text);
        });
    });
}

/// Monospace text a reader may copy: a command, a path, an address.
// Used by the install steps of streams and by Sync without Link.
#[cfg_attr(
    all(
        feature = "link",
        not(any(feature = "ndi", feature = "v4l2", feature = "syphon"))
    ),
    allow(dead_code)
)]
pub fn code(ui: &mut Ui, text: &str) {
    let tc = theme_colors(ui.ctx());
    egui::Frame::new()
        .fill(tc.widget_bg)
        .corner_radius(4.0)
        .inner_margin(Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.add(egui::Label::new(RichText::new(text).monospace().size(12.0)).wrap());
        });
}

/// The On/Off switch at the end of a block's head. The knob's side, the
/// fill and the word all say which, so none rests on color.
pub fn switch(ui: &mut Ui, on: bool, what: &str) -> egui::Response {
    let tc = theme_colors(ui.ctx());
    let size = Vec2::new(78.0, ROW_H);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    resp.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Checkbox, ui.is_enabled(), on, what)
    });
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let r = rect.height() / 2.0;
        let knob_r = r - 4.0;
        let hovered = resp.hovered();
        if on {
            p.rect_filled(rect, r, tc.selection);
            let c = egui::pos2(rect.right() - r, rect.center().y);
            p.circle_filled(c, knob_r, tc.on_selection);
            p.text(
                egui::pos2(rect.left() + 14.0, rect.center().y),
                egui::Align2::LEFT_CENTER,
                "On",
                egui::FontId::proportional(13.0),
                tc.on_selection,
            );
        } else {
            let line = if hovered {
                tc.text_primary
            } else {
                tc.text_secondary
            };
            p.rect_stroke(
                rect,
                r,
                Stroke::new(1.5_f32, line),
                egui::StrokeKind::Inside,
            );
            let c = egui::pos2(rect.left() + r, rect.center().y);
            p.circle_stroke(c, knob_r, Stroke::new(1.5_f32, line));
            p.text(
                egui::pos2(rect.right() - 14.0, rect.center().y),
                egui::Align2::RIGHT_CENTER,
                "Off",
                egui::FontId::proportional(13.0),
                tc.text_secondary,
            );
        }
        if resp.has_focus() {
            p.rect_stroke(
                rect.expand(3.0),
                r + 3.0,
                Stroke::new(2.0_f32, tc.text_primary),
                egui::StrokeKind::Outside,
            );
        }
    }
    resp.on_hover_text(format!("{what}: {}", if on { "on" } else { "off" }))
}

/// A combo box of `options` at `width`, showing `selected` closed. Returns
/// the index picked this frame, if one was picked that was not `current`.
pub fn pick(
    ui: &mut Ui,
    id: &str,
    width: f32,
    selected: &str,
    options: &[String],
    current: Option<usize>,
) -> Option<usize> {
    let mut picked = None;
    egui::ComboBox::from_id_salt(id)
        .width(width)
        .selected_text(RichText::new(selected).size(LABEL_SIZE))
        .show_ui(ui, |ui| {
            for (i, o) in options.iter().enumerate() {
                if ui
                    .selectable_label(current == Some(i), RichText::new(o).size(LABEL_SIZE))
                    .clicked()
                    && current != Some(i)
                {
                    picked = Some(i);
                }
            }
        });
    picked
}

/// Send `value` as the request `key`, the way every panel talks to
/// `main.rs`.
pub fn send<T: 'static + Clone + Send + Sync>(ui: &Ui, key: &str, value: T) {
    ui.ctx()
        .data_mut(|d| d.insert_temp(egui::Id::new(key), value));
}

/// A text field that sends `key` with its text when it loses focus with a
/// change, as the stream names do.
#[cfg_attr(
    not(any(feature = "ndi", feature = "spout", feature = "syphon")),
    allow(dead_code)
)]
pub fn name_field(ui: &mut Ui, id: &str, current: &str, key: &str) {
    let draft_id = egui::Id::new(id);
    let mut text: String = ui
        .ctx()
        .data(|d| d.get_temp(draft_id))
        .unwrap_or_else(|| current.to_string());
    let resp = ui.add(
        egui::TextEdit::singleline(&mut text)
            .desired_width(240.0)
            .font(egui::FontId::proportional(LABEL_SIZE)),
    );
    if resp.lost_focus() {
        if text != current {
            send(ui, key, text.clone());
        }
        ui.ctx().data_mut(|d| d.remove_temp::<String>(draft_id));
    } else if resp.has_focus() {
        ui.ctx().data_mut(|d| d.insert_temp(draft_id, text));
    }
}
