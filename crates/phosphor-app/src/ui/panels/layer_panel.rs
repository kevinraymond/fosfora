use std::sync::Arc;

use egui::{Color32, CornerRadius, Rect, RichText, Stroke, StrokeKind, Ui, Vec2};

use crate::gpu::layer::{BlendMode, LayerInfo};
use crate::ui::theme::colors::theme_colors;
use crate::ui::theme::tokens::*;

// Layer type colors — matching effect panel palette
const TYPE_COLOR_EFFECT: Color32 = Color32::from_rgb(0x77, 0x66, 0xEE); // purple (same as shader)
const TYPE_COLOR_MEDIA: Color32 = Color32::from_rgb(0xFF, 0x88, 0x33); // orange
const TYPE_COLOR_WEBCAM: Color32 = Color32::from_rgb(0x33, 0xCC, 0xAA); // teal

fn layer_type_color(layer: &LayerInfo) -> Color32 {
    if layer.media_is_live {
        TYPE_COLOR_WEBCAM
    } else if layer.is_media {
        TYPE_COLOR_MEDIA
    } else {
        TYPE_COLOR_EFFECT
    }
}

fn layer_type_label(layer: &LayerInfo) -> &'static str {
    if layer.media_is_live {
        "WC"
    } else if layer.is_media {
        "MD"
    } else {
        "FX"
    }
}

// --- Custom icon buttons (painted as vector shapes, no font dependency) ---

fn icon_button(
    ui: &mut Ui,
    _id: &str,
    color: Color32,
    paint: impl FnOnce(&egui::Painter, Rect, Color32),
) -> egui::Response {
    let size = Vec2::splat(16.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let c = if response.hovered() {
        Color32::WHITE
    } else {
        color
    };
    paint(ui.painter(), rect, c);
    response
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
        painter.line_segment([egui::pos2(cx, cy + 1.5), egui::pos2(cx, cy + 5.5)], stroke);
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

fn draw_layer_type_legend(ui: &mut Ui, tc: &crate::ui::theme::colors::ThemeColors) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        for (color, label, tooltip) in [
            (TYPE_COLOR_EFFECT, "FX", "Effect (shader) layer"),
            (TYPE_COLOR_MEDIA, "MD", "Media (image/GIF/video) layer"),
            (TYPE_COLOR_WEBCAM, "WC", "Webcam (live camera) layer"),
        ] {
            let resp = ui
                .horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = ui.spacing().item_spacing.x;
                    let (rect, _) =
                        ui.allocate_exact_size(Vec2::new(3.0, 10.0), egui::Sense::hover());
                    ui.painter().rect_filled(rect, 1.0, color);
                    ui.label(
                        RichText::new(label)
                            .size(8.0)
                            .color(tc.text_secondary),
                    );
                })
                .response;
            resp.on_hover_text(tooltip);
        }
    });
    ui.add_space(4.0);
    ui.separator();
    ui.add_space(2.0);
}

/// Apply alpha to a color.
fn with_alpha(c: Color32, a: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), (c.a() as f32 * a) as u8)
}

