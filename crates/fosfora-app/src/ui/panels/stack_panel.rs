//! The v2 layer stack (#3123): Master on top, then each layer, top first.
//!
//! Every row carries a picture. In the default view it is the stack *so far* —
//! this layer blended onto everything beneath it — with the layer on its own
//! inset in the corner, so the column reads bottom to top like the trama
//! canvas reads left to right. The other view shows each layer alone. Master's
//! picture is the finished output.
//!
//! Rows send the same intents the v1 layer panel sends (`select_layer`,
//! `layer_toggle_enable`, `layer_move`, …), so `main.rs` handles both layouts
//! with one set of handlers. What is new is the Master *selection*: the
//! inspector shows Master's settings instead of a layer's while it is set.

use std::fmt::Write as _;

use egui::{Color32, RichText, Sense, Stroke, TextureId, Ui, UiBuilder, Vec2};

use crate::gpu::layer::{ChainBadge, LayerInfo};
use crate::gpu::layer_thumbs::{LayerThumbs, ThumbKind};
use crate::ui::panels::catalog_panel::{self, CatalogDrop, EffectDrag};
use crate::ui::theme::colors::theme_colors;

const MAX_LAYERS: usize = crate::bindings::catalog::MAX_LAYERS;

/// Rows at or under this count get the explanatory line between them; past it
/// the stack would not fit on a laptop screen with them.
const ROOMY_ROWS: usize = 4;

fn master_id() -> egui::Id {
    egui::Id::new("v2_master_selected")
}

/// Is Master, rather than the active layer, what the inspector shows?
pub fn master_selected(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp(master_id()).unwrap_or(false))
}

fn select_master(ctx: &egui::Context, on: bool) {
    ctx.data_mut(|d| d.insert_temp(master_id(), on));
}

/// Put the inspector on the selected layer rather than Master.
pub fn show_layer_in_inspector(ctx: &egui::Context) {
    select_master(ctx, false);
}

fn select_layer(ctx: &egui::Context, i: usize) {
    select_master(ctx, false);
    ctx.data_mut(|d| d.insert_temp(egui::Id::new("select_layer"), i));
}

/// Which picture the rows show.
fn alone_view(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp(egui::Id::new("v2_stack_alone")).unwrap_or(false))
}

/// Where the rows' pictures come from.
pub struct StackPictures<'a> {
    /// `None` until the first frame registers them — rows then draw an empty
    /// frame of the same size, so nothing jumps when they arrive.
    pub thumbs: Option<&'a LayerThumbs>,
    /// Width over height of the output; the thumbnails are drawn at it.
    pub aspect: f32,
}

impl StackPictures<'_> {
    fn tex(&self, kind: ThumbKind, slot: usize) -> Option<TextureId> {
        self.thumbs.and_then(|t| t.tex(kind, slot))
    }
}

/// What kind of layer, in a word.
pub fn layer_kind(layer: &LayerInfo) -> &'static str {
    if layer.media_is_live {
        "Camera"
    } else if layer.is_media {
        "Media"
    } else {
        "Effect"
    }
}

/// What a row calls its layer: the name someone gave it, else what it shows.
/// The slot name ("Layer 3") comes last — the row's number already says it.
pub fn layer_name(layer: &LayerInfo) -> &str {
    layer
        .custom_name
        .as_deref()
        .or(layer.effect_name.as_deref())
        .or(layer.media_file_name.as_deref())
        .unwrap_or(&layer.name)
}

fn chain_words(badge: ChainBadge) -> String {
    let nodes = match badge.nodes {
        1 => "1 node".to_string(),
        n => format!("{n} nodes"),
    };
    if badge.active {
        format!("chain · {nodes}")
    } else {
        format!("chain · {nodes}, inactive")
    }
}

/// `"Screen 70%"`.
pub fn blend_words(layer: &LayerInfo) -> String {
    format!(
        "{} {:.0}%",
        layer.blend_mode.display_name(),
        layer.opacity * 100.0
    )
}

