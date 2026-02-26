use egui::{Color32, CornerRadius, RichText, Stroke, Ui, Vec2};

use crate::effect::loader::EffectLoader;
use crate::ui::theme::colors::theme_colors;
use crate::ui::theme::tokens::*;

const COLS: usize = 3;

pub fn draw_effect_panel(ui: &mut Ui, loader: &EffectLoader) {
    let tc = theme_colors(ui.ctx());

    if loader.effects.is_empty() {
        ui.label(RichText::new("No effects found").size(SMALL_SIZE).color(tc.text_secondary));
        return;
    }

    // Split effects into built-in and user
    let builtin: Vec<(usize, &crate::effect::format::PfxEffect)> = loader
        .effects
        .iter()
        .enumerate()
        .filter(|(_, e)| EffectLoader::is_builtin(e) && !e.hidden)
        .collect();
    let user: Vec<(usize, &crate::effect::format::PfxEffect)> = loader
        .effects
        .iter()
        .enumerate()
        .filter(|(_, e)| !EffectLoader::is_builtin(e) && !e.hidden)
        .collect();

    // Read pending delete state
    let now = ui.input(|i| i.time);
    let pending_delete: Option<(usize, f64)> = ui
        .ctx()
        .data_mut(|d| d.get_temp(egui::Id::new("pending_delete_effect")));
    let pending_delete = pending_delete.filter(|(_, t)| now - t < 3.0);
    let mut new_pending: Option<(usize, f64)> = pending_delete;

    let warning_color = Color32::from_rgb(200, 60, 60);
    let gap = 4.0;
    let btn_height = 22.0;

    // Built-in section
    if !builtin.is_empty() {
        egui::CollapsingHeader::new(
            RichText::new("Built-in").size(SMALL_SIZE).color(tc.text_secondary),
        )
        .default_open(true)
        .show(ui, |ui| {
            draw_effect_grid(
                ui,
                &builtin,
                loader,
                btn_height,
                gap,
                &tc,
                true, // is_builtin_section
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
    .default_open(true)
    .show(ui, |ui| {
        if user.is_empty() {
            ui.label(RichText::new("(none)").size(SMALL_SIZE).color(tc.text_secondary));
        } else {
            draw_effect_grid(
                ui,
                &user,
                loader,
                btn_height,
                gap,
                &tc,
                false, // not builtin section
                pending_delete,
                warning_color,
                &mut new_pending,
            );
        }
    });

    // Persist pending delete state
    ui.ctx().data_mut(|d| {
        if let Some(pd) = new_pending {
            d.insert_temp(egui::Id::new("pending_delete_effect"), pd);
        } else {
            d.remove_temp::<(usize, f64)>(egui::Id::new("pending_delete_effect"));
        }
    });

    // Request repaint while armed (for timeout expiry)
    if new_pending.is_some() {
        ui.ctx().request_repaint();
    }

    // Bottom buttons: Edit/Copy Shader + New
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;

        let has_effect = loader.current_effect.is_some();
        let current_is_builtin = loader
            .current_effect
            .and_then(|i| loader.effects.get(i))
            .map_or(false, |e| EffectLoader::is_builtin(e));

        if current_is_builtin {
            // "Copy Shader" for built-in effects
            if ui
                .add_enabled(
                    has_effect,
                    egui::Button::new(
                        RichText::new("Copy Shader")
                            .size(SMALL_SIZE)
                            .color(if has_effect { tc.text_primary } else { tc.text_secondary }),
                    )
                    .fill(tc.card_bg)
                    .stroke(Stroke::new(1.0, tc.card_border))
                    .corner_radius(CornerRadius::same(4)),
                )
                .on_hover_text("Copy built-in to a new editable effect")
                .clicked()
            {
                ui.ctx()
                    .data_mut(|d| d.insert_temp(egui::Id::new("copy_builtin_prompt"), true));
            }
        } else {
            // "Edit Shader" for user effects
            if ui
                .add_enabled(
                    has_effect,
                    egui::Button::new(
                        RichText::new("Edit Shader")
                            .size(SMALL_SIZE)
                            .color(if has_effect { tc.text_primary } else { tc.text_secondary }),
                    )
                    .fill(tc.card_bg)
                    .stroke(Stroke::new(1.0, tc.card_border))
                    .corner_radius(CornerRadius::same(4)),
                )
                .on_hover_text("Open shader in editor")
                .clicked()
            {
                ui.ctx()
                    .data_mut(|d| d.insert_temp(egui::Id::new("open_shader_editor"), true));
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
            .on_hover_text("Create a new effect from template")
            .clicked()
        {
            ui.ctx()
                .data_mut(|d| d.insert_temp(egui::Id::new("new_effect_prompt"), true));
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn draw_effect_grid(
    ui: &mut Ui,
    effects: &[(usize, &crate::effect::format::PfxEffect)],
    loader: &EffectLoader,
    btn_height: f32,
    gap: f32,
    tc: &crate::ui::theme::colors::ThemeColors,
    is_builtin_section: bool,
    pending_delete: Option<(usize, f64)>,
    warning_color: Color32,
    new_pending: &mut Option<(usize, f64)>,
) {
    let now = ui.input(|i| i.time);
    // Calculate button width from the inner available width (after collapsing header indent)
    let available_width = ui.available_width();
    let total_gaps = (COLS - 1) as f32 * gap;
    let btn_width = ((available_width - total_gaps) / COLS as f32).max(40.0);

    for row in effects.chunks(COLS) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for &(i, effect) in row {
                let is_current = loader.current_effect == Some(i);
                let is_armed = !is_builtin_section
                    && pending_delete.map_or(false, |(idx, _)| idx == i);

                let (fill, text_color, stroke) = if is_armed {
                    (warning_color, Color32::WHITE, Stroke::NONE)
                } else if is_current {
                    (tc.accent, Color32::WHITE, Stroke::NONE)
                } else {
                    (tc.card_bg, tc.text_primary, Stroke::new(1.0, tc.card_border))
                };

                let btn = egui::Button::new(
                    RichText::new(truncate_name(&effect.name, 10))
                        .size(SMALL_SIZE)
                        .color(text_color),
                )
                .fill(fill)
                .stroke(stroke)
                .corner_radius(CornerRadius::same(4));

                let response = ui.add_sized(Vec2::new(btn_width, btn_height), btn);

                // Left click: load effect (also clears pending delete)
                if response.clicked() && !is_current {
                    *new_pending = None;
                    ui.ctx()
                        .data_mut(|d| d.insert_temp(egui::Id::new("pending_effect"), i));
                }

                // Right click: two-stage delete for user effects only
                if !is_builtin_section && response.secondary_clicked() {
                    if is_armed {
                        // Second right-click: confirm delete
                        ui.ctx()
                            .data_mut(|d| d.insert_temp(egui::Id::new("delete_effect"), i));
                        *new_pending = None;
                    } else {
                        // First right-click: arm for delete
                        *new_pending = Some((i, now));
                    }
                }

                // Hover text
                let hover_text = if is_armed {
                    "Right-click again to DELETE".to_string()
                } else if is_builtin_section {
                    if effect.description.is_empty() {
                        effect.name.clone()
                    } else {
                        effect.description.clone()
                    }
                } else {
                    if effect.description.is_empty() {
                        format!("{} (right-click to delete)", effect.name)
                    } else {
                        format!("{} (right-click to delete)", effect.description)
                    }
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
