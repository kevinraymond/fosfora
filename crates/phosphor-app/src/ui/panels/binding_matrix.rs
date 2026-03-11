use std::collections::{HashMap, HashSet};

use egui::{
    epaint::CubicBezierShape, pos2, Color32, Context, Id, Order, Pos2, Rect, RichText,
    ScrollArea, Sense, Stroke, StrokeKind,
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

pub struct BindingMatrixState {
    pub open: bool,
    pub scope_tab: ScopeTab,
    pub collapsed_source_groups: HashSet<String>,
    pub collapsed_target_groups: HashSet<String>,
    pub expanded_binding_id: Option<String>,
    // Position tracking for connection lines (rebuilt each frame)
    pub source_positions: HashMap<String, Pos2>,
    pub target_positions: HashMap<String, Pos2>,
    pub card_positions: HashMap<String, (Pos2, Pos2)>,
    pub flow_phase: f32,
    /// Cached texture handles for WS bridge preview thumbnails.
    pub preview_textures: HashMap<String, egui::TextureHandle>,
}

impl BindingMatrixState {
    pub fn new() -> Self {
        Self {
            open: false,
            scope_tab: ScopeTab::Effect,
            collapsed_source_groups: HashSet::new(),
            collapsed_target_groups: HashSet::new(),
            expanded_binding_id: None,
            source_positions: HashMap::new(),
            target_positions: HashMap::new(),
            card_positions: HashMap::new(),
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

    #[allow(deprecated)]
    let screen = ctx.input(|i| i.screen_rect());
    let tc = theme_colors(ctx);

    // Backdrop — paint a dimming rectangle over the whole screen
    let backdrop_layer = egui::LayerId::new(Order::Middle, Id::new("matrix_backdrop"));
    let painter = ctx.layer_painter(backdrop_layer);
    painter.rect_filled(screen, 0.0, tc.backdrop);

    // Escape closes
    let esc_pressed = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    if esc_pressed {
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

                // Three columns
                let avail = ui.available_size();
                let col_source_w = 240.0;
                let col_target_w = 240.0;
                let col_center_w = (avail.x - col_source_w - col_target_w - 24.0).max(200.0);

                // Fully opaque side panels (tc.panel may have ~90% alpha)
                let side_bg = Color32::from_rgb(tc.panel.r(), tc.panel.g(), tc.panel.b());
                let side_pad = 6.0;

                ui.horizontal(|ui| {
                    // Left: Sources — padded with bg
                    let left_bg_idx = ui.painter().add(egui::Shape::Noop);
                    let left_resp = ui.vertical(|ui| {
                        ui.set_width(col_source_w + side_pad * 2.0);
                        ui.set_height(avail.y - 60.0);
                        ui.add_space(side_pad);
                        ui.indent(Id::new("src_pad"), |ui| {
                            ui.set_width(col_source_w);
                            draw_source_column(ui, state, bus);
                        });
                    });
                    ui.painter().set(left_bg_idx, egui::Shape::rect_filled(
                        left_resp.response.rect, 6.0, side_bg,
                    ));

                    ui.add_space(8.0);

                    // Center: Binding cards (semi-transparent bg)
                    let center_bg_idx = ui.painter().add(egui::Shape::Noop);
                    let center_resp = ui.vertical(|ui| {
                        ui.set_width(col_center_w);
                        ui.set_height(avail.y - 60.0);
                        draw_center_column(ui, state, bus, info);
                    });
                    ui.painter().set(center_bg_idx, egui::Shape::rect_filled(
                        center_resp.response.rect, 6.0,
                        Color32::from_rgba_unmultiplied(tc.panel.r(), tc.panel.g(), tc.panel.b(), 128),
                    ));

                    ui.add_space(8.0);

                    // Right: Targets — padded with bg
                    let right_bg_idx = ui.painter().add(egui::Shape::Noop);
                    let right_resp = ui.vertical(|ui| {
                        ui.set_width(col_target_w + side_pad * 2.0);
                        ui.set_height(avail.y - 60.0);
                        ui.add_space(side_pad);
                        ui.indent(Id::new("tgt_pad"), |ui| {
                            ui.set_width(col_target_w);
                            draw_target_column(ui, state, bus, info);
                        });
                    });
                    ui.painter().set(right_bg_idx, egui::Shape::rect_filled(
                        right_resp.response.rect, 6.0, side_bg,
                    ));
                });

                // Footer
                draw_footer(ui, bus);
            });
        });

    // Close on click outside the content area
    let clicked_outside = ctx.input(|i| {
        i.pointer.any_click()
            && i.pointer
                .interact_pos()
                .is_some_and(|pos| !area_resp.response.rect.contains(pos))
    });
    if clicked_outside {
        state.open = false;
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
                ui.label(
                    RichText::new(label)
                        .size(8.0)
                        .color(tc.text_secondary),
                );
            });
        };
        legend(ui, AUDIO_COLOR, "Audio");
        legend(ui, MIDI_COLOR, "MIDI");
        legend(ui, OSC_COLOR, "OSC");
        legend(ui, WS_COLOR, "WS");

        ui.add_space(12.0);

        // Scope tabs
        let tab_btn = |ui: &mut egui::Ui, label: &str, active: bool| -> bool {
            let color = if active { tc.text_primary } else { tc.text_secondary };
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
        if tab_btn(ui, "Effect", state.scope_tab == ScopeTab::Effect) {
            state.scope_tab = ScopeTab::Effect;
        }
        if tab_btn(ui, "Global", state.scope_tab == ScopeTab::Global) {
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
                        bus.apply_template(tmpl, info.active_effect_name(), info.active_param_names());
                    }
                }
            });

        // Right: close button
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add(
                    egui::Button::new(RichText::new("\u{00d7}").size(16.0).color(tc.text_secondary))
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

fn draw_source_column(
    ui: &mut egui::Ui,
    state: &mut BindingMatrixState,
    bus: &BindingBus,
) {
    let tc = theme_colors(ui.ctx());

    ui.horizontal(|ui| {
        ui.label(
            RichText::new("SOURCES")
                .size(8.0)
                .strong()
                .color(tc.text_secondary),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let all_collapsed = !state.collapsed_source_groups.is_empty();
            let icon = if all_collapsed { "\u{25bc}" } else { "\u{25b6}" };
            let hint = if all_collapsed { "Expand all" } else { "Collapse all" };
            if ui.add(egui::Button::new(RichText::new(icon).size(7.0).color(tc.text_dim))
                .frame(false).min_size(egui::vec2(14.0, 14.0)))
                .on_hover_text(hint)
                .clicked()
            {
                if all_collapsed {
                    state.collapsed_source_groups.clear();
                } else {
                    for id in ["audio_bands", "audio_features", "audio_beat", "audio_mfcc", "audio_chroma", "midi", "osc"] {
                        state.collapsed_source_groups.insert(id.to_string());
                    }
                    for key in bus.last_snapshot.keys() {
                        if let Some(rest) = key.strip_prefix("ws.") {
                            if let Some(name) = rest.split('.').next() {
                                state.collapsed_source_groups.insert(format!("ws.{name}"));
                            }
                        }
                    }
                }
            }
        });
    });
    ui.add_space(4.0);

    ScrollArea::vertical()
        .id_salt("matrix_sources")
        .show(ui, |ui| {
            ui.set_width(ui.available_width());

            // Build source groups
            let bound_sources: HashSet<&str> = bus
                .bindings
                .iter()
                .filter(|b| !b.source.is_empty())
                .map(|b| b.source.as_str())
                .collect();

            // Audio groups
            let audio_groups: &[(&str, &str, &[&str])] = &[
                (
                    "Audio \u{00b7} Bands",
                    "audio_bands",
                    &[
                        "audio.band.0",
                        "audio.band.1",
                        "audio.band.2",
                        "audio.band.3",
                        "audio.band.4",
                        "audio.band.5",
                        "audio.band.6",
                        "audio.rms",
                    ],
                ),
                (
                    "Audio \u{00b7} Features",
                    "audio_features",
                    &[
                        "audio.kick",
                        "audio.centroid",
                        "audio.flux",
                        "audio.flatness",
                        "audio.rolloff",
                        "audio.bandwidth",
                        "audio.zcr",
                    ],
                ),
                (
                    "Audio \u{00b7} Beat",
                    "audio_beat",
                    &[
                        "audio.onset",
                        "audio.beat",
                        "audio.beat_phase",
                        "audio.bpm",
                        "audio.beat_strength",
                    ],
                ),
            ];

            for &(group_label, group_id, keys) in audio_groups {
                let mapped_count = keys.iter().filter(|k| bound_sources.contains(*k)).count();
                draw_source_group(
                    ui,
                    state,
                    bus,
                    group_label,
                    group_id,
                    AUDIO_COLOR,
                    keys,
                    mapped_count,
                    &bound_sources,
                );
            }

            // MFCC (dynamic)
            let mut mfcc_keys: Vec<String> = bus
                .last_snapshot
                .keys()
                .filter(|k| k.starts_with("audio.mfcc."))
                .cloned()
                .collect();
            if !mfcc_keys.is_empty() {
                mfcc_keys.sort_by_key(|k| {
                    k.strip_prefix("audio.mfcc.")
                        .and_then(|n| n.parse::<u32>().ok())
                        .unwrap_or(99)
                });
                let mfcc_refs: Vec<&str> = mfcc_keys.iter().map(|s| s.as_str()).collect();
                let mapped = mfcc_refs
                    .iter()
                    .filter(|k| bound_sources.contains(*k))
                    .count();
                draw_source_group(
                    ui,
                    state,
                    bus,
                    "Audio \u{00b7} MFCC",
                    "audio_mfcc",
                    AUDIO_COLOR,
                    &mfcc_refs,
                    mapped,
                    &bound_sources,
                );
            }

            // Chroma (dominant + individual notes)
            {
                let mut chroma_keys: Vec<String> = Vec::new();
                // dominant_chroma first
                if bus.last_snapshot.contains_key("audio.dominant_chroma") {
                    chroma_keys.push("audio.dominant_chroma".to_string());
                }
                // Then individual chroma bins
                let mut note_keys: Vec<String> = bus
                    .last_snapshot
                    .keys()
                    .filter(|k| k.starts_with("audio.chroma."))
                    .cloned()
                    .collect();
                note_keys.sort_by_key(|k| {
                    k.strip_prefix("audio.chroma.")
                        .and_then(|n| n.parse::<u32>().ok())
                        .unwrap_or(99)
                });
                chroma_keys.extend(note_keys);

                if !chroma_keys.is_empty() {
                    let chroma_refs: Vec<&str> = chroma_keys.iter().map(|s| s.as_str()).collect();
                    let mapped = chroma_refs
                        .iter()
                        .filter(|k| bound_sources.contains(*k))
                        .count();
                    draw_source_group(
                        ui,
                        state,
                        bus,
                        "Audio \u{00b7} Chroma",
                        "audio_chroma",
                        AUDIO_COLOR,
                        &chroma_refs,
                        mapped,
                        &bound_sources,
                    );
                }
            }

            // MIDI sources (dynamic)
            let mut midi_keys: Vec<String> = bus
                .last_snapshot
                .keys()
                .filter(|k| k.starts_with("midi."))
                .cloned()
                .collect();
            if !midi_keys.is_empty() {
                midi_keys.sort();
                let midi_refs: Vec<&str> = midi_keys.iter().map(|s| s.as_str()).collect();
                let mapped = midi_refs
                    .iter()
                    .filter(|k| bound_sources.contains(*k))
                    .count();
                draw_source_group(
                    ui,
                    state,
                    bus,
                    "MIDI",
                    "midi",
                    MIDI_COLOR,
                    &midi_refs,
                    mapped,
                    &bound_sources,
                );
            }

            // OSC sources (dynamic)
            let mut osc_keys: Vec<String> = bus
                .last_snapshot
                .keys()
                .filter(|k| k.starts_with("osc."))
                .cloned()
                .collect();
            if !osc_keys.is_empty() {
                osc_keys.sort();
                let osc_refs: Vec<&str> = osc_keys.iter().map(|s| s.as_str()).collect();
                let mapped = osc_refs
                    .iter()
                    .filter(|k| bound_sources.contains(*k))
                    .count();
                draw_source_group(
                    ui,
                    state,
                    bus,
                    "OSC",
                    "osc",
                    OSC_COLOR,
                    &osc_refs,
                    mapped,
                    &bound_sources,
                );
            }

            // WS sources (dynamic) — sub-grouped by source name
            {
                let mut ws_keys: Vec<String> = bus
                    .last_snapshot
                    .keys()
                    .filter(|k| k.starts_with("ws."))
                    .cloned()
                    .collect();
                ws_keys.sort();

                // Group by source name: ws.{source}.{field} → source
                let mut ws_groups: Vec<(String, Vec<String>)> = Vec::new();
                for key in &ws_keys {
                    let source_name = key
                        .strip_prefix("ws.")
                        .and_then(|rest| rest.split('.').next())
                        .unwrap_or("ws");
                    if ws_groups.last().is_some_and(|(name, _)| name == source_name) {
                        ws_groups.last_mut().unwrap().1.push(key.clone());
                    } else {
                        ws_groups.push((source_name.to_string(), vec![key.clone()]));
                    }
                }

                let active_sources: HashSet<String> = ws_groups.iter()
                    .map(|(name, _)| name.clone()).collect();

                for (source_name, keys) in &ws_groups {
                    let refs: Vec<&str> = keys.iter().map(|s| s.as_str()).collect();
                    let mapped = refs
                        .iter()
                        .filter(|k| bound_sources.contains(*k))
                        .count();
                    let label = ws_source_display_name(source_name);
                    let group_id = format!("ws.{source_name}");
                    draw_source_group(
                        ui,
                        state,
                        bus,
                        &label,
                        &group_id,
                        WS_COLOR,
                        &refs,
                        mapped,
                        &bound_sources,
                    );
                }

                // Clean up preview textures for expired sources
                state.preview_textures.retain(|k, _| active_sources.contains(k));
            }
        });
}

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
) {
    let tc = theme_colors(ui.ctx());
    let collapsed = state.collapsed_source_groups.contains(group_id);

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
        Stroke::new(1.0, if hovered { tc.card_border } else { tc.hover_border }),
        StrokeKind::Inside,
    );

    // Left color accent bar
    let accent_rect = Rect::from_min_size(
        header_rect.min,
        egui::vec2(3.0, header_rect.height()),
    );
    ui.painter().rect_filled(accent_rect, egui::CornerRadius { nw: 3, sw: 3, ..Default::default() }, color.linear_multiply(0.6));

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
    ui.painter().galley(Pos2::new(x, cy - label_galley.size().y * 0.5), label_galley, tc.text_primary);

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
        if collapsed {
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
            let val = bus
                .last_snapshot
                .get(key)
                .map(|(v, _)| *v)
                .unwrap_or(0.0);

            let info = audio_source_info(key);
            let friendly = if key.starts_with("audio.") {
                info.friendly
            } else {
                friendly_source(key)
            };

            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.add_space(12.0); // indent

                // Mapped dot
                let dot_color = if is_bound {
                    source_color(key)
                } else {
                    tc.text_dim
                };
                let (dot_rect, _) =
                    ui.allocate_exact_size(egui::vec2(6.0, 6.0), Sense::hover());
                ui.painter()
                    .circle_filled(dot_rect.center(), 2.5, dot_color);

                // Label
                let label_color = if is_bound {
                    tc.text_primary
                } else {
                    tc.text_secondary
                };
                ui.label(
                    RichText::new(&friendly).size(9.0).color(label_color),
                );

                // Mini bar (32x3)
                let (bar_rect, _) =
                    ui.allocate_exact_size(egui::vec2(32.0, 3.0), Sense::hover());
                ui.painter().rect_filled(
                    bar_rect,
                    1.0,
                    tc.meter_bg,
                );
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
        }
    }

    ui.add_space(2.0);
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
    let handle = state.preview_textures.entry(source_name.to_string()).or_insert_with(|| {
        ui.ctx().load_texture(&tex_id, color_image.clone(), egui::TextureOptions::LINEAR)
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

fn draw_target_column(
    ui: &mut egui::Ui,
    state: &mut BindingMatrixState,
    bus: &BindingBus,
    info: &BindingPanelInfo,
) {
    let tc = theme_colors(ui.ctx());

    ui.horizontal(|ui| {
        ui.label(
            RichText::new("TARGETS")
                .size(8.0)
                .strong()
                .color(tc.text_secondary),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let all_collapsed = !state.collapsed_target_groups.is_empty();
            let icon = if all_collapsed { "\u{25bc}" } else { "\u{25b6}" };
            let hint = if all_collapsed { "Expand all" } else { "Collapse all" };
            if ui.add(egui::Button::new(RichText::new(icon).size(7.0).color(tc.text_dim))
                .frame(false).min_size(egui::vec2(14.0, 14.0)))
                .on_hover_text(hint)
                .clicked()
            {
                if all_collapsed {
                    state.collapsed_target_groups.clear();
                } else {
                    for id in ["Params", "Layers", "PostFX", "Particles", "Uniforms", "Scene", "Global"] {
                        state.collapsed_target_groups.insert(id.to_string());
                    }
                }
            }
        });
    });
    ui.add_space(4.0);

    ScrollArea::vertical()
        .id_salt("matrix_targets")
        .show(ui, |ui| {
            ui.set_width(ui.available_width());

            let targets = build_target_options(info);
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
                        target_bindings
                            .entry(new_id)
                            .or_default()
                            .push(color);
                    }
                }
            }

            let mut last_header_rect: Option<Rect> = None;

            for opt in &targets {
                if opt.group != current_group {
                    current_group = opt.group;
                    let collapsed = state.collapsed_target_groups.contains(current_group);
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
                        Stroke::new(1.0, if hovered { tc.card_border } else { tc.hover_border }),
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
                        if collapsed {
                            state.collapsed_target_groups.remove(current_group);
                        } else {
                            state
                                .collapsed_target_groups
                                .insert(current_group.to_string());
                        }
                    }
                    if hovered {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }

                    last_header_rect = Some(hdr_rect);
                }

                if state.collapsed_target_groups.contains(opt.group) {
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
                        ui.painter().circle_filled(
                            dot_rect.center(),
                            2.0,
                            tc.text_dim,
                        );
                    }

                    // Label
                    let label_color = if is_bound {
                        tc.text_primary
                    } else {
                        tc.text_secondary
                    };
                    ui.label(
                        RichText::new(&opt.label).size(9.0).color(label_color),
                    );

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
                        .filter(|b| b.enabled && (b.target == opt.id || old_format_id.as_deref() == Some(b.target.as_str())))
                        .filter_map(|b| bus.runtime(&b.id).and_then(|r| r.last_output))
                        .last()
                        .unwrap_or(0.0);
                    if output_val > 0.001 || is_bound {
                        let (bar_rect, _) =
                            ui.allocate_exact_size(egui::vec2(28.0, 3.0), Sense::hover());
                        ui.painter().rect_filled(
                            bar_rect,
                            1.0,
                            tc.meter_bg,
                        );
                        let fill_w = 28.0 * output_val.clamp(0.0, 1.0);
                        if fill_w > 0.5 {
                            let fill_rect =
                                egui::Rect::from_min_size(bar_rect.min, egui::vec2(fill_w, 3.0));
                            ui.painter().rect_filled(
                                fill_rect,
                                1.0,
                                tc.text_secondary,
                            );
                        }
                    }
                });

                // Record position: left-center of the row
                let rect = row_resp.response.rect;
                let pos = pos2(rect.left(), rect.center().y);
                // Also store old-format key so bezier lines find old bindings
                let parts: Vec<&str> = opt.id.split('.').collect();
                if parts.len() == 4 && parts[0] == "param" {
                    let old_key = format!("{}.{}.{}", parts[0], parts[2], parts[3]);
                    state.target_positions.insert(old_key, pos);
                }
                state.target_positions.insert(opt.id.clone(), pos);
            }
        });
}

