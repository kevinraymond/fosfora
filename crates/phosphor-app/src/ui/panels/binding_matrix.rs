use std::collections::{HashMap, HashSet};

use egui::{
    Color32, Context, Id, Order, Pos2, Rect, RichText, ScrollArea, Sense, Stroke, StrokeKind,
    epaint::CubicBezierShape, pos2,
};

use crate::bindings::bus::BindingBus;
use crate::bindings::templates;
use crate::bindings::types::*;
use crate::ui::theme::colors::theme_colors;

use super::binding_helpers::*;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
pub enum ScopeTab {
    Effect,
    Global,
}

/// One end of a binding the user has clicked, waiting for the other.
///
/// The two side columns used to be read-only meters: the only way to make a
/// binding was to scroll the middle column to the bottom, press "+ New Binding",
/// then pick both ends from combo boxes that re-listed all ~330 sources and
/// ~150 targets with no filter. Clicking a row on either side now arms it, and
/// clicking a row on the other side completes the binding.
#[derive(Clone, PartialEq)]
pub enum ArmedEnd {
    Source(String),
    Target(String),
}

pub struct BindingMatrixState {
    pub open: bool,
    pub scope_tab: ScopeTab,
    /// Half-finished binding: click a source then a target, or the reverse.
    pub armed: Option<ArmedEnd>,
    /// Case-insensitive row filters for the two side columns. Without these the
    /// only way through the list was scrolling.
    pub source_filter: String,
    pub target_filter: String,
    pub collapsed_source_groups: HashSet<String>,
    /// What the collapse-all button reports. Tracked explicitly because the source
    /// list now starts with the long groups collapsed, so "is the collapsed set empty"
    /// is no longer the same question.
    pub source_groups_collapsed: bool,
    /// Target groups the user has opened. Inverted relative to the source side,
    /// and deliberately: target group ids are partly dynamic (`Layer 2 • Raster`),
    /// so a *collapsed* set could only ever be filled from a hardcoded list of the
    /// seven fixed ones — which is exactly how "Collapse all" came to silently
    /// skip the per-layer param groups, the biggest ones on screen. Tracking what
    /// is open needs no such list: collapse-all is `clear()`.
    pub expanded_target_groups: HashSet<String>,
    pub expanded_binding_id: Option<String>,
    // Position tracking for connection lines (rebuilt each frame)
    pub source_positions: HashMap<String, Pos2>,
    pub target_positions: HashMap<String, Pos2>,
    pub card_positions: HashMap<String, (Pos2, Pos2)>,
    /// Visible band of each column's scroll area this frame — curve endpoints
    /// are clamped into these so scrolled-off rows don't paint cables across
    /// the header/footer.
    pub source_viewport: Rect,
    pub card_viewport: Rect,
    pub target_viewport: Rect,
    /// Card under the pointer this frame — its cables brighten, others dim.
    pub hovered_binding_id: Option<String>,
    pub flow_phase: f32,
    /// Cached texture handles for WS bridge preview thumbnails.
    pub preview_textures: HashMap<String, egui::TextureHandle>,
}

impl BindingMatrixState {
    pub fn new() -> Self {
        Self {
            open: false,
            scope_tab: ScopeTab::Effect,
            armed: None,
            source_filter: String::new(),
            target_filter: String::new(),
            // Bands, Features and Beat stay open — they are what most patches start
            // from. The rest would push 74 sources' worth of rows above the fold.
            collapsed_source_groups: [
                "audio_loudness",
                "audio_timbre",
                "audio_structure",
                "audio_harmonic",
                "audio_stereo",
                "audio_pitch",
                "audio_key",
                "audio_mfcc",
                "audio_chroma",
                "audio_mel",
                "audio_dmfcc",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            source_groups_collapsed: false,
            // Empty = everything collapsed. A 3-layer session lists 90-150 targets
            // and they used to all render on first open in a 240px column.
            expanded_target_groups: HashSet::new(),
            expanded_binding_id: None,
            source_positions: HashMap::new(),
            target_positions: HashMap::new(),
            card_positions: HashMap::new(),
            source_viewport: Rect::NOTHING,
            card_viewport: Rect::NOTHING,
            target_viewport: Rect::NOTHING,
            hovered_binding_id: None,
            flow_phase: 0.0,
            preview_textures: HashMap::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Main draw function
// ---------------------------------------------------------------------------

pub fn draw_binding_matrix(
    ctx: &Context,
    state: &mut BindingMatrixState,
    bus: &mut BindingBus,
    info: &BindingPanelInfo,
) {
    if !state.open {
        return;
    }

    ctx.request_repaint();

    // Advance flow phase
    let dt = ctx.input(|i| i.predicted_dt);
    state.flow_phase = (state.flow_phase + dt * 0.8) % 1.0;

    // Clear position maps
    state.source_positions.clear();
    state.target_positions.clear();
    state.card_positions.clear();
    state.hovered_binding_id = None;

    #[allow(deprecated)]
    let screen = ctx.input(|i| i.screen_rect());
    let tc = theme_colors(ctx);

    // Backdrop — paint a dimming rectangle over the whole screen
    let backdrop_layer = egui::LayerId::new(Order::Middle, Id::new("matrix_backdrop"));
    let painter = ctx.layer_painter(backdrop_layer);
    painter.rect_filled(screen, 0.0, tc.backdrop);

    // Reads LAST frame's popup state (sampled before this frame's widgets),
    // so the click/Esc that dismisses a popup — source picker, target combo,
    // "+ Add", all of which overhang the panel — can't also close the overlay.
    let popup_was_open = egui::Popup::is_any_open(ctx);

    // Escape closes (egui itself closes an open popup on Escape)
    let esc_pressed = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    if esc_pressed && !popup_was_open {
        state.open = false;
        return;
    }

    // Area for the matrix content — centered on window width, max width capped
    let margin = 40.0;
    let frame_pad = 12.0; // inner_margin on the frame below
    let max_w = 1200.0;
    let actual_w = (screen.width() - margin * 2.0).min(max_w);
    let x_offset = (screen.width() - actual_w) / 2.0;
    let content_rect = Rect::from_min_max(
        pos2(screen.min.x + x_offset, screen.min.y + margin),
        pos2(screen.min.x + x_offset + actual_w, screen.max.y - margin),
    );

    let area_resp = egui::Area::new(Id::new("binding_matrix_area"))
        .order(Order::Foreground)
        .fixed_pos(content_rect.min)
        .show(ctx, |ui| {
            let frame = egui::Frame::new()
                .fill(Color32::TRANSPARENT)
                .corner_radius(8.0)
                .inner_margin(egui::Margin::same(12));

            frame.show(ui, |ui| {
                // Subtract frame inner margin so the outer Area matches content_rect exactly
                let inner_size = content_rect.size() - egui::vec2(frame_pad * 2.0, frame_pad * 2.0);
                ui.set_min_size(inner_size);
                ui.set_max_size(inner_size);

                // Ensure all popup/select list backgrounds are fully opaque
                ui.visuals_mut().window_fill = tc.card_bg;
                ui.visuals_mut().extreme_bg_color = tc.meter_bg;

                // Header
                draw_header(ui, state, bus, info);
                ui.add_space(8.0);

                // One target-option build per frame, shared by both columns.
                let targets = build_target_options(info);

                // Three columns
                let avail = ui.available_size();
                let col_source_w = 240.0;
                let col_target_w = 240.0;
                let col_center_w = (avail.x - col_source_w - col_target_w - 24.0).max(200.0);

                // Fully opaque side panels (tc.panel may have ~90% alpha)
                let side_bg = Color32::from_rgb(tc.panel.r(), tc.panel.g(), tc.panel.b());
                let side_pad = 6.0;

                // Set by whichever column completed a click-to-bind this frame;
                // both borrow the bus immutably for their meters, so the insert
                // happens once, after the columns are done.
                let mut pending_bind: Option<(String, String)> = None;

                ui.horizontal(|ui| {
                    // Left: Sources — padded with bg
                    let left_bg_idx = ui.painter().add(egui::Shape::Noop);
                    let left_resp = ui.vertical(|ui| {
                        ui.set_width(col_source_w + side_pad * 2.0);
                        ui.set_height(avail.y - 60.0);
                        ui.add_space(side_pad);
                        ui.indent(Id::new("src_pad"), |ui| {
                            ui.set_width(col_source_w);
                            if let Some(b) = draw_source_column(ui, state, bus) {
                                pending_bind = Some(b);
                            }
                        });
                    });
                    ui.painter().set(
                        left_bg_idx,
                        egui::Shape::rect_filled(left_resp.response.rect, 6.0, side_bg),
                    );

                    ui.add_space(8.0);

                    // Center: Binding cards (semi-transparent bg)
                    let center_bg_idx = ui.painter().add(egui::Shape::Noop);
                    let center_resp = ui.vertical(|ui| {
                        ui.set_width(col_center_w);
                        ui.set_height(avail.y - 60.0);
                        draw_center_column(ui, state, bus, info, &targets);
                    });
                    ui.painter().set(
                        center_bg_idx,
                        egui::Shape::rect_filled(
                            center_resp.response.rect,
                            6.0,
                            Color32::from_rgba_unmultiplied(
                                tc.panel.r(),
                                tc.panel.g(),
                                tc.panel.b(),
                                128,
                            ),
                        ),
                    );

                    ui.add_space(8.0);

                    // Right: Targets — padded with bg
                    let right_bg_idx = ui.painter().add(egui::Shape::Noop);
                    let right_resp = ui.vertical(|ui| {
                        ui.set_width(col_target_w + side_pad * 2.0);
                        ui.set_height(avail.y - 60.0);
                        ui.add_space(side_pad);
                        ui.indent(Id::new("tgt_pad"), |ui| {
                            ui.set_width(col_target_w);
                            if let Some(b) = draw_target_column(ui, state, bus, &targets) {
                                pending_bind = Some(b);
                            }
                        });
                    });
                    ui.painter().set(
                        right_bg_idx,
                        egui::Shape::rect_filled(right_resp.response.rect, 6.0, side_bg),
                    );
                });

                // A click-to-bind completed: create it and open its card, so the
                // transforms and the name are one click away rather than a hunt.
                if let Some((source, target)) = pending_bind {
                    let scope = match state.scope_tab {
                        ScopeTab::Effect => BindingScope::Preset,
                        ScopeTab::Global => BindingScope::Global,
                    };
                    let id = bus.add_binding(source, target, scope);
                    state.expanded_binding_id = Some(id);
                }

                // Footer
                draw_footer(ui, state, bus);
            });
        });

    // Close on click outside the content area (skip on the frame the matrix was just opened)
    let just_opened = ctx.data_mut(|d| {
        d.get_temp::<bool>(egui::Id::new("binding_matrix_just_opened"))
            .unwrap_or(false)
    });
    if just_opened {
        ctx.data_mut(|d| {
            d.insert_temp(egui::Id::new("binding_matrix_just_opened"), false);
        });
    } else {
        let clicked_outside = ctx.input(|i| {
            i.pointer.any_click()
                && i.pointer
                    .interact_pos()
                    .is_some_and(|pos| !area_resp.response.rect.contains(pos))
        });
        if clicked_outside && !popup_was_open {
            state.open = false;
        }
    }

    // Draw connection lines (Pass 2 — after layout)
    draw_connections(ctx, state, bus);
}

// ---------------------------------------------------------------------------
// Header
// ---------------------------------------------------------------------------

fn draw_header(
    ui: &mut egui::Ui,
    state: &mut BindingMatrixState,
    bus: &mut BindingBus,
    info: &BindingPanelInfo,
) {
    let tc = theme_colors(ui.ctx());

    ui.horizontal(|ui| {
        // Title
        ui.label(
            RichText::new("BINDING MATRIX")
                .size(14.0)
                .strong()
                .color(tc.text_primary),
        );

        ui.add_space(12.0);

        // Source type legend dots
        let legend = |ui: &mut egui::Ui, color: Color32, label: &str| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                let (r, _) = ui.allocate_exact_size(egui::vec2(6.0, 6.0), Sense::hover());
                ui.painter().circle_filled(r.center(), 3.0, color);
                ui.label(RichText::new(label).size(8.0).color(tc.text_secondary));
            });
        };
        legend(ui, AUDIO_COLOR, "Audio");
        legend(ui, MIDI_COLOR, "MIDI");
        legend(ui, OSC_COLOR, "OSC");
        legend(ui, WS_COLOR, "WS");

        ui.add_space(12.0);

        // Scope tabs
        let tab_btn = |ui: &mut egui::Ui, label: &str, active: bool| -> bool {
            let color = if active {
                tc.text_primary
            } else {
                tc.text_secondary
            };
            let fill = if active {
                tc.hover_fill
            } else {
                Color32::TRANSPARENT
            };
            ui.add(
                egui::Button::new(RichText::new(label).size(10.0).color(color))
                    .fill(fill)
                    .corner_radius(4.0)
                    .min_size(egui::vec2(60.0, 20.0)),
            )
            .clicked()
        };
        let (fx_on, gl_on) = bus.bindings.iter().fold((0usize, 0usize), |(f, g), b| {
            if !b.enabled {
                (f, g)
            } else if b.scope == BindingScope::Preset {
                (f + 1, g)
            } else {
                (f, g + 1)
            }
        });
        let fx_label = if fx_on > 0 {
            format!("Effect ({fx_on})")
        } else {
            "Effect".to_string()
        };
        let gl_label = if gl_on > 0 {
            format!("Global ({gl_on})")
        } else {
            "Global".to_string()
        };
        if tab_btn(ui, &fx_label, state.scope_tab == ScopeTab::Effect) {
            state.scope_tab = ScopeTab::Effect;
        }
        if tab_btn(ui, &gl_label, state.scope_tab == ScopeTab::Global) {
            state.scope_tab = ScopeTab::Global;
        }

        // Templates
        ui.add_space(8.0);
        egui::ComboBox::from_id_salt("matrix_templates")
            .selected_text(RichText::new("Templates").size(9.0))
            .width(100.0)
            .show_ui(ui, |ui| {
                for tmpl in templates::builtin_templates() {
                    if ui
                        .button(RichText::new(tmpl.name).size(9.0))
                        .on_hover_text(tmpl.description)
                        .clicked()
                    {
                        bus.apply_template(
                            tmpl,
                            info.active_layer,
                            info.active_effect_name(),
                            info.active_param_names(),
                        );
                        // Templates are Preset-scoped; land the user where the
                        // new cards actually are instead of a silent no-op view.
                        state.scope_tab = ScopeTab::Effect;
                    }
                }
            });

        // Right: close button
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add(
                    egui::Button::new(
                        RichText::new("\u{00d7}")
                            .size(16.0)
                            .color(tc.text_secondary),
                    )
                    .frame(false),
                )
                .clicked()
            {
                state.open = false;
            }
        });
    });
}

