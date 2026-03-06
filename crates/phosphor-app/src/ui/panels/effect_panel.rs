use egui::{Color32, CornerRadius, Rect, RichText, Stroke, Ui, Vec2};

use crate::effect::format::{EffectType, PfxEffect};
use crate::effect::loader::EffectLoader;
use crate::ui::theme::colors::theme_colors;
use crate::ui::theme::tokens::*;

const COLS: usize = 3;

// Type colors — matched to audio panel frequency band hues
const TYPE_COLOR_SHADER: Color32 = Color32::from_rgb(0x77, 0x66, 0xEE); // purple
const TYPE_COLOR_PARTICLE: Color32 = Color32::from_rgb(0xFF, 0x88, 0x33); // orange
const TYPE_COLOR_FEEDBACK: Color32 = Color32::from_rgb(0x33, 0xCC, 0xAA); // teal

fn type_color(et: EffectType) -> Color32 {
    match et {
        EffectType::Shader => TYPE_COLOR_SHADER,
        EffectType::Particle => TYPE_COLOR_PARTICLE,
        EffectType::Feedback => TYPE_COLOR_FEEDBACK,
    }
}

fn type_label(et: EffectType) -> &'static str {
    match et {
        EffectType::Shader => "SH",
        EffectType::Particle => "PS",
        EffectType::Feedback => "FB",
    }
}

fn type_title(et: EffectType) -> &'static str {
    match et {
        EffectType::Shader => "Shader",
        EffectType::Particle => "Particle + Shader",
        EffectType::Feedback => "Feedback",
    }
}

pub fn draw_effect_panel(ui: &mut Ui, loader: &EffectLoader) {
    let tc = theme_colors(ui.ctx());

    if loader.effects.is_empty() {
        ui.label(
            RichText::new("No effects found")
                .size(SMALL_SIZE)
                .color(tc.text_secondary),
        );
        return;
    }

    // Type legend
    draw_type_legend(ui, &tc);

    // Split effects into built-in and user
    let builtin: Vec<(usize, &PfxEffect)> = loader
        .effects
        .iter()
        .enumerate()
        .filter(|(_, e)| EffectLoader::is_builtin(e) && !e.hidden)
        .collect();
    let user: Vec<(usize, &PfxEffect)> = loader
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
            RichText::new("Built-in")
                .size(SMALL_SIZE)
                .color(tc.text_secondary),
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
                true,
                pending_delete,
                warning_color,
                &mut new_pending,
            );
        });
    }

    // User section
    egui::CollapsingHeader::new(
        RichText::new("User")
            .size(SMALL_SIZE)
            .color(tc.text_secondary),
    )
    .default_open(true)
    .show(ui, |ui| {
        if user.is_empty() {
            ui.label(
                RichText::new("(none)")
                    .size(SMALL_SIZE)
                    .color(tc.text_secondary),
            );
        } else {
            draw_effect_grid(
                ui,
                &user,
                loader,
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
            d.insert_temp(egui::Id::new("pending_delete_effect"), pd);
        } else {
            d.remove_temp::<(usize, f64)>(egui::Id::new("pending_delete_effect"));
        }
    });

    // Request repaint while armed (for timeout expiry)
    if new_pending.is_some() {
        ui.ctx().request_repaint();
    }

    // Bottom buttons: Copy + Edit + New
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;

        let current = loader
            .current_effect
            .and_then(|i| loader.effects.get(i));
        let can_copy = current.map_or(false, |e| !e.hidden);
        let is_user = current.map_or(false, |e| !EffectLoader::is_builtin(e));

        // Copy Effect — enabled for any visible (non-hidden) effect
        if ui
            .add_enabled(
                can_copy,
                egui::Button::new(RichText::new("Copy").size(SMALL_SIZE).color(
                    if can_copy {
                        tc.text_primary
                    } else {
                        tc.text_secondary
                    },
                ))
                .fill(tc.card_bg)
                .stroke(Stroke::new(1.0, tc.card_border))
                .corner_radius(CornerRadius::same(4)),
            )
            .on_hover_text("Copy to a new editable effect")
            .clicked()
        {
            ui.ctx()
                .data_mut(|d| d.insert_temp(egui::Id::new("copy_builtin_prompt"), true));
        }

        // Edit Effect — only for user effects
        if ui
            .add_enabled(
                is_user,
                egui::Button::new(RichText::new("Edit").size(SMALL_SIZE).color(
                    if is_user {
                        tc.text_primary
                    } else {
                        tc.text_secondary
                    },
                ))
                .fill(tc.card_bg)
                .stroke(Stroke::new(1.0, tc.card_border))
                .corner_radius(CornerRadius::same(4)),
            )
            .on_hover_text("Open effect in editor")
            .clicked()
        {
            ui.ctx()
                .data_mut(|d| d.insert_temp(egui::Id::new("open_shader_editor"), true));
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

    // Footer: type breakdown
    draw_footer(ui, &builtin, &user, &tc);
}

// ── Type legend row ──────────────────────────────────────────────────

fn type_tooltip(et: EffectType) -> &'static str {
    match et {
        EffectType::Shader => "Fragment shader effect \u{2014} runs a pixel shader each frame",
        EffectType::Particle => "Particle system with compute simulation + render shader",
        EffectType::Feedback => "Feedback loop \u{2014} accumulates prior frames with decay",
    }
}

