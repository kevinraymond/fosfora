use egui::{Color32, CornerRadius, Rect, RichText, Stroke, Ui, Vec2};

use crate::gpu::layer::{BlendMode, LayerInfo};
use crate::ui::theme::tokens::*;

// --- Custom icon buttons (painted as vector shapes, no font dependency) ---

fn icon_button(ui: &mut Ui, id: &str, color: Color32, paint: impl FnOnce(&egui::Painter, Rect, Color32)) -> egui::Response {
    let size = Vec2::splat(16.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let c = if response.hovered() { Color32::WHITE } else { color };
    paint(ui.painter(), rect, c);
    response
}

fn arrow_up_button(ui: &mut Ui, id: &str, color: Color32) -> egui::Response {
    icon_button(ui, id, color, |painter, rect, c| {
        let center = rect.center();
        let s = 4.0;
        let points = vec![
            egui::pos2(center.x, center.y - s),
            egui::pos2(center.x - s, center.y + s * 0.6),
            egui::pos2(center.x + s, center.y + s * 0.6),
        ];
        painter.add(egui::Shape::convex_polygon(points, c, Stroke::NONE));
    })
}

fn arrow_down_button(ui: &mut Ui, id: &str, color: Color32) -> egui::Response {
    icon_button(ui, id, color, |painter, rect, c| {
        let center = rect.center();
        let s = 4.0;
        let points = vec![
            egui::pos2(center.x, center.y + s),
            egui::pos2(center.x - s, center.y - s * 0.6),
            egui::pos2(center.x + s, center.y - s * 0.6),
        ];
        painter.add(egui::Shape::convex_polygon(points, c, Stroke::NONE));
    })
}

fn close_button(ui: &mut Ui, id: &str, color: Color32) -> egui::Response {
    icon_button(ui, id, color, |painter, rect, c| {
        let center = rect.center();
        let s = 3.5;
        let stroke = Stroke::new(1.5, c);
        painter.line_segment(
            [egui::pos2(center.x - s, center.y - s), egui::pos2(center.x + s, center.y + s)],
            stroke,
        );
        painter.line_segment(
            [egui::pos2(center.x + s, center.y - s), egui::pos2(center.x - s, center.y + s)],
            stroke,
        );
    })
}

/// Draw the layer management panel.
pub fn draw_layer_panel(ui: &mut Ui, layers: &[LayerInfo], active_layer: usize) {
    let max_layers = 8;
    let num_layers = layers.len();

    for (i, layer) in layers.iter().enumerate() {
        let is_active = i == active_layer;

        let text_color = if is_active {
            Color32::WHITE
        } else if !layer.enabled {
            DARK_TEXT_SECONDARY
        } else {
            DARK_TEXT_PRIMARY
        };

        // Outer container: groups header + blend/opacity controls per layer
        let outer_stroke = if is_active {
            Stroke::new(1.0, DARK_ACCENT)
        } else {
            Stroke::new(1.0, CARD_BORDER)
        };

        egui::Frame::new()
            .fill(CARD_BG)
            .stroke(outer_stroke)
            .corner_radius(CornerRadius::same(4))
            .inner_margin(egui::Margin::same(0))
            .outer_margin(egui::Margin::symmetric(0, 1))
            .show(ui, |ui| {
                // Header row
                let header_fill = if is_active { DARK_ACCENT } else { CARD_BG };
                egui::Frame::new()
                    .fill(header_fill)
                    .corner_radius(if is_active && num_layers > 1 {
                        CornerRadius { nw: 4, ne: 4, sw: 0, se: 0 }
                    } else {
                        CornerRadius::same(4)
                    })
                    .inner_margin(egui::Margin::symmetric(6, 3))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 3.0;

                            // Enable checkbox
                            let mut enabled = layer.enabled;
                            let cb = ui.checkbox(&mut enabled, "");
                            if cb.changed() {
                                ui.ctx().data_mut(|d| {
                                    d.insert_temp(egui::Id::new("layer_toggle_enable"), (i, enabled));
                                });
                            }
                            cb.on_hover_text(if enabled { "Disable layer" } else { "Enable layer" });

                            // Layer name / effect — fills remaining space, clickable to select
                            let effect_display = layer.effect_name.as_deref().unwrap_or("(empty)");
                            let display = format!("{} {}", i + 1, effect_display);

                            // Calculate how much space the right buttons take
                            let btn_count = (if i > 0 { 1 } else { 0 })
                                + (if i < num_layers - 1 { 1 } else { 0 })
                                + (if num_layers > 1 { 1 } else { 0 });
                            let btns_width = btn_count as f32 * 19.0;
                            let label_width = (ui.available_width() - btns_width).max(20.0);

                            let label = ui.add_sized(
                                Vec2::new(label_width, ui.spacing().interact_size.y),
                                egui::Label::new(
                                    RichText::new(&display).size(BODY_SIZE).color(text_color),
                                )
                                .selectable(false)
                                .sense(egui::Sense::click()),
                            );
                            if label.clicked() {
                                ui.ctx().data_mut(|d| {
                                    d.insert_temp(egui::Id::new("select_layer"), i);
                                });
                            }
                            if !is_active {
                                label.on_hover_text("Click to select layer");
                            }

                            // Move up
                            if i > 0 {
                                let up = arrow_up_button(ui, &format!("layer_up_{i}"), text_color);
                                if up.clicked() {
                                    ui.ctx().data_mut(|d| {
                                        d.insert_temp(egui::Id::new("layer_move"), (i, i - 1));
                                    });
                                }
                                up.on_hover_text("Move up");
                            }

                            // Move down
                            if i < num_layers - 1 {
                                let down = arrow_down_button(ui, &format!("layer_dn_{i}"), text_color);
                                if down.clicked() {
                                    ui.ctx().data_mut(|d| {
                                        d.insert_temp(egui::Id::new("layer_move"), (i, i + 1));
                                    });
                                }
                                down.on_hover_text("Move down");
                            }

                            // Delete
                            if num_layers > 1 {
                                let del = close_button(ui, &format!("layer_del_{i}"), text_color);
                                if del.clicked() {
                                    ui.ctx().data_mut(|d| {
                                        d.insert_temp(egui::Id::new("remove_layer"), i);
                                    });
                                }
                                del.on_hover_text("Delete layer");
                            }
                        });
                    });

                // Blend mode + opacity shown BELOW the active layer header
                if is_active && num_layers > 1 {
                    egui::Frame::new()
                        .fill(DARK_WIDGET_BG)
                        .corner_radius(CornerRadius { nw: 0, ne: 0, sw: 4, se: 4 })
                        .inner_margin(egui::Margin::symmetric(6, 4))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new("Blend")
                                        .size(SMALL_SIZE)
                                        .color(DARK_TEXT_SECONDARY),
                                );
                                let current_mode = layer.blend_mode;
                                egui::ComboBox::from_id_salt(format!("blend_mode_{i}"))
                                    .selected_text(
                                        RichText::new(current_mode.display_name()).size(SMALL_SIZE),
                                    )
                                    .width(ui.available_width() - 4.0)
                                    .show_ui(ui, |ui| {
                                        for &mode in BlendMode::ALL {
                                            let r = ui.selectable_label(
                                                mode == current_mode,
                                                RichText::new(mode.display_name()).size(SMALL_SIZE),
                                            );
                                            if r.clicked() && mode != current_mode {
                                                ui.ctx().data_mut(|d| {
                                                    d.insert_temp(
                                                        egui::Id::new("layer_blend"),
                                                        mode.as_u32(),
                                                    );
                                                });
                                            }
                                        }
                                    });
                            });

                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new("Opacity")
                                        .size(SMALL_SIZE)
                                        .color(DARK_TEXT_SECONDARY),
                                );
                                // Override slider rail color so it's visible against DARK_WIDGET_BG
                                let saved_bg = ui.visuals().widgets.inactive.bg_fill;
                                ui.visuals_mut().widgets.inactive.bg_fill = METER_BG;
                                let mut opacity = layer.opacity;
                                let slider = ui.add(
                                    egui::Slider::new(&mut opacity, 0.0..=1.0)
                                        .show_value(true)
                                        .custom_formatter(|v, _| format!("{:.0}%", v * 100.0))
                                        .text(""),
                                );
                                ui.visuals_mut().widgets.inactive.bg_fill = saved_bg;
                                if slider.changed() {
                                    ui.ctx().data_mut(|d| {
                                        d.insert_temp(egui::Id::new("layer_opacity"), opacity);
                                    });
                                }
                            });
                        });
                }
            });
    }

    // Add Layer button
    ui.add_space(4.0);
    let can_add = num_layers < max_layers;
    let add_btn = ui.add_enabled(
        can_add,
        egui::Button::new(
            RichText::new("+ Add Layer")
                .size(SMALL_SIZE)
                .color(if can_add { DARK_ACCENT } else { DARK_TEXT_SECONDARY }),
        )
        .fill(Color32::TRANSPARENT)
        .stroke(Stroke::new(1.0, CARD_BORDER))
        .corner_radius(CornerRadius::same(4))
        .min_size(Vec2::new(ui.available_width(), MIN_INTERACT_HEIGHT)),
    );
    if add_btn.clicked() {
        ui.ctx()
            .data_mut(|d| d.insert_temp(egui::Id::new("add_layer"), true));
    }
    if can_add {
        add_btn.on_hover_text("Add a new layer (max 8)");
    } else {
        add_btn.on_hover_text("Maximum 8 layers reached");
    }
}