// ---------------------------------------------------------------------------
// Source column (left)
// ---------------------------------------------------------------------------

/// A one-line filter with a clear button, shared by both side columns.
fn draw_filter_box(ui: &mut egui::Ui, filter: &mut String, id: &str) {
    let tc = theme_colors(ui.ctx());
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        let width = ui.available_width() - if filter.is_empty() { 0.0 } else { 16.0 };
        ui.add(
            egui::TextEdit::singleline(filter)
                .id_salt(id)
                .hint_text(RichText::new("filter").size(8.0))
                .desired_width(width)
                .font(egui::FontId::proportional(9.0)),
        );
        if !filter.is_empty()
            && ui
                .add(
                    egui::Button::new(RichText::new("\u{00d7}").size(9.0).color(tc.text_dim))
                        .frame(false)
                        .min_size(egui::vec2(14.0, 14.0)),
                )
                .on_hover_text("Clear filter")
                .clicked()
        {
            filter.clear();
        }
    });
}

/// Returns `(source, target)` when a click here completed a binding.
fn draw_source_column(
    ui: &mut egui::Ui,
    state: &mut BindingMatrixState,
    bus: &BindingBus,
) -> Option<(String, String)> {
    let tc = theme_colors(ui.ctx());

    ui.horizontal(|ui| {
        ui.label(
            RichText::new("SOURCES")
                .size(8.0)
                .strong()
                .color(tc.text_secondary),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // Explicit flag rather than `!collapsed_source_groups.is_empty()`: with the
            // long groups collapsed by default the set is non-empty on first open, and
            // deriving from emptiness showed "Expand all" with the wrong caret.
            let all_collapsed = state.source_groups_collapsed;
            let icon = if all_collapsed {
                "\u{25bc}"
            } else {
                "\u{25b6}"
            };
            let hint = if all_collapsed {
                "Expand all"
            } else {
                "Collapse all"
            };
            if ui
                .add(
                    egui::Button::new(RichText::new(icon).size(7.0).color(tc.text_dim))
                        .frame(false)
                        .min_size(egui::vec2(14.0, 14.0)),
                )
                .on_hover_text(hint)
                .clicked()
            {
                state.source_groups_collapsed = !all_collapsed;
                if all_collapsed {
                    state.collapsed_source_groups.clear();
                } else {
                    // Every group actually present, so nothing is missed the way
                    // the target side used to miss its per-layer groups.
                    for g in all_source_groups(&bus.last_snapshot) {
                        state.collapsed_source_groups.insert(g.id);
                    }
                }
            }
        });
    });
    ui.add_space(2.0);
    draw_filter_box(ui, &mut state.source_filter, "matrix_source_filter");
    ui.add_space(4.0);

    // A live filter opens every group with a hit, so a match cannot hide inside
    // a collapsed group.
    let filtering = !state.source_filter.trim().is_empty();

    ScrollArea::vertical()
        .id_salt("matrix_sources")
        .show(ui, |ui| {
            state.source_viewport = ui.clip_rect();
            ui.set_width(ui.available_width());

            let mut pending_bind: Option<(String, String)> = None;

            let bound_sources: HashSet<&str> = bus
                .bindings
                .iter()
                .filter(|b| !b.source.is_empty())
                .map(|b| b.source.as_str())
                .collect();

            // One list, shared with the expanded-card picker. The two used to be
            // built separately and drifted, leaving 28 collected features listed
            // in neither.
            let groups = all_source_groups(&bus.last_snapshot);
            let mut any_row = false;

            for group in &groups {
                let keys = filter_source_keys(group, &state.source_filter);
                if keys.is_empty() {
                    continue;
                }
                any_row = true;
                let mapped = keys.iter().filter(|k| bound_sources.contains(*k)).count();
                if let Some(bind) = draw_source_group(
                    ui,
                    state,
                    bus,
                    &group.label,
                    &group.id,
                    group.color,
                    &keys,
                    mapped,
                    &bound_sources,
                    filtering,
                ) {
                    pending_bind = Some(bind);
                }
            }

            if !any_row {
                ui.add_space(6.0);
                ui.label(
                    RichText::new(if filtering {
                        "no sources match"
                    } else {
                        "no sources yet"
                    })
                    .size(8.0)
                    .color(tc.text_dim),
                );
            }

            // Drop cached preview textures for bridges that stopped reporting.
            let live: HashSet<String> = groups
                .iter()
                .filter_map(|g| g.id.strip_prefix("ws.").map(|s| s.to_string()))
                .collect();
            state.preview_textures.retain(|k, _| live.contains(k));

            pending_bind
        })
        .inner
}