/// The layer stack. `postfx_on` names the post-processing stages switched on,
/// for Master's row.
pub fn draw_stack(
    ui: &mut Ui,
    layers: &[LayerInfo],
    active_layer: usize,
    master_chain: Option<ChainBadge>,
    postfx_on: &[&str],
    pics: &StackPictures<'_>,
) {
    let tc = theme_colors(ui.ctx());
    let ctx = ui.ctx().clone();
    let alone = alone_view(&ctx);
    let roomy = layers.len() <= ROOMY_ROWS;
    let master_on = master_selected(&ctx);
    // The bottom-most enabled layer is composited first: nothing beneath it,
    // so its blend is never applied and its stack-so-far is itself.
    let bottom_enabled = layers.iter().rposition(|l| l.enabled);

    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Pictures")
                .size(11.0)
                .color(tc.text_secondary),
        );
        for (on, label, tip) in [
            (
                false,
                "Blended so far",
                "Each picture is this layer blended onto everything beneath it",
            ),
            (
                true,
                "Each layer alone",
                "Each picture is only its own layer",
            ),
        ] {
            if ui
                .selectable_label(alone == on, RichText::new(label).size(12.0))
                .on_hover_text(tip)
                .clicked()
            {
                ctx.data_mut(|d| d.insert_temp(egui::Id::new("v2_stack_alone"), on));
            }
        }
    });
    ui.add_space(4.0);

    // Master.
    let master_sub = {
        let chain = master_chain.map_or("chain empty".to_string(), chain_words);
        format!("Post-processing · {chain}")
    };
    let master_what = if alone {
        "no source of its own"
    } else {
        "the final output"
    };
    let master_pic = if alone {
        None
    } else {
        pics.tex(ThumbKind::Master, 0)
    };
    let resp = row(
        ui,
        "v2_row_master",
        RowText {
            badge: "M",
            name: "Master",
            sub: &master_sub,
            what: master_what,
        },
        Picture {
            main: master_pic,
            inset: None,
            aspect: pics.aspect,
            compact: !roomy,
        },
        master_on,
        false,
        |_| {},
    );
    if resp.clicked() {
        select_master(&ctx, true);
    }
    // Master has no effect of its own: an effect dropped on it goes on top.
    drop_target(ui, &resp, layers, |_| Some(CatalogDrop::Insert(0)));
    if roomy {
        let line = if postfx_on.is_empty() {
            "Master passes the full blend through".to_string()
        } else {
            format!("Master applies {} to the full blend", postfx_on.join(", "))
        };
        flow_line(ui, &line);
    }

    for (i, layer) in layers.iter().enumerate() {
        let is_bottom = bottom_enabled.is_some_and(|b| i >= b);
        let mut sub = layer_kind(layer).to_string();
        if layer.enabled {
            if !is_bottom {
                sub.push_str(" · ");
                sub.push_str(&blend_words(layer));
            } else if layer.opacity < 1.0 {
                let _ = write!(sub, " · {:.0}%", layer.opacity * 100.0);
            }
        } else {
            sub.push_str(" · hidden");
        }
        if let Some(badge) = layer.chain {
            sub.push_str(" · ");
            sub.push_str(&chain_words(badge));
        }
        if layer.locked {
            sub.push_str(" · locked");
        }

        // A hidden layer has no stage of its own: what reaches this row is
        // whatever the enabled layer beneath it produced.
        let below = (i..layers.len()).find(|&j| layers[j].enabled);
        let (main, inset, what) = if alone {
            (
                pics.tex(ThumbKind::Alone, i),
                None,
                "this layer alone".to_string(),
            )
        } else if !layer.enabled {
            // No inset: a hidden layer is not rendered, and one hidden since
            // the preset loaded has no picture to show — an empty box read as
            // a broken one.
            (
                below.and_then(|j| pics.tex(ThumbKind::Blended, j)),
                None,
                match below {
                    Some(_) => "hidden: the picture below passes up".to_string(),
                    None => "hidden, with nothing below".to_string(),
                },
            )
        } else if is_bottom {
            (
                pics.tex(ThumbKind::Blended, i),
                None,
                "the bottom of the stack".to_string(),
            )
        } else {
            let under = layers[i + 1..].iter().filter(|l| l.enabled).count();
            (
                pics.tex(ThumbKind::Blended, i),
                pics.tex(ThumbKind::Alone, i),
                match under {
                    1 => "blended onto the layer below".to_string(),
                    n => format!("blended onto the {n} layers below"),
                },
            )
        };
        let badge = format!("{}", i + 1);
        let what = if layer.needs_layer_below {
            "reads the layers beneath, and there are none".to_string()
        } else {
            what
        };
        let resp = row(
            ui,
            &format!("v2_row_{i}"),
            RowText {
                badge: &badge,
                name: layer_name(layer),
                sub: &sub,
                what: &what,
            },
            Picture {
                main,
                inset,
                aspect: pics.aspect,
                compact: !roomy,
            },
            !master_on && i == active_layer,
            !layer.enabled,
            |ui| {
                ui.add_enabled_ui(!layer.locked, |ui| {
                    let label = if layer.enabled { "Hide" } else { "Hidden" };
                    let tip = if layer.locked {
                        "The layer is locked"
                    } else if layer.enabled {
                        "Hide this layer — the picture below passes up unchanged"
                    } else {
                        "Show this layer again"
                    };
                    if ui
                        .selectable_label(!layer.enabled, RichText::new(label).size(12.0))
                        .on_hover_text(tip)
                        .clicked()
                    {
                        ui.ctx().data_mut(|d| {
                            d.insert_temp(
                                egui::Id::new("layer_toggle_enable"),
                                (i, !layer.enabled),
                            );
                        });
                    }
                });
            },
        );
        if resp.clicked() {
            select_layer(&ctx, i);
        }
        resp.context_menu(|ui| row_menu(ui, layers, i));
        drop_target(ui, &resp, layers, |y| Some(row_zone(resp.rect, y, i)));

        if roomy && layer.enabled && !is_bottom {
            flow_line(
                ui,
                &format!(
                    "{} blends onto the picture below: {}",
                    layer_name(layer),
                    blend_words(layer)
                ),
            );
        }
    }

    // Media still decoding: the layer will land at the bottom, here.
    super::media_panel::draw_loading(ui);
    ui.add_space(6.0);
    let below = ui.scope(|ui| add_buttons(ui, layers.len())).response;
    drop_target(ui, &below, layers, |_| {
        Some(CatalogDrop::Insert(layers.len()))
    });
    ui.add_space(4.0);
    ui.label(
        RichText::new(if alone {
            "Each picture shows only its own layer, before blending. Master has \
             no source of its own, so it is empty in this view."
        } else {
            "Read it bottom to top: each picture is the stack so far, with that \
             layer blended onto everything beneath it; the inset is the layer on \
             its own. Master is what goes out. Right-click a row for more."
        })
        .size(11.0)
        .color(tc.text_secondary),
    );
}

