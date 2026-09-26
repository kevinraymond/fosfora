use egui::{
    Color32, CornerRadius, Frame, Margin, RichText, Stroke, Ui, Vec2,
    collapsing_header::CollapsingState,
};

use crate::preset::PresetStore;
use crate::ui::theme::colors::theme_colors;
use crate::ui::theme::tokens::*;
use crate::ui::widgets;

/// Narrowest a preset tile gets. The grid fits as many columns as that
/// allows, two at the least, and a name wraps onto a second line rather than
/// widening its tile.
const TILE_MIN_WIDTH: f32 = 96.0;
const TILE_MAX_COLS: usize = 6;
const AMBER: Color32 = Color32::from_rgb(0xFB, 0x92, 0x3C);
const AMBER_TEXT: Color32 = Color32::from_rgb(0xFD, 0xBA, 0x74);

/// Draw the pulsing amber dot indicator for dirty state.
fn draw_pulse_dot(ui: &mut Ui, time: f64) {
    let size = 10.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    let center = rect.center();

    // Outer ring: expands and fades over 1.6s cycle
    let phase = (time % 1.6) / 1.6;
    let ring_scale = 0.8 + phase * 1.0; // 0.8 -> 1.8
    let ring_alpha = if phase < 0.7 {
        (1.0 - phase / 0.7) * 0.4
    } else {
        0.0
    };
    let ring_radius = size * 0.5 * ring_scale as f32;
    ui.painter().circle(
        center,
        ring_radius,
        Color32::from_rgba_unmultiplied(
            AMBER.r(),
            AMBER.g(),
            AMBER.b(),
            (ring_alpha * 255.0) as u8,
        ),
        Stroke::NONE,
    );

    // Inner solid dot
    ui.painter().circle(center, 3.0, AMBER, Stroke::NONE);
}

/// Top-level preset section with custom header (replaces widgets::section for presets).
pub fn draw_preset_section(ui: &mut Ui, store: &PresetStore) {
    draw_preset_section_open(ui, store, true);
}

/// [`draw_preset_section`], choosing whether it starts open. The v2 Build
/// workspace starts it closed: there the stack is what the column is for.
pub fn draw_preset_section_open(ui: &mut Ui, store: &PresetStore, default_open: bool) {
    let tc = theme_colors(ui.ctx());
    let dirty = store.dirty;
    let time = ui.input(|i| i.time);

    let badge_text = if store.presets.is_empty() {
        None
    } else {
        Some(format!("{}", store.presets.len()))
    };

    let id = ui.make_persistent_id("sec_presets");
    let state = CollapsingState::load_with_default_open(ui.ctx(), id, default_open);

    let arrow_color = if dirty { AMBER } else { tc.text_secondary };
    let title_color = if dirty { AMBER } else { tc.text_secondary };

    let mut frame = widgets::card_frame(ui);
    if dirty {
        // Pulse the card border between ambient and amber over 1.6s
        let pulse = ((time * std::f64::consts::TAU / 1.6).sin() * 0.5 + 0.5) as f32;
        let alpha = (pulse * 0.35 + 0.15) * 255.0; // 15%–50%
        frame.stroke = Stroke::new(
            1.0_f32,
            Color32::from_rgba_unmultiplied(AMBER.r(), AMBER.g(), AMBER.b(), alpha as u8),
        );
    }
    frame.show(ui, |ui| {
        let header_response = widgets::header_row(ui, MIN_INTERACT_HEIGHT, |ui| {
            widgets::draw_section_arrow(ui, state.is_open(), arrow_color);
            ui.label(
                RichText::new("PRESETS")
                    .size(HEADING_SIZE)
                    .color(title_color)
                    .strong(),
            );

            // Dirty: show *preset_name inline after title
            if dirty {
                if let Some(name) = store.current_name() {
                    let truncated = widgets::truncate_chars(name, 14);
                    ui.label(
                        RichText::new(format!("*{truncated}"))
                            .size(SMALL_SIZE)
                            .color(Color32::from_rgba_unmultiplied(
                                AMBER_TEXT.r(),
                                AMBER_TEXT.g(),
                                AMBER_TEXT.b(),
                                153, // ~60%
                            )),
                    );
                }
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some(ref badge) = badge_text {
                    ui.label(RichText::new(badge).size(SMALL_SIZE).color(tc.accent));
                }
                if dirty {
                    draw_pulse_dot(ui, time);
                }
            });
        });

        // Toggle on header click
        if header_response.clicked() {
            let mut state = CollapsingState::load_with_default_open(ui.ctx(), id, true);
            state.toggle(ui);
            state.store(ui.ctx());
        }

        // Body
        if state.is_open() {
            ui.add_space(4.0);
            draw_preset_panel(ui, store);
        }
    });
}