/// Returns `(source, target)` when a click here completed a binding.
#[allow(clippy::too_many_arguments)]
fn draw_source_group(
    ui: &mut egui::Ui,
    state: &mut BindingMatrixState,
    bus: &BindingBus,
    label: &str,
    group_id: &str,
    color: Color32,
    keys: &[&str],
    mapped_count: usize,
    bound_sources: &HashSet<&str>,
    filtering: bool,
) -> Option<(String, String)> {
    let tc = theme_colors(ui.ctx());
    let mut pending_bind: Option<(String, String)> = None;
    // While filtering, a group with a surviving row is shown open — otherwise the
    // hit would sit inside a collapsed group and the column would look empty.
    let collapsed = !filtering && state.collapsed_source_groups.contains(group_id);

    // Group header — full-width clickable bar with dark bg
    let avail_w = ui.available_width();
    let (header_rect, header_resp) =
        ui.allocate_exact_size(egui::vec2(avail_w, 20.0), Sense::click());
    let hovered = header_resp.hovered();

    // Dark background + subtle border
    let bg = if hovered {
        tc.hover_fill
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect(
        header_rect,
        3.0,
        bg,
        Stroke::new(
            1.0_f32,
            if hovered {
                tc.card_border
            } else {
                tc.hover_border
            },
        ),
        StrokeKind::Inside,
    );

    // Left color accent bar
    let accent_rect = Rect::from_min_size(header_rect.min, egui::vec2(3.0, header_rect.height()));
    ui.painter().rect_filled(
        accent_rect,
        egui::CornerRadius {
            nw: 3,
            sw: 3,
            ..Default::default()
        },
        color.linear_multiply(0.6),
    );

    // Draw contents manually positioned
    let cy = header_rect.center().y;
    let mut x = header_rect.left() + 8.0;

    // Caret
    let caret = if collapsed { "\u{25b6}" } else { "\u{25bc}" };
    ui.painter().text(
        Pos2::new(x, cy),
        egui::Align2::LEFT_CENTER,
        caret,
        egui::FontId::proportional(7.0),
        tc.text_secondary,
    );
    x += 12.0;

    // Label
    let label_galley = ui.painter().layout_no_wrap(
        label.to_string(),
        egui::FontId::proportional(9.0),
        tc.text_primary,
    );
    ui.painter().galley(
        Pos2::new(x, cy - label_galley.size().y * 0.5),
        label_galley,
        tc.text_primary,
    );

    // Mapped count badge (right-aligned)
    if mapped_count > 0 {
        ui.painter().text(
            Pos2::new(header_rect.right() - 6.0, cy),
            egui::Align2::RIGHT_CENTER,
            format!("{mapped_count}"),
            egui::FontId::proportional(7.0),
            color,
        );
    }

    // Toggle on click
    if header_resp.clicked() {
        if state.collapsed_source_groups.contains(group_id) {
            state.collapsed_source_groups.remove(group_id);
        } else {
            state.collapsed_source_groups.insert(group_id.to_string());
        }
    }
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    // Record group header position for collapsed-group line anchoring
    if collapsed {
        let anchor = pos2(header_rect.right(), header_rect.center().y);
        for &key in keys {
            state.source_positions.insert(key.to_string(), anchor);
        }
    } else {
        // Preview thumbnail for WS bridge sources
        if let Some(source_name) = group_id.strip_prefix("ws.") {
            if let Some(jpeg_data) = bus.ws_preview_images.get(source_name) {
                draw_ws_preview(ui, state, source_name, jpeg_data);
            }
        }

        // Draw individual source rows
        for &key in keys {
            let is_bound = bound_sources.contains(key);
            let val = bus.last_snapshot.get(key).map(|(v, _)| *v).unwrap_or(0.0);

            let friendly = friendly_source_label(key);
            let armed_here = state.armed.as_ref() == Some(&ArmedEnd::Source(key.to_string()));

            let row_resp = ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.add_space(12.0); // indent

                // Mapped dot
                let dot_color = if is_bound {
                    source_color(key)
                } else {
                    tc.text_dim
                };
                let (dot_rect, _) = ui.allocate_exact_size(egui::vec2(6.0, 6.0), Sense::hover());
                ui.painter()
                    .circle_filled(dot_rect.center(), 2.5, dot_color);

                // Label
                let label_color = if is_bound {
                    tc.text_primary
                } else {
                    tc.text_secondary
                };
                ui.label(RichText::new(&friendly).size(9.0).color(label_color));

                // Mini bar (32x3)
                let (bar_rect, _) = ui.allocate_exact_size(egui::vec2(32.0, 3.0), Sense::hover());
                ui.painter().rect_filled(bar_rect, 1.0, tc.meter_bg);
                let fill_w = 32.0 * val.clamp(0.0, 1.0);
                if fill_w > 0.5 {
                    let fill_rect =
                        egui::Rect::from_min_size(bar_rect.min, egui::vec2(fill_w, 3.0));
                    ui.painter().rect_filled(
                        fill_rect,
                        1.0,
                        source_color(key).linear_multiply(0.6),
                    );
                }

                // Value
                ui.label(
                    RichText::new(format!("{val:.2}"))
                        .size(7.0)
                        .color(tc.text_dim),
                );

                // Right anchor dot for bezier lines
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (anchor_rect, _) =
                        ui.allocate_exact_size(egui::vec2(6.0, 6.0), Sense::hover());
                    let anchor_color = if is_bound {
                        source_color(key).linear_multiply(0.7)
                    } else {
                        tc.hover_border
                    };
                    ui.painter()
                        .circle_filled(anchor_rect.center(), 3.0, anchor_color);
                    // Store anchor position for bezier
                    state
                        .source_positions
                        .insert(key.to_string(), anchor_rect.center());
                });
            });

            // Clicking a source arms it, or completes a binding from an armed
            // target. Both side columns were read-only meters before this.
            let rect = row_resp.response.rect;
            let click = ui.interact(
                rect,
                ui.make_persistent_id(("matrix_source_row", key)),
                Sense::click(),
            );
            if armed_here || click.hovered() {
                let fill = if armed_here {
                    tc.accent.linear_multiply(0.22)
                } else {
                    tc.hover_fill
                };
                ui.painter()
                    .rect_filled(rect.expand2(egui::vec2(2.0, 1.0)), 3.0, fill);
            }
            if click.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if click.clicked() {
                match state.armed.clone() {
                    Some(ArmedEnd::Target(t)) => {
                        pending_bind = Some((key.to_string(), t));
                        state.armed = None;
                    }
                    Some(ArmedEnd::Source(s)) if s == key => state.armed = None,
                    _ => state.armed = Some(ArmedEnd::Source(key.to_string())),
                }
            }
        }
    }

    ui.add_space(2.0);
    pending_bind
}

/// Render a WS bridge preview thumbnail inline in the source column.
fn draw_ws_preview(
    ui: &mut egui::Ui,
    state: &mut BindingMatrixState,
    source_name: &str,
    jpeg_data: &[u8],
) {
    // Decode JPEG → egui ColorImage, then create/update texture
    let color_image = match image::load_from_memory(jpeg_data) {
        Ok(img) => {
            let rgba = img.to_rgba8();
            let size = [rgba.width() as usize, rgba.height() as usize];
            egui::ColorImage::from_rgba_unmultiplied(size, &rgba)
        }
        Err(_) => return,
    };

    let tex_id = format!("ws_preview_{source_name}");
    let handle = state
        .preview_textures
        .entry(source_name.to_string())
        .or_insert_with(|| {
            ui.ctx()
                .load_texture(&tex_id, color_image.clone(), egui::TextureOptions::LINEAR)
        });
    handle.set(color_image, egui::TextureOptions::LINEAR);

    // Size to fit panel width, maintaining aspect ratio
    let avail_w = ui.available_width() - 12.0; // match indent
    let aspect = 160.0 / 120.0;
    let img_w = avail_w.min(160.0);
    let img_h = img_w / aspect;

    ui.horizontal(|ui| {
        ui.add_space(12.0);
        let img = egui::Image::new(handle as &egui::TextureHandle)
            .fit_to_exact_size(egui::vec2(img_w, img_h))
            .corner_radius(3.0);
        ui.add(img);
    });
    ui.add_space(2.0);
}

// ---------------------------------------------------------------------------
// Target column (right)
// ---------------------------------------------------------------------------

