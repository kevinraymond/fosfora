use egui::{Color32, CornerRadius, Rect, RichText, Stroke, Ui, Vec2};

use crate::effect::format::{EffectType, PfxEffect};
use crate::effect::loader::EffectLoader;
use crate::ui::theme::colors::{ThemeColors, theme_colors};
use crate::ui::theme::tokens::*;
use crate::ui::widgets::rows;

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

/// Shared per-grid drawing context (replaces a 10-argument parameter list).
struct GridCtx<'a> {
    loader: &'a EffectLoader,
    tc: &'a ThemeColors,
    favorites: &'a [String],
    btn_height: f32,
    gap: f32,
    /// Right-click two-stage delete (user section only; favorites row never deletes).
    allow_delete: bool,
    warning_color: Color32,
}

pub fn draw_effect_panel(ui: &mut Ui, loader: &EffectLoader, favorites: &[String]) {
    let tc = theme_colors(ui.ctx());

    if loader.effects.is_empty() {
        ui.label(
            RichText::new("No effects found")
                .size(SMALL_SIZE)
                .color(tc.text_secondary),
        );
        return;
    }

    // ── Search + type filter state (session-only, in egui data) ──────
    let id_search = egui::Id::new("fx_search");
    let mut query: String = ui
        .ctx()
        .data_mut(|d| d.get_temp(id_search))
        .unwrap_or_default();
    let id_types = egui::Id::new("fx_type_filter");
    let mut types_on: (bool, bool, bool) = ui
        .ctx()
        .data_mut(|d| d.get_temp(id_types))
        .unwrap_or((true, true, true));

    // Search row. Deliberately never auto-focused — typing must not be hijacked live.
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let clear_w = 16.0;
        ui.add(
            egui::TextEdit::singleline(&mut query)
                .hint_text("Search\u{2026}")
                .font(egui::TextStyle::Small)
                .desired_width(ui.available_width() - clear_w - 6.0),
        );
        if !query.is_empty()
            && ui
                .add(
                    egui::Button::new(RichText::new("\u{00d7}").size(SMALL_SIZE))
                        .min_size(Vec2::new(clear_w, 18.0))
                        .frame(false),
                )
                .on_hover_text("Clear search")
                .clicked()
        {
            query.clear();
        }
    });

    // Type filter chips (the old passive legend, now clickable).
    draw_type_filter(ui, &tc, &mut types_on);

    ui.ctx().data_mut(|d| {
        d.insert_temp(id_search, query.clone());
        d.insert_temp(id_types, types_on);
    });

    let q = query.to_lowercase();
    let type_on = |et: EffectType| match et {
        EffectType::Shader => types_on.0,
        EffectType::Particle => types_on.1,
        EffectType::Feedback => types_on.2,
    };
    let matches = |e: &PfxEffect| {
        type_on(e.effect_type()) && (q.is_empty() || e.name.to_lowercase().contains(&q))
    };
    let filtering = !(q.is_empty() && types_on.0 && types_on.1 && types_on.2);

    // ── Partition: favorites / built-in / user, filter applied ───────
    let fav: Vec<(usize, &PfxEffect)> = favorites
        .iter()
        .filter_map(|name| {
            loader
                .effects
                .iter()
                .position(|e| &e.name == name && !e.hidden)
                .map(|i| (i, &loader.effects[i]))
        })
        .filter(|(_, e)| matches(e))
        .collect();
    let builtin_all: Vec<(usize, &PfxEffect)> = loader
        .effects
        .iter()
        .enumerate()
        .filter(|(_, e)| EffectLoader::is_builtin(e) && !e.hidden)
        .collect();
    let user_all: Vec<(usize, &PfxEffect)> = loader
        .effects
        .iter()
        .enumerate()
        .filter(|(_, e)| !EffectLoader::is_builtin(e) && !e.hidden)
        .collect();
    let builtin: Vec<(usize, &PfxEffect)> = builtin_all
        .iter()
        .copied()
        .filter(|(_, e)| matches(e))
        .collect();
    let user: Vec<(usize, &PfxEffect)> = user_all
        .iter()
        .copied()
        .filter(|(_, e)| matches(e))
        .collect();

    // Read pending delete state
    let now = ui.input(|i| i.time);
    let pending_delete: Option<(usize, f64)> = ui
        .ctx()
        .data_mut(|d| d.get_temp(egui::Id::new("pending_delete_effect")));
    let pending_delete = pending_delete.filter(|(_, t)| now - t < 3.0);
    let mut new_pending: Option<(usize, f64)> = pending_delete;

    let ctx = GridCtx {
        loader,
        tc: &tc,
        favorites,
        btn_height: 22.0,
        gap: 4.0,
        allow_delete: false,
        warning_color: Color32::from_rgb(200, 60, 60),
    };

    // ── Favorites row (always on top, never collapsible) ─────────────
    if !fav.is_empty() {
        rows::group_label(ui, "\u{2605} Favorites");
        draw_effect_grid(ui, &fav, &ctx, pending_delete, &mut new_pending);
        ui.add_space(2.0);
    }

    // ── Built-in section ─────────────────────────────────────────────
    if !builtin.is_empty() {
        egui::CollapsingHeader::new(
            RichText::new("Built-in")
                .size(SMALL_SIZE)
                .color(tc.text_secondary),
        )
        .default_open(true)
        .show(ui, |ui| {
            draw_effect_grid(ui, &builtin, &ctx, pending_delete, &mut new_pending);
        });
    }

    // ── User section (hidden while a filter excludes everything in it)
    if !filtering || !user.is_empty() {
        let user_ctx = GridCtx {
            allow_delete: true,
            ..ctx
        };
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
                draw_effect_grid(ui, &user, &user_ctx, pending_delete, &mut new_pending);
            }
        });
    }

    if filtering && fav.is_empty() && builtin.is_empty() && user.is_empty() {
        ui.label(
            RichText::new("No matches")
                .size(SMALL_SIZE)
                .color(tc.text_secondary),
        );
    }

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
        ui.spacing_mut().item_spacing.x = 4.0;

        let current = loader.current_effect.and_then(|i| loader.effects.get(i));
        let can_copy = current.map_or(false, |e| !e.hidden);
        let is_user = current.map_or(false, |e| !EffectLoader::is_builtin(e));

        // Copy Effect — enabled for any visible (non-hidden) effect
        if ui
            .add_enabled(
                can_copy,
                egui::Button::new(RichText::new("Copy").size(SMALL_SIZE).color(if can_copy {
                    tc.text_primary
                } else {
                    tc.text_secondary
                }))
                .fill(tc.card_bg)
                .stroke(Stroke::new(1.0_f32, tc.card_border))
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
                egui::Button::new(RichText::new("Edit").size(SMALL_SIZE).color(if is_user {
                    tc.text_primary
                } else {
                    tc.text_secondary
                }))
                .fill(tc.card_bg)
                .stroke(Stroke::new(1.0_f32, tc.card_border))
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
                .stroke(Stroke::new(1.0_f32, tc.card_border))
                .corner_radius(CornerRadius::same(4)),
            )
            .on_hover_text("Create a new effect from template")
            .clicked()
        {
            ui.ctx()
                .data_mut(|d| d.insert_temp(egui::Id::new("new_effect_prompt"), true));
        }
    });

    // Footer: type breakdown (+ shown count while filtering)
    let shown = builtin.len() + user.len();
    draw_footer(ui, &builtin_all, &user_all, filtering.then_some(shown), &tc);
}