fn draw_preset_panel(ui: &mut Ui, store: &PresetStore) {
    let tc = theme_colors(ui.ctx());
    let time = ui.input(|i| i.time);

    // Read async loading state
    let loading_index: Option<usize> = ui.ctx().data_mut(|d| {
        d.get_temp::<crate::preset::loader::PresetLoadingState>(egui::Id::new(
            "preset_loading_state",
        ))
        .and_then(|s| match s {
            crate::preset::loader::PresetLoadingState::Loading { preset_index, .. } => {
                Some(preset_index)
            }
            _ => None,
        })
    });

    // Styled dirty bar
    if store.dirty {
        if let Some(current_idx) = store.current_preset {
            if let Some(current_name) = store.current_name() {
                let current_name = current_name.to_string();
                let is_user_preset = !store.is_builtin(current_idx);

                Frame {
                    fill: Color32::from_rgba_unmultiplied(AMBER.r(), AMBER.g(), AMBER.b(), 26), // ~10%
                    stroke: Stroke::new(
                        1.0_f32,
                        Color32::from_rgba_unmultiplied(AMBER.r(), AMBER.g(), AMBER.b(), 77), // ~30%
                    ),
                    corner_radius: CornerRadius::same(4),
                    inner_margin: Margin::symmetric(8, 4),
                    ..Default::default()
                }
                .show(ui, |ui| {
                    // Buttons first, from the right; the name is cut to what
                    // they leave rather than pushing them past the edge.
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        {
                            // Reset button (neutral style)
                            let reset_btn = ui.add(
                                egui::Button::new(
                                    RichText::new("Reset")
                                        .size(SMALL_SIZE)
                                        .color(tc.text_secondary),
                                )
                                .fill(Color32::from_rgba_unmultiplied(255, 255, 255, 13)) // ~5%
                                .stroke(Stroke::new(
                                    1.0_f32,
                                    Color32::from_rgba_unmultiplied(255, 255, 255, 31), // ~12%
                                ))
                                .corner_radius(CornerRadius::same(3)),
                            );
                            if reset_btn
                                .on_hover_text("Discard changes and reload preset")
                                .clicked()
                            {
                                ui.ctx().data_mut(|d| {
                                    d.insert_temp(egui::Id::new("pending_preset"), current_idx);
                                });
                            }

                            // Update button (amber, only for user presets)
                            if is_user_preset {
                                let update_btn = ui.add(
                                    egui::Button::new(
                                        RichText::new("Update").size(SMALL_SIZE).color(AMBER_TEXT),
                                    )
                                    .fill(Color32::from_rgba_unmultiplied(
                                        AMBER.r(),
                                        AMBER.g(),
                                        AMBER.b(),
                                        64, // ~25%
                                    ))
                                    .stroke(Stroke::new(
                                        1.0_f32,
                                        Color32::from_rgba_unmultiplied(
                                            AMBER.r(),
                                            AMBER.g(),
                                            AMBER.b(),
                                            128, // ~50%
                                        ),
                                    ))
                                    .corner_radius(CornerRadius::same(3)),
                                );
                                if update_btn
                                    .on_hover_text("Save changes to current preset")
                                    .clicked()
                                {
                                    ui.ctx().data_mut(|d| {
                                        d.insert_temp(
                                            egui::Id::new("save_preset"),
                                            current_name.clone(),
                                        );
                                    });
                                }
                            }
                        }
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            draw_pulse_dot(ui, time);
                            ui.add(
                                egui::Label::new(
                                    RichText::new(&current_name)
                                        .size(SMALL_SIZE)
                                        .color(AMBER_TEXT),
                                )
                                .truncate(),
                            );
                        });
                    });
                });

                ui.add_space(6.0);
            }
        }
    }

    // Compact save row
    ui.label(
        RichText::new("Save current state as preset:")
            .size(SMALL_SIZE)
            .color(tc.text_secondary),
    );
    let mut name = ui
        .ctx()
        .data_mut(|d| d.get_temp::<String>(egui::Id::new("preset_save_name")))
        .unwrap_or_default();

    // Right to left, the button first: the field gets whatever the button
    // leaves. A width guessed for the button overflowed once text grew, and
    // an overflowing row widens a resizable column a little every frame.
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let save_btn = ui.add_enabled(
            !name.trim().is_empty(),
            egui::Button::new(RichText::new("SAVE").size(SMALL_SIZE).strong()),
        );
        let response = ui.add(
            egui::TextEdit::singleline(&mut name)
                .desired_width(ui.available_width())
                .hint_text("Name...")
                .font(egui::FontId::proportional(SMALL_SIZE)),
        );
        if save_btn.clicked()
            || (response.lost_focus()
                && ui.input(|i| i.key_pressed(egui::Key::Enter))
                && !name.trim().is_empty())
        {
            let trimmed = name.trim().to_string();
            ui.ctx().data_mut(|d| {
                d.insert_temp(egui::Id::new("save_preset"), trimmed);
            });
            name.clear();
        }
    });

    ui.ctx()
        .data_mut(|d| d.insert_temp(egui::Id::new("preset_save_name"), name));

    if store.presets.is_empty() {
        ui.label(
            RichText::new("No presets")
                .size(SMALL_SIZE)
                .color(tc.text_secondary),
        );
        return;
    }

    ui.add_space(4.0);

    // Read pending_delete state from temp data
    let now = ui.input(|i| i.time);
    let pending_delete: Option<(usize, f64)> = ui
        .ctx()
        .data_mut(|d| d.get_temp(egui::Id::new("pending_delete_preset")));

    // Expire after 3 seconds
    let pending_delete = pending_delete.filter(|(_, t)| now - t < 3.0);

    let mut new_pending: Option<(usize, f64)> = pending_delete;

    let gap = 4.0;
    // Two lines of name.
    let btn_height = (2.0 * SMALL_SIZE * 1.3 + 8.0).round();

    // Split into built-in and user presets
    let builtin: Vec<(usize, &(String, _))> = store
        .presets
        .iter()
        .enumerate()
        .filter(|(i, _)| store.is_builtin(*i))
        .collect();
    let user: Vec<(usize, &(String, _))> = store
        .presets
        .iter()
        .enumerate()
        .filter(|(i, _)| !store.is_builtin(*i))
        .collect();

    // Built-in section
    if !builtin.is_empty() {
        egui::CollapsingHeader::new(
            RichText::new("Built-in")
                .size(SMALL_SIZE)
                .color(tc.text_secondary),
        )
        .id_salt("preset_builtin")
        .default_open(true)
        .show(ui, |ui| {
            draw_preset_grid(
                ui,
                &builtin,
                store,
                btn_height,
                gap,
                &tc,
                true,
                pending_delete,
                &mut new_pending,
                loading_index,
            );
        });
    }

    // User section
    egui::CollapsingHeader::new(
        RichText::new("User")
            .size(SMALL_SIZE)
            .color(tc.text_secondary),
    )
    .id_salt("preset_user")
    .default_open(true)
    .show(ui, |ui| {
        if user.is_empty() {
            ui.label(
                RichText::new("(none)")
                    .size(SMALL_SIZE)
                    .color(tc.text_secondary),
            );
        } else {
            draw_preset_grid(
                ui,
                &user,
                store,
                btn_height,
                gap,
                &tc,
                false,
                pending_delete,
                &mut new_pending,
                loading_index,
            );
        }
    });

    // Persist pending delete state
    ui.ctx().data_mut(|d| {
        if let Some(pd) = new_pending {
            d.insert_temp(egui::Id::new("pending_delete_preset"), pd);
        } else {
            d.remove_temp::<(usize, f64)>(egui::Id::new("pending_delete_preset"));
        }
    });

    // Request repaint while armed (for timeout expiry), loading (for pulse), or dirty (for amber animations)
    if new_pending.is_some() || loading_index.is_some() || store.dirty {
        ui.ctx().request_repaint();
    }

    // Bottom button row
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;

        let current_is_builtin = store.current_preset.map_or(false, |i| store.is_builtin(i));

        if current_is_builtin {
            if ui
                .add(
                    egui::Button::new(
                        RichText::new("Copy Preset")
                            .size(SMALL_SIZE)
                            .color(tc.text_primary),
                    )
                    .fill(tc.card_bg)
                    .stroke(Stroke::new(1.0_f32, tc.card_border))
                    .corner_radius(CornerRadius::same(4)),
                )
                .on_hover_text("Copy built-in to a new editable user preset")
                .clicked()
            {
                if let Some(idx) = store.current_preset {
                    ui.ctx().data_mut(|d| {
                        d.insert_temp(egui::Id::new("copy_preset_index"), idx);
                    });
                }
            }
        }

        let dirty_and_selected = store.current_preset.is_some() && store.dirty;
        let now = ui.input(|i| i.time);
        let new_armed: Option<f64> = ui
            .ctx()
            .data_mut(|d| d.get_temp(egui::Id::new("new_preset_armed")));
        let is_armed = new_armed.map_or(false, |t| now - t < 3.0);

        let (label, fill, stroke_color) = if dirty_and_selected && is_armed {
            (
                "Discard changes?",
                tc.warning.linear_multiply(0.25),
                tc.warning,
            )
        } else {
            ("+ New", tc.card_bg, tc.card_border)
        };

        if ui
            .add(
                egui::Button::new(RichText::new(label).size(SMALL_SIZE).color(tc.text_primary))
                    .fill(fill)
                    .stroke(Stroke::new(1.0_f32, stroke_color))
                    .corner_radius(CornerRadius::same(4)),
            )
            .on_hover_text("Clear all layers and start fresh")
            .clicked()
        {
            if dirty_and_selected && !is_armed {
                // First click: arm for confirmation
                ui.ctx().data_mut(|d| {
                    d.insert_temp(egui::Id::new("new_preset_armed"), now);
                });
            } else {
                // Not dirty, or second click: proceed
                ui.ctx().data_mut(|d| {
                    d.remove_temp::<f64>(egui::Id::new("new_preset_armed"));
                    d.insert_temp(egui::Id::new("new_preset"), true);
                    d.insert_temp(egui::Id::new("preset_save_name"), String::new());
                });
            }
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn draw_preset_grid(
    ui: &mut Ui,
    presets: &[(usize, &(String, crate::preset::Preset))],
    store: &PresetStore,
    btn_height: f32,
    gap: f32,
    tc: &crate::ui::theme::colors::ThemeColors,
    is_builtin_section: bool,
    pending_delete: Option<(usize, f64)>,
    new_pending: &mut Option<(usize, f64)>,
    loading_index: Option<usize>,
) {
    let warning_color = Color32::from_rgb(0xE0, 0x60, 0x40);
    let now = ui.input(|i| i.time);
    let available_width = ui.available_width();
    let cols = tile_cols(available_width, gap);
    let total_gaps = (cols - 1) as f32 * gap;
    // floor(): a tile a fraction of a point too wide is a row too wide.
    let btn_width = ((available_width - total_gaps) / cols as f32)
        .floor()
        .max(1.0);

    for row in presets.chunks(cols) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for &(i, (pname, _)) in row {
                let is_current = store.current_preset == Some(i);
                let is_loading = loading_index == Some(i);
                let is_armed =
                    !is_builtin_section && pending_delete.map_or(false, |(idx, _)| idx == i);
                let is_dirty_selected = is_current && store.dirty;

                let (fill, text_color, stroke) = if is_armed {
                    (warning_color, Color32::WHITE, Stroke::NONE)
                } else if is_loading {
                    // Pulsing border for loading preset
                    let pulse = ((now * 3.0).sin() * 0.5 + 0.5) as f32;
                    let border_alpha = (pulse * 200.0 + 55.0) as u8;
                    (
                        tc.card_bg,
                        tc.accent,
                        Stroke::new(
                            2.0_f32,
                            Color32::from_rgba_unmultiplied(
                                tc.accent.r(),
                                tc.accent.g(),
                                tc.accent.b(),
                                border_alpha,
                            ),
                        ),
                    )
                } else if is_dirty_selected {
                    // Amber-tinted dirty selected tile
                    (
                        Color32::from_rgba_unmultiplied(AMBER.r(), AMBER.g(), AMBER.b(), 46), // ~18%
                        AMBER_TEXT,
                        Stroke::new(
                            1.0_f32,
                            Color32::from_rgba_unmultiplied(
                                AMBER.r(),
                                AMBER.g(),
                                AMBER.b(),
                                115, // ~45%
                            ),
                        ),
                    )
                } else if is_current {
                    // Clean selected: semi-transparent accent
                    (
                        Color32::from_rgba_unmultiplied(
                            tc.accent.r(),
                            tc.accent.g(),
                            tc.accent.b(),
                            64, // ~25%
                        ),
                        Color32::from_rgba_unmultiplied(255, 255, 255, 242), // near-white
                        Stroke::new(
                            1.0_f32,
                            Color32::from_rgba_unmultiplied(
                                tc.accent.r(),
                                tc.accent.g(),
                                tc.accent.b(),
                                128, // ~50%
                            ),
                        ),
                    )
                } else {
                    (
                        tc.card_bg,
                        tc.text_primary,
                        Stroke::new(1.0_f32, tc.card_border),
                    )
                };

                let display_name = if is_dirty_selected {
                    format!("*{pname}")
                } else {
                    pname.clone()
                };
                let response = preset_tile(
                    ui,
                    &display_name,
                    Vec2::new(btn_width, btn_height),
                    fill,
                    stroke,
                    text_color,
                    is_current,
                );

                // Left click: load/reload preset (also clears pending delete)
                if response.clicked() {
                    *new_pending = None;
                    ui.ctx()
                        .data_mut(|d| d.insert_temp(egui::Id::new("pending_preset"), i));
                }

                // Right click: two-stage delete for user presets only
                if !is_builtin_section && response.secondary_clicked() {
                    if is_armed {
                        // Second right-click: confirm delete
                        ui.ctx()
                            .data_mut(|d| d.insert_temp(egui::Id::new("delete_preset"), i));
                        *new_pending = None;
                    } else if is_current && !store.dirty {
                        // Right-click current (clean) preset: deselect
                        ui.ctx().data_mut(|d| {
                            d.insert_temp(egui::Id::new("deselect_preset"), true);
                        });
                        *new_pending = None;
                    } else {
                        // First right-click: arm for delete
                        *new_pending = Some((i, now));
                    }
                }

                // Right-click on built-in: deselect if current + clean
                if is_builtin_section && response.secondary_clicked() {
                    if is_current && !store.dirty {
                        ui.ctx().data_mut(|d| {
                            d.insert_temp(egui::Id::new("deselect_preset"), true);
                        });
                    }
                }

                let hover_text = if is_armed {
                    "Right-click again to DELETE".to_string()
                } else if is_builtin_section {
                    pname.clone()
                } else if is_current && store.dirty {
                    format!("{pname} — click to reload, right-click to delete")
                } else if is_current {
                    format!("{pname} — click to reload, right-click to deselect")
                } else {
                    format!("{pname} (right-click to delete)")
                };
                response.on_hover_text(hover_text);
            }
        });
    }
}

