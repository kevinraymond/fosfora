use std::sync::Arc;

use egui::{Color32, CornerRadius, Rect, RichText, Stroke, StrokeKind, Ui, Vec2};

use crate::gpu::layer::{BlendMode, LayerInfo};
use crate::ui::theme::tokens::*;

// --- Custom icon buttons (painted as vector shapes, no font dependency) ---

fn icon_button(
    ui: &mut Ui,
    _id: &str,
    color: Color32,
    paint: impl FnOnce(&egui::Painter, Rect, Color32),
) -> egui::Response {
    let size = Vec2::splat(16.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let c = if response.hovered() { Color32::WHITE } else { color };
    paint(ui.painter(), rect, c);
    response
}

fn close_button(ui: &mut Ui, id: &str, color: Color32) -> egui::Response {
    icon_button(ui, id, color, |painter, rect, c| {
        let center = rect.center();
        let s = 3.5;
        let stroke = Stroke::new(1.5, c);
        painter.line_segment(
            [
                egui::pos2(center.x - s, center.y - s),
                egui::pos2(center.x + s, center.y + s),
            ],
            stroke,
        );
        painter.line_segment(
            [
                egui::pos2(center.x + s, center.y - s),
                egui::pos2(center.x - s, center.y + s),
            ],
            stroke,
        );
    })
}

/// Drag handle icon (three horizontal lines). Returns response with drag sense.
fn drag_handle(ui: &mut Ui, color: Color32) -> egui::Response {
    let size = Vec2::new(12.0, 16.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::drag());
    let c = if response.hovered() || response.dragged() {
        Color32::WHITE
    } else {
        color
    };
    let painter = ui.painter();
    let cx = rect.center().x;
    let cy = rect.center().y;
    let stroke = Stroke::new(1.2, c);
    for dy in [-3.0_f32, 0.0, 3.0] {
        painter.line_segment(
            [egui::pos2(cx - 3.5, cy + dy), egui::pos2(cx + 3.5, cy + dy)],
            stroke,
        );
    }
    response
}

/// Lock icon — open padlock (unlocked) or closed padlock (locked).
fn lock_button(ui: &mut Ui, id: &str, locked: bool, color: Color32) -> egui::Response {
    icon_button(ui, id, color, |painter, rect, c| {
        let cx = rect.center().x;
        let cy = rect.center().y;
        let stroke = Stroke::new(1.2, c);
        // Body (rectangle)
        let body = Rect::from_center_size(egui::pos2(cx, cy + 1.5), Vec2::new(8.0, 6.0));
        if locked {
            painter.rect_filled(body, CornerRadius::same(1), c);
        } else {
            painter.rect_stroke(body, CornerRadius::same(1), stroke, StrokeKind::Outside);
        }
        // Shackle (arc)
        let shackle_top = if locked { cy - 4.0 } else { cy - 5.5 };
        let shackle_left = cx - 2.5;
        let shackle_right = cx + 2.5;
        if locked {
            painter.line_segment(
                [
                    egui::pos2(shackle_left, cy - 1.5),
                    egui::pos2(shackle_left, shackle_top),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(shackle_left, shackle_top),
                    egui::pos2(shackle_right, shackle_top),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(shackle_right, shackle_top),
                    egui::pos2(shackle_right, cy - 1.5),
                ],
                stroke,
            );
        } else {
            painter.line_segment(
                [
                    egui::pos2(shackle_left, cy - 1.5),
                    egui::pos2(shackle_left, shackle_top),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(shackle_left, shackle_top),
                    egui::pos2(shackle_right, shackle_top),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(shackle_right, shackle_top),
                    egui::pos2(shackle_right, shackle_top + 1.5),
                ],
                stroke,
            );
        }
    })
}

/// Pin icon — filled when pinned, outline when unpinned.
fn pin_button(ui: &mut Ui, id: &str, pinned: bool, color: Color32) -> egui::Response {
    icon_button(ui, id, color, |painter, rect, c| {
        let cx = rect.center().x;
        let cy = rect.center().y;
        let stroke = Stroke::new(1.2, c);
        if pinned {
            painter.circle_filled(egui::pos2(cx, cy - 2.0), 3.5, c);
        } else {
            painter.circle_stroke(egui::pos2(cx, cy - 2.0), 3.5, stroke);
        }
        painter.line_segment(
            [egui::pos2(cx, cy + 1.5), egui::pos2(cx, cy + 5.5)],
            stroke,
        );
    })
}

/// Find which slot (0..=num_layers) the pointer is closest to, based on card rects.
/// Slot i means "insert before card i"; slot num_layers means "insert at end".
fn find_drop_slot(pos_y: f32, card_rects: &[Rect]) -> usize {
    for (i, rect) in card_rects.iter().enumerate() {
        if pos_y < rect.center().y {
            return i;
        }
    }
    card_rects.len()
}

/// Draw the layer management panel.
pub fn draw_layer_panel(ui: &mut Ui, layers: &[LayerInfo], active_layer: usize) {
    let max_layers = 8;
    let num_layers = layers.len();
    let ctx = ui.ctx().clone();

    // Check if a drag is in progress
    let dragging_idx: Option<usize> = egui::DragAndDrop::payload::<usize>(&ctx)
        .map(|p: Arc<usize>| *p);
    let is_dragging = dragging_idx.is_some();

    // Collect card rects during rendering
    let mut card_rects: Vec<Rect> = Vec::with_capacity(num_layers);

    let (_drop_inner, drop_payload) =
        ui.dnd_drop_zone::<usize, ()>(egui::Frame::new(), |ui| {
            for (i, layer) in layers.iter().enumerate() {
                let is_active = i == active_layer;
                let is_being_dragged = dragging_idx == Some(i);

                // Fade non-dragged layers while dragging
                let alpha = if is_dragging && !is_being_dragged {
                    0.4_f32
                } else {
                    1.0
                };

                // Title/pin color: not dimmed by lock
                let title_color = {
                    let base = if is_active {
                        Color32::WHITE
                    } else if !layer.enabled {
                        DARK_TEXT_SECONDARY
                    } else {
                        DARK_TEXT_PRIMARY
                    };
                    if alpha < 1.0 {
                        Color32::from_rgba_unmultiplied(
                            base.r(),
                            base.g(),
                            base.b(),
                            (base.a() as f32 * alpha) as u8,
                        )
                    } else {
                        base
                    }
                };

                // Controls color: dimmed when locked
                let ctrl_color = if layer.locked {
                    let base = DARK_TEXT_SECONDARY;
                    if alpha < 1.0 {
                        Color32::from_rgba_unmultiplied(
                            base.r(),
                            base.g(),
                            base.b(),
                            (base.a() as f32 * alpha) as u8,
                        )
                    } else {
                        base
                    }
                } else {
                    title_color
                };

                let outer_stroke = {
                    let base_color = if is_active { DARK_ACCENT } else { CARD_BORDER };
                    if alpha < 1.0 {
                        Stroke::new(
                            1.0,
                            Color32::from_rgba_unmultiplied(
                                base_color.r(),
                                base_color.g(),
                                base_color.b(),
                                (base_color.a() as f32 * alpha) as u8,
                            ),
                        )
                    } else {
                        Stroke::new(1.0, base_color)
                    }
                };

                let card_fill = if layer.locked {
                    Color32::from_rgba_unmultiplied(
                        CARD_BG.r(),
                        CARD_BG.g(),
                        CARD_BG.b(),
                        (180.0 * alpha) as u8,
                    )
                } else {
                    Color32::from_rgba_unmultiplied(
                        CARD_BG.r(),
                        CARD_BG.g(),
                        CARD_BG.b(),
                        (CARD_BG.a() as f32 * alpha) as u8,
                    )
                };

                let card_resp = egui::Frame::new()
                    .fill(card_fill)
                    .stroke(outer_stroke)
                    .corner_radius(CornerRadius::same(4))
                    .inner_margin(egui::Margin::same(0))
                    .outer_margin(egui::Margin::symmetric(0, 1))
                    .show(ui, |ui| {
                        // Header row
                        let header_fill = if is_active {
                            let c = DARK_ACCENT;
                            Color32::from_rgba_unmultiplied(
                                c.r(),
                                c.g(),
                                c.b(),
                                (c.a() as f32 * alpha) as u8,
                            )
                        } else {
                            card_fill
                        };
                        egui::Frame::new()
                            .fill(header_fill)
                            .corner_radius(if is_active && num_layers > 1 {
                                CornerRadius {
                                    nw: 4,
                                    ne: 4,
                                    sw: 0,
                                    se: 0,
                                }
                            } else {
                                CornerRadius::same(4)
                            })
                            .inner_margin(egui::Margin::symmetric(6, 3))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 3.0;

                                    // Drag handle — only this starts drags
                                    if !layer.pinned {
                                        let handle_color = if is_active {
                                            Color32::from_white_alpha(
                                                (100.0 * alpha) as u8,
                                            )
                                        } else {
                                            ctrl_color
                                        };
                                        let handle = drag_handle(ui, handle_color);
                                        if handle.drag_started() {
                                            egui::DragAndDrop::set_payload(
                                                ui.ctx(),
                                                i,
                                            );
                                        }
                                    }

                                    // Enable checkbox (disabled when locked)
                                    ui.add_enabled_ui(!layer.locked, |ui| {
                                        let mut enabled = layer.enabled;
                                        let cb = ui.checkbox(&mut enabled, "");
                                        if cb.changed() {
                                            ui.ctx().data_mut(|d| {
                                                d.insert_temp(
                                                    egui::Id::new("layer_toggle_enable"),
                                                    (i, enabled),
                                                );
                                            });
                                        }
                                        cb.on_hover_text(if layer.locked {
                                            "Layer is locked"
                                        } else if enabled {
                                            "Disable layer"
                                        } else {
                                            "Enable layer"
                                        });
                                    });

                                    // Lock button
                                    let lock_color = if layer.locked {
                                        Color32::from_rgb(255, 180, 60)
                                    } else {
                                        title_color
                                    };
                                    let lock = lock_button(
                                        ui,
                                        &format!("layer_lock_{i}"),
                                        layer.locked,
                                        lock_color,
                                    );
                                    if lock.clicked() {
                                        ui.ctx().data_mut(|d| {
                                            d.insert_temp(
                                                egui::Id::new("layer_toggle_lock"),
                                                (i, !layer.locked),
                                            );
                                        });
                                    }
                                    lock.on_hover_text(if layer.locked {
                                        "Unlock layer"
                                    } else {
                                        "Lock layer (prevent changes)"
                                    });

                                    // Pin button
                                    let pin_color = if layer.pinned {
                                        Color32::from_rgb(100, 180, 255)
                                    } else {
                                        title_color
                                    };
                                    let pin = pin_button(
                                        ui,
                                        &format!("layer_pin_{i}"),
                                        layer.pinned,
                                        pin_color,
                                    );
                                    if pin.clicked() {
                                        ui.ctx().data_mut(|d| {
                                            d.insert_temp(
                                                egui::Id::new("layer_toggle_pin"),
                                                (i, !layer.pinned),
                                            );
                                        });
                                    }
                                    pin.on_hover_text(if layer.pinned {
                                        "Unpin layer"
                                    } else {
                                        "Pin layer (prevent reordering)"
                                    });

                                    // Layer name / effect
                                    let effect_display =
                                        layer.effect_name.as_deref().unwrap_or("(empty)");
                                    let display = format!("{} {}", i + 1, effect_display);

                                    let btns_width =
                                        if num_layers > 1 { 19.0 } else { 0.0 };
                                    let label_width =
                                        (ui.available_width() - btns_width).max(20.0);

                                    let label = ui.add_sized(
                                        Vec2::new(
                                            label_width,
                                            ui.spacing().interact_size.y,
                                        ),
                                        egui::Label::new(
                                            RichText::new(&display)
                                                .size(BODY_SIZE)
                                                .color(title_color),
                                        )
                                        .selectable(false)
                                        .sense(egui::Sense::click()),
                                    );
                                    if label.clicked() {
                                        ui.ctx().data_mut(|d| {
                                            d.insert_temp(
                                                egui::Id::new("select_layer"),
                                                i,
                                            );
                                        });
                                    }
                                    if !is_active {
                                        label.on_hover_text("Click to select layer");
                                    }

                                    // Delete
                                    if num_layers > 1 {
                                        let del = close_button(
                                            ui,
                                            &format!("layer_del_{i}"),
                                            ctrl_color,
                                        );
                                        if del.clicked() {
                                            ui.ctx().data_mut(|d| {
                                                d.insert_temp(
                                                    egui::Id::new("remove_layer"),
                                                    i,
                                                );
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
                                .corner_radius(CornerRadius {
                                    nw: 0,
                                    ne: 0,
                                    sw: 4,
                                    se: 4,
                                })
                                .inner_margin(egui::Margin::symmetric(6, 4))
                                .show(ui, |ui| {
                                    ui.add_enabled_ui(!layer.locked, |ui| {
                                        ui.horizontal(|ui| {
                                            ui.label(
                                                RichText::new("Blend")
                                                    .size(SMALL_SIZE)
                                                    .color(DARK_TEXT_SECONDARY),
                                            );
                                            let current_mode = layer.blend_mode;
                                            egui::ComboBox::from_id_salt(format!(
                                                "blend_mode_{i}"
                                            ))
                                            .selected_text(
                                                RichText::new(
                                                    current_mode.display_name(),
                                                )
                                                .size(SMALL_SIZE),
                                            )
                                            .width(ui.available_width() - 4.0)
                                            .show_ui(ui, |ui| {
                                                for &mode in BlendMode::ALL {
                                                    let r = ui.selectable_label(
                                                        mode == current_mode,
                                                        RichText::new(
                                                            mode.display_name(),
                                                        )
                                                        .size(SMALL_SIZE),
                                                    );
                                                    if r.clicked() && mode != current_mode
                                                    {
                                                        ui.ctx().data_mut(|d| {
                                                            d.insert_temp(
                                                                egui::Id::new(
                                                                    "layer_blend",
                                                                ),
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
                                            let saved_bg =
                                                ui.visuals().widgets.inactive.bg_fill;
                                            ui.visuals_mut().widgets.inactive.bg_fill =
                                                METER_BG;
                                            let mut opacity = layer.opacity;
                                            let slider = ui.add(
                                                egui::Slider::new(
                                                    &mut opacity,
                                                    0.0..=1.0,
                                                )
                                                .show_value(true)
                                                .custom_formatter(|v, _| {
                                                    format!("{:.0}%", v * 100.0)
                                                })
                                                .text(""),
                                            );
                                            ui.visuals_mut().widgets.inactive.bg_fill =
                                                saved_bg;
                                            if slider.changed() {
                                                ui.ctx().data_mut(|d| {
                                                    d.insert_temp(
                                                        egui::Id::new("layer_opacity"),
                                                        opacity,
                                                    );
                                                });
                                            }
                                        });
                                    });
                                });
                        }
                    });

                card_rects.push(card_resp.response.rect);
            }

            // Bottom drop spacer — allows dropping below the last card
            if is_dragging {
                ui.allocate_exact_size(
                    Vec2::new(ui.available_width(), 20.0),
                    egui::Sense::hover(),
                );
            }
        });

    // Draw drop indicator line and handle drop
    if let Some(drag_from) = dragging_idx {
        if !layers[drag_from].pinned && !card_rects.is_empty() {
            if let Some(pos) = ctx.pointer_hover_pos() {
                let slot = find_drop_slot(pos.y, &card_rects);

                // Draw indicator: thick accent line with small circles at endpoints
                let line_y = if slot < card_rects.len() {
                    card_rects[slot].top() - 1.0
                } else {
                    card_rects.last().unwrap().bottom() + 1.0
                };
                let left = card_rects[0].left() + 2.0;
                let right = card_rects[0].right() - 2.0;
                let painter = ui.painter();

                // Line
                painter.line_segment(
                    [egui::pos2(left, line_y), egui::pos2(right, line_y)],
                    Stroke::new(2.5, DARK_ACCENT),
                );
                // Endpoint dots
                painter.circle_filled(egui::pos2(left, line_y), 3.0, DARK_ACCENT);
                painter.circle_filled(egui::pos2(right, line_y), 3.0, DARK_ACCENT);
            }
        }
    }

    // Handle drop
    if let Some(dropped) = drop_payload {
        let from = *dropped;
        if !layers[from].pinned && !card_rects.is_empty() {
            if let Some(pos) = ctx.pointer_hover_pos() {
                let slot = find_drop_slot(pos.y, &card_rects);

                // Convert slot (insertion point) to move target index
                // slot = position before which to insert
                // move_layer(from, to) removes from `from` then inserts at `to`
                let target = if slot > from {
                    // Moving down: account for removal shifting indices
                    (slot - 1).min(layers.len() - 1)
                } else {
                    slot.min(layers.len() - 1)
                };

                if from != target {
                    ctx.data_mut(|d| {
                        d.insert_temp(egui::Id::new("layer_move"), (from, target));
                    });
                }
            }
        }
    }

    // Add Layer button
    ui.add_space(4.0);
    let can_add = num_layers < max_layers;
    let add_btn = ui.add_enabled(
        can_add,
        egui::Button::new(
            RichText::new("+ Add Layer")
                .size(SMALL_SIZE)
                .color(if can_add {
                    DARK_ACCENT
                } else {
                    DARK_TEXT_SECONDARY
                }),
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
