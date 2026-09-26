pub mod rows;

use egui::{
    Color32, CornerRadius, Frame, Margin, RichText, Shape, Stroke, Ui,
    collapsing_header::CollapsingState, pos2,
};

use super::theme::colors::theme_colors;
use super::theme::tokens::*;

/// Format a float value compactly: no trailing zeros, max 2 decimal places.
pub fn fmt_val(v: f64) -> String {
    if v == v.round() {
        format!("{v:.0}")
    } else if (v * 10.0).round() == v * 10.0 {
        format!("{v:.1}")
    } else {
        format!("{v:.2}")
    }
}

/// Char-safe truncation with an ellipsis. Safe on multibyte names (a byte-indexed
/// `&name[..n]` panics on a UTF-8 boundary — several panels used to do exactly that).
pub fn truncate_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max_chars.saturating_sub(1)).collect();
        out.push('\u{2026}');
        out
    }
}

/// Styled card frame for panel sections.
pub fn card_frame(ui: &Ui) -> Frame {
    let tc = theme_colors(ui.ctx());
    Frame {
        fill: tc.card_bg,
        stroke: Stroke::new(1.0_f32, tc.card_border),
        corner_radius: CornerRadius::same(CARD_ROUNDING),
        inner_margin: Margin::same(CARD_PADDING as i8),
        outer_margin: Margin::symmetric(0, CARD_MARGIN as i8),
        ..Default::default()
    }
}

/// A header that takes a click anywhere on it: the full width, and at least
/// `min_height` tall. Its labels are made unselectable, because a selectable
/// label keeps the click for itself — headers built as a row of labels only
/// toggled on the gaps between them. Buttons inside still get their own
/// clicks: they sit above the row.
pub fn header_row(ui: &mut Ui, min_height: f32, add: impl FnOnce(&mut Ui)) -> egui::Response {
    ui.scope_builder(egui::UiBuilder::new().sense(egui::Sense::click()), |ui| {
        ui.style_mut().interaction.selectable_labels = false;
        // A row of exactly this size. `with_layout` would center the contents
        // in all the height left below, and the header grew to fill it.
        let size = egui::vec2(ui.available_width(), min_height);
        ui.allocate_ui_with_layout(
            size,
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.set_min_size(size);
                add(ui);
            },
        );
    })
    .response
}

/// One row of controls in `layout`, exactly `MIN_INTERACT_HEIGHT` tall and
/// the full width. A bare `with_layout` takes all the height left below it —
/// the row, and whatever frame holds it, grows to fill the column.
pub fn layout_row<R>(ui: &mut Ui, layout: egui::Layout, add: impl FnOnce(&mut Ui) -> R) -> R {
    let size = egui::vec2(ui.available_width(), MIN_INTERACT_HEIGHT);
    ui.allocate_ui_with_layout(size, layout, add).inner
}

/// Collapsible section with card styling.
/// Returns the inner `Ui` response if the section is open.
pub fn section(
    ui: &mut Ui,
    id: &str,
    title: &str,
    badge: Option<&str>,
    default_open: bool,
    add_body: impl FnOnce(&mut Ui),
) {
    let tc = theme_colors(ui.ctx());
    let id = ui.make_persistent_id(id);
    let state = CollapsingState::load_with_default_open(ui.ctx(), id, default_open);

    card_frame(ui).show(ui, |ui| {
        let header_response = header_row(ui, MIN_INTERACT_HEIGHT, |ui| {
            draw_section_arrow(ui, state.is_open(), tc.text_secondary);
            ui.label(
                RichText::new(title.to_uppercase())
                    .size(HEADING_SIZE)
                    .color(tc.text_secondary)
                    .strong(),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some(badge_text) = badge {
                    ui.label(RichText::new(badge_text).size(SMALL_SIZE).color(tc.accent));
                }
            });
        });

        if header_response.clicked() {
            let mut state = CollapsingState::load_with_default_open(ui.ctx(), id, default_open);
            state.toggle(ui);
            state.store(ui.ctx());
        }

        // Body
        if state.is_open() {
            ui.add_space(4.0);
            add_body(ui);
        }
    });
}