/// Returns `(source, target)` when a click here completed a binding — the bus is
/// borrowed immutably for the meters, so the caller does the insert.
fn draw_target_column(
    ui: &mut egui::Ui,
    state: &mut BindingMatrixState,
    bus: &BindingBus,
    targets: &[TargetOption],
) -> Option<(String, String)> {
    let tc = theme_colors(ui.ctx());

    ui.horizontal(|ui| {
        ui.label(
            RichText::new("TARGETS")
                .size(8.0)
                .strong()
                .color(tc.text_secondary),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let any_open = !state.expanded_target_groups.is_empty();
            let icon = if any_open { "\u{25b6}" } else { "\u{25bc}" };
            let hint = if any_open {
                "Collapse all"
            } else {
                "Expand all"
            };
            if ui
                .add(
                    egui::Button::new(RichText::new(icon).size(7.0).color(tc.text_dim))
                        .frame(false)
                        .min_size(egui::vec2(14.0, 14.0)),
                )
                .on_hover_text(hint)
                .clicked()
            {
                if any_open {
                    // No hardcoded id list, so the per-layer groups cannot be missed.
                    state.expanded_target_groups.clear();
                } else {
                    for opt in targets {
                        state
                            .expanded_target_groups
                            .insert(opt.group.as_ref().to_string());
                    }
                }
            }
        });
    });
    ui.add_space(2.0);
    draw_filter_box(ui, &mut state.target_filter, "matrix_target_filter");
    ui.add_space(4.0);

    // A live filter opens every group with a hit — otherwise a match could sit
    // inside a collapsed group and the column would look empty.
    let filtering = !state.target_filter.trim().is_empty();

    ScrollArea::vertical()
        .id_salt("matrix_targets")
        .show(ui, |ui| {
            state.target_viewport = ui.clip_rect();
            ui.set_width(ui.available_width());

            let mut pending_bind: Option<(String, String)> = None;
            let mut current_group: &str = "";

            // Build reverse map: old-format target → new-format target id
            // e.g. "param.Phosphor.warp" → "param.0.Phosphor.warp"
            let old_to_new: HashMap<String, &str> = targets
                .iter()
                .filter_map(|opt| {
                    // New format: param.{idx}.{effect}.{name}
                    let parts: Vec<&str> = opt.id.split('.').collect();
                    if parts.len() == 4 && parts[0] == "param" {
                        // Old format would be: param.{effect}.{name}
                        let old_key = format!("{}.{}.{}", parts[0], parts[2], parts[3]);
                        Some((old_key, opt.id.as_str()))
                    } else {
                        None
                    }
                })
                .collect();

            // Build lookup: target_id → list of source colors bound to it
            // Checks both the exact binding target and the new-format equivalent
            let mut target_bindings: HashMap<&str, Vec<Color32>> = HashMap::new();
            for b in &bus.bindings {
                if !b.target.is_empty() {
                    let color = source_color(&b.source);
                    target_bindings
                        .entry(b.target.as_str())
                        .or_default()
                        .push(color);
                    // Also index under the new-format key so target dots light up
                    if let Some(&new_id) = old_to_new.get(&b.target) {
                        target_bindings.entry(new_id).or_default().push(color);
                    }
                }
            }

            let mut last_header_rect: Option<Rect> = None;

            // Groups with no surviving row are dropped entirely while filtering,
            // so the column shows hits rather than a list of empty headers.
            let visible: Vec<&TargetOption> = targets
                .iter()
                .filter(|opt| target_matches(opt, &state.target_filter))
                .collect();
            if filtering && visible.is_empty() {
                ui.add_space(6.0);
                ui.label(
                    RichText::new("no targets match")
                        .size(8.0)
                        .color(tc.text_dim),
                );
            }

            for opt in visible {
                if opt.group != current_group {
                    current_group = opt.group.as_ref();
                    let collapsed =
                        !filtering && !state.expanded_target_groups.contains(current_group);
                    let caret = if collapsed { "\u{25b6}" } else { "\u{25bc}" };

                    // Full-width clickable header bar
                    let avail_w = ui.available_width();
                    let (hdr_rect, hdr_resp) =
                        ui.allocate_exact_size(egui::vec2(avail_w, 20.0), Sense::click());
                    let hovered = hdr_resp.hovered();

                    let bg = if hovered {
                        tc.hover_fill
                    } else {
                        Color32::TRANSPARENT
                    };
                    ui.painter().rect(
                        hdr_rect,
                        3.0,
                        bg,
                        Stroke::new(
                            1.0_f32,
                            if hovered {
                                tc.card_border
                            } else {
                                tc.hover_border
                            },
                        ),
                        StrokeKind::Inside,
                    );

                    let cy = hdr_rect.center().y;
                    let mut x = hdr_rect.left() + 6.0;

                    ui.painter().text(
                        Pos2::new(x, cy),
                        egui::Align2::LEFT_CENTER,
                        caret,
                        egui::FontId::proportional(7.0),
                        tc.text_secondary,
                    );
                    x += 12.0;

                    let label_galley = ui.painter().layout_no_wrap(
                        current_group.to_string(),
                        egui::FontId::proportional(9.0),
                        tc.text_primary,
                    );
                    ui.painter().galley(
                        Pos2::new(x, cy - label_galley.size().y * 0.5),
                        label_galley,
                        tc.text_primary,
                    );

                    if hdr_resp.clicked() {
                        if state.expanded_target_groups.contains(current_group) {
                            state.expanded_target_groups.remove(current_group);
                        } else {
                            state
                                .expanded_target_groups
                                .insert(current_group.to_string());
                        }
                    }
                    if hovered {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }

                    last_header_rect = Some(hdr_rect);
                }

                if !filtering && !state.expanded_target_groups.contains(opt.group.as_ref()) {
                    // Record position at group header for collapsed targets
                    if let Some(hr) = last_header_rect {
                        let anchor = pos2(hr.left(), hr.center().y);
                        let parts: Vec<&str> = opt.id.split('.').collect();
                        if parts.len() == 4 && parts[0] == "param" {
                            let old_key = format!("{}.{}.{}", parts[0], parts[2], parts[3]);
                            state.target_positions.insert(old_key, anchor);
                        }
                        state.target_positions.insert(opt.id.clone(), anchor);
                    }
                    continue;
                }

                let colors = target_bindings.get(opt.id.as_str());
                let is_bound = colors.is_some_and(|c| !c.is_empty());
                let armed_here = state.armed.as_ref() == Some(&ArmedEnd::Target(opt.id.clone()));

                let row_resp = ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;

                    // Multi-source colored dots
                    if let Some(colors) = colors {
                        for (i, &c) in colors.iter().take(4).enumerate() {
                            let (dot_rect, _) =
                                ui.allocate_exact_size(egui::vec2(5.0, 5.0), Sense::hover());
                            ui.painter().circle_filled(
                                dot_rect.center(),
                                2.0,
                                c.linear_multiply(0.8),
                            );
                            if i > 0 {
                                // tiny overlap offset handled by spacing
                            }
                        }
                    } else {
                        let (dot_rect, _) =
                            ui.allocate_exact_size(egui::vec2(5.0, 5.0), Sense::hover());
                        ui.painter()
                            .circle_filled(dot_rect.center(), 2.0, tc.text_dim);
                    }

                    // Label
                    let label_color = if is_bound {
                        tc.text_primary
                    } else {
                        tc.text_secondary
                    };
                    ui.label(RichText::new(&opt.label).size(9.0).color(label_color));

                    // Output bar — show last value from any binding targeting this
                    // Match both new-format (param.0.Effect.name) and old-format (param.Effect.name)
                    let old_format_id = {
                        let p: Vec<&str> = opt.id.split('.').collect();
                        if p.len() == 4 && p[0] == "param" {
                            Some(format!("{}.{}.{}", p[0], p[2], p[3]))
                        } else {
                            None
                        }
                    };
                    let output_val = bus
                        .bindings
                        .iter()
                        .filter(|b| {
                            b.enabled
                                && (b.target == opt.id
                                    || old_format_id.as_deref() == Some(b.target.as_str()))
                        })
                        .filter_map(|b| bus.runtime(&b.id).and_then(|r| r.last_output))
                        .next_back()
                        .unwrap_or(0.0);
                    if output_val > 0.001 || is_bound {
                        let (bar_rect, _) =
                            ui.allocate_exact_size(egui::vec2(28.0, 3.0), Sense::hover());
                        ui.painter().rect_filled(bar_rect, 1.0, tc.meter_bg);
                        let fill_w = 28.0 * output_val.clamp(0.0, 1.0);
                        if fill_w > 0.5 {
                            let fill_rect =
                                egui::Rect::from_min_size(bar_rect.min, egui::vec2(fill_w, 3.0));
                            ui.painter().rect_filled(fill_rect, 1.0, tc.text_secondary);
                        }
                    }
                });

                // Clicking a target arms it, or completes a binding from an armed
                // source. This row used to be a read-only meter — the only way in
                // was two unfiltered combo boxes inside a card.
                let rect = row_resp.response.rect;
                let click = ui.interact(
                    rect,
                    ui.make_persistent_id(("matrix_target_row", &opt.id)),
                    Sense::click(),
                );
                if armed_here || click.hovered() {
                    let fill = if armed_here {
                        tc.accent.linear_multiply(0.22)
                    } else {
                        tc.hover_fill
                    };
                    ui.painter()
                        .rect_filled(rect.expand2(egui::vec2(2.0, 1.0)), 3.0, fill);
                }
                if click.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                if click.clicked() {
                    match state.armed.clone() {
                        Some(ArmedEnd::Source(src)) => {
                            pending_bind = Some((src, opt.id.clone()));
                            state.armed = None;
                        }
                        Some(ArmedEnd::Target(t)) if t == opt.id => state.armed = None,
                        _ => state.armed = Some(ArmedEnd::Target(opt.id.clone())),
                    }
                }

                // Record position: left-center of the row
                let pos = pos2(rect.left(), rect.center().y);
                // Also store old-format key so bezier lines find old bindings
                let parts: Vec<&str> = opt.id.split('.').collect();
                if parts.len() == 4 && parts[0] == "param" {
                    let old_key = format!("{}.{}.{}", parts[0], parts[2], parts[3]);
                    state.target_positions.insert(old_key, pos);
                }
                state.target_positions.insert(opt.id.clone(), pos);
            }
            pending_bind
        })
        .inner
}

// ---------------------------------------------------------------------------
// Center column — binding cards
// ---------------------------------------------------------------------------

fn draw_center_column(
    ui: &mut egui::Ui,
    state: &mut BindingMatrixState,
    bus: &mut BindingBus,
    info: &BindingPanelInfo,
    targets: &[TargetOption],
) {
    let tc = theme_colors(ui.ctx());

    ui.label(
        RichText::new("BINDINGS")
            .size(8.0)
            .strong()
            .color(tc.text_secondary),
    );
    ui.add_space(4.0);

    // Pending delete state for two-stage confirmation
    let now = ui.input(|i| i.time);
    let pending_delete: Option<(String, f64)> = ui
        .ctx()
        .data_mut(|d| d.get_temp(Id::new("pending_delete_binding")));
    let pending_delete = pending_delete.filter(|(_, t)| now - *t < 3.0);
    let mut new_pending: Option<(String, f64)> = pending_delete.clone();

    ScrollArea::vertical()
        .id_salt("matrix_bindings")
        .show(ui, |ui| {
            state.card_viewport = ui.clip_rect();
            ui.set_width(ui.available_width());

            let scope_filter = match state.scope_tab {
                ScopeTab::Effect => BindingScope::Preset,
                ScopeTab::Global => BindingScope::Global,
            };

            // Group cards by target (untargeted last) so multiple bindings
            // driving one parameter sit together; ties keep id order.
            let mut binding_meta: Vec<(bool, String, String)> = bus
                .bindings
                .iter()
                .filter(|b| b.scope == scope_filter)
                .map(|b| (b.target.is_empty(), b.target.clone(), b.id.clone()))
                .collect();
            binding_meta.sort();
            let binding_ids: Vec<String> = binding_meta.into_iter().map(|(_, _, id)| id).collect();

            if binding_ids.is_empty() {
                ui.add_space(20.0);
                ui.label(
                    RichText::new("No bindings in this scope")
                        .size(10.0)
                        .color(tc.text_secondary),
                );
                ui.add_space(8.0);
            }

            let expanded_id = state.expanded_binding_id.clone();
            let mut to_remove: Vec<String> = Vec::new();

            for id in &binding_ids {
                let is_expanded = expanded_id.as_deref() == Some(id.as_str());
                let is_armed = new_pending.as_ref().map_or(false, |(pid, _)| pid == id);
                let result =
                    draw_binding_card(ui, state, bus, id, is_expanded, is_armed, info, targets);
                match result {
                    CardAction::Expand => {
                        state.expanded_binding_id = Some(id.clone());
                    }
                    CardAction::Collapse => {
                        state.expanded_binding_id = None;
                    }
                    CardAction::Delete => {
                        to_remove.push(id.clone());
                        new_pending = None;
                    }
                    CardAction::ArmDelete => {
                        new_pending = Some((id.clone(), now));
                    }
                    CardAction::Duplicate => {
                        if let Some(new_id) = bus.duplicate_binding(id) {
                            state.expanded_binding_id = Some(new_id);
                        }
                    }
                    CardAction::None => {}
                }
            }

            for id in to_remove {
                bus.remove_binding(&id);
                if state.expanded_binding_id.as_deref() == Some(id.as_str()) {
                    state.expanded_binding_id = None;
                }
            }

            // "+ New Binding" dashed button
            ui.add_space(8.0);
            let btn_rect = ui.available_rect_before_wrap();
            let btn_rect = Rect::from_min_size(btn_rect.min, egui::vec2(btn_rect.width(), 32.0));
            let btn_resp = ui.allocate_rect(btn_rect, Sense::click());
            let btn_fill = if btn_resp.hovered() {
                tc.hover_fill
            } else {
                Color32::TRANSPARENT
            };
            ui.painter().rect(
                btn_rect,
                6.0,
                btn_fill,
                Stroke::new(
                    1.0_f32,
                    if btn_resp.hovered() {
                        tc.card_border
                    } else {
                        tc.hover_border
                    },
                ),
                StrokeKind::Outside,
            );
            ui.painter().text(
                btn_rect.center(),
                egui::Align2::CENTER_CENTER,
                "+ New Binding",
                egui::FontId::proportional(10.0),
                if btn_resp.hovered() {
                    tc.text_primary
                } else {
                    tc.text_secondary
                },
            );
            if btn_resp.clicked() {
                let scope = match state.scope_tab {
                    ScopeTab::Effect => BindingScope::Preset,
                    ScopeTab::Global => BindingScope::Global,
                };
                let new_id = bus.add_binding(String::new(), String::new(), scope);
                state.expanded_binding_id = Some(new_id);
            }
        });

    // Persist pending delete state
    ui.ctx().data_mut(|d| {
        if let Some(pd) = new_pending {
            d.insert_temp(Id::new("pending_delete_binding"), pd);
        } else {
            d.remove_temp::<(String, f64)>(Id::new("pending_delete_binding"));
        }
    });
}

