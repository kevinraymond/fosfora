//! Appearance and accessibility (#3125): the theme, and how big the
//! interface is drawn.

use egui::{Align2, CornerRadius, FontId, RichText, Sense, Stroke, StrokeKind, Ui};

use crate::ui::accessibility::focus::draw_focus_ring;
use crate::ui::theme::ThemeMode;
use crate::ui::theme::colors::theme_colors;
use crate::ui::theme::custom;
use crate::ui::theme::palette::Palette;
use crate::ui::theme::tokens::*;
use crate::ui::widgets::rows;

/// What the theme controls ask `main.rs` to do, under [`ThemeIntent::ID`].
#[derive(Debug, Clone, PartialEq)]
pub enum ThemeIntent {
    Use(ThemeMode),
    /// Save the theme in use as a new file in the themes folder, and use it.
    SaveCopy,
    OpenFolder,
    /// Read the themes folder again.
    Reload,
}

impl ThemeIntent {
    pub const ID: &str = "theme_intent";

    fn send(self, ui: &Ui) {
        ui.ctx()
            .data_mut(|d| d.insert_temp(egui::Id::new(Self::ID), self));
    }
}

/// The interface scale's range, in percent. The mockup's 80–200 %.
pub const SCALE_MIN: u32 = 80;
pub const SCALE_MAX: u32 = 200;
const SCALE_STEP: u32 = 10;

/// Temp-data key for a scale the user has let go of, read by `main.rs`.
pub const SET_UI_SCALE: &str = "set_ui_scale";
/// Temp-data key for the value under the pointer while the slider is held.
const HELD_SCALE: &str = "appearance_scale_held";

/// Clamp a stored or keyboard-zoomed scale into the slider's range.
pub fn clamp_scale(scale: f32) -> f32 {
    if !scale.is_finite() {
        return 1.0;
    }
    scale.clamp(SCALE_MIN as f32 / 100.0, SCALE_MAX as f32 / 100.0)
}

/// What to do with the slider this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
enum ScaleEdit {
    /// Nothing moved.
    None,
    /// Held: show the value, don't rescale yet.
    Held(u32),
    /// Let go, or stepped from the keyboard: rescale to this.
    Apply(u32),
}

/// Rescaling while the pointer holds the slider moves the slider out from
/// under it, so a drag only shows its value; the scale applies on release.
/// A keyboard step has no pointer to lose and applies at once.
fn scale_edit(value: u32, changed: bool, dragged: bool, released: bool) -> ScaleEdit {
    if released {
        ScaleEdit::Apply(value)
    } else if dragged {
        ScaleEdit::Held(value)
    } else if changed {
        ScaleEdit::Apply(value)
    } else {
        ScaleEdit::None
    }
}

/// `theme` and `scale` are the ones in use (scale 1.0 = 100 %).
pub fn draw_appearance_panel(ui: &mut Ui, theme: &ThemeMode, scale: f32) {
    theme_picker(ui, theme);
    ui.add_space(10.0);
    scale_row(ui, scale);
}

fn theme_picker(ui: &mut Ui, theme: &ThemeMode) {
    let tc = theme_colors(ui.ctx());
    let custom = custom::published(ui.ctx());

    ui.horizontal_wrapped(|ui| {
        for t in ThemeMode::BUILT_IN {
            let palette = t.built_in_palette().unwrap_or(Palette::GRAY);
            if theme_card(ui, &t.display_name(&[]), &palette, *theme == t, &[]).clicked() {
                ThemeIntent::Use(t).send(ui);
            }
        }
        for c in custom.iter() {
            let t = ThemeMode::Custom(c.stem.clone());
            if theme_card(ui, &c.name, &c.palette, *theme == t, &c.problems).clicked() {
                ThemeIntent::Use(t).send(ui);
            }
        }
    });
    ui.label(
        RichText::new(
            "Light, Gray and Black use no hue; Blue and orange adds a pair that stays \
             apart for red–green color blindness. In every theme, what is selected or \
             on also shows by fill, weight or words.",
        )
        .size(SMALL_SIZE)
        .color(tc.text_secondary),
    );

    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        if ui
            .button("Save a copy to edit")
            .on_hover_text(
                "Write the theme in use to a file in the themes folder and switch to it. \
                 Change its colors in any text editor, then press Reload.",
            )
            .clicked()
        {
            ThemeIntent::SaveCopy.send(ui);
        }
        if ui.button("Open themes folder").clicked() {
            ThemeIntent::OpenFolder.send(ui);
        }
        if ui
            .button("Reload")
            .on_hover_text("Read the theme files again")
            .clicked()
        {
            ThemeIntent::Reload.send(ui);
        }
    });
    ui.label(
        RichText::new(custom::themes_dir().display().to_string())
            .size(SMALL_SIZE)
            .monospace()
            .color(tc.text_secondary),
    );

    if let ThemeMode::Custom(stem) = theme
        && let Some(c) = custom::find(&custom, stem)
        && !c.problems.is_empty()
    {
        ui.add_space(4.0);
        ui.label(RichText::new(format!("To fix in {stem}.json:")).strong());
        for p in &c.problems {
            ui.label(RichText::new(format!("• {p}")).size(SMALL_SIZE));
        }
    }
}