/// Draw a solid triangle indicator for collapsible sections.
pub(crate) fn draw_section_arrow(ui: &mut Ui, is_open: bool, color: Color32) {
    draw_section_arrow_sized(ui, is_open, color, HEADING_SIZE);
}

/// Draw a solid triangle indicator at a specific size.
pub(crate) fn draw_section_arrow_sized(ui: &mut Ui, is_open: bool, color: Color32, size: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    let c = rect.center();
    let half = size * 0.3;
    let points = if is_open {
        // Down-pointing triangle
        vec![
            pos2(c.x - half, c.y - half * 0.5),
            pos2(c.x + half, c.y - half * 0.5),
            pos2(c.x, c.y + half * 0.5),
        ]
    } else {
        // Right-pointing triangle
        vec![
            pos2(c.x - half * 0.5, c.y - half),
            pos2(c.x + half * 0.5, c.y),
            pos2(c.x - half * 0.5, c.y + half),
        ]
    };
    ui.painter()
        .add(Shape::convex_polygon(points, color, Stroke::NONE));
}

/// Collapsible section with card styling and custom header content (e.g. status dots).
pub fn section_with_header(
    ui: &mut Ui,
    id: &str,
    title: &str,
    add_header: impl FnOnce(&mut Ui),
    default_open: bool,
    add_body: impl FnOnce(&mut Ui),
) {
    let tc = theme_colors(ui.ctx());
    let id = ui.make_persistent_id(id);
    let state = CollapsingState::load_with_default_open(ui.ctx(), id, default_open);

    card_frame(ui).show(ui, |ui| {
        let header_response = header_row(ui, MIN_INTERACT_HEIGHT, |ui| {
            draw_section_arrow(ui, state.is_open(), tc.text_secondary);
            ui.label(
                RichText::new(title.to_uppercase())
                    .size(HEADING_SIZE)
                    .color(tc.text_secondary)
                    .strong(),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                add_header(ui);
            });
        });

        if header_response.clicked() {
            let mut state = CollapsingState::load_with_default_open(ui.ctx(), id, default_open);
            state.toggle(ui);
            state.store(ui.ctx());
        }

        if state.is_open() {
            add_body(ui);
        }
    });
}

/// Subsection title: a step below the section heading, at the text floor.
const SUBSECTION_SIZE: f32 = SMALL_SIZE;
/// Subsection arrow size.
const SUBSECTION_ARROW: f32 = 8.0;
/// Subsection badge font size.
const SUBSECTION_BADGE: f32 = SMALL_SIZE;

/// Lightweight collapsible subsection (no card frame) for nesting inside a parent section.
/// Matches the JSX `SectionLabel` style: small arrow + uppercase title + ON/OFF badge.
pub fn subsection(
    ui: &mut Ui,
    id: &str,
    title: &str,
    badge_text: Option<&str>,
    badge_color: Color32,
    default_open: bool,
    add_body: impl FnOnce(&mut Ui),
) {
    let tc = theme_colors(ui.ctx());
    let id = ui.make_persistent_id(id);
    let state = CollapsingState::load_with_default_open(ui.ctx(), id, default_open);

    // JSX: marginTop 10 on every SectionLabel
    ui.add_space(10.0);

    let header_response = header_row(ui, SUBSECTION_SIZE + 6.0, |ui| {
        ui.spacing_mut().item_spacing.x = 5.0;
        // Smaller arrow than parent section (JSX font-size 8 vs 11)
        draw_section_arrow_sized(ui, state.is_open(), tc.text_secondary, SUBSECTION_ARROW);
        ui.label(
            RichText::new(title.to_uppercase())
                .size(SUBSECTION_SIZE)
                .color(tc.text_secondary),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if let Some(text) = badge_text {
                ui.label(
                    RichText::new(text)
                        .size(SUBSECTION_BADGE)
                        .color(badge_color)
                        .strong(),
                );
            }
        });
    });

    if header_response.clicked() {
        let mut state = CollapsingState::load_with_default_open(ui.ctx(), id, default_open);
        state.toggle(ui);
        state.store(ui.ctx());
    }

    // JSX: marginBottom 6 — gap between header and body content
    if state.is_open() {
        ui.add_space(6.0);
        add_body(ui);
    }
}