enum CardAction {
    None,
    Expand,
    Collapse,
    Delete,
    ArmDelete,
    Duplicate,
}

/// Short icon for a transform type (matching JSX mockup).
fn transform_icon(t: &TransformDef) -> &'static str {
    match t {
        TransformDef::Smooth { .. } => "~",
        TransformDef::Remap { .. } => "\u{2194}", // ↔
        TransformDef::Gate { .. } => "\u{25a3}",  // ▣  (was ⊞)
        TransformDef::Invert => "\u{21c5}",       // ⇅  (was ⊘)
        TransformDef::Quantize { .. } => "#",
        TransformDef::Deadzone { .. } => "\u{2013}", // –
        TransformDef::Scale { .. } => "\u{00d7}",    // ×
        TransformDef::Clamp { .. } => "[ ]",
        TransformDef::Offset { .. } => "+",
        TransformDef::Curve { .. } => "S",
    }
}

fn transform_tooltip(t: &TransformDef) -> &'static str {
    match t {
        TransformDef::Smooth { .. } => {
            "Exponential smoothing (low-pass filter). Higher factor = smoother/slower."
        }
        TransformDef::Remap { .. } => "Remap input range to output range (linear interpolation).",
        TransformDef::Gate { .. } => "Output 0 when input is below threshold, pass through above.",
        TransformDef::Invert => "Flip the value: output = 1 - input.",
        TransformDef::Quantize { .. } => "Snap to N discrete steps (staircase).",
        TransformDef::Deadzone { .. } => "Output 0 when input is between lo and hi.",
        TransformDef::Scale { .. } => "Multiply the value by a constant factor.",
        TransformDef::Clamp { .. } => "Clamp output to [lo, hi] range.",
        TransformDef::Offset { .. } => "Add a constant offset to the value.",
        TransformDef::Curve { .. } => "Apply an easing curve to shape the response.",
    }
}

/// Short label for a transform type (no params).
fn transform_type_label(t: &TransformDef) -> &'static str {
    match t {
        TransformDef::Smooth { .. } => "Smooth",
        TransformDef::Remap { .. } => "Remap",
        TransformDef::Gate { .. } => "Gate",
        TransformDef::Invert => "Invert",
        TransformDef::Quantize { .. } => "Quantize",
        TransformDef::Deadzone { .. } => "Deadzone",
        TransformDef::Scale { .. } => "Scale",
        TransformDef::Clamp { .. } => "Clamp",
        TransformDef::Offset { .. } => "Offset",
        TransformDef::Curve { .. } => "Curve",
    }
}

#[allow(dead_code)]
fn transform_detail(t: &TransformDef) -> String {
    match t {
        TransformDef::Smooth { factor } => format!("{factor:.1}"),
        TransformDef::Remap { out_lo, out_hi, .. } => format!("{out_lo:.1}\u{2013}{out_hi:.1}"),
        TransformDef::Gate { threshold } => format!(">{threshold:.1}"),
        TransformDef::Scale { factor } => format!("\u{00d7}{factor:.1}"),
        TransformDef::Quantize { steps } => format!("{steps}"),
        TransformDef::Clamp { lo, hi } => format!("{lo:.1}\u{2013}{hi:.1}"),
        TransformDef::Deadzone { lo, hi } => format!("{lo:.2}\u{2013}{hi:.2}"),
        TransformDef::Offset { value } => format!("{value:+.2}"),
        TransformDef::Curve { curve_type } => curve_type.clone(),
        TransformDef::Invert => String::new(),
    }
}

/// Helper: tint a source color to an RGBA with given alpha byte.
fn src_rgba(color: Color32, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
}

/// Draw a unified binding card (compact + optionally expanded).
fn draw_binding_card(
    ui: &mut egui::Ui,
    state: &mut BindingMatrixState,
    bus: &mut BindingBus,
    id: &str,
    expanded: bool,
    is_armed: bool,
    _info: &BindingPanelInfo,
    targets: &[TargetOption],
) -> CardAction {
    let tc = theme_colors(ui.ctx());
    let mut action = CardAction::None;

    let Some(binding) = bus.get_binding(id) else {
        return CardAction::None;
    };

    let src_color = source_color(&binding.source);
    let runtime = bus.runtime(id);
    let last_output = runtime.and_then(|r| r.last_output).unwrap_or(0.0);
    let last_input = runtime.and_then(|r| r.last_input).unwrap_or(0.0);
    let enabled = binding.enabled;
    let binding_source = binding.source.clone();
    let binding_target = binding.target.clone();
    let binding_name = binding.name.clone();
    let binding_scope = binding.scope.clone();
    let binding_transforms = binding.transforms.clone();

    // Card container: source-colored background & border
    // Expanded: fully opaque panel bg + tint overlay
    // Collapsed: semi-transparent panel bg + subtle tint
    let border_color = if expanded {
        src_rgba(src_color, 80) // ${color}50
    } else {
        src_rgba(src_color, 32) // ${color}20
    };

    let frame = egui::Frame::new()
        .fill(Color32::TRANSPARENT)
        .corner_radius(6.0)
        .stroke(Stroke::new(1.0_f32, border_color))
        .inner_margin(egui::Margin::ZERO);

    // Paint card background before frame content
    let card_bg_rect_idx = ui.painter().add(egui::Shape::Noop);

    let frame_resp = frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        if !enabled {
            ui.multiply_opacity(0.4);
        }

        // ─── Compact row (always visible) ───
        let compact_resp = ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let margin = egui::Margin::symmetric(10, 6);
            ui.add_space(margin.left as f32);

            // Enable dot (clickable)
            let dot_size = 8.0;
            let (dot_rect, dot_resp) =
                ui.allocate_exact_size(egui::vec2(dot_size, dot_size), Sense::click());
            if enabled {
                // Filled dot with glow
                ui.painter()
                    .circle_filled(dot_rect.center(), 3.5, src_color);
                ui.painter().circle_stroke(
                    dot_rect.center(),
                    4.0,
                    Stroke::new(1.0_f32, src_rgba(src_color, 100)),
                );
            } else {
                ui.painter().circle_stroke(
                    dot_rect.center(),
                    3.5,
                    Stroke::new(1.0_f32, tc.text_dim),
                );
            }
            if dot_resp.clicked() {
                if let Some(b) = bus.get_binding_mut(id) {
                    b.enabled = !b.enabled;
                }
            }
            dot_resp.on_hover_text(if enabled { "Disable" } else { "Enable" });

            // Identity: the custom name, or the auto "Source \u{2192} Target".
            // Without this an unnamed collapsed card reads as just its pills.
            let display = if binding_name.is_empty() {
                make_display_name(&binding_source, &binding_target)
            } else {
                binding_name.clone()
            };
            ui.label(
                RichText::new(crate::ui::widgets::truncate_chars(&display, 24))
                    .size(9.0)
                    .strong()
                    .color(tc.text_primary),
            )
            .on_hover_text(&display);

            // Dead-source warning: an enabled binding whose source key isn't in
            // the live snapshot (unplugged device, un-substituted template).
            if enabled
                && !binding_source.is_empty()
                && !bus.last_snapshot.contains_key(&binding_source)
            {
                ui.label(RichText::new("\u{26a0}").size(8.0).color(tc.warning))
                    .on_hover_text(
                        "Source not currently available \u{2014} check the device or re-Learn",
                    );
            }

            // Transform chain summary pills
            if binding_transforms.is_empty() {
                ui.label(RichText::new("passthrough").size(8.0).color(tc.text_dim));
            } else {
                for t in &binding_transforms {
                    let icon = transform_icon(t);
                    let label = transform_type_label(t);
                    // Colored pill: icon + label
                    let pill_text = format!("{icon} {label}");
                    ui.add(
                        egui::Button::new(
                            RichText::new(pill_text)
                                .size(8.0)
                                .color(src_rgba(src_color, 170)),
                        )
                        .fill(src_rgba(src_color, 20))
                        .corner_radius(2.0)
                        .min_size(egui::vec2(0.0, 16.0))
                        .sense(Sense::hover()),
                    );
                }
            }

            // Right-aligned: chevron + input value → [transforms] → output bar + value
            // (right-to-left layout, so output first, then input)
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(10.0);

                // Collapse/expand chevron
                let chevron = if expanded { "\u{25b2}" } else { "\u{25bc}" };
                ui.label(RichText::new(chevron).size(7.0).color(tc.text_secondary));

                // Output value + bar
                if enabled && last_output > 0.001 {
                    ui.label(
                        RichText::new(format!("{:.2}", last_output))
                            .size(8.0)
                            .color(src_rgba(src_color, 145)),
                    );

                    let (bar_rect, _) =
                        ui.allocate_exact_size(egui::vec2(28.0, 3.0), Sense::hover());
                    ui.painter().rect_filled(bar_rect, 2.0, tc.meter_bg);
                    let fill_w = 28.0 * last_output.clamp(0.0, 1.0);
                    if fill_w > 0.5 {
                        let fill_rect =
                            egui::Rect::from_min_size(bar_rect.min, egui::vec2(fill_w, 3.0));
                        ui.painter().rect_filled(fill_rect, 2.0, src_color);
                    }
                }

                ui.add_space(6.0);

                // Input value (dimmer, to the left of transforms)
                if enabled && last_input > 0.001 {
                    ui.label(
                        RichText::new(format!("{:.2}", last_input))
                            .size(8.0)
                            .color(tc.text_dim),
                    );

                    let (bar_rect, _) =
                        ui.allocate_exact_size(egui::vec2(20.0, 3.0), Sense::hover());
                    ui.painter().rect_filled(bar_rect, 2.0, tc.hover_border);
                    let fill_w = 20.0 * last_input.clamp(0.0, 1.0);
                    if fill_w > 0.5 {
                        let fill_rect =
                            egui::Rect::from_min_size(bar_rect.min, egui::vec2(fill_w, 3.0));
                        ui.painter().rect_filled(fill_rect, 2.0, tc.text_dim);
                    }
                }
            });
        });

        // Click compact row to expand/collapse
        if compact_resp.response.interact(Sense::click()).clicked() {
            action = if expanded {
                CardAction::Collapse
            } else {
                CardAction::Expand
            };
        }

        // ─── Expanded details ───
        if expanded {
            // Separator line
            let sep_rect = ui.available_rect_before_wrap();
            let sep_y = sep_rect.min.y;
            ui.painter().line_segment(
                [
                    pos2(sep_rect.min.x + 10.0, sep_y),
                    pos2(sep_rect.max.x - 10.0, sep_y),
                ],
                Stroke::new(1.0_f32, src_rgba(src_color, 32)),
            );

            ui.add_space(4.0);

            // Expanded content with padding
            let expanded_margin = egui::Margin::symmetric(10, 4);
            egui::Frame::new()
                .inner_margin(expanded_margin)
                .show(ui, |ui| {
                    draw_expanded_content(
                        ui,
                        state,
                        bus,
                        id,
                        &binding_source,
                        &binding_target,
                        &binding_name,
                        enabled,
                        &binding_scope,
                        &binding_transforms,
                        src_color,
                        targets,
                        &tc,
                        is_armed,
                        &mut action,
                    );
                });

            ui.add_space(6.0);
        }
    });

    // Paint card background (behind frame content via reserved index)
    {
        let card_rect = frame_resp.response.rect;
        if expanded {
            // Opaque panel bg + source tint
            ui.painter().set(
                card_bg_rect_idx,
                egui::Shape::rect_filled(card_rect, 6.0, tc.panel),
            );
            // Tint overlay on top (painted after, so it's above the opaque bg)
            ui.painter()
                .rect_filled(card_rect, 6.0, src_rgba(src_color, 20));
        } else {
            // Semi-transparent panel bg + subtle tint
            let semi_panel =
                Color32::from_rgba_unmultiplied(tc.panel.r(), tc.panel.g(), tc.panel.b(), 128);
            ui.painter().set(
                card_bg_rect_idx,
                egui::Shape::rect_filled(card_rect, 6.0, semi_panel),
            );
            ui.painter()
                .rect_filled(card_rect, 6.0, src_rgba(src_color, 8));
        }
    }

    if frame_resp.response.contains_pointer() {
        state.hovered_binding_id = Some(id.to_string());
    }

    // Record card position for bezier lines
    let rect = frame_resp.response.rect;
    state.card_positions.insert(
        id.to_string(),
        (
            pos2(rect.left(), rect.center().y),
            pos2(rect.right(), rect.center().y),
        ),
    );

    ui.add_space(4.0);
    action
}