/// One theme as a card: its colors as stripes, its name, and whether it is
/// in use. In use is a heavy outline and the words, never a color.
fn theme_card(
    ui: &mut Ui,
    name: &str,
    p: &Palette,
    selected: bool,
    problems: &[String],
) -> egui::Response {
    let tc = theme_colors(ui.ctx());
    let size = egui::vec2(124.0, 88.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    resp.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, name)
    });
    let painter = ui.painter_at(rect.expand(3.0));
    let r = CornerRadius::same(WIDGET_ROUNDING);
    if resp.hovered() {
        painter.rect_filled(rect, r, tc.hover_fill);
    }

    // Stripes: ground, panel, well, then text and accent as narrow bars.
    let strip = egui::Rect::from_min_size(
        rect.min + egui::vec2(6.0, 6.0),
        egui::vec2(rect.width() - 12.0, 34.0),
    );
    let narrow = 12.0;
    let bars: Vec<(egui::Color32, f32)> = {
        let mut v = vec![(p.bg, 1.0), (p.panel, 1.0), (p.well, 1.0), (p.text, 0.0)];
        if p.accent != p.text {
            v.push((p.accent, 0.0));
        }
        v
    };
    let fixed = bars.iter().filter(|b| b.1 == 0.0).count() as f32 * narrow;
    let unit = (strip.width() - fixed) / bars.iter().map(|b| b.1).sum::<f32>();
    let mut x = strip.left();
    for (c, grow) in bars {
        let w = if grow > 0.0 { unit * grow } else { narrow };
        painter.rect_filled(
            egui::Rect::from_min_size(egui::pos2(x, strip.top()), egui::vec2(w, strip.height())),
            0.0,
            c,
        );
        x += w;
    }
    painter.rect_stroke(
        strip,
        0.0,
        Stroke::new(1.0_f32, tc.card_border),
        StrokeKind::Inside,
    );

    painter.text(
        egui::pos2(strip.left(), strip.bottom() + 6.0),
        Align2::LEFT_TOP,
        name,
        FontId::proportional(BODY_SIZE),
        tc.text_primary,
    );
    let state = if selected {
        "✓ In use".to_string()
    } else if !problems.is_empty() {
        format!("{} to fix", problems.len())
    } else {
        "Use".to_string()
    };
    painter.text(
        egui::pos2(strip.left(), rect.bottom() - 6.0),
        Align2::LEFT_BOTTOM,
        state,
        FontId::proportional(SMALL_SIZE),
        if selected {
            tc.text_primary
        } else {
            tc.text_secondary
        },
    );
    let outline = if selected {
        Stroke::new(2.5_f32, tc.text_primary)
    } else {
        Stroke::new(1.0_f32, tc.card_border)
    };
    painter.rect_stroke(rect, r, outline, StrokeKind::Inside);
    draw_focus_ring(ui, &resp);

    if problems.is_empty() {
        resp
    } else {
        resp.on_hover_text(problems.join("\n"))
    }
}