// ── Type filter chips ────────────────────────────────────────────────

fn type_tooltip(et: EffectType) -> &'static str {
    match et {
        EffectType::Shader => "Fragment shader effect \u{2014} runs a pixel shader each frame",
        EffectType::Particle => "Particle system with compute simulation + render shader",
        EffectType::Feedback => "Feedback loop \u{2014} accumulates prior frames with decay",
    }
}

fn draw_type_filter(ui: &mut Ui, tc: &ThemeColors, types_on: &mut (bool, bool, bool)) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        for (chip_idx, et) in [
            EffectType::Shader,
            EffectType::Particle,
            EffectType::Feedback,
        ]
        .into_iter()
        .enumerate()
        {
            let on = match chip_idx {
                0 => types_on.0,
                1 => types_on.1,
                _ => types_on.2,
            };
            let color = type_color(et);
            let resp = ui
                .horizontal(|ui| {
                    let (rect, _) =
                        ui.allocate_exact_size(Vec2::new(3.0, 10.0), egui::Sense::hover());
                    let strip = if on {
                        color
                    } else {
                        Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 60)
                    };
                    ui.painter().rect_filled(rect, 1.0, strip);
                    ui.label(RichText::new(type_title(et)).size(8.0).color(if on {
                        tc.text_secondary
                    } else {
                        tc.text_dim
                    }));
                })
                .response
                .interact(egui::Sense::click());
            if resp.clicked() {
                let all_on = types_on.0 && types_on.1 && types_on.2;
                if all_on {
                    // Solo the clicked type — the common "only particles" gesture.
                    *types_on = (chip_idx == 0, chip_idx == 1, chip_idx == 2);
                } else {
                    match chip_idx {
                        0 => types_on.0 = !types_on.0,
                        1 => types_on.1 = !types_on.1,
                        _ => types_on.2 = !types_on.2,
                    }
                    // Never allow an empty (all-off) state — snap back to all on.
                    if !(types_on.0 || types_on.1 || types_on.2) {
                        *types_on = (true, true, true);
                    }
                }
            }
            resp.on_hover_text(format!(
                "{}\nClick to filter (click again to reset)",
                type_tooltip(et)
            ));
        }
    });
    ui.add_space(4.0);
    ui.separator();
    ui.add_space(2.0);
}