/// Badge label in accent color at small size.
#[allow(dead_code)]
pub fn badge(ui: &mut Ui, text: &str, color: Color32) {
    ui.label(RichText::new(text).size(SMALL_SIZE).color(color));
}

/// Draw diagonal stripes over a rect (clipped). Used for transition effects.
pub fn draw_diagonal_stripes(
    painter: &egui::Painter,
    rect: egui::Rect,
    color: Color32,
    spacing: f32,
) {
    let clipped = painter.with_clip_rect(rect);
    let stroke = Stroke::new(1.5_f32, color);
    let h = rect.height();
    let mut offset = -h;
    while offset < rect.width() {
        let from = egui::pos2(rect.left() + offset, rect.bottom());
        let to = egui::pos2(rect.left() + offset + h, rect.top());
        clipped.line_segment([from, to], stroke);
        offset += spacing;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_val_compact() {
        assert_eq!(fmt_val(3.0), "3");
        assert_eq!(fmt_val(0.5), "0.5");
        assert_eq!(fmt_val(0.85), "0.85");
        assert_eq!(fmt_val(0.333), "0.33");
        assert_eq!(fmt_val(-2.2), "-2.2");
    }

    #[test]
    fn truncate_chars_short_passthrough() {
        assert_eq!(truncate_chars("Beam", 10), "Beam");
        assert_eq!(truncate_chars("exactly-10", 10), "exactly-10");
    }

    #[test]
    fn truncate_chars_truncates_with_ellipsis() {
        assert_eq!(truncate_chars("Lattice Pyroclastic", 10), "Lattice P…");
    }

    #[test]
    fn truncate_chars_multibyte_safe() {
        // A byte-indexed slice at 10 would panic inside the multibyte char.
        let s = "Ätherwellen — Über";
        assert_eq!(truncate_chars(s, 10), "Ätherwell…");
        assert_eq!(truncate_chars("日本語テスト名前長い", 5), "日本語テ…");
    }

    /// Click a section's header at `x` (points from the left) and report
    /// whether its body is open afterwards.
    fn section_open_after_click(x: f32) -> bool {
        let ctx = egui::Context::default();
        let mut open = false;
        let run = |events: Vec<egui::Event>, open: &mut bool| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 300.0),
                )),
                events,
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    *open = false;
                    section(ui, "t", "Presets and scenes", Some("3"), false, |_| {
                        *open = true;
                    });
                });
            });
        };
        run(vec![], &mut open);
        run(vec![], &mut open);
        // The header's middle: 8 panel margin + 4 card margin + 8 padding
        // + half a row.
        let pos = egui::pos2(x, 8.0 + 4.0 + 8.0 + MIN_INTERACT_HEIGHT / 2.0);
        let button = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        run(
            vec![egui::Event::PointerMoved(pos), button(true)],
            &mut open,
        );
        run(vec![button(false)], &mut open);
        run(vec![], &mut open);
        open
    }

    #[test]
    fn a_section_header_toggles_from_its_title_text() {
        assert!(section_open_after_click(80.0), "on the title text");
    }

    #[test]
    fn a_section_header_toggles_from_its_far_end() {
        assert!(section_open_after_click(300.0), "right of the title");
    }

    #[test]
    fn a_header_is_one_row_tall_however_much_room_is_below_it() {
        let ctx = egui::Context::default();
        let mut h = 0.0;
        for _ in 0..3 {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 900.0),
                )),
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    h = header_row(ui, MIN_INTERACT_HEIGHT, |ui| {
                        ui.label("PRESETS");
                    })
                    .rect
                    .height();
                });
            });
        }
        assert!(h <= MIN_INTERACT_HEIGHT + 1.0, "the header is {h} tall");
    }
}
