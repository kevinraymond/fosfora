use egui::{
    Color32, CornerRadius, Frame, Margin, RichText, Shape, Stroke, Ui,
    collapsing_header::CollapsingState, pos2,
};

use super::theme::colors::theme_colors;
use super::theme::tokens::*;

/// Styled card frame for panel sections.
pub fn card_frame(ui: &Ui) -> Frame {
    let tc = theme_colors(ui.ctx());
    Frame {
        fill: tc.card_bg,
        stroke: Stroke::new(1.0, tc.card_border),
        corner_radius: CornerRadius::same(CARD_ROUNDING),
        inner_margin: Margin::same(CARD_PADDING as i8),
        outer_margin: Margin::symmetric(0, CARD_MARGIN as i8),
        ..Default::default()
    }
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
        let full_width = ui.available_width();

        // Header row — always full width
        let header_response = ui.horizontal(|ui| {
            ui.set_min_width(full_width);
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

        // Toggle on header click
        if header_response
            .response
            .interact(egui::Sense::click())
            .clicked()
        {
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
    ui.painter().add(Shape::convex_polygon(
        points,
        color,
        Stroke::NONE,
    ));
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
        let full_width = ui.available_width();

        let header_response = ui.horizontal(|ui| {
            ui.set_min_width(full_width);
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

        if header_response
            .response
            .interact(egui::Sense::click())
            .clicked()
        {
            let mut state = CollapsingState::load_with_default_open(ui.ctx(), id, default_open);
            state.toggle(ui);
            state.store(ui.ctx());
        }

        if state.is_open() {
            add_body(ui);
        }
    });
}

/// Subsection font size — smaller than parent section heading (JSX: 9px vs 11px).
const SUBSECTION_SIZE: f32 = 9.0;
/// Subsection arrow size (JSX: font-size 8).
const SUBSECTION_ARROW: f32 = 8.0;
/// Subsection badge font size (JSX: font-size 8).
const SUBSECTION_BADGE: f32 = 8.0;

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

    let full_width = ui.available_width();
    let header_response = ui.horizontal(|ui| {
        ui.set_min_width(full_width);
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

    if header_response
        .response
        .interact(egui::Sense::click())
        .clicked()
    {
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
    let stroke = Stroke::new(1.5, color);
    let h = rect.height();
    let mut offset = -h;
    while offset < rect.width() {
        let from = egui::pos2(rect.left() + offset, rect.bottom());
        let to = egui::pos2(rect.left() + offset + h, rect.top());
        clipped.line_segment([from, to], stroke);
        offset += spacing;
    }
}