// ── Effect grid ──────────────────────────────────────────────────────

fn draw_effect_grid(
    ui: &mut Ui,
    effects: &[(usize, &PfxEffect)],
    ctx: &GridCtx<'_>,
    pending_delete: Option<(usize, f64)>,
    new_pending: &mut Option<(usize, f64)>,
) {
    let now = ui.input(|i| i.time);
    let tc = ctx.tc;
    // Long names get a 2-column grid (~140px buttons — "Lattice Pyroclastic"
    // fits whole); short-named sets keep the denser 3 columns.
    let cols = if effects.iter().any(|(_, e)| e.name.chars().count() > 12) {
        2
    } else {
        3
    };
    let available_width = ui.available_width();
    let total_gaps = (cols - 1) as f32 * ctx.gap;
    let btn_width = ((available_width - total_gaps) / cols as f32).max(40.0);

    for row in effects.chunks(cols) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = ctx.gap;
            for &(i, effect) in row {
                let is_current = ctx.loader.current_effect == Some(i);
                let is_armed =
                    ctx.allow_delete && pending_delete.map_or(false, |(idx, _)| idx == i);
                let is_fav = ctx.favorites.iter().any(|f| f == &effect.name);

                let et = effect.effect_type();
                let et_color = type_color(et);

                let (fill, text_color, stroke) = if is_armed {
                    (ctx.warning_color, Color32::WHITE, Stroke::NONE)
                } else if is_current {
                    (tc.accent, Color32::WHITE, Stroke::NONE)
                } else {
                    (
                        tc.card_bg,
                        tc.text_primary,
                        Stroke::new(1.0_f32, tc.card_border),
                    )
                };

                let btn = egui::Button::new(
                    RichText::new(truncate_name(&effect.name, 22))
                        .size(SMALL_SIZE)
                        .color(text_color),
                )
                .fill(fill)
                .stroke(stroke)
                .corner_radius(CornerRadius::same(4));

                let response = ui.add_sized(Vec2::new(btn_width, ctx.btn_height), btn);
                let rect = response.rect;

                // Left type color strip (3px)
                let strip_rect =
                    Rect::from_min_size(rect.left_top(), Vec2::new(3.0, rect.height()));
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

                // Right-edge slot: favorite star (pinned, or on hover) else type badge.
                // Registered after the button so the star owns clicks in its zone.
                let star_rect = Rect::from_min_max(
                    egui::pos2(rect.right() - 16.0, rect.top()),
                    rect.right_bottom(),
                );
                let star_resp =
                    ui.interact(star_rect, response.id.with("fav"), egui::Sense::click());
                if is_fav || star_resp.hovered() {
                    let (glyph, color) = if is_fav {
                        ("\u{2605}", tc.warning) // ★ gold
                    } else {
                        ("\u{2606}", tc.text_secondary) // ☆ ghost
                    };
                    ui.painter().text(
                        star_rect.center(),
                        egui::Align2::CENTER_CENTER,
                        glyph,
                        egui::FontId::proportional(10.0),
                        color,
                    );
                } else {
                    let badge_color = if is_armed {
                        Color32::from_rgba_unmultiplied(255, 255, 255, 140)
                    } else if is_current {
                        Color32::from_rgba_unmultiplied(255, 255, 255, 180)
                    } else {
                        Color32::from_rgba_unmultiplied(
                            et_color.r(),
                            et_color.g(),
                            et_color.b(),
                            128,
                        )
                    };
                    let badge_pos = egui::pos2(rect.right() - 14.0, rect.center().y);
                    ui.painter().text(
                        badge_pos,
                        egui::Align2::LEFT_CENTER,
                        type_label(et),
                        egui::FontId::monospace(7.0),
                        badge_color,
                    );
                }
                if star_resp.clicked() {
                    ui.ctx().data_mut(|d| {
                        d.insert_temp(egui::Id::new("toggle_favorite_effect"), effect.name.clone());
                    });
                }
                let star_tip = if is_fav {
                    "Unpin from Favorites"
                } else {
                    "Pin to Favorites"
                };
                star_resp.on_hover_text(star_tip);

                // Left click: load effect (also clears pending delete)
                if response.clicked() && !is_current {
                    *new_pending = None;
                    ui.ctx()
                        .data_mut(|d| d.insert_temp(egui::Id::new("pending_effect"), i));
                }

                // Right click: two-stage delete for user effects only
                if ctx.allow_delete && response.secondary_clicked() {
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
                    let mut text = base.clone();
                    if let Some(s) = &particle_suffix {
                        text.push_str(s);
                    }
                    text.push_str(&type_suffix);
                    if ctx.allow_delete {
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
    shown: Option<usize>,
    tc: &ThemeColors,
) {
    let all: Vec<&PfxEffect> = builtin.iter().chain(user.iter()).map(|(_, e)| *e).collect();
    let sh = all
        .iter()
        .filter(|e| e.effect_type() == EffectType::Shader)
        .count();
    let ps = all
        .iter()
        .filter(|e| e.effect_type() == EffectType::Particle)
        .count();
    let fb = all
        .iter()
        .filter(|e| e.effect_type() == EffectType::Feedback)
        .count();

    let text = match shown {
        Some(n) => format!("{sh} shader · {ps} particle · {fb} feedback · {n} shown"),
        None => format!("{sh} shader · {ps} particle · {fb} feedback"),
    };
    ui.add_space(4.0);
    ui.separator();
    ui.label(RichText::new(text).size(7.0).color(tc.text_secondary));
}

// ── Helpers ──────────────────────────────────────────────────────────

fn truncate_name(name: &str, max_len: usize) -> String {
    crate::ui::widgets::truncate_chars(name, max_len)
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