/// How many tiles fit across `width`.
fn tile_cols(width: f32, gap: f32) -> usize {
    (((width + gap) / (TILE_MIN_WIDTH + gap)).floor() as usize).clamp(2, TILE_MAX_COLS)
}

/// One preset as a tile of exactly `size`: the name wraps onto a second line
/// and ends in an ellipsis past that, so a long name never widens the grid.
fn preset_tile(
    ui: &mut Ui,
    name: &str,
    size: Vec2,
    fill: Color32,
    stroke: Stroke,
    text_color: Color32,
    selected: bool,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    response
        .widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, selected, name));
    let visuals = ui.style().interact(&response);
    let stroke = if response.hovered() {
        visuals.bg_stroke
    } else {
        stroke
    };
    ui.painter().rect(
        rect,
        CornerRadius::same(4),
        fill,
        stroke,
        egui::StrokeKind::Inside,
    );
    let mut job = egui::text::LayoutJob::simple(
        name.to_string(),
        egui::FontId::proportional(SMALL_SIZE),
        text_color,
        rect.width() - 10.0,
    );
    job.wrap.max_rows = 2;
    job.halign = egui::Align::Center;
    let galley = ui.painter().layout_job(job);
    let pos = egui::pos2(rect.center().x, rect.center().y - galley.size().y / 2.0);
    ui.painter().galley(pos, galley, text_color);
    crate::ui::accessibility::focus::draw_focus_ring(ui, &response);
    response
}