#[allow(clippy::too_many_arguments)]
fn draw_expanded_content(
    ui: &mut egui::Ui,
    state: &mut BindingMatrixState,
    bus: &mut BindingBus,
    id: &str,
    source_init: &str,
    target_init: &str,
    name_init: &str,
    enabled: bool,
    _scope_init: &BindingScope,
    transforms: &[TransformDef],
    src_color: Color32,
    targets: &[TargetOption],
    tc: &crate::ui::theme::colors::ThemeColors,
    is_armed: bool,
    action: &mut CardAction,
) {
    let mut source = source_init.to_string();
    let mut target = target_init.to_string();
    let mut name = name_init.to_string();
    let mut enabled_val = enabled;

    // ─── Top row: name + collapse + delete ───
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let auto_name = make_display_name(&source, &target);
        ui.add(
            egui::TextEdit::singleline(&mut name)
                .desired_width(140.0)
                .font(egui::TextStyle::Small)
                .hint_text(&auto_name),
        );
        ui.checkbox(&mut enabled_val, RichText::new("Enabled").size(8.0));

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // Delete — two-stage confirmation
            let warning_color = Color32::from_rgb(0xE0, 0x60, 0x60);
            let (del_label, del_color) = if is_armed {
                ("Confirm?", warning_color)
            } else {
                ("Delete", warning_color)
            };
            if ui
                .add(
                    egui::Button::new(RichText::new(del_label).size(8.0).color(del_color))
                        .fill(if is_armed {
                            Color32::from_rgba_unmultiplied(0xE0, 0x60, 0x60, 30)
                        } else {
                            Color32::TRANSPARENT
                        })
                        .frame(is_armed),
                )
                .clicked()
            {
                if is_armed {
                    *action = CardAction::Delete;
                } else {
                    *action = CardAction::ArmDelete;
                }
            }

            if ui
                .add(
                    egui::Button::new(
                        RichText::new("Duplicate")
                            .size(8.0)
                            .color(tc.text_secondary),
                    )
                    .frame(false),
                )
                .on_hover_text("Clone this binding (same source/target/transforms)")
                .clicked()
            {
                *action = CardAction::Duplicate;
            }
        });
    });

    ui.add_space(6.0);

    // ─── Pickers row: Source | Target (above the dark inset) ───
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        // Source picker
        draw_matrix_source_picker(ui, state, bus, id, &mut source, tc);

        ui.label(RichText::new("\u{2192}").size(10.0).color(tc.text_dim));

        // Target picker
        let total_w = ui.available_width();
        let current_label = target_display_label(&target, targets);
        egui::ComboBox::from_id_salt(format!("matrix_target_{id}"))
            .selected_text(RichText::new(&current_label).size(9.0))
            .width((total_w - 10.0).min(200.0))
            .show_ui(ui, |ui| {
                // Same filter the target column uses, so this popup narrows too
                // instead of re-listing every param of every effect on every layer.
                draw_filter_box(ui, &mut state.target_filter, "matrix_tpicker_filter");
                ui.add_space(2.0);

                let mut current_group: &str = "";
                let mut any = false;
                for opt in targets
                    .iter()
                    .filter(|o| target_matches(o, &state.target_filter))
                {
                    any = true;
                    if opt.group != current_group {
                        current_group = opt.group.as_ref();
                        ui.label(
                            RichText::new(current_group)
                                .size(8.0)
                                .strong()
                                .color(tc.text_secondary),
                        );
                    }
                    let selected = target == opt.id;
                    if ui
                        .selectable_label(selected, RichText::new(&opt.label).size(9.0))
                        .clicked()
                    {
                        target = opt.id.clone();
                    }
                }
                if !any {
                    ui.label(
                        RichText::new("no targets match")
                            .size(8.0)
                            .color(tc.text_dim),
                    );
                }
            });
    });

    ui.add_space(4.0);

    // ─── Full-width dark inset: Source preview | Transforms | Output ───
    let inset_frame = egui::Frame::new()
        .fill(tc.meter_bg)
        .corner_radius(egui::CornerRadius {
            sw: 6,
            se: 6,
            ..Default::default()
        })
        .inner_margin(egui::Margin::symmetric(8, 6));
    inset_frame.show(ui, |ui| {
        ui.set_width(ui.available_width());

        // Three columns inside the inset
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            let total_w = ui.available_width();
            let source_w = 160.0;
            let target_w = 120.0;
            let transforms_w = (total_w - source_w - target_w - 16.0).max(160.0);

            // ── Source preview ──
            ui.vertical(|ui| {
                ui.set_width(source_w);
                if let Some(runtime) = bus.runtime(id) {
                    if let Some(ref raw) = runtime.last_raw {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Raw").size(7.0).color(tc.text_dim));
                            ui.label(
                                RichText::new(&raw.display)
                                    .size(8.0)
                                    .color(tc.text_secondary),
                            );
                        });
                    }
                    if let Some(input) = runtime.last_input {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Norm").size(7.0).color(tc.text_dim));
                            draw_inline_bar(ui, input, 40.0, 3.0, tc.text_dim, tc.meter_bg);
                            ui.label(
                                RichText::new(format!("{:.3}", input))
                                    .size(8.0)
                                    .color(tc.text_secondary),
                            );
                        });
                    }
                } else {
                    ui.label(RichText::new("--").size(8.0).color(tc.text_dim));
                }
            });

            ui.add_space(8.0);

            // ── Transforms ──
            ui.vertical(|ui| {
                ui.set_width(transforms_w);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Transforms")
                            .size(8.0)
                            .strong()
                            .color(tc.text_secondary),
                    );

                    // "+ Transform" add button
                    let add_resp = ui.add(
                        egui::Button::new(
                            RichText::new("+ Add").size(7.0).color(tc.text_secondary),
                        )
                        .fill(tc.hover_fill)
                        .corner_radius(3.0)
                        .min_size(egui::vec2(0.0, 16.0)),
                    );

                    let popup_id = Id::new(format!("matrix_add_xform_popup_{id}"));
                    egui::Popup::from_toggle_button_response(&add_resp)
                        .id(popup_id)
                        .close_behavior(egui::PopupCloseBehavior::CloseOnClick)
                        .show(|ui| {
                            ui.set_min_width(110.0);
                            let items = [
                                ("~ Smooth", TransformDef::Smooth { factor: 0.8 }),
                                (
                                    "\u{2194} Remap",
                                    TransformDef::Remap {
                                        in_lo: 0.0,
                                        in_hi: 1.0,
                                        out_lo: 0.0,
                                        out_hi: 1.0,
                                    },
                                ),
                                ("\u{25a3} Gate", TransformDef::Gate { threshold: 0.5 }),
                                ("\u{00d7} Scale", TransformDef::Scale { factor: 1.0 }),
                                ("+ Offset", TransformDef::Offset { value: 0.0 }),
                                ("# Quantize", TransformDef::Quantize { steps: 4 }),
                                ("\u{21c5} Invert", TransformDef::Invert),
                                ("[ ] Clamp", TransformDef::Clamp { lo: 0.0, hi: 1.0 }),
                                (
                                    "\u{2013} Deadzone",
                                    TransformDef::Deadzone { lo: 0.45, hi: 0.55 },
                                ),
                                (
                                    "S Curve",
                                    TransformDef::Curve {
                                        curve_type: "ease_in_out".into(),
                                    },
                                ),
                            ];
                            for (label, default) in items {
                                if ui
                                    .add(
                                        egui::Button::new(
                                            RichText::new(label).size(9.0).color(tc.text_primary),
                                        )
                                        .frame(false)
                                        .min_size(egui::vec2(100.0, 22.0)),
                                    )
                                    .clicked()
                                {
                                    if let Some(b) = bus.get_binding_mut(id) {
                                        b.transforms.push(default);
                                    }
                                }
                            }
                        });
                });

                ui.add_space(2.0);

                let label_w = 60.0;
                if transforms.is_empty() {
                    ui.label(RichText::new("passthrough").size(8.0).color(tc.text_dim));
                } else {
                    let mut to_remove: Option<usize> = None;
                    for (i, t) in transforms.iter().enumerate() {
                        let icon = transform_icon(t);
                        let label = transform_type_label(t);
                        let tooltip = transform_tooltip(t);

                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;

                            // Fixed-width icon + label column
                            let (label_rect, _) =
                                ui.allocate_exact_size(egui::vec2(label_w, 18.0), Sense::hover());
                            let label_resp = ui.interact(
                                label_rect,
                                Id::new(format!("xf_label_{id}_{i}")),
                                Sense::hover(),
                            );
                            ui.painter().text(
                                Pos2::new(label_rect.left() + 2.0, label_rect.center().y),
                                egui::Align2::LEFT_CENTER,
                                format!("{icon} {label}"),
                                egui::FontId::proportional(8.0),
                                tc.text_secondary,
                            );
                            label_resp.on_hover_text(tooltip);

                            // Editable params
                            draw_transform_params_inline(ui, bus, id, i, t);

                            // × remove button (right aligned)
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui
                                        .add(
                                            egui::Button::new(
                                                RichText::new("\u{00d7}")
                                                    .size(9.0)
                                                    .color(tc.text_dim),
                                            )
                                            .frame(false)
                                            .min_size(egui::vec2(12.0, 12.0)),
                                        )
                                        .on_hover_text("Remove")
                                        .clicked()
                                    {
                                        to_remove = Some(i);
                                    }
                                },
                            );
                        });
                        // Remap second line: out_lo, out_hi
                        if matches!(t, TransformDef::Remap { .. }) {
                            let remap_out: Option<(f32, f32)> = ui.memory(|m| {
                                m.data.get_temp(Id::new(format!("remap_out_{id}_{i}")))
                            });
                            if let Some((out_lo, out_hi)) = remap_out {
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 4.0;
                                    ui.add_space(label_w + 4.0);
                                    let small_drag = |ui: &mut egui::Ui,
                                                      val: f32,
                                                      lo: f32,
                                                      hi: f32,
                                                      lbl: &str|
                                     -> f32 {
                                        let mut v = val;
                                        ui.add(
                                            egui::DragValue::new(&mut v)
                                                .range(lo..=hi)
                                                .speed(0.01)
                                                .max_decimals(2)
                                                .prefix(format!("{lbl}: "))
                                                .min_decimals(1)
                                                .update_while_editing(true),
                                        );
                                        v
                                    };
                                    let new_olo = small_drag(ui, out_lo, 0.0, 1.0, "out_lo");
                                    let new_ohi = small_drag(ui, out_hi, 0.0, 1.0, "out_hi");
                                    if (new_olo - out_lo).abs() > 0.001
                                        || (new_ohi - out_hi).abs() > 0.001
                                    {
                                        if let Some(b) = bus.get_binding_mut(id) {
                                            if let TransformDef::Remap {
                                                ref mut out_lo,
                                                ref mut out_hi,
                                                ..
                                            } = b.transforms[i]
                                            {
                                                *out_lo = new_olo;
                                                *out_hi = new_ohi;
                                            }
                                        }
                                    }
                                });
                            }
                        }
                    }
                    if let Some(idx) = to_remove {
                        if let Some(b) = bus.get_binding_mut(id) {
                            b.transforms.remove(idx);
                        }
                    }
                }
            });

            ui.add_space(8.0);

            // ── Output preview ──
            ui.vertical(|ui| {
                ui.set_width(target_w);
                if let Some(runtime) = bus.runtime(id) {
                    if let Some(output) = runtime.last_output {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Out").size(7.0).color(tc.text_dim));
                            draw_inline_bar(ui, output, 40.0, 3.0, src_color, tc.meter_bg);
                            ui.label(
                                RichText::new(format!("{:.3}", output))
                                    .size(8.0)
                                    .color(src_color),
                            );
                        });
                    }
                } else {
                    ui.label(RichText::new("--").size(8.0).color(tc.text_dim));
                }
            });
        });
    });

    // Apply changes back — only on a real change: get_binding_mut marks the
    // bus dirty, and an unconditional write meant a rewrite of the bindings
    // file every debounce interval for as long as a card sat expanded.
    if source != source_init || target != target_init || name != name_init || enabled_val != enabled
    {
        if let Some(b) = bus.get_binding_mut(id) {
            b.source = source;
            b.target = target;
            b.name = name;
            b.enabled = enabled_val;
        }
    }
}