fn draw_type_legend(ui: &mut Ui, tc: &crate::ui::theme::colors::ThemeColors) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        for et in [EffectType::Shader, EffectType::Particle, EffectType::Feedback] {
            let color = type_color(et);
            let resp = ui
                .horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = ui.spacing().item_spacing.x;
                    let (rect, _) =
                        ui.allocate_exact_size(Vec2::new(3.0, 10.0), egui::Sense::hover());
                    ui.painter().rect_filled(rect, 1.0, color);
                    ui.label(
                        RichText::new(type_title(et))
                            .size(8.0)
                            .color(tc.text_secondary),
                    );
                })
                .response;
            resp.on_hover_text(type_tooltip(et));
        }
    });
    ui.add_space(4.0);
    ui.separator();
    ui.add_space(2.0);
}

// ── Effect grid ──────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn draw_effect_grid(
    ui: &mut Ui,
    effects: &[(usize, &PfxEffect)],
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
    let available_width = ui.available_width();
    let total_gaps = (COLS - 1) as f32 * gap;
    let btn_width = ((available_width - total_gaps) / COLS as f32).max(40.0);

    for row in effects.chunks(COLS) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for &(i, effect) in row {
                let is_current = loader.current_effect == Some(i);
                let is_armed =
                    !is_builtin_section && pending_delete.map_or(false, |(idx, _)| idx == i);

                let et = effect.effect_type();
                let et_color = type_color(et);

                let (fill, text_color, stroke) = if is_armed {
                    (warning_color, Color32::WHITE, Stroke::NONE)
                } else if is_current {
                    (tc.accent, Color32::WHITE, Stroke::NONE)
                } else {
                    (
                        tc.card_bg,
                        tc.text_primary,
                        Stroke::new(1.0, tc.card_border),
                    )
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
                let rect = response.rect;

                // Left type color strip (3px)
                let strip_rect = Rect::from_min_size(
                    rect.left_top(),
                    Vec2::new(3.0, rect.height()),
                );
                let strip_alpha = if is_current || is_armed { 1.0 } else { 0.6 };
                let strip_color = Color32::from_rgba_unmultiplied(
                    et_color.r(),
                    et_color.g(),
                    et_color.b(),
                    (255.0 * strip_alpha) as u8,
                );
                ui.painter().rect_filled(
                    strip_rect,
                    CornerRadius {
                        nw: 4,
                        sw: 4,
                        ne: 0,
                        se: 0,
                    },
                    strip_color,
                );

                // Two-char type badge at right edge
                let badge_color = if is_armed {
                    Color32::from_rgba_unmultiplied(255, 255, 255, 140)
                } else if is_current {
                    Color32::from_rgba_unmultiplied(255, 255, 255, 180)
                } else {
                    Color32::from_rgba_unmultiplied(
                        et_color.r(),
                        et_color.g(),
                        et_color.b(),
                        if is_current { 230 } else { 128 },
                    )
                };
                let badge_pos =
                    egui::pos2(rect.right() - 14.0, rect.center().y);
                ui.painter().text(
                    badge_pos,
                    egui::Align2::LEFT_CENTER,
                    type_label(et),
                    egui::FontId::monospace(7.0),
                    badge_color,
                );

                // Left click: load effect (also clears pending delete)
                if response.clicked() && !is_current {
                    *new_pending = None;
                    ui.ctx()
                        .data_mut(|d| d.insert_temp(egui::Id::new("pending_effect"), i));
                }

                // Right click: two-stage delete for user effects only
                if !is_builtin_section && response.secondary_clicked() {
                    if is_armed {
                        ui.ctx()
                            .data_mut(|d| d.insert_temp(egui::Id::new("delete_effect"), i));
                        *new_pending = None;
                    } else {
                        *new_pending = Some((i, now));
                    }
                }

                // Hover text
                let particle_suffix = effect
                    .particles
                    .as_ref()
                    .map(|p| format!(" [{}]", format_count_short(p.max_count)));
                let type_suffix = format!(" ({})", type_title(et));
                let hover_text = if is_armed {
                    "Right-click again to DELETE".to_string()
                } else {
                    let base = if effect.description.is_empty() {
                        &effect.name
                    } else {
                        &effect.description
                    };
                    let mut text = base.to_string();
                    if let Some(s) = &particle_suffix {
                        text.push_str(s);
                    }
                    text.push_str(&type_suffix);
                    if !is_builtin_section {
                        text.push_str(" — right-click to delete");
                    }
                    text
                };
                response.on_hover_text(hover_text);
            }
        });
    }
}