struct RowText<'a> {
    badge: &'a str,
    name: &'a str,
    sub: &'a str,
    what: &'a str,
}

struct Picture {
    main: Option<TextureId>,
    inset: Option<TextureId>,
    aspect: f32,
    compact: bool,
}

/// One clickable row. `trailing` draws controls at the right edge; they take
/// clicks before the row does.
#[allow(clippy::too_many_arguments)]
fn row(
    ui: &mut Ui,
    id: &str,
    text: RowText<'_>,
    pic: Picture,
    selected: bool,
    dimmed: bool,
    trailing: impl FnOnce(&mut Ui),
) -> egui::Response {
    let tc = theme_colors(ui.ctx());
    // Selection is fill plus a strong outline, never a hue.
    let (fill, stroke) = if selected {
        (tc.hover_fill, Stroke::new(1.5_f32, tc.text_primary))
    } else {
        (tc.card_bg, Stroke::new(1.0_f32, tc.card_border))
    };
    let text_color = if dimmed {
        tc.text_secondary
    } else {
        tc.text_primary
    };
    let resp = ui
        .push_id(id, |ui| {
            ui.scope_builder(UiBuilder::new().sense(Sense::click()), |ui| {
                egui::Frame::new()
                    .fill(fill)
                    .stroke(stroke)
                    .corner_radius(4.0)
                    .inner_margin(egui::Margin::same(6))
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.add_sized(
                                [18.0, 18.0],
                                egui::Label::new(
                                    RichText::new(text.badge)
                                        .size(12.0)
                                        .strong()
                                        .color(tc.text_secondary),
                                ),
                            );
                            picture(ui, &pic, dimmed);
                            ui.add_space(4.0);
                            ui.vertical(|ui| {
                                ui.label(
                                    RichText::new(text.name)
                                        .size(14.0)
                                        .strong()
                                        .color(text_color),
                                );
                                ui.label(
                                    RichText::new(text.sub).size(12.0).color(tc.text_secondary),
                                );
                                ui.label(
                                    RichText::new(text.what)
                                        .size(11.0)
                                        .monospace()
                                        .color(tc.text_secondary),
                                );
                            });
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                trailing,
                            );
                        });
                    });
            })
            .response
        })
        .inner;
    // Where each row is, for the drag-and-drop test to aim at.
    #[cfg(test)]
    ui.ctx()
        .data_mut(|d| d.insert_temp(egui::Id::new(id).with("rect"), resp.rect));
    resp
}