#[cfg(test)]
mod layout_tests {
    use super::*;

    fn store(names: &[&str]) -> PresetStore {
        let preset: crate::preset::Preset =
            serde_json::from_str(r#"{"layers":[]}"#).expect("a minimal preset parses");
        PresetStore {
            presets: names
                .iter()
                .map(|n| (n.to_string(), preset.clone()))
                .collect(),
            current_preset: Some(0),
            dirty: false,
            builtin_count: 2,
        }
    }

    /// Width of a resizable side panel holding the open preset section,
    /// after `frames` frames.
    fn panel_width(store: &PresetStore, frames: usize) -> f32 {
        let ctx = egui::Context::default();
        let mut w = 0.0;
        for _ in 0..frames {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1400.0, 900.0),
                )),
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                w = egui::SidePanel::left("p")
                    .default_width(300.0)
                    .width_range(280.0..=800.0)
                    .resizable(true)
                    .show(ctx, |ui| {
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            draw_preset_section_open(ui, store, true);
                        });
                    })
                    .response
                    .rect
                    .width();
            });
        }
        w
    }

    #[test]
    fn opening_presets_does_not_widen_its_column() {
        let s = store(&[
            "Aurora Borealis Deluxe Edition",
            "Murmur Mirror",
            "Fluvid over camera with Color Key",
            "Tide",
            "Pegboard",
        ]);
        let w = panel_width(&s, 40);
        assert!(w <= 301.0, "the column grew to {w}");
    }

    #[test]
    fn unsaved_changes_to_a_long_named_preset_do_not_widen_its_column() {
        let mut s = store(&[
            "Tide",
            "Murmur",
            "A user preset with a name far longer than the column is wide",
        ]);
        s.current_preset = Some(2);
        s.dirty = true;
        let w = panel_width(&s, 40);
        assert!(w <= 301.0, "the column grew to {w}");
    }

    #[test]
    fn narrow_columns_keep_two_tiles_and_wide_ones_fit_more() {
        assert_eq!(tile_cols(270.0, 4.0), 2);
        assert_eq!(tile_cols(420.0, 4.0), 4);
        assert_eq!(tile_cols(2000.0, 4.0), TILE_MAX_COLS);
    }
}