/// Draw inline editable params for a transform
fn draw_transform_params_inline(
    ui: &mut egui::Ui,
    bus: &mut BindingBus,
    binding_id: &str,
    transform_idx: usize,
    t: &TransformDef,
) {
    let small_drag = |ui: &mut egui::Ui, val: f32, lo: f32, hi: f32, label: &str| -> f32 {
        let mut v = val;
        ui.add(
            egui::DragValue::new(&mut v)
                .range(lo..=hi)
                .speed(0.01)
                .max_decimals(2)
                .prefix(format!("{label}: "))
                .min_decimals(1)
                .update_while_editing(true),
        )
        .on_hover_text(label);
        v
    };

    let small_drag_int = |ui: &mut egui::Ui, val: u32, lo: u32, hi: u32, label: &str| -> u32 {
        let mut v = val as i32;
        ui.add(
            egui::DragValue::new(&mut v)
                .range(lo as i32..=hi as i32)
                .speed(0.1)
                .prefix(format!("{label}: "))
                .update_while_editing(true),
        )
        .on_hover_text(label);
        v.max(lo as i32) as u32
    };

    match t {
        TransformDef::Smooth { factor } => {
            let new_f = small_drag(ui, *factor, 0.0, 1.0, "factor");
            if (new_f - factor).abs() > 0.001 {
                if let Some(b) = bus.get_binding_mut(binding_id) {
                    b.transforms[transform_idx] = TransformDef::Smooth { factor: new_f };
                }
            }
        }
        TransformDef::Gate { threshold } => {
            let new_t = small_drag(ui, *threshold, 0.0, 1.0, "thresh");
            if (new_t - threshold).abs() > 0.001 {
                if let Some(b) = bus.get_binding_mut(binding_id) {
                    b.transforms[transform_idx] = TransformDef::Gate { threshold: new_t };
                }
            }
        }
        TransformDef::Scale { factor } => {
            let new_f = small_drag(ui, *factor, -10.0, 10.0, "factor");
            if (new_f - factor).abs() > 0.001 {
                if let Some(b) = bus.get_binding_mut(binding_id) {
                    b.transforms[transform_idx] = TransformDef::Scale { factor: new_f };
                }
            }
        }
        TransformDef::Offset { value } => {
            let new_v = small_drag(ui, *value, -10.0, 10.0, "value");
            if (new_v - value).abs() > 0.001 {
                if let Some(b) = bus.get_binding_mut(binding_id) {
                    b.transforms[transform_idx] = TransformDef::Offset { value: new_v };
                }
            }
        }
        TransformDef::Quantize { steps } => {
            let new_s = small_drag_int(ui, *steps, 2, 128, "steps");
            if new_s != *steps {
                if let Some(b) = bus.get_binding_mut(binding_id) {
                    b.transforms[transform_idx] = TransformDef::Quantize { steps: new_s };
                }
            }
        }
        TransformDef::Clamp { lo, hi } => {
            let new_lo = small_drag(ui, *lo, 0.0, 1.0, "lo");
            let new_hi = small_drag(ui, *hi, 0.0, 1.0, "hi");
            if (new_lo - lo).abs() > 0.001 || (new_hi - hi).abs() > 0.001 {
                if let Some(b) = bus.get_binding_mut(binding_id) {
                    b.transforms[transform_idx] = TransformDef::Clamp {
                        lo: new_lo,
                        hi: new_hi,
                    };
                }
            }
        }
        TransformDef::Deadzone { lo, hi } => {
            let new_lo = small_drag(ui, *lo, 0.0, 1.0, "lo");
            let new_hi = small_drag(ui, *hi, 0.0, 1.0, "hi");
            if (new_lo - lo).abs() > 0.001 || (new_hi - hi).abs() > 0.001 {
                if let Some(b) = bus.get_binding_mut(binding_id) {
                    b.transforms[transform_idx] = TransformDef::Deadzone {
                        lo: new_lo,
                        hi: new_hi,
                    };
                }
            }
        }
        TransformDef::Remap {
            in_lo,
            in_hi,
            out_lo,
            out_hi,
        } => {
            let new_ilo = small_drag(ui, *in_lo, 0.0, 1.0, "in_lo");
            let new_ihi = small_drag(ui, *in_hi, 0.0, 1.0, "in_hi");
            // out_lo / out_hi drawn on second line via remap_second_line flag
            if (new_ilo - in_lo).abs() > 0.001 || (new_ihi - in_hi).abs() > 0.001 {
                if let Some(b) = bus.get_binding_mut(binding_id) {
                    if let TransformDef::Remap {
                        ref mut in_lo,
                        ref mut in_hi,
                        ..
                    } = b.transforms[transform_idx]
                    {
                        *in_lo = new_ilo;
                        *in_hi = new_ihi;
                    }
                }
            }
            // Store out values for second line
            ui.memory_mut(|m| {
                m.data.insert_temp(
                    Id::new(format!("remap_out_{binding_id}_{transform_idx}")),
                    (*out_lo, *out_hi),
                );
            });
        }
        TransformDef::Curve { curve_type } => {
            // Sourced from transforms.rs so the picker cannot offer a curve
            // `apply_curve` does not implement — four of the seven it used to list
            // evaluated to identity.
            let curves = crate::bindings::transforms::CURVE_TYPES;
            let mut selected = curve_type.clone();
            egui::ComboBox::from_id_salt(format!("xf_curve_{binding_id}_{transform_idx}"))
                .selected_text(RichText::new(&selected).size(8.0))
                .width(100.0)
                .show_ui(ui, |ui| {
                    for &ct in curves {
                        if ui
                            .selectable_label(selected == ct, RichText::new(ct).size(8.0))
                            .clicked()
                        {
                            selected = ct.to_string();
                        }
                    }
                });
            if selected != *curve_type {
                if let Some(b) = bus.get_binding_mut(binding_id) {
                    b.transforms[transform_idx] = TransformDef::Curve {
                        curve_type: selected,
                    };
                }
            }
        }
        TransformDef::Invert => {
            // No params
        }
    }
}