fn picture(ui: &mut Ui, pic: &Picture, dimmed: bool) {
    let tc = theme_colors(ui.ctx());
    // The picture is what the row is for, so it takes what the text does not
    // need — up to a cap, and shrinking with the column on a small window.
    let cap = if pic.compact { 168.0 } else { 224.0 };
    let w = (ui.available_width() * 0.42).clamp(96.0, cap).round();
    let size = Vec2::new(w, (w / pic.aspect.max(0.01)).round());
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let painter = ui.painter();
    let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    // A hidden layer's picture is shown, but faded: it is not in the output.
    let tint = if dimmed {
        Color32::from_white_alpha(110)
    } else {
        Color32::WHITE
    };
    painter.rect_filled(rect, 3.0, Color32::BLACK);
    if let Some(tex) = pic.main {
        painter.image(tex, rect, uv, tint);
    }
    painter.rect_stroke(
        rect,
        3.0,
        Stroke::new(1.0_f32, tc.card_border),
        egui::StrokeKind::Inside,
    );
    if let Some(tex) = pic.inset {
        let iw = (w * 0.34).round();
        let isize = Vec2::new(iw, (iw / pic.aspect.max(0.01)).round());
        let irect =
            egui::Rect::from_min_size(rect.right_bottom() - isize - Vec2::splat(4.0), isize);
        painter.rect_filled(irect.expand(1.0), 2.0, tc.card_bg);
        painter.image(tex, irect, uv, tint);
        painter.rect_stroke(
            irect.expand(1.0),
            2.0,
            Stroke::new(1.0_f32, tc.text_secondary),
            egui::StrokeKind::Outside,
        );
    }
}

/// The line between two rows saying what happens at that step.
fn flow_line(ui: &mut Ui, text: &str) {
    let tc = theme_colors(ui.ctx());
    ui.horizontal(|ui| {
        ui.add_space(26.0);
        ui.label(
            RichText::new(format!("↑  {text}"))
                .size(11.0)
                .color(tc.text_secondary),
        );
    });
}