/// Draw the layer management panel.
pub fn draw_layer_panel(ui: &mut Ui, layers: &[LayerInfo], active_layer: usize) {
    let tc = theme_colors(ui.ctx());
    let max_layers = 8;
    let num_layers = layers.len();
    let ctx = ui.ctx().clone();

    // Type legend
    draw_layer_type_legend(ui, &tc);

    // Check if a drag is in progress
    let dragging_idx: Option<usize> =
        egui::DragAndDrop::payload::<usize>(&ctx).map(|p: Arc<usize>| *p);
    let is_dragging = dragging_idx.is_some();

    // Collect card rects during rendering
    let mut card_rects: Vec<Rect> = Vec::with_capacity(num_layers);

    let (_drop_inner, drop_payload) = ui.dnd_drop_zone::<usize, ()>(egui::Frame::new(), |ui| {
        for (i, layer) in layers.iter().enumerate() {
            let is_active = i == active_layer;
            let is_being_dragged = dragging_idx == Some(i);

            // Fade non-dragged layers while dragging
            let alpha = if is_dragging && !is_being_dragged {
                0.4_f32
            } else {
                1.0
            };

            // Title/pin color: dimmed more aggressively for disabled layers
            let title_color = {
                let base = if is_active {
                    Color32::WHITE
                } else if !layer.enabled {
                    with_alpha(tc.text_secondary, 0.25)
                } else {
                    tc.text_primary
                };
                if alpha < 1.0 {
                    with_alpha(base, alpha)
                } else {
                    base
                }
            };

            // Controls color: dimmed when locked
            let ctrl_color = if layer.locked {
                let base = tc.text_secondary;
                if alpha < 1.0 {
                    with_alpha(base, alpha)
                } else {
                    base
                }
            } else {
                title_color
            };

            let outer_stroke = {
                let base_color = if is_active { tc.accent } else { tc.card_border };
                if alpha < 1.0 {
                    Stroke::new(1.0, with_alpha(base_color, alpha))
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

            let type_color = layer_type_color(layer);
            let mut header_center_y = 0.0_f32;

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
                        with_alpha(c, alpha)
                    } else {
                        card_fill
                    };
                    let _header_resp = egui::Frame::new()
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
                                        Color32::from_white_alpha((180.0 * alpha) as u8)
                                    } else {
                                        ctrl_color
                                    };
                                    let handle = drag_handle(ui, handle_color);
                                    if handle.drag_started() {
                                        egui::DragAndDrop::set_payload(ui.ctx(), i);
                                    }
                                }

                                // Layer index number
                                ui.label(
                                    RichText::new(format!("{}", i + 1))
                                        .size(SMALL_SIZE)
                                        .color(tc.text_secondary),
                                );

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
                                let rename_idx: Option<usize> = ui
                                    .ctx()
                                    .data_mut(|d| d.get_temp(egui::Id::new("layer_rename_idx")));
                                let is_renaming = rename_idx == Some(i);

                                // Reserve space for type badge + delete button on right
                                let right_btns_width = if num_layers > 1 { 46.0 } else { 28.0 };
                                let label_width = (ui.available_width() - right_btns_width).max(20.0);

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
                                    if te.gained_focus()
                                        || ui.ctx().data_mut(|d| {
                                            d.get_temp::<bool>(egui::Id::new("layer_rename_focus"))
                                                .unwrap_or(true)
                                        })
                                    {
                                        te.request_focus();
                                        ui.ctx().data_mut(|d| {
                                            d.insert_temp(
                                                egui::Id::new("layer_rename_focus"),
                                                false,
                                            );
                                        });
                                    }
                                    // Enter: commit
                                    if te.lost_focus()
                                        && ui.input(|inp| inp.key_pressed(egui::Key::Enter))
                                    {
                                        let new_name = if text.trim().is_empty() {
                                            None
                                        } else {
                                            Some(text.trim().to_string())
                                        };
                                        ui.ctx().data_mut(|d| {
                                            d.insert_temp(
                                                egui::Id::new("layer_rename"),
                                                (i, new_name),
                                            );
                                            d.remove_temp::<usize>(egui::Id::new(
                                                "layer_rename_idx",
                                            ));
                                            d.remove_temp::<String>(egui::Id::new(
                                                "layer_rename_text",
                                            ));
                                            d.remove_temp::<bool>(egui::Id::new(
                                                "layer_rename_focus",
                                            ));
                                        });
                                    }
                                    // Escape: cancel
                                    if ui.input(|inp| inp.key_pressed(egui::Key::Escape)) {
                                        ui.ctx().data_mut(|d| {
                                            d.remove_temp::<usize>(egui::Id::new(
                                                "layer_rename_idx",
                                            ));
                                            d.remove_temp::<String>(egui::Id::new(
                                                "layer_rename_text",
                                            ));
                                            d.remove_temp::<bool>(egui::Id::new(
                                                "layer_rename_focus",
                                            ));
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
                                        Vec2::new(label_width, ui.spacing().interact_size.y),
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
                                            d.insert_temp(egui::Id::new("select_layer"), i);
                                        });
                                    }
                                    // Double-click to rename (when not locked)
                                    if label.double_clicked() && !layer.locked {
                                        let initial = layer.custom_name.clone().unwrap_or_default();
                                        ui.ctx().data_mut(|d| {
                                            d.insert_temp(egui::Id::new("layer_rename_idx"), i);
                                            d.insert_temp(
                                                egui::Id::new("layer_rename_text"),
                                                initial,
                                            );
                                            d.insert_temp(
                                                egui::Id::new("layer_rename_focus"),
                                                true,
                                            );
                                        });
                                    }
                                    label.on_hover_text(&display);
                                }

                                // Type badge (FX/MD/WC)
                                let badge_alpha = if layer.enabled { 1.0 } else { 0.5 };
                                ui.label(
                                    RichText::new(layer_type_label(layer))
                                        .size(SMALL_SIZE)
                                        .color(with_alpha(type_color, badge_alpha)),
                                );

                            });
                        });
                    header_center_y = _header_resp.response.rect.center().y;

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
                                        egui::ComboBox::from_id_salt(format!("blend_mode_{i}"))
                                            .selected_text(
                                                RichText::new(current_mode.display_name())
                                                    .size(SMALL_SIZE),
                                            )
                                            .width(combo_width)
                                            .show_ui(ui, |ui| {
                                                for &mode in BlendMode::ALL {
                                                    let r = ui.selectable_label(
                                                        mode == current_mode,
                                                        RichText::new(mode.display_name())
                                                            .size(SMALL_SIZE),
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
                                        // Info button — painted as circled "i"
                                        let btn_size = Vec2::splat(16.0);
                                        let (rect, help_btn) =
                                            ui.allocate_exact_size(btn_size, egui::Sense::click());
                                        if ui.is_rect_visible(rect) {
                                            let center = rect.center();
                                            let radius = 7.0;
                                            let color = if help_btn.hovered() {
                                                tc.text_primary
                                            } else {
                                                tc.text_secondary
                                            };
                                            ui.painter().circle_stroke(
                                                center,
                                                radius,
                                                Stroke::new(1.0, color),
                                            );
                                            ui.painter().text(
                                                center,
                                                egui::Align2::CENTER_CENTER,
                                                "i",
                                                egui::FontId::proportional(SMALL_SIZE),
                                                color,
                                            );
                                        }
                                        if help_btn.clicked() {
                                            let open: bool = ui
                                                .ctx()
                                                .data(|d| d.get_temp(help_id).unwrap_or(false));
                                            ui.ctx().data_mut(|d| d.insert_temp(help_id, !open));
                                        }
                                        let blend_help_open: bool =
                                            ui.ctx().data(|d| d.get_temp(help_id).unwrap_or(false));
                                        if blend_help_open {
                                            let area_resp = egui::Area::new(help_id.with("popup"))
                                                .order(egui::Order::Foreground)
                                                .fixed_pos(help_btn.rect.right_bottom())
                                                .show(ui.ctx(), |ui| {
                                                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                                                        for &mode in BlendMode::ALL {
                                                            ui.horizontal(|ui| {
                                                                ui.label(
                                                                    RichText::new(
                                                                        mode.display_name(),
                                                                    )
                                                                    .size(SMALL_SIZE)
                                                                    .strong()
                                                                    .color(tc.text_primary),
                                                                );
                                                                ui.label(
                                                                    RichText::new(
                                                                        mode.description(),
                                                                    )
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
                                                ui.ctx()
                                                    .data_mut(|d| d.insert_temp(help_id, false));
                                            }
                                        }
                                    });

                                    ui.horizontal(|ui| {
                                        ui.label(
                                            RichText::new("Opacity")
                                                .size(SMALL_SIZE)
                                                .color(tc.text_secondary),
                                        );
                                        let saved_bg = ui.visuals().widgets.inactive.bg_fill;
                                        ui.visuals_mut().widgets.inactive.bg_fill = tc.meter_bg;
                                        let mut opacity = layer.opacity;
                                        let slider = ui.add(
                                            egui::Slider::new(&mut opacity, 0.0..=1.0)
                                                .show_value(true)
                                                .custom_formatter(|v, _| {
                                                    format!("{:.0}%", v * 100.0)
                                                })
                                                .text(""),
                                        );
                                        ui.visuals_mut().widgets.inactive.bg_fill = saved_bg;
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

            let card_rect = card_resp.response.rect;
            let card_hovered = card_resp.response.hovered()
                || ui.rect_contains_pointer(card_rect);

            // Left type color strip (3px)
            let strip_alpha = if layer.enabled { 1.0 } else { 0.5 };
            let strip_sw = if is_active && num_layers > 1 { 0 } else { 4 };
            let strip_rect =
                Rect::from_min_size(card_rect.left_top(), Vec2::new(3.0, card_rect.height()));
            ui.painter().rect_filled(
                strip_rect,
                CornerRadius {
                    nw: 4,
                    sw: strip_sw,
                    ne: 0,
                    se: 0,
                },
                with_alpha(type_color, strip_alpha * alpha),
            );

            // Delete button overlay (only on hover or active)
            if num_layers > 1 && (card_hovered || is_active) {
                let del_size = Vec2::splat(16.0);
                let del_pos = egui::pos2(
                    card_rect.right() - del_size.x - 4.0,
                    header_center_y - del_size.x * 0.5,
                );
                let del_rect = Rect::from_min_size(del_pos, del_size);
                let del_id = egui::Id::new(format!("layer_del_{i}"));
                let del_resp = ui.interact(del_rect, del_id, egui::Sense::click());

                // Paint X
                let center = del_rect.center();
                let s = 3.5;
                let del_color = if del_resp.hovered() {
                    Color32::WHITE
                } else {
                    ctrl_color
                };
                let stroke = Stroke::new(1.5, del_color);
                ui.painter().line_segment(
                    [
                        egui::pos2(center.x - s, center.y - s),
                        egui::pos2(center.x + s, center.y + s),
                    ],
                    stroke,
                );
                ui.painter().line_segment(
                    [
                        egui::pos2(center.x + s, center.y - s),
                        egui::pos2(center.x - s, center.y + s),
                    ],
                    stroke,
                );

                if del_resp.clicked() {
                    ui.ctx().data_mut(|d| {
                        d.insert_temp(egui::Id::new("remove_layer"), i);
                    });
                }
                del_resp.on_hover_text("Delete layer");
            }

            card_rects.push(card_rect);
        }

        // Bottom drop spacer — allows dropping below the last card
        if is_dragging {
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 20.0), egui::Sense::hover());
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
                let target = if slot > from {
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

    // Type-colored button helper
    let type_btn = |ui: &mut Ui, label: &str, type_color: Color32, can_add: bool, width: f32| -> egui::Response {
        let (fill, stroke_color, text_color) = if can_add {
            (
                Color32::from_rgba_unmultiplied(type_color.r(), type_color.g(), type_color.b(), 18), // ~7%
                Color32::from_rgba_unmultiplied(type_color.r(), type_color.g(), type_color.b(), 54), // ~21%
                type_color,
            )
        } else {
            (
                Color32::from_rgba_unmultiplied(255, 255, 255, 5), // ~2%
                tc.card_border,
                tc.text_secondary,
            )
        };
        ui.add_enabled(
            can_add,
            egui::Button::new(RichText::new(label).size(SMALL_SIZE).color(text_color))
                .fill(fill)
                .stroke(Stroke::new(1.0, stroke_color))
                .corner_radius(CornerRadius::same(4))
                .min_size(Vec2::new(width, MIN_INTERACT_HEIGHT)),
        )
    };

    ui.horizontal(|ui| {
        #[cfg(feature = "webcam")]
        let btn_count = 3.0_f32;
        #[cfg(not(feature = "webcam"))]
        let btn_count = 2.0_f32;
        let spacing = ui.spacing().item_spacing.x;
        let btn_width = ((ui.available_width() - spacing * (btn_count - 1.0)) / btn_count).max(30.0);

        let add_btn = type_btn(ui, "+ Effect", TYPE_COLOR_EFFECT, can_add, btn_width);
        if add_btn.clicked() {
            ui.ctx()
                .data_mut(|d| d.insert_temp(egui::Id::new("add_layer"), true));
        }
        if can_add {
            add_btn.on_hover_text("Add an effect layer (max 8)");
        } else {
            add_btn.on_hover_text("Maximum 8 layers reached");
        }

        let media_btn = type_btn(ui, "+ Media", TYPE_COLOR_MEDIA, can_add, btn_width);
        if media_btn.clicked() {
            ui.ctx()
                .data_mut(|d| d.insert_temp(egui::Id::new("add_media_layer"), true));
        }
        if can_add {
            media_btn.on_hover_text("Add an image/GIF layer (max 8)");
        } else {
            media_btn.on_hover_text("Maximum 8 layers reached");
        }

        #[cfg(feature = "webcam")]
        {
            let webcam_btn = type_btn(ui, "+ Webcam", TYPE_COLOR_WEBCAM, can_add, btn_width);
            if webcam_btn.clicked() {
                // Use stored default webcam device index, fallback to 0
                let device_idx: u32 = ui
                    .ctx()
                    .data(|d| d.get_temp(egui::Id::new("webcam_default_device")))
                    .unwrap_or(0);
                ui.ctx()
                    .data_mut(|d| d.insert_temp(egui::Id::new("add_webcam_layer"), device_idx));
            }
            if can_add {
                webcam_btn.on_hover_text("Add a live webcam layer (max 8)");
            } else {
                webcam_btn.on_hover_text("Maximum 8 layers reached");
            }
        }
    });

    // Clear All — subtle text link with 2-second armed confirmation
    if num_layers > 1 {
        ui.add_space(2.0);
        let now = ui.input(|i| i.time);
        let clear_armed: Option<f64> = ui
            .ctx()
            .data_mut(|d| d.get_temp(egui::Id::new("clear_all_armed")));
        let clear_armed = clear_armed.filter(|t| now - t < 2.0);
        let is_armed = clear_armed.is_some();

        let red = Color32::from_rgb(0xEF, 0x44, 0x44);
        let (label_text, base_color) = if is_armed {
            ("Confirm?", red)
        } else {
            ("Clear All", with_alpha(tc.text_secondary, 0.6))
        };

        ui.horizontal(|ui| {
            let avail = ui.available_width();
            // Center the label
            let text_width = 60.0; // approximate
            ui.add_space((avail - text_width) * 0.5);

            let resp = ui.allocate_response(
                Vec2::new(text_width, MIN_INTERACT_HEIGHT),
                egui::Sense::click(),
            );
            let color = if resp.hovered() && !is_armed {
                with_alpha(red, 0.6)
            } else {
                base_color
            };
            ui.painter().text(
                resp.rect.center(),
                egui::Align2::CENTER_CENTER,
                label_text,
                egui::FontId::proportional(SMALL_SIZE),
                color,
            );

            if resp.clicked() {
                if is_armed {
                    ui.ctx().data_mut(|d| {
                        d.insert_temp(egui::Id::new("clear_all_layers"), true);
                        d.remove_temp::<f64>(egui::Id::new("clear_all_armed"));
                    });
                } else {
                    ui.ctx().data_mut(|d| {
                        d.insert_temp(egui::Id::new("clear_all_armed"), now);
                    });
                }
            }
            resp.on_hover_text(if is_armed {
                "Click again to clear all layers"
            } else {
                "Remove all layers and start fresh"
            });
        });

        // Repaint while armed (for timeout expiry)
        if is_armed {
            ui.ctx().request_repaint();
        }
    }

    // Footer: type breakdown counts
    let fx_count = layers
        .iter()
        .filter(|l| !l.is_media && !l.media_is_live)
        .count();
    let media_count = layers
        .iter()
        .filter(|l| l.is_media && !l.media_is_live)
        .count();
    let webcam_count = layers.iter().filter(|l| l.media_is_live).count();

    ui.add_space(4.0);
    ui.separator();
    ui.label(
        RichText::new(format!(
            "{fx_count} effect · {media_count} media · {webcam_count} webcam"
        ))
        .size(7.0)
        .color(tc.text_secondary),
    );
}