// ---------------------------------------------------------------------------
// Center column — binding cards
// ---------------------------------------------------------------------------

fn draw_center_column(
    ui: &mut egui::Ui,
    state: &mut BindingMatrixState,
    bus: &mut BindingBus,
    info: &BindingPanelInfo,
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
            ui.set_width(ui.available_width());

            let scope_filter = match state.scope_tab {
                ScopeTab::Effect => BindingScope::Preset,
                ScopeTab::Global => BindingScope::Global,
            };

            let binding_ids: Vec<String> = bus
                .bindings
                .iter()
                .filter(|b| b.scope == scope_filter)
                .map(|b| b.id.clone())
                .collect();

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
            let targets = build_target_options(info);
            let mut to_remove: Vec<String> = Vec::new();

            for id in &binding_ids {
                let is_expanded = expanded_id.as_deref() == Some(id.as_str());
                let is_armed = new_pending.as_ref().map_or(false, |(pid, _)| pid == id);
                let result = draw_binding_card(ui, state, bus, id, is_expanded, is_armed, info, &targets);
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
            let btn_rect = Rect::from_min_size(
                btn_rect.min,
                egui::vec2(btn_rect.width(), 32.0),
            );
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
                    1.0,
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
}

/// Short icon for a transform type (matching JSX mockup).
fn transform_icon(t: &TransformDef) -> &'static str {
    match t {
        TransformDef::Smooth { .. } => "~",
        TransformDef::Remap { .. } => "\u{2194}", // ↔
        TransformDef::Gate { .. } => "\u{25a3}",   // ▣  (was ⊞)
        TransformDef::Invert => "\u{21c5}",        // ⇅  (was ⊘)
        TransformDef::Quantize { .. } => "#",
        TransformDef::Deadzone { .. } => "\u{2013}", // –
        TransformDef::Scale { .. } => "\u{00d7}",   // ×
        TransformDef::Clamp { .. } => "[ ]",
        TransformDef::Offset { .. } => "+",
        TransformDef::Curve { .. } => "S",
    }
}