fn row_menu(ui: &mut Ui, layers: &[LayerInfo], i: usize) {
    let layer = &layers[i];
    let send = |ui: &Ui, key: &'static str, v: (usize, bool)| {
        ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new(key), v));
    };

    // Rename in place: the field edits a draft, Enter or the button commits.
    let draft_id = egui::Id::new("v2_rename_draft").with(i);
    let mut draft: String = ui
        .ctx()
        .data(|d| d.get_temp(draft_id))
        .unwrap_or_else(|| layer_name(layer).to_string());
    ui.horizontal(|ui| {
        let field = ui.add(egui::TextEdit::singleline(&mut draft).desired_width(160.0));
        let commit = ui.button("Rename").clicked()
            || (field.lost_focus() && ui.input(|inp| inp.key_pressed(egui::Key::Enter)));
        if commit {
            let name = draft.trim();
            let new = (!name.is_empty() && name != layer.name).then(|| name.to_string());
            ui.ctx()
                .data_mut(|d| d.insert_temp(egui::Id::new("layer_rename"), (i, new)));
            ui.ctx().data_mut(|d| d.remove_temp::<String>(draft_id));
            ui.close();
        } else {
            ui.ctx()
                .data_mut(|d| d.insert_temp(draft_id, draft.clone()));
        }
    });
    ui.separator();

    if ui
        .add_enabled(i > 0 && !layer.pinned, egui::Button::new("Move up"))
        .clicked()
    {
        ui.ctx()
            .data_mut(|d| d.insert_temp(egui::Id::new("layer_move"), (i, i - 1)));
        ui.close();
    }
    if ui
        .add_enabled(
            i + 1 < layers.len() && !layer.pinned,
            egui::Button::new("Move down"),
        )
        .clicked()
    {
        ui.ctx()
            .data_mut(|d| d.insert_temp(egui::Id::new("layer_move"), (i, i + 1)));
        ui.close();
    }
    ui.separator();
    if ui
        .button(if layer.locked { "Unlock" } else { "Lock" })
        .on_hover_text("A locked layer ignores edits and is kept when a preset loads")
        .clicked()
    {
        send(ui, "layer_toggle_lock", (i, !layer.locked));
        ui.close();
    }
    if ui
        .button(if layer.pinned {
            "Unpin position"
        } else {
            "Pin position"
        })
        .on_hover_text("A pinned layer cannot be moved up or down")
        .clicked()
    {
        send(ui, "layer_toggle_pin", (i, !layer.pinned));
        ui.close();
    }
    if ui.button("Edit chain").clicked() {
        ui.ctx()
            .data_mut(|d| d.insert_temp(egui::Id::new("open_trama_on_layer"), i));
        ui.close();
    }
    ui.separator();
    if ui
        .add_enabled(layers.len() > 1, egui::Button::new("Remove layer"))
        .clicked()
    {
        ui.ctx()
            .data_mut(|d| d.insert_temp(egui::Id::new("remove_layer"), i));
        ui.close();
    }
}

fn add_buttons(ui: &mut Ui, count: usize) {
    let can_add = count < MAX_LAYERS;
    let full = format!("The stack is full ({MAX_LAYERS} layers)");
    ui.horizontal(|ui| {
        let b = ui
            .add_enabled(can_add, egui::Button::new("+ Effect layer"))
            .on_hover_text("Add a layer running an effect")
            .on_disabled_hover_text(&full);
        if b.clicked() {
            ui.ctx()
                .data_mut(|d| d.insert_temp(egui::Id::new("add_layer"), true));
        }
        let b = ui
            .add_enabled(can_add, egui::Button::new("+ Media layer"))
            .on_hover_text("Add an image, GIF or video layer")
            .on_disabled_hover_text(&full);
        if b.clicked() {
            ui.ctx()
                .data_mut(|d| d.insert_temp(egui::Id::new("add_media_layer"), true));
        }
        #[cfg(feature = "webcam")]
        {
            let b = ui
                .add_enabled(can_add, egui::Button::new("+ Camera layer"))
                .on_hover_text("Add a live camera layer")
                .on_disabled_hover_text(&full);
            if b.clicked() {
                let device_idx: u32 = ui
                    .ctx()
                    .data(|d| d.get_temp(egui::Id::new("webcam_default_device")))
                    .unwrap_or(0);
                ui.ctx()
                    .data_mut(|d| d.insert_temp(egui::Id::new("add_webcam_layer"), device_idx));
            }
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            clear_all(ui, count);
        });
    });
}

/// Two clicks within three seconds: clearing the stack is not undoable.
fn clear_all(ui: &mut Ui, count: usize) {
    let armed_id = egui::Id::new("v2_clear_all_armed");
    let now = ui.input(|i| i.time);
    let armed = ui
        .ctx()
        .data(|d| d.get_temp::<f64>(armed_id))
        .is_some_and(|t| now - t < 3.0);
    let label = if armed {
        format!("Click again: replace all {count} with one fresh layer")
    } else {
        "Clear stack…".to_string()
    };
    if ui
        .add_enabled(
            count > 0,
            egui::Button::new(RichText::new(label).size(12.0)),
        )
        .on_hover_text("Replace every layer with a single default layer")
        .clicked()
    {
        ui.ctx().data_mut(|d| {
            if armed {
                d.insert_temp(egui::Id::new("clear_all_layers"), true);
                d.remove_temp::<f64>(armed_id);
            } else {
                d.insert_temp(armed_id, now);
            }
        });
    }
    if armed {
        ui.ctx().request_repaint();
    }
}