// ── Footer ───────────────────────────────────────────────────────────

fn draw_footer(
    ui: &mut Ui,
    builtin: &[(usize, &PfxEffect)],
    user: &[(usize, &PfxEffect)],
    tc: &crate::ui::theme::colors::ThemeColors,
) {
    let all: Vec<&PfxEffect> = builtin
        .iter()
        .chain(user.iter())
        .map(|(_, e)| *e)
        .collect();
    let sh = all.iter().filter(|e| e.effect_type() == EffectType::Shader).count();
    let ps = all.iter().filter(|e| e.effect_type() == EffectType::Particle).count();
    let fb = all.iter().filter(|e| e.effect_type() == EffectType::Feedback).count();

    ui.add_space(4.0);
    ui.separator();
    ui.label(
        RichText::new(format!("{sh} shader · {ps} particle · {fb} feedback"))
            .size(7.0)
            .color(tc.text_secondary),
    );
}

// ── Helpers ──────────────────────────────────────────────────────────

fn truncate_name(name: &str, max_len: usize) -> String {
    if name.len() <= max_len {
        name.to_string()
    } else {
        format!("{}\u{2026}", &name[..max_len - 1])
    }
}

/// Format a particle count for display: 1000 → "1K", 70000 → "70K", 1000000 → "1M".
fn format_count_short(count: u32) -> String {
    if count >= 1_000_000 {
        let m = count as f32 / 1_000_000.0;
        if (m - m.round()).abs() < 0.05 {
            format!("{}M particles", m.round() as u32)
        } else {
            format!("{m:.1}M particles")
        }
    } else if count >= 1_000 {
        let k = count as f32 / 1_000.0;
        if (k - k.round()).abs() < 0.05 {
            format!("{}K particles", k.round() as u32)
        } else {
            format!("{k:.1}K particles")
        }
    } else {
        format!("{count} particles")
    }
}