fn transform_tooltip(t: &TransformDef) -> &'static str {
    match t {
        TransformDef::Smooth { .. } => "Exponential smoothing (low-pass filter). Higher factor = smoother/slower.",
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
        src_rgba(src_color, 80)     // ${color}50
    } else {
        src_rgba(src_color, 32)     // ${color}20
    };

    let frame = egui::Frame::new()
        .fill(Color32::TRANSPARENT)
        .corner_radius(6.0)
        .stroke(Stroke::new(1.0, border_color))
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
                    Stroke::new(1.0, src_rgba(src_color, 100)),
                );
            } else {
                ui.painter().circle_stroke(
                    dot_rect.center(),
                    3.5,
                    Stroke::new(1.0, tc.text_dim),
                );
            }
            if dot_resp.clicked() {
                if let Some(b) = bus.get_binding_mut(id) {
                    b.enabled = !b.enabled;
                }
            }
            dot_resp.on_hover_text(if enabled { "Disable" } else { "Enable" });

            // Transform chain summary pills
            if binding_transforms.is_empty() {
                ui.label(
                    RichText::new("passthrough")
                        .size(8.0)
                        .color(tc.text_dim),
                );
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
                    ui.painter().rect_filled(
                        bar_rect,
                        2.0,
                        tc.meter_bg,
                    );
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
                    ui.painter().rect_filled(
                        bar_rect,
                        2.0,
                        tc.hover_border,
                    );
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
                Stroke::new(1.0, src_rgba(src_color, 32)),
            );

            ui.add_space(4.0);

            // Expanded content with padding
            let expanded_margin = egui::Margin::symmetric(10, 4);
            egui::Frame::new()
                .inner_margin(expanded_margin)
                .show(ui, |ui| {
                    draw_expanded_content(ui, state, bus, id, &binding_source, &binding_target,
                        &binding_name, enabled, &binding_scope, &binding_transforms,
                        src_color, targets, &tc, is_armed, &mut action);
                });

            ui.add_space(6.0);
        }
    });

    // Paint card background (behind frame content via reserved index)
    {
        let card_rect = frame_resp.response.rect;
        if expanded {
            // Opaque panel bg + source tint
            ui.painter().set(card_bg_rect_idx, egui::Shape::rect_filled(
                card_rect, 6.0, tc.panel,
            ));
            // Tint overlay on top (painted after, so it's above the opaque bg)
            ui.painter().rect_filled(card_rect, 6.0, src_rgba(src_color, 20));
        } else {
            // Semi-transparent panel bg + subtle tint
            let semi_panel = Color32::from_rgba_unmultiplied(
                tc.panel.r(), tc.panel.g(), tc.panel.b(), 128,
            );
            ui.painter().set(card_bg_rect_idx, egui::Shape::rect_filled(
                card_rect, 6.0, semi_panel,
            ));
            ui.painter().rect_filled(card_rect, 6.0, src_rgba(src_color, 8));
        }
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
    _state: &mut BindingMatrixState,
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
                    egui::Button::new(
                        RichText::new(del_label)
                            .size(8.0)
                            .color(del_color),
                    )
                    .fill(if is_armed { Color32::from_rgba_unmultiplied(0xE0, 0x60, 0x60, 30) } else { Color32::TRANSPARENT })
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
        });
    });

    ui.add_space(6.0);

    // ─── Pickers row: Source | Target (above the dark inset) ───
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        // Source picker
        draw_matrix_source_picker(ui, bus, id, &mut source, tc);

        ui.label(RichText::new("\u{2192}").size(10.0).color(tc.text_dim));

        // Target picker
        let total_w = ui.available_width();
        let current_label = target_display_label(&target, targets);
        egui::ComboBox::from_id_salt(format!("matrix_target_{id}"))
            .selected_text(RichText::new(&current_label).size(9.0))
            .width((total_w - 10.0).min(200.0))
            .show_ui(ui, |ui| {
                let mut current_group: &str = "";
                for opt in targets {
                    if opt.group != current_group {
                        current_group = opt.group;
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
            });
    });

    ui.add_space(4.0);

    // ─── Full-width dark inset: Source preview | Transforms | Output ───
    let inset_frame = egui::Frame::new()
        .fill(tc.meter_bg)
        .corner_radius(egui::CornerRadius { sw: 6, se: 6, ..Default::default() })
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
                            ui.label(RichText::new(&raw.display).size(8.0).color(tc.text_secondary));
                        });
                    }
                    if let Some(input) = runtime.last_input {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Norm").size(7.0).color(tc.text_dim));
                            draw_inline_bar(ui, input, 40.0, 3.0, tc.text_dim, tc.meter_bg);
                            ui.label(RichText::new(format!("{:.3}", input)).size(8.0).color(tc.text_secondary));
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
                    ui.label(RichText::new("Transforms").size(8.0).strong().color(tc.text_secondary));

                    // "+ Transform" add button
                    let add_resp = ui.add(
                        egui::Button::new(
                            RichText::new("+ Add")
                                .size(7.0)
                                .color(tc.text_secondary),
                        )
                        .fill(tc.hover_fill)
                        .corner_radius(3.0)
                        .min_size(egui::vec2(0.0, 16.0)),
                    );

                    let popup_id = Id::new(format!("matrix_add_xform_popup_{id}"));
                    if add_resp.clicked() {
                        #[allow(deprecated)]
                        ui.memory_mut(|m| m.toggle_popup(popup_id));
                    }
                    #[allow(deprecated)]
                    egui::popup_below_widget(ui, popup_id, &add_resp, egui::PopupCloseBehavior::CloseOnClick, |ui| {
                        ui.set_min_width(110.0);
                        let items = [
                            ("~ Smooth", TransformDef::Smooth { factor: 0.8 }),
                            ("\u{2194} Remap", TransformDef::Remap { in_lo: 0.0, in_hi: 1.0, out_lo: 0.0, out_hi: 1.0 }),
                            ("\u{25a3} Gate", TransformDef::Gate { threshold: 0.5 }),
                            ("\u{00d7} Scale", TransformDef::Scale { factor: 1.0 }),
                            ("+ Offset", TransformDef::Offset { value: 0.0 }),
                            ("# Quantize", TransformDef::Quantize { steps: 4 }),
                            ("\u{21c5} Invert", TransformDef::Invert),
                            ("[ ] Clamp", TransformDef::Clamp { lo: 0.0, hi: 1.0 }),
                            ("\u{2013} Deadzone", TransformDef::Deadzone { lo: 0.45, hi: 0.55 }),
                            ("S Curve", TransformDef::Curve { curve_type: "ease_in_out".into() }),
                        ];
                        for (label, default) in items {
                            if ui
                                .add(
                                    egui::Button::new(RichText::new(label).size(9.0).color(tc.text_primary))
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
                            let label_resp = ui.interact(label_rect, Id::new(format!("xf_label_{id}_{i}")), Sense::hover());
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
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
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
                            });
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
                                    let small_drag = |ui: &mut egui::Ui, val: f32, lo: f32, hi: f32, lbl: &str| -> f32 {
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
                                    if (new_olo - out_lo).abs() > 0.001 || (new_ohi - out_hi).abs() > 0.001 {
                                        if let Some(b) = bus.get_binding_mut(id) {
                                            if let TransformDef::Remap { ref mut out_lo, ref mut out_hi, .. } = b.transforms[i] {
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
                            ui.label(RichText::new(format!("{:.3}", output)).size(8.0).color(src_color));
                        });
                    }
                } else {
                    ui.label(RichText::new("--").size(8.0).color(tc.text_dim));
                }
            });
        });
    });

    // Apply changes back
    if let Some(b) = bus.get_binding_mut(id) {
        b.source = source;
        b.target = target;
        b.name = name;
        b.enabled = enabled_val;
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
        ).on_hover_text(label);
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
        ).on_hover_text(label);
        v.max(lo as i32) as u32
    };

    match t.clone() {
        TransformDef::Smooth { factor } => {
            let new_f = small_drag(ui, factor, 0.0, 1.0, "factor");
            if (new_f - factor).abs() > 0.001 {
                if let Some(b) = bus.get_binding_mut(binding_id) {
                    b.transforms[transform_idx] = TransformDef::Smooth { factor: new_f };
                }
            }
        }
        TransformDef::Gate { threshold } => {
            let new_t = small_drag(ui, threshold, 0.0, 1.0, "thresh");
            if (new_t - threshold).abs() > 0.001 {
                if let Some(b) = bus.get_binding_mut(binding_id) {
                    b.transforms[transform_idx] = TransformDef::Gate { threshold: new_t };
                }
            }
        }
        TransformDef::Scale { factor } => {
            let new_f = small_drag(ui, factor, -10.0, 10.0, "factor");
            if (new_f - factor).abs() > 0.001 {
                if let Some(b) = bus.get_binding_mut(binding_id) {
                    b.transforms[transform_idx] = TransformDef::Scale { factor: new_f };
                }
            }
        }
        TransformDef::Offset { value } => {
            let new_v = small_drag(ui, value, -10.0, 10.0, "value");
            if (new_v - value).abs() > 0.001 {
                if let Some(b) = bus.get_binding_mut(binding_id) {
                    b.transforms[transform_idx] = TransformDef::Offset { value: new_v };
                }
            }
        }
        TransformDef::Quantize { steps } => {
            let new_s = small_drag_int(ui, steps, 2, 128, "steps");
            if new_s != steps {
                if let Some(b) = bus.get_binding_mut(binding_id) {
                    b.transforms[transform_idx] = TransformDef::Quantize { steps: new_s };
                }
            }
        }
        TransformDef::Clamp { lo, hi } => {
            let new_lo = small_drag(ui, lo, 0.0, 1.0, "lo");
            let new_hi = small_drag(ui, hi, 0.0, 1.0, "hi");
            if (new_lo - lo).abs() > 0.001 || (new_hi - hi).abs() > 0.001 {
                if let Some(b) = bus.get_binding_mut(binding_id) {
                    b.transforms[transform_idx] = TransformDef::Clamp { lo: new_lo, hi: new_hi };
                }
            }
        }
        TransformDef::Deadzone { lo, hi } => {
            let new_lo = small_drag(ui, lo, 0.0, 1.0, "lo");
            let new_hi = small_drag(ui, hi, 0.0, 1.0, "hi");
            if (new_lo - lo).abs() > 0.001 || (new_hi - hi).abs() > 0.001 {
                if let Some(b) = bus.get_binding_mut(binding_id) {
                    b.transforms[transform_idx] = TransformDef::Deadzone { lo: new_lo, hi: new_hi };
                }
            }
        }
        TransformDef::Remap { in_lo, in_hi, out_lo, out_hi } => {
            let new_ilo = small_drag(ui, in_lo, 0.0, 1.0, "in_lo");
            let new_ihi = small_drag(ui, in_hi, 0.0, 1.0, "in_hi");
            // out_lo / out_hi drawn on second line via remap_second_line flag
            if (new_ilo - in_lo).abs() > 0.001 || (new_ihi - in_hi).abs() > 0.001 {
                if let Some(b) = bus.get_binding_mut(binding_id) {
                    if let TransformDef::Remap { ref mut in_lo, ref mut in_hi, .. } = b.transforms[transform_idx] {
                        *in_lo = new_ilo;
                        *in_hi = new_ihi;
                    }
                }
            }
            // Store out values for second line
            ui.memory_mut(|m| m.data.insert_temp(
                Id::new(format!("remap_out_{binding_id}_{transform_idx}")),
                (out_lo, out_hi),
            ));
        }
        TransformDef::Curve { ref curve_type } => {
            let curves = ["ease_in", "ease_out", "ease_in_out", "ease_in_quad", "ease_out_quad", "ease_in_cubic", "ease_out_cubic"];
            let mut selected = curve_type.clone();
            egui::ComboBox::from_id_salt(format!("xf_curve_{binding_id}_{transform_idx}"))
                .selected_text(RichText::new(&selected).size(8.0))
                .width(100.0)
                .show_ui(ui, |ui| {
                    for &ct in &curves {
                        if ui.selectable_label(selected == ct, RichText::new(ct).size(8.0)).clicked() {
                            selected = ct.to_string();
                        }
                    }
                });
            if selected != *curve_type {
                if let Some(b) = bus.get_binding_mut(binding_id) {
                    b.transforms[transform_idx] = TransformDef::Curve { curve_type: selected };
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
    bus: &mut BindingBus,
    id: &str,
    source: &mut String,
    _tc: &crate::ui::theme::colors::ThemeColors,
) {
    ui.horizontal(|ui| {
        let current_display = if source.is_empty() {
            "(select source)".to_string()
        } else if source.starts_with("audio.") {
            audio_source_info(source).friendly
        } else {
            friendly_source(source)
        };

        egui::ComboBox::from_id_salt(format!("matrix_source_{id}"))
            .selected_text(RichText::new(&current_display).size(9.0))
            .width(200.0)
            .height(350.0)
            .show_ui(ui, |ui| {
                ui.set_min_width(260.0);
                ui.spacing_mut().item_spacing.y = 1.0;

                let group_header = |ui: &mut egui::Ui, label: &str, color: Color32| {
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(label)
                            .size(7.0)
                            .strong()
                            .color(color.linear_multiply(0.7)),
                    );
                    ui.add_space(1.0);
                };

                let has_audio = bus.last_snapshot.keys().any(|k| k.starts_with("audio."));
                if has_audio {
                    let mut current_sub_group = "";
                    for &key in AUDIO_SOURCE_ORDER {
                        let info = audio_source_info(key);
                        if info.sub_group != current_sub_group {
                            current_sub_group = info.sub_group;
                            group_header(
                                ui,
                                &format!("Audio \u{2014} {current_sub_group}"),
                                AUDIO_COLOR,
                            );
                        }
                        let val = bus
                            .last_snapshot
                            .get(key)
                            .map(|(v, _)| *v)
                            .unwrap_or(0.0);
                        draw_source_row(
                            ui,
                            key,
                            &info.friendly,
                            &info.uniform,
                            val,
                            AUDIO_COLOR,
                            source.as_str() == key,
                            source,
                        );
                    }

                    // MFCC
                    let mut mfcc_keys: Vec<String> = bus
                        .last_snapshot
                        .keys()
                        .filter(|k| k.starts_with("audio.mfcc."))
                        .cloned()
                        .collect();
                    if !mfcc_keys.is_empty() {
                        mfcc_keys.sort_by_key(|k| {
                            k.strip_prefix("audio.mfcc.")
                                .and_then(|n| n.parse::<u32>().ok())
                                .unwrap_or(99)
                        });
                        group_header(ui, "Audio \u{2014} MFCC", AUDIO_COLOR);
                        for key in &mfcc_keys {
                            let info = audio_source_info(key);
                            let val = bus
                                .last_snapshot
                                .get(key.as_str())
                                .map(|(v, _)| *v)
                                .unwrap_or(0.0);
                            draw_source_row(
                                ui,
                                key,
                                &info.friendly,
                                &info.uniform,
                                val,
                                AUDIO_COLOR,
                                source.as_str() == key.as_str(),
                                source,
                            );
                        }
                    }

                    // Chroma
                    let mut chroma_keys: Vec<String> = bus
                        .last_snapshot
                        .keys()
                        .filter(|k| k.starts_with("audio.chroma."))
                        .cloned()
                        .collect();
                    if !chroma_keys.is_empty() {
                        chroma_keys.sort_by_key(|k| {
                            k.strip_prefix("audio.chroma.")
                                .and_then(|n| n.parse::<u32>().ok())
                                .unwrap_or(99)
                        });
                        group_header(ui, "Audio \u{2014} Chroma", AUDIO_COLOR);
                        for key in &chroma_keys {
                            let info = audio_source_info(key);
                            let val = bus
                                .last_snapshot
                                .get(key.as_str())
                                .map(|(v, _)| *v)
                                .unwrap_or(0.0);
                            draw_source_row(
                                ui,
                                key,
                                &info.friendly,
                                &info.uniform,
                                val,
                                AUDIO_COLOR,
                                source.as_str() == key.as_str(),
                                source,
                            );
                        }
                    }
                }

                // MIDI
                let mut midi_keys: Vec<&String> = bus
                    .last_snapshot
                    .keys()
                    .filter(|k| k.starts_with("midi."))
                    .collect();
                if !midi_keys.is_empty() {
                    midi_keys.sort();
                    group_header(ui, "MIDI", MIDI_COLOR);
                    for key in &midi_keys {
                        let val = bus
                            .last_snapshot
                            .get(key.as_str())
                            .map(|(v, _)| *v)
                            .unwrap_or(0.0);
                        let display = friendly_source(key);
                        draw_source_row(
                            ui,
                            key,
                            &display,
                            "",
                            val,
                            MIDI_COLOR,
                            source.as_str() == key.as_str(),
                            source,
                        );
                    }
                }

                // OSC
                let mut osc_keys: Vec<&String> = bus
                    .last_snapshot
                    .keys()
                    .filter(|k| k.starts_with("osc."))
                    .collect();
                if !osc_keys.is_empty() {
                    osc_keys.sort();
                    group_header(ui, "OSC", OSC_COLOR);
                    for key in &osc_keys {
                        let val = bus
                            .last_snapshot
                            .get(key.as_str())
                            .map(|(v, _)| *v)
                            .unwrap_or(0.0);
                        let display = friendly_source(key);
                        draw_source_row(
                            ui,
                            key,
                            &display,
                            "",
                            val,
                            OSC_COLOR,
                            source.as_str() == key.as_str(),
                            source,
                        );
                    }
                }

                // WS — sub-grouped by source name
                {
                    let mut ws_keys: Vec<&String> = bus
                        .last_snapshot
                        .keys()
                        .filter(|k| k.starts_with("ws."))
                        .collect();
                    ws_keys.sort();

                    let mut current_source: Option<&str> = None;
                    for key in &ws_keys {
                        let src = key
                            .strip_prefix("ws.")
                            .and_then(|rest| rest.split('.').next())
                            .unwrap_or("ws");
                        if current_source != Some(src) {
                            let label = ws_source_display_name(src);
                            group_header(ui, &label, WS_COLOR);
                            current_source = Some(src);
                        }
                        let val = bus
                            .last_snapshot
                            .get(key.as_str())
                            .map(|(v, _)| *v)
                            .unwrap_or(0.0);
                        let display = friendly_source(key);
                        draw_source_row(
                            ui,
                            key,
                            &display,
                            "",
                            val,
                            WS_COLOR,
                            source.as_str() == key.as_str(),
                            source,
                        );
                    }
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
            let color =
                Color32::from_rgba_unmultiplied(0xE0, 0xA0, 0x40, (alpha * 255.0) as u8);
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
    let painter = ctx.layer_painter(line_layer);

    for binding in &bus.bindings {
        let src_pos = state.source_positions.get(&binding.source);
        let card_pos = state.card_positions.get(&binding.id);
        let tgt_pos = state.target_positions.get(&binding.target);

        let src_color = source_color(&binding.source);
        let (stroke_width, alpha) = if binding.enabled {
            (1.5, 0.6_f32)
        } else {
            (1.0, 0.12)
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

        // Source → card left edge
        if let (Some(&from), Some(&(card_left, _))) = (src_pos, card_pos) {
            draw_bezier_connection(&painter, from, card_left, stroke);

            // Flow dot for enabled bindings with nonzero output
            if binding.enabled {
                let runtime = bus.runtime(&binding.id);
                let output = runtime.and_then(|r| r.last_output).unwrap_or(0.0);
                if output > 0.001 {
                    let t = state.flow_phase;
                    let dot_pos = eval_cubic_bezier(from, card_left, t);
                    painter.circle_filled(dot_pos, 3.0, src_color.linear_multiply(0.8));
                }
            }
        }

        // Card right edge → target
        if let (Some(&(_, card_right)), Some(&to)) = (card_pos, tgt_pos) {
            draw_bezier_connection(&painter, card_right, to, stroke);

            // Flow dot
            if binding.enabled {
                let runtime = bus.runtime(&binding.id);
                let output = runtime.and_then(|r| r.last_output).unwrap_or(0.0);
                if output > 0.001 {
                    let t = (state.flow_phase + 0.3) % 1.0;
                    let dot_pos = eval_cubic_bezier(card_right, to, t);
                    painter.circle_filled(dot_pos, 3.0, src_color.linear_multiply(0.8));
                }
            }
        }
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

fn draw_footer(ui: &mut egui::Ui, bus: &BindingBus) {
    let tc = theme_colors(ui.ctx());
    let unique_targets: HashSet<&str> = bus
        .bindings
        .iter()
        .filter(|b| !b.target.is_empty())
        .map(|b| b.target.as_str())
        .collect();

    ui.add_space(4.0);
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
}