// ---------------------------------------------------------------------------
// Matrix source picker (simplified for the expanded card)
// ---------------------------------------------------------------------------

fn draw_matrix_source_picker(
    ui: &mut egui::Ui,
    state: &mut BindingMatrixState,
    bus: &mut BindingBus,
    id: &str,
    source: &mut String,
    _tc: &crate::ui::theme::colors::ThemeColors,
) {
    ui.horizontal(|ui| {
        let current_display = if source.is_empty() {
            "(select source)".to_string()
        } else {
            friendly_source_label(source)
        };

        egui::ComboBox::from_id_salt(format!("matrix_source_{id}"))
            .selected_text(RichText::new(&current_display).size(9.0))
            .width(200.0)
            .height(350.0)
            .show_ui(ui, |ui| {
                ui.set_min_width(260.0);
                ui.spacing_mut().item_spacing.y = 1.0;

                // Same filter the column uses, so typing "kick" here narrows the
                // popup instead of leaving ~330 rows to scroll.
                draw_filter_box(ui, &mut state.source_filter, "matrix_picker_filter");
                ui.add_space(2.0);

                let tc = theme_colors(ui.ctx());
                let mut any = false;
                // One list, shared with the column — see all_source_groups.
                for group in all_source_groups(&bus.last_snapshot) {
                    let keys = filter_source_keys(&group, &state.source_filter);
                    if keys.is_empty() {
                        continue;
                    }
                    any = true;
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(&group.label)
                            .size(7.0)
                            .strong()
                            .color(group.color.linear_multiply(0.7)),
                    );
                    ui.add_space(1.0);
                    for key in keys {
                        let val = bus.last_snapshot.get(key).map(|(v, _)| *v).unwrap_or(0.0);
                        let info = audio_source_info(key);
                        let friendly = friendly_source_label(key);
                        let uniform = if key.starts_with("audio.") {
                            info.uniform
                        } else {
                            String::new()
                        };
                        draw_source_row(
                            ui,
                            key,
                            &friendly,
                            &uniform,
                            val,
                            group.color,
                            source.as_str() == key,
                            source,
                        );
                    }
                }
                if !any {
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new("no sources match")
                            .size(8.0)
                            .color(tc.text_dim),
                    );
                }
            });

        // Learn button
        let is_learning = bus
            .learn_target
            .as_ref()
            .is_some_and(|l| l.binding_id == id && l.field == LearnField::Source);
        if is_learning {
            let t = ui.input(|i| i.time) as f32;
            let alpha = ((t * 4.0).sin() * 0.3 + 0.7).clamp(0.4, 1.0);
            let color = Color32::from_rgba_unmultiplied(0xE0, 0xA0, 0x40, (alpha * 255.0) as u8);
            if ui
                .add(egui::Button::new(
                    RichText::new("..").color(color).size(9.0),
                ))
                .on_hover_text("Cancel learn")
                .clicked()
            {
                bus.learn_target = None;
            }
            ui.ctx().request_repaint();
        } else if ui
            .add(egui::Button::new(RichText::new("Learn").size(9.0)))
            .on_hover_text("Learn from next MIDI/OSC")
            .clicked()
        {
            bus.learn_target = Some(LearnState {
                binding_id: id.to_string(),
                field: LearnField::Source,
            });
        }
    });
}

// ---------------------------------------------------------------------------
// Connection lines (bezier curves)
// ---------------------------------------------------------------------------

fn draw_connections(ctx: &Context, state: &BindingMatrixState, bus: &BindingBus) {
    let tc = theme_colors(ctx);
    let line_layer = egui::LayerId::new(Order::Foreground, Id::new("matrix_lines"));
    // Hard-stop all cables at the column area (union covers the inter-column
    // gaps the curves cross) so nothing paints over the header or footer.
    let clip = state
        .source_viewport
        .union(state.card_viewport)
        .union(state.target_viewport)
        .expand(4.0);
    let painter = ctx.layer_painter(line_layer).with_clip_rect(clip);

    let hovered_id = state.hovered_binding_id.as_deref();
    for binding in &bus.bindings {
        let src_pos = state.source_positions.get(&binding.source);
        let card_pos = state.card_positions.get(&binding.id);
        let tgt_pos = state.target_positions.get(&binding.target);

        let src_color = source_color(&binding.source);
        let is_hovered = hovered_id == Some(binding.id.as_str());
        // Hovering a card spotlights its cables and dims every other cable.
        let (stroke_width, alpha) = if is_hovered {
            (2.0_f32, 0.9_f32)
        } else if binding.enabled {
            (1.5_f32, if hovered_id.is_some() { 0.15 } else { 0.6 })
        } else {
            (1.0_f32, if hovered_id.is_some() { 0.06 } else { 0.12 })
        };

        let stroke_color = if binding.enabled {
            Color32::from_rgba_unmultiplied(
                src_color.r(),
                src_color.g(),
                src_color.b(),
                (alpha * 255.0) as u8,
            )
        } else {
            Color32::from_rgba_unmultiplied(
                tc.text_dim.r(),
                tc.text_dim.g(),
                tc.text_dim.b(),
                (alpha * 255.0) as u8,
            )
        };
        let stroke = Stroke::new(stroke_width, stroke_color);

        // A clamped (scrolled-off) endpoint draws as a faded stub — mirrors the
        // collapsed-group behavior of snapping to a fixed anchor — and its
        // segment skips the flow dot (an animated dot on a stub reads wrong).
        let faded = Stroke::new(stroke_width, stroke_color.gamma_multiply(0.4));

        // Source → card left edge
        if let (Some(&from), Some(&(card_left, _))) = (src_pos, card_pos) {
            let (from_c, fe) = clamp_endpoint(from, state.source_viewport);
            let (to_c, te) = clamp_endpoint(card_left, state.card_viewport);
            // Both endpoints off the same edge: nothing of the curve is visible.
            if fe == 0 || fe != te {
                let visible = fe == 0 && te == 0;
                draw_bezier_connection(
                    &painter,
                    from_c,
                    to_c,
                    if visible { stroke } else { faded },
                );

                // Flow dot for enabled bindings with nonzero output
                if visible && binding.enabled {
                    let runtime = bus.runtime(&binding.id);
                    let output = runtime.and_then(|r| r.last_output).unwrap_or(0.0);
                    if output > 0.001 {
                        let t = state.flow_phase;
                        let dot_pos = eval_cubic_bezier(from_c, to_c, t);
                        painter.circle_filled(dot_pos, 3.0, src_color.linear_multiply(0.8));
                    }
                }
            }
        }

        // Card right edge → target
        if let (Some(&(_, card_right)), Some(&to)) = (card_pos, tgt_pos) {
            let (from_c, fe) = clamp_endpoint(card_right, state.card_viewport);
            let (to_c, te) = clamp_endpoint(to, state.target_viewport);
            if fe == 0 || fe != te {
                let visible = fe == 0 && te == 0;
                draw_bezier_connection(
                    &painter,
                    from_c,
                    to_c,
                    if visible { stroke } else { faded },
                );

                // Flow dot
                if visible && binding.enabled {
                    let runtime = bus.runtime(&binding.id);
                    let output = runtime.and_then(|r| r.last_output).unwrap_or(0.0);
                    if output > 0.001 {
                        let t = (state.flow_phase + 0.3) % 1.0;
                        let dot_pos = eval_cubic_bezier(from_c, to_c, t);
                        painter.circle_filled(dot_pos, 3.0, src_color.linear_multiply(0.8));
                    }
                }
            }
        }
    }
}

/// Clamp a curve endpoint's y into its column's visible band.
/// Returns the clamped point and where the original sat: -1 above, 0 inside,
/// +1 below the viewport.
fn clamp_endpoint(pos: Pos2, viewport: Rect) -> (Pos2, i8) {
    let top = viewport.top() + 4.0;
    let bottom = viewport.bottom() - 4.0;
    if pos.y < top {
        (pos2(pos.x, top), -1)
    } else if pos.y > bottom {
        (pos2(pos.x, bottom), 1)
    } else {
        (pos, 0)
    }
}

fn draw_bezier_connection(painter: &egui::Painter, from: Pos2, to: Pos2, stroke: Stroke) {
    let dx = (to.x - from.x).abs() * 0.4;
    let cp1 = pos2(from.x + dx, from.y);
    let cp2 = pos2(to.x - dx, to.y);
    painter.add(CubicBezierShape::from_points_stroke(
        [from, cp1, cp2, to],
        false,
        Color32::TRANSPARENT,
        stroke,
    ));
}

fn eval_cubic_bezier(from: Pos2, to: Pos2, t: f32) -> Pos2 {
    let dx = (to.x - from.x).abs() * 0.4;
    let cp1 = pos2(from.x + dx, from.y);
    let cp2 = pos2(to.x - dx, to.y);

    let u = 1.0 - t;
    let tt = t * t;
    let uu = u * u;
    let uuu = uu * u;
    let ttt = tt * t;

    pos2(
        uuu * from.x + 3.0 * uu * t * cp1.x + 3.0 * u * tt * cp2.x + ttt * to.x,
        uuu * from.y + 3.0 * uu * t * cp1.y + 3.0 * u * tt * cp2.y + ttt * to.y,
    )
}

// ---------------------------------------------------------------------------
// Footer
// ---------------------------------------------------------------------------

fn draw_footer(ui: &mut egui::Ui, state: &BindingMatrixState, bus: &BindingBus) {
    let tc = theme_colors(ui.ctx());
    let unique_targets: HashSet<&str> = bus
        .bindings
        .iter()
        .filter(|b| !b.target.is_empty())
        .map(|b| b.target.as_str())
        .collect();

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!(
                "{} sources \u{00b7} {} bindings \u{00b7} {} targets",
                bus.last_snapshot.len(),
                bus.bindings.len(),
                unique_targets.len(),
            ))
            .size(9.0)
            .color(tc.text_secondary),
        );
        // Click-to-bind is only discoverable if the half-finished state says so.
        if let Some(armed) = &state.armed {
            let (what, name) = match armed {
                ArmedEnd::Source(s) => ("source", friendly_source_label(s)),
                ArmedEnd::Target(t) => ("target", friendly_target(t)),
            };
            let other = if matches!(armed, ArmedEnd::Source(_)) {
                "target"
            } else {
                "source"
            };
            ui.label(
                RichText::new(format!(
                    "\u{2014}  {what} \u{201c}{name}\u{201d} armed \u{00b7} pick a {other} to bind, Esc to cancel"
                ))
                .size(9.0)
                .color(tc.accent),
            );
        }
    });
}
