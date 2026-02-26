use std::sync::Arc;

use egui::{Color32, CornerRadius, Rect, RichText, Stroke, StrokeKind, Ui, Vec2};

use crate::gpu::layer::{BlendMode, LayerInfo};
use crate::ui::theme::colors::theme_colors;
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
    let cursor = if response.dragged() {
        egui::CursorIcon::Grabbing
    } else {
        egui::CursorIcon::Grab
    };
    response.on_hover_cursor(cursor)
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
    let tc = theme_colors(ui.ctx());
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
                        tc.text_secondary
                    } else {
                        tc.text_primary
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
                    let base = tc.text_secondary;
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
                    let base_color = if is_active { tc.accent } else { tc.card_border };
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
                        tc.card_bg.r(),
                        tc.card_bg.g(),
                        tc.card_bg.b(),
                        (180.0 * alpha) as u8,
                    )
                } else {
                    Color32::from_rgba_unmultiplied(
                        tc.card_bg.r(),
                        tc.card_bg.g(),
                        tc.card_bg.b(),
                        (tc.card_bg.a() as f32 * alpha) as u8,
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
                            let c = tc.accent;
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
                                                (180.0 * alpha) as u8,
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

                                    // Layer name / effect (with inline rename)
                                    let rename_idx: Option<usize> = ui.ctx().data_mut(|d| {
                                        d.get_temp(egui::Id::new("layer_rename_idx"))
                                    });
                                    let is_renaming = rename_idx == Some(i);

                                    let btns_width =
                                        if num_layers > 1 { 19.0 } else { 0.0 };
                                    let label_width =
                                        (ui.available_width() - btns_width).max(20.0);

                                    if is_renaming {
                                        let mut text = ui.ctx().data_mut(|d| {
                                            d.get_temp::<String>(egui::Id::new("layer_rename_text"))
                                                .unwrap_or_default()
                                        });
                                        let te_id = egui::Id::new(format!("layer_rename_edit_{i}"));
                                        let te = ui.add_sized(
                                            Vec2::new(label_width, ui.spacing().interact_size.y),
                                            egui::TextEdit::singleline(&mut text)
                                                .id(te_id)
                                                .font(egui::FontId::proportional(BODY_SIZE))
                                                .desired_width(label_width),
                                        );
                                        // Auto-focus on first frame
                                        if te.gained_focus() || ui.ctx().data_mut(|d| {
                                            d.get_temp::<bool>(egui::Id::new("layer_rename_focus")).unwrap_or(true)
                                        }) {
                                            te.request_focus();
                                            ui.ctx().data_mut(|d| {
                                                d.insert_temp(egui::Id::new("layer_rename_focus"), false);
                                            });
                                        }
                                        // Enter: commit
                                        if te.lost_focus() && ui.input(|inp| inp.key_pressed(egui::Key::Enter)) {
                                            let new_name = if text.trim().is_empty() { None } else { Some(text.trim().to_string()) };
                                            ui.ctx().data_mut(|d| {
                                                d.insert_temp(egui::Id::new("layer_rename"), (i, new_name));
                                                d.remove_temp::<usize>(egui::Id::new("layer_rename_idx"));
                                                d.remove_temp::<String>(egui::Id::new("layer_rename_text"));
                                                d.remove_temp::<bool>(egui::Id::new("layer_rename_focus"));
                                            });
                                        }
                                        // Escape: cancel
                                        if ui.input(|inp| inp.key_pressed(egui::Key::Escape)) {
                                            ui.ctx().data_mut(|d| {
                                                d.remove_temp::<usize>(egui::Id::new("layer_rename_idx"));
                                                d.remove_temp::<String>(egui::Id::new("layer_rename_text"));
                                                d.remove_temp::<bool>(egui::Id::new("layer_rename_focus"));
                                            });
                                        }
                                        ui.ctx().data_mut(|d| {
                                            d.insert_temp(egui::Id::new("layer_rename_text"), text);
                                        });
                                    } else {
                                        let effect_display = if layer.is_media {
                                            layer.media_file_name.as_deref().unwrap_or("media")
                                        } else {
                                            layer.effect_name.as_deref().unwrap_or("(empty)")
                                        };
                                        let display = match &layer.custom_name {
                                            Some(cn) => cn.clone(),
                                            None => format!("{} {}", i + 1, effect_display),
                                        };

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
                                            .truncate()
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
                                        // Double-click to rename (when not locked)
                                        if label.double_clicked() && !layer.locked {
                                            let initial = layer.custom_name.clone().unwrap_or_default();
                                            ui.ctx().data_mut(|d| {
                                                d.insert_temp(egui::Id::new("layer_rename_idx"), i);
                                                d.insert_temp(egui::Id::new("layer_rename_text"), initial);
                                                d.insert_temp(egui::Id::new("layer_rename_focus"), true);
                                            });
                                        }
                                        label.on_hover_text(&display);
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
                                .fill(tc.widget_bg)
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
                                                    .color(tc.text_secondary),
                                            );
                                            let help_id = egui::Id::new(format!("blend_help_{i}"));
                                            let current_mode = layer.blend_mode;
                                            let combo_width = ui.available_width() - 22.0;
                                            egui::ComboBox::from_id_salt(format!(
                                                "blend_mode_{i}"
                                            ))
                                            .selected_text(
                                                RichText::new(
                                                    current_mode.display_name(),
                                                )
                                                .size(SMALL_SIZE),
                                            )
                                            .width(combo_width)
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
                                            // Info button — painted as circled "i"
                                            let btn_size = Vec2::splat(16.0);
                                            let (rect, help_btn) = ui.allocate_exact_size(btn_size, egui::Sense::click());
                                            if ui.is_rect_visible(rect) {
                                                let center = rect.center();
                                                let radius = 7.0;
                                                let color = if help_btn.hovered() {
                                                    tc.text_primary
                                                } else {
                                                    tc.text_secondary
                                                };
                                                ui.painter().circle_stroke(center, radius, Stroke::new(1.0, color));
                                                ui.painter().text(
                                                    center,
                                                    egui::Align2::CENTER_CENTER,
                                                    "i",
                                                    egui::FontId::proportional(SMALL_SIZE),
                                                    color,
                                                );
                                            }
                                            if help_btn.clicked() {
                                                let open: bool = ui.ctx().data(|d| d.get_temp(help_id).unwrap_or(false));
                                                ui.ctx().data_mut(|d| d.insert_temp(help_id, !open));
                                            }
                                            let blend_help_open: bool = ui.ctx().data(|d| d.get_temp(help_id).unwrap_or(false));
                                            if blend_help_open {
                                                let area_resp = egui::Area::new(help_id.with("popup"))
                                                    .order(egui::Order::Foreground)
                                                    .fixed_pos(help_btn.rect.right_bottom())
                                                    .show(ui.ctx(), |ui| {
                                                        egui::Frame::popup(ui.style()).show(ui, |ui| {
                                                            for &mode in BlendMode::ALL {
                                                                ui.horizontal(|ui| {
                                                                    ui.label(
                                                                        RichText::new(mode.display_name())
                                                                            .size(SMALL_SIZE)
                                                                            .strong()
                                                                            .color(tc.text_primary),
                                                                    );
                                                                    ui.label(
                                                                        RichText::new(mode.description())
                                                                            .size(SMALL_SIZE)
                                                                            .color(tc.text_secondary),
                                                                    );
                                                                });
                                                            }
                                                        });
                                                    });
                                                // Close on click outside popup
                                                if ui.input(|i| i.pointer.any_click())
                                                    && !area_resp.response.hovered()
                                                    && !help_btn.hovered()
                                                {
                                                    ui.ctx().data_mut(|d| d.insert_temp(help_id, false));
                                                }
                                            }
                                        });

                                        ui.horizontal(|ui| {
                                            ui.label(
                                                RichText::new("Opacity")
                                                    .size(SMALL_SIZE)
                                                    .color(tc.text_secondary),
                                            );
                                            let saved_bg =
                                                ui.visuals().widgets.inactive.bg_fill;
                                            ui.visuals_mut().widgets.inactive.bg_fill =
                                                tc.meter_bg;
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
                    Stroke::new(2.5, tc.accent),
                );
                // Endpoint dots
                painter.circle_filled(egui::pos2(left, line_y), 3.0, tc.accent);
                painter.circle_filled(egui::pos2(right, line_y), 3.0, tc.accent);
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

    // Add Layer / Add Media buttons
    ui.add_space(4.0);
    let can_add = num_layers < max_layers;
    ui.horizontal(|ui| {
        let half_width = (ui.available_width() - ui.spacing().item_spacing.x) * 0.5;
        let add_btn = ui.add_enabled(
            can_add,
            egui::Button::new(
                RichText::new("+ Effect")
                    .size(SMALL_SIZE)
                    .color(if can_add {
                        tc.accent
                    } else {
                        tc.text_secondary
                    }),
            )
            .fill(Color32::TRANSPARENT)
            .stroke(Stroke::new(1.0, tc.card_border))
            .corner_radius(CornerRadius::same(4))
            .min_size(Vec2::new(half_width, MIN_INTERACT_HEIGHT)),
        );
        if add_btn.clicked() {
            ui.ctx()
                .data_mut(|d| d.insert_temp(egui::Id::new("add_layer"), true));
        }
        if can_add {
            add_btn.on_hover_text("Add an effect layer (max 8)");
        } else {
            add_btn.on_hover_text("Maximum 8 layers reached");
        }

        let media_btn = ui.add_enabled(
            can_add,
            egui::Button::new(
                RichText::new("+ Media")
                    .size(SMALL_SIZE)
                    .color(if can_add {
                        Color32::from_rgb(0x80, 0xC0, 0x80)
                    } else {
                        tc.text_secondary
                    }),
            )
            .fill(Color32::TRANSPARENT)
            .stroke(Stroke::new(1.0, tc.card_border))
            .corner_radius(CornerRadius::same(4))
            .min_size(Vec2::new(half_width, MIN_INTERACT_HEIGHT)),
        );
        if media_btn.clicked() {
            ui.ctx()
                .data_mut(|d| d.insert_temp(egui::Id::new("add_media_layer"), true));
        }
        if can_add {
            media_btn.on_hover_text("Add an image/GIF layer (max 8)");
        } else {
            media_btn.on_hover_text("Maximum 8 layers reached");
        }
    });

    // Clear All button (only when >1 layer)
    if num_layers > 1 {
        let now = ui.input(|i| i.time);
        let clear_armed: Option<f64> = ui
            .ctx()
            .data_mut(|d| d.get_temp(egui::Id::new("clear_all_armed")));
        let clear_armed = clear_armed.filter(|t| now - t < 2.0);
        let is_armed = clear_armed.is_some();

        let (label, color) = if is_armed {
            ("Confirm?", Color32::from_rgb(0xE0, 0x60, 0x40))
        } else {
            ("Clear All", tc.text_secondary)
        };

        let clear_btn = ui.add(
            egui::Button::new(RichText::new(label).size(SMALL_SIZE).color(color))
                .fill(Color32::TRANSPARENT)
                .stroke(Stroke::new(1.0, if is_armed { color } else { tc.card_border }))
                .corner_radius(CornerRadius::same(4))
                .min_size(Vec2::new(ui.available_width(), MIN_INTERACT_HEIGHT)),
        );
        if clear_btn.clicked() {
            if is_armed {
                // Confirmed — emit signal
                ui.ctx().data_mut(|d| {
                    d.insert_temp(egui::Id::new("clear_all_layers"), true);
                    d.remove_temp::<f64>(egui::Id::new("clear_all_armed"));
                });
            } else {
                // Arm
                ui.ctx().data_mut(|d| {
                    d.insert_temp(egui::Id::new("clear_all_armed"), now);
                });
            }
        }
        clear_btn.on_hover_text(if is_armed {
            "Click again to clear all layers"
        } else {
            "Remove all layers and start fresh"
        });

        // Repaint while armed (for timeout expiry)
        if is_armed {
            ui.ctx().request_repaint();
        }
    }
}