/// `scale` is the one in use (1.0 = 100 %).
fn scale_row(ui: &mut Ui, scale: f32) {
    let tc = theme_colors(ui.ctx());
    let current = (scale * 100.0).round() as u32;
    let held_id = egui::Id::new(HELD_SCALE);
    let mut pct: u32 = ui
        .ctx()
        .data(|d| d.get_temp(held_id))
        .unwrap_or(current)
        .clamp(SCALE_MIN, SCALE_MAX);

    let row = rows::ParamRow::new("Interface scale")
        .tooltip(
            "How large the interface is drawn: text, controls and spacing together. \
             The output is not affected. Ctrl + and Ctrl − change it too; Ctrl 0 resets.",
        )
        .formatter(|v| format!("{v:.0}%"))
        .show_slider(ui, &mut pct, SCALE_MIN..=SCALE_MAX);
    // Snap to the slider's steps: a drag lands anywhere in between.
    pct = ((pct + SCALE_STEP / 2) / SCALE_STEP * SCALE_STEP).clamp(SCALE_MIN, SCALE_MAX);

    match scale_edit(
        pct,
        row.changed,
        row.response.dragged(),
        row.response.drag_stopped(),
    ) {
        ScaleEdit::Held(v) => ui.ctx().data_mut(|d| d.insert_temp(held_id, v)),
        ScaleEdit::Apply(v) => ui.ctx().data_mut(|d| {
            d.remove::<u32>(held_id);
            if v != current {
                d.insert_temp(egui::Id::new(SET_UI_SCALE), v as f32 / 100.0);
            }
        }),
        ScaleEdit::None => {}
    }

    // A line of text at the size the scale under the pointer would give, so a
    // drag previews its result before it applies.
    let preview = BODY_SIZE * pct as f32 / current.max(1) as f32;
    egui::Frame::new()
        .fill(tc.widget_bg)
        .corner_radius(WIDGET_ROUNDING)
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new("Opacity, Blend, Splat force: labels at this size.").size(preview),
            );
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drag_shows_its_value_and_applies_only_on_release() {
        assert_eq!(scale_edit(150, true, true, false), ScaleEdit::Held(150));
        assert_eq!(scale_edit(150, false, true, false), ScaleEdit::Held(150));
        assert_eq!(scale_edit(150, false, false, true), ScaleEdit::Apply(150));
    }

    #[test]
    fn a_keyboard_step_applies_at_once() {
        assert_eq!(scale_edit(110, true, false, false), ScaleEdit::Apply(110));
        assert_eq!(scale_edit(110, false, false, false), ScaleEdit::None);
    }

    /// The panel in a 420-point column, driven by real pointer events: one
    /// call is one frame. Returns the scale handed to `main.rs`, if any.
    fn frame(ctx: &egui::Context, t: &mut f64, events: Vec<egui::Event>) -> Option<f32> {
        *t += 1.0 / 60.0;
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(420.0, 300.0),
            )),
            time: Some(*t),
            events,
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| scale_row(ui, 1.0));
        });
        ctx.data_mut(|d| {
            let id = egui::Id::new(SET_UI_SCALE);
            let v = d.get_temp(id);
            d.remove::<f32>(id);
            v
        })
    }

    fn button(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        }
    }

    #[test]
    fn dragging_the_slider_rescales_once_on_release() {
        let ctx = egui::Context::default();
        let mut t = 0.0;
        frame(&ctx, &mut t, vec![]);
        // The slider's track sits between the label and value columns of the
        // first row.
        let from = egui::pos2(rows::LABEL_WIDTH + 60.0, 8.0 + MIN_INTERACT_HEIGHT / 2.0);
        let to = from + egui::vec2(160.0, 0.0);
        assert_eq!(
            frame(
                &ctx,
                &mut t,
                vec![egui::Event::PointerMoved(from), button(from, true)]
            ),
            None,
            "pressing must not rescale"
        );
        for k in 1..=6 {
            let p = from + (to - from) * (k as f32 / 6.0);
            assert_eq!(
                frame(&ctx, &mut t, vec![egui::Event::PointerMoved(p)]),
                None,
                "a held slider must not rescale (step {k})"
            );
        }
        let applied = frame(&ctx, &mut t, vec![button(to, false)]).expect("release applies");
        assert!(applied > 1.0, "dragged right, got {applied}");
        assert_eq!(
            (applied * 100.0).round() as u32 % SCALE_STEP,
            0,
            "lands on a step"
        );
        assert_eq!(frame(&ctx, &mut t, vec![]), None, "applies once");
    }

    #[test]
    fn stored_scales_are_clamped_into_the_range() {
        assert_eq!(clamp_scale(0.2), 0.8);
        assert_eq!(clamp_scale(5.0), 2.0);
        assert_eq!(clamp_scale(1.3), 1.3);
        assert_eq!(clamp_scale(f32::NAN), 1.0);
    }
}