/// Which part of a layer row an effect is dropped on: the middle replaces the
/// layer's effect, the top or bottom edge adds a layer above or below it.
fn row_zone(rect: egui::Rect, y: f32, i: usize) -> CatalogDrop {
    let edge = (rect.height() * 0.25).min(28.0);
    if y < rect.top() + edge {
        CatalogDrop::Insert(i)
    } else if y > rect.bottom() - edge {
        CatalogDrop::Insert(i + 1)
    } else {
        CatalogDrop::Replace(i)
    }
}

/// Why a drop could not happen, or `None` when it can.
fn refusal(layers: &[LayerInfo], at: CatalogDrop) -> Option<&'static str> {
    match at {
        CatalogDrop::Replace(i) if layers.get(i).is_some_and(|l| l.locked) => {
            Some("This layer is locked")
        }
        CatalogDrop::Insert(_) if layers.len() >= MAX_LAYERS => Some("The stack is full"),
        _ => None,
    }
}

/// While a catalog effect is dragged over `resp`, show where it would land —
/// an outline for "replace", a bar for "new layer here", each with words —
/// and send the drop when it is released.
fn drop_target(
    ui: &Ui,
    resp: &egui::Response,
    layers: &[LayerInfo],
    zone: impl FnOnce(f32) -> Option<CatalogDrop>,
) {
    let ctx = ui.ctx();
    if !resp.contains_pointer() || !egui::DragAndDrop::has_payload_of_type::<EffectDrag>(ctx) {
        return;
    }
    let Some(pos) = ctx.pointer_latest_pos() else {
        return;
    };
    let Some(at) = zone(pos.y) else {
        return;
    };
    let Some(drag) = egui::DragAndDrop::payload::<EffectDrag>(ctx) else {
        return;
    };
    let refused = refusal(layers, at);
    let tc = theme_colors(ctx);
    let layer = egui::LayerId::new(egui::Order::Foreground, egui::Id::new("v2_drop_hint"));
    let painter = ctx.layer_painter(layer);
    let r = resp.rect;
    let (anchor, words) = match (refused, at) {
        (Some(why), _) => (r.center(), why.to_string()),
        (None, CatalogDrop::Replace(i)) => {
            painter.rect_stroke(
                r,
                4.0,
                Stroke::new(3.0_f32, tc.text_primary),
                egui::StrokeKind::Inside,
            );
            let on = layers.get(i).map_or("this layer", |l| layer_name(l));
            (r.center(), format!("Replace {on} with {}", drag.name))
        }
        (None, CatalogDrop::Insert(_)) => {
            // Above the row for its top edge (and for Master, and the add
            // buttons at the bottom), below it for its bottom edge.
            let y = if pos.y > r.center().y && r.height() > 40.0 {
                r.bottom() + 1.0
            } else {
                r.top() - 1.0
            };
            painter.line_segment(
                [egui::pos2(r.left(), y), egui::pos2(r.right(), y)],
                Stroke::new(4.0_f32, tc.text_primary),
            );
            (
                egui::pos2(r.center().x, y),
                format!("New layer here: {}", drag.name),
            )
        }
    };
    let galley = painter.layout_no_wrap(words, egui::FontId::proportional(13.0), tc.text_primary);
    let pill = egui::Rect::from_center_size(anchor, galley.size() + Vec2::new(16.0, 8.0));
    painter.rect_filled(pill, 4.0, tc.panel);
    painter.rect_stroke(
        pill,
        4.0,
        Stroke::new(1.0_f32, tc.text_primary),
        egui::StrokeKind::Outside,
    );
    painter.galley(pill.min + Vec2::new(8.0, 4.0), galley, tc.text_primary);

    if refused.is_none() && ctx.input(|i| i.pointer.any_released()) {
        if let Some(drag) = egui::DragAndDrop::take_payload::<EffectDrag>(ctx) {
            select_master(ctx, false);
            catalog_panel::send_drop(ctx, drag.name.clone(), at);
        }
    }
}
