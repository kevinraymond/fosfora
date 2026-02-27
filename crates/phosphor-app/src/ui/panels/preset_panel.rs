use egui::{Color32, CornerRadius, RichText, Stroke, Ui, Vec2};

use crate::preset::PresetStore;
use crate::ui::theme::colors::theme_colors;
use crate::ui::theme::tokens::*;

const COLS: usize = 2;

pub fn draw_preset_panel(ui: &mut Ui, store: &PresetStore) {
    let tc = theme_colors(ui.ctx());
    let warning_color = Color32::from_rgb(0xE0, 0x60, 0x40);

    // Compact save row
    let mut name = ui
        .ctx()
        .data_mut(|d| d.get_temp::<String>(egui::Id::new("preset_save_name")))
        .unwrap_or_default();

    // "Update" button when dirty and a user preset is loaded
    if store.dirty {
        if let Some(current_idx) = store.current_preset {
            if !store.is_builtin(current_idx) {
                if let Some(current_name) = store.current_name() {
                    let current_name = current_name.to_string();
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!("*{}", current_name))
                                .size(SMALL_SIZE)
                                .color(warning_color),
                        );
                        if ui
                            .button(RichText::new("Update").size(SMALL_SIZE).strong())
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
                        if ui
                            .button(RichText::new("Reset").size(SMALL_SIZE))
                            .on_hover_text("Discard changes and reload preset")
                            .clicked()
                        {
                            ui.ctx().data_mut(|d| {
                                d.insert_temp(
                                    egui::Id::new("pending_preset"),
                                    current_idx,
                                );
                            });
                        }
                    });
                    ui.add_space(2.0);
                }
            }
        }
    }

    ui.horizontal(|ui| {
        let save_width = 40.0;
        let spacing = ui.spacing().item_spacing.x;
        let text_width = (ui.available_width() - save_width - spacing).max(1.0);
        let response = ui.add(
            egui::TextEdit::singleline(&mut name)
                .desired_width(text_width)
                .hint_text("Name...")
                .font(egui::FontId::proportional(SMALL_SIZE)),
        );
        let save_btn = ui.add_enabled(
            !name.trim().is_empty(),
            egui::Button::new(RichText::new("SAVE").size(SMALL_SIZE).strong()),
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
        ui.label(RichText::new("No presets").size(SMALL_SIZE).color(tc.text_secondary));
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
    let btn_height = 22.0;

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
            RichText::new("Built-in").size(SMALL_SIZE).color(tc.text_secondary),
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
                warning_color,
                &mut new_pending,
            );
        });
    }

    // User section
    egui::CollapsingHeader::new(
        RichText::new("User").size(SMALL_SIZE).color(tc.text_secondary),
    )
    .id_salt("preset_user")
    .default_open(true)
    .show(ui, |ui| {
        if user.is_empty() {
            ui.label(RichText::new("(none)").size(SMALL_SIZE).color(tc.text_secondary));
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
                warning_color,
                &mut new_pending,
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

    // Request repaint while armed (for timeout expiry)
    if new_pending.is_some() {
        ui.ctx().request_repaint();
    }

    // Bottom button row
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;

        let current_is_builtin = store
            .current_preset
            .map_or(false, |i| store.is_builtin(i));

        if current_is_builtin {
            if ui
                .add(
                    egui::Button::new(
                        RichText::new("Copy Preset")
                            .size(SMALL_SIZE)
                            .color(tc.text_primary),
                    )
                    .fill(tc.card_bg)
                    .stroke(Stroke::new(1.0, tc.card_border))
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

        if ui
            .add(
                egui::Button::new(
                    RichText::new("+ New")
                        .size(SMALL_SIZE)
                        .color(tc.text_primary),
                )
                .fill(tc.card_bg)
                .stroke(Stroke::new(1.0, tc.card_border))
                .corner_radius(CornerRadius::same(4)),
            )
            .on_hover_text("Deselect current preset to save as new")
            .clicked()
        {
            ui.ctx().data_mut(|d| {
                d.insert_temp(egui::Id::new("deselect_preset"), true);
            });
            // Focus the name input by clearing it
            ui.ctx().data_mut(|d| {
                d.insert_temp(egui::Id::new("preset_save_name"), String::new());
            });
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
    warning_color: Color32,
    new_pending: &mut Option<(usize, f64)>,
) {
    let now = ui.input(|i| i.time);
    let available_width = ui.available_width();
    let total_gaps = (COLS - 1) as f32 * gap;
    let btn_width = ((available_width - total_gaps) / COLS as f32).max(40.0);

    for row in presets.chunks(COLS) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for &(i, (pname, _)) in row {
                let is_current = store.current_preset == Some(i);
                let is_armed = !is_builtin_section
                    && pending_delete.map_or(false, |(idx, _)| idx == i);

                let (fill, text_color, stroke) = if is_armed {
                    (warning_color, Color32::WHITE, Stroke::NONE)
                } else if is_current {
                    (tc.accent, Color32::WHITE, Stroke::NONE)
                } else {
                    (tc.card_bg, tc.text_primary, Stroke::new(1.0, tc.card_border))
                };

                let display_name = if is_current && store.dirty {
                    format!("*{}", truncate_name(pname, 17))
                } else {
                    truncate_name(pname, 18)
                };
                let btn = egui::Button::new(
                    RichText::new(&display_name).size(SMALL_SIZE).color(text_color),
                )
                .fill(fill)
                .stroke(stroke)
                .corner_radius(CornerRadius::same(4));

                let response = ui.add_sized(Vec2::new(btn_width, btn_height), btn);

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
                        ui.ctx()
                            .data_mut(|d| {
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
                        ui.ctx()
                            .data_mut(|d| {
                                d.insert_temp(egui::Id::new("deselect_preset"), true);
                            });
                    }
                }

                let hover_text = if is_armed {
                    "Right-click again to DELETE".to_string()
                } else if is_builtin_section {
                    pname.to_string()
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

fn truncate_name(name: &str, max_len: usize) -> String {
    if name.len() <= max_len {
        name.to_string()
    } else {
        format!("{}\u{2026}", &name[..max_len - 1])
    }
}
