//! The workspace's Scenes tab (#3173): the scenes on the left, and the
//! current one as a single strip of cue cards in play order.
//!
//! The Classic panel draws the same cues twice, as a list and as a timeline
//! bar. Here they are drawn once: each card is a cue (its preset's picture,
//! its name, how long it holds), each transition is the joint between two
//! cards, and the playhead runs through the cards and joints themselves.
//!
//! A click selects a cue for the edit row under the strip; a double-click,
//! or Play from here, is what jumps the show to it. Editing a cue never cuts
//! to it.

use egui::{Align, Color32, Id, Layout, Rect, RichText, ScrollArea, Sense, Stroke, TextureId, Ui};
use egui::{Vec2, pos2, vec2};

use super::scene_panel::{CueDisplayInfo, SceneInfo};
use crate::scene::timeline::{TimelineInfo, TimelineInfoState};
use crate::scene::types::{AdvanceMode, TransitionType};
use crate::ui::theme::colors::{ThemeColors, theme_colors};
use crate::ui::theme::tokens::MIN_INTERACT_HEIGHT;
use crate::ui::widgets;

/// Looks up the catalog picture for an effect by name.
pub type Pictures<'a> = dyn FnMut(&egui::Context, &str) -> Option<TextureId> + 'a;

const PIC: Vec2 = vec2(144.0, 81.0);
const PAD: f32 = 5.0;
const NAME_H: f32 = 20.0;
const TIME_H: f32 = 16.0;
const CARD_H: f32 = PAD + PIC.y + 4.0 + NAME_H + TIME_H + PAD;
const CARD_MIN_W: f32 = PIC.x + 2.0 * PAD;
const CARD_MAX_W: f32 = 3.0 * CARD_MIN_W;
const JOINT_W: f32 = 48.0;
/// The scene list's width.
const LIST_W: f32 = 210.0;

/// A preset dragged from the preset tiles onto the strip.
#[derive(Clone)]
pub struct PresetDrag {
    pub name: String,
}

/// A cue card being dragged to a new place.
#[derive(Clone, Copy)]
struct CueDrag {
    index: usize,
}

fn send<T: Clone + Send + Sync + 'static>(ctx: &egui::Context, key: &str, v: T) {
    ctx.data_mut(|d| d.insert_temp(Id::new(key), v));
}

fn selected_id() -> Id {
    Id::new("v2_cue_selected")
}

/// The cue selected for editing, as (scene, cue): a selection belongs to
/// the scene it was made in.
fn selected(ctx: &egui::Context, info: &SceneInfo) -> Option<usize> {
    let (scene, cue) = ctx.data(|d| d.get_temp::<(usize, usize)>(selected_id()))?;
    (Some(scene) == info.current_scene && cue < info.cue_list.len()).then_some(cue)
}

fn select(ctx: &egui::Context, info: &SceneInfo, cue: Option<usize>) {
    ctx.data_mut(|d| match (info.current_scene, cue) {
        (Some(scene), Some(cue)) => d.insert_temp(selected_id(), (scene, cue)),
        _ => d.remove::<(usize, usize)>(selected_id()),
    });
}

/// A drag value that holds its own number while it is being dragged or
/// typed in: the app applies a change a frame later, and reading the old
/// value back each frame made a drag stutter. Returns the new value when it
/// changed.
fn live_drag<T>(
    ui: &mut Ui,
    id: Id,
    current: T,
    make: impl FnOnce(&mut T) -> egui::DragValue<'_>,
) -> Option<T>
where
    T: egui::emath::Numeric + Send + Sync + 'static,
{
    let held = ui.data(|d| d.get_temp::<T>(id));
    let mut v = held.unwrap_or(current);
    let r = ui.add(make(&mut v));
    if r.dragged() || r.has_focus() {
        ui.data_mut(|d| d.insert_temp(id, v));
    } else {
        ui.data_mut(|d| d.remove::<T>(id));
    }
    r.changed().then_some(v)
}

/// The whole tab, in the space it is given.
pub fn draw(ui: &mut Ui, info: &SceneInfo, pics: &mut Pictures<'_>) {
    let h = ui.available_height();
    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(
            vec2(LIST_W, h),
            Layout::top_down_justified(Align::Min),
            |ui| {
                ScrollArea::vertical()
                    .id_salt("v2_scene_list")
                    .show(ui, |ui| scene_list(ui, info));
            },
        );
        ui.add_space(8.0);
        ui.allocate_ui_with_layout(
            vec2(ui.available_width(), h),
            Layout::top_down(Align::Min),
            |ui| {
                ScrollArea::vertical()
                    .id_salt("v2_scene_body")
                    .show(ui, |ui| match (info.current_scene, &info.timeline) {
                        (Some(_), Some(tl)) => {
                            transport(ui, tl, !info.cue_list.is_empty());
                            ui.add_space(6.0);
                            strip(ui, info, tl, pics);
                            ui.add_space(6.0);
                            edit_row(ui, info, tl);
                        }
                        _ => {
                            let tc = theme_colors(ui.ctx());
                            let words = if info.scene_store_names.is_empty() {
                                "No scenes yet. A scene plays presets in order, as a strip of \
                                 cues: by hand, on a timer or on the beat. Click New scene to \
                                 start one."
                            } else {
                                "Pick a scene on the left to see its cues. Double-click one \
                                 to play it."
                            };
                            ui.add_space(8.0);
                            ui.label(RichText::new(words).size(13.0).color(tc.text_secondary));
                        }
                    });
            },
        );
    });
}

// ── The scene list ──────────────────────────────────────────────────────

fn scene_list(ui: &mut Ui, info: &SceneInfo) {
    let tc = theme_colors(ui.ctx());
    let ctx = ui.ctx().clone();
    // (scene name, text so far), while a row is being renamed.
    let rename_id = Id::new("v2_scene_renaming");
    let armed_id = Id::new("v2_scene_delete_armed");
    let renaming: Option<(String, String)> = ctx.data(|d| d.get_temp(rename_id));
    let armed: Option<String> = ctx.data(|d| d.get_temp(armed_id));
    let playing = info.timeline.as_ref().is_some_and(|t| t.active);

    for (i, name) in info.scene_store_names.iter().enumerate() {
        let current = info.current_scene == Some(i);
        if let Some((_, mut text)) = renaming.clone().filter(|(n, _)| n == name) {
            let r = widgets::layout_row(ui, Layout::left_to_right(Align::Center), |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut text)
                        .desired_width(ui.available_width())
                        .font(egui::FontId::proportional(13.0)),
                )
            });
            if !r.has_focus() && !r.lost_focus() {
                r.request_focus();
            }
            let (enter, esc) = ui.input(|i| {
                (
                    i.key_pressed(egui::Key::Enter),
                    i.key_pressed(egui::Key::Escape),
                )
            });
            if r.lost_focus() || esc {
                if enter && !esc && text.trim() != name.as_str() {
                    send(&ctx, "scene_rename", (i, text.trim().to_string()));
                }
                ctx.data_mut(|d| d.remove::<(String, String)>(rename_id));
            } else {
                ctx.data_mut(|d| d.insert_temp(rename_id, (name.clone(), text)));
            }
            continue;
        }
        if armed.as_deref() == Some(name.as_str()) {
            widgets::layout_row(ui, Layout::left_to_right(Align::Center), |ui| {
                ui.label(
                    RichText::new(format!("Delete {}?", widgets::truncate_chars(name, 14)))
                        .size(13.0)
                        .strong(),
                );
                if ui.button(RichText::new("Delete").size(12.0)).clicked() {
                    send(&ctx, "delete_scene", i);
                    ctx.data_mut(|d| d.remove::<String>(armed_id));
                }
                if ui.button(RichText::new("Keep").size(12.0)).clicked() {
                    ctx.data_mut(|d| d.remove::<String>(armed_id));
                }
            });
            continue;
        }

        let r = scene_row(ui, name, current, current && playing, &tc);
        if r.double_clicked() {
            send(&ctx, "load_scene", i);
        } else if r.clicked() && !current {
            send(&ctx, "open_scene", i);
        }
        let r = r.on_hover_text("Click to open, double-click to play. Right-click for more.");
        r.context_menu(|ui| {
            if ui.button("Play").clicked() {
                send(&ctx, "load_scene", i);
            }
            if ui.button("Rename").clicked() {
                ctx.data_mut(|d| d.insert_temp(rename_id, (name.clone(), name.clone())));
            }
            if ui.button("Duplicate").clicked() {
                send(&ctx, "scene_duplicate", i);
            }
            ui.separator();
            if ui.button("Delete…").clicked() {
                ctx.data_mut(|d| d.insert_temp(armed_id, name.clone()));
            }
        });
    }

    ui.add_space(4.0);
    let new = widgets::layout_row(ui, Layout::left_to_right(Align::Center), |ui| {
        ui.button(RichText::new("+ New scene").size(13.0))
    });
    if new.clicked() {
        let name = crate::scene::store::first_free_name(&info.scene_store_names, "Scene");
        send(&ctx, "save_scene", name.clone());
        // Named at once, ready to rename.
        ctx.data_mut(|d| d.insert_temp(rename_id, (name.clone(), name)));
    }
}

/// One scene: the current one takes the selection fill, the playing one
/// ends in a ▶.
fn scene_row(
    ui: &mut Ui,
    name: &str,
    current: bool,
    playing: bool,
    tc: &ThemeColors,
) -> egui::Response {
    let size = vec2(ui.available_width(), MIN_INTERACT_HEIGHT + 2.0);
    let (rect, r) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let (fill, text) = if current {
            (tc.selection, tc.on_selection)
        } else if r.hovered() {
            (tc.widget_bg, tc.text_primary)
        } else {
            (Color32::TRANSPARENT, tc.text_primary)
        };
        p.rect_filled(rect, 4.0, fill);
        let mut t = RichText::new(widgets::truncate_chars(name, 22)).size(13.0);
        if current {
            t = t.strong();
        }
        let g = egui::WidgetText::from(t).into_galley(
            ui,
            Some(egui::TextWrapMode::Truncate),
            rect.width() - 36.0,
            egui::TextStyle::Body,
        );
        p.galley(
            pos2(rect.left() + 8.0, rect.center().y - g.size().y / 2.0),
            g,
            text,
        );
        if playing {
            p.text(
                rect.right_center() - vec2(10.0, 0.0),
                egui::Align2::RIGHT_CENTER,
                "\u{25B6}",
                egui::FontId::proportional(13.0),
                text,
            );
        }
    }
    r
}

// ── Transport ───────────────────────────────────────────────────────────

fn transport(ui: &mut Ui, tl: &TimelineInfo, has_cues: bool) {
    let tc = theme_colors(ui.ctx());
    let ctx = ui.ctx().clone();
    widgets::layout_row(ui, Layout::left_to_right(Align::Center), |ui| {
        if !tl.active {
            let b = ui
                .add_enabled(
                    has_cues,
                    egui::Button::new(RichText::new("\u{25B6}  Play").size(13.0)),
                )
                .on_hover_text("Play from the first cue (T)")
                .on_disabled_hover_text("Add a cue first.");
            if b.clicked() {
                send(&ctx, "scene_toggle_play", true);
            }
        } else {
            if ui
                .button(RichText::new("\u{25A0}  Stop").size(13.0))
                .on_hover_text("Stop (T)")
                .clicked()
            {
                send(&ctx, "scene_toggle_play", true);
            }
            if ui
                .button(RichText::new("\u{25C0}  Prev").size(13.0))
                .clicked()
            {
                send(&ctx, "scene_go_prev", true);
            }
            let go = egui::Button::new(
                RichText::new("Go  \u{25B6}")
                    .size(13.0)
                    .strong()
                    .color(tc.on_selection),
            )
            .fill(tc.selection)
            .min_size(vec2(96.0, 0.0));
            if ui.add(go).on_hover_text("Next cue (Space)").clicked() {
                send(&ctx, "scene_go_next", true);
            }
        }

        ui.add_space(20.0);
        ui.label(RichText::new("Advance").size(13.0).color(tc.text_secondary));
        let mode: u32 = match tl.advance_mode {
            AdvanceMode::Manual => 0,
            AdvanceMode::Timer => 1,
            AdvanceMode::BeatSync { .. } => 2,
        };
        for (m, label, tip) in [
            (0u32, "By hand", "Each cue waits for Go."),
            (
                1,
                "Timer",
                "Each cue holds for its own time, then the next begins.",
            ),
            (2, "Beats", "Every cue holds for the same number of beats."),
        ] {
            if ui
                .selectable_label(m == mode, RichText::new(label).size(13.0))
                .on_hover_text(tip)
                .clicked()
                && m != mode
            {
                send(&ctx, "scene_set_advance_mode", m);
            }
        }
        if let AdvanceMode::BeatSync { beats_per_cue } = tl.advance_mode {
            if let Some(b) = live_drag(ui, Id::new("v2_beats_per_cue"), beats_per_cue, |v| {
                egui::DragValue::new(v)
                    .range(1..=64)
                    .speed(0.1)
                    .suffix(" beats")
            }) {
                send(&ctx, "scene_set_beats_per_cue", b);
            }
        }

        ui.add_space(20.0);
        let mut looping = tl.loop_mode;
        if ui
            .checkbox(&mut looping, RichText::new("Loop").size(13.0))
            .on_hover_text("After the last cue, go back to the first.")
            .changed()
        {
            send(&ctx, "scene_set_loop", looping);
        }
    });
}

// ── The strip ───────────────────────────────────────────────────────────

/// Each card's width. In Timer mode a card is as wide as its hold time,
/// scaled so the whole strip fits `avail` where it can, and never narrower
/// than its picture; in the other modes nothing sets a cue's length, so the
/// cards are all alike.
fn card_widths(cues: &[CueDisplayInfo], mode: &AdvanceMode, avail: f32) -> Vec<f32> {
    if *mode != AdvanceMode::Timer {
        return vec![CARD_MIN_W; cues.len()];
    }
    let total: f32 = cues.iter().filter_map(|c| c.hold_secs).sum();
    let room = avail - cues.len() as f32 * JOINT_W - JOINT_W - CARD_MIN_W;
    let per_sec = if total > 0.0 {
        (room / total).clamp(4.0, 40.0)
    } else {
        0.0
    };
    cues.iter()
        .map(|c| {
            c.hold_secs
                .map_or(CARD_MIN_W, |h| (h * per_sec).clamp(CARD_MIN_W, CARD_MAX_W))
        })
        .collect()
}

/// What a card says under its name.
fn time_words(cue: &CueDisplayInfo, mode: &AdvanceMode) -> String {
    match mode {
        AdvanceMode::Timer => match cue.hold_secs {
            Some(h) => format!("{} s", widgets::fmt_val(h as f64)),
            None => "waits for Go".to_string(),
        },
        AdvanceMode::BeatSync { beats_per_cue } => format!("{beats_per_cue} beats"),
        AdvanceMode::Manual => String::new(),
    }
}

/// How a cue stands in playback.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Play {
    /// On screen and holding, `Some(fraction)` of its hold gone when it
    /// has one.
    Holding(Option<f32>),
    /// On screen and fading out.
    Leaving,
    /// Coming in, this far through the transition.
    Arriving(f32),
    Not,
}

fn play_state(tl: &TimelineInfo, i: usize) -> Play {
    if !tl.active {
        return Play::Not;
    }
    match tl.state {
        TimelineInfoState::Holding { elapsed, hold_secs } if tl.current_cue == i => {
            Play::Holding(hold_secs.map(|h| (elapsed / h.max(0.001)).clamp(0.0, 1.0)))
        }
        TimelineInfoState::Transitioning { from, .. } if from == i => Play::Leaving,
        TimelineInfoState::Transitioning { to, progress, .. } if to == i => {
            Play::Arriving(progress.clamp(0.0, 1.0))
        }
        _ => Play::Not,
    }
}

/// A playhead that reads on any picture: a dark line under a text-colored
/// one.
fn playhead(p: &egui::Painter, x: f32, top: f32, bottom: f32, tc: &ThemeColors) {
    let a = pos2(x, top);
    let b = pos2(x, bottom);
    p.line_segment([a, b], Stroke::new(4.0_f32, Color32::from_black_alpha(150)));
    p.line_segment([a, b], Stroke::new(2.0_f32, tc.text_primary));
}

fn strip(ui: &mut Ui, info: &SceneInfo, tl: &TimelineInfo, pics: &mut Pictures<'_>) {
    let ctx = ui.ctx().clone();
    let sel = selected(&ctx, info);
    let widths = card_widths(&info.cue_list, &tl.advance_mode, ui.available_width());
    // Where each card was drawn, to aim a drop between them.
    let mut cards: Vec<Rect> = Vec::with_capacity(info.cue_list.len());
    let mut joints: Vec<Rect> = Vec::with_capacity(info.cue_list.len() + 1);

    let out = ScrollArea::horizontal()
        .id_salt("v2_cue_strip")
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                for (i, cue) in info.cue_list.iter().enumerate() {
                    let st = play_state(tl, i);
                    let j = joint(ui, i, cue, st, sel == Some(i));
                    if j.clicked() {
                        select(&ctx, info, Some(i));
                    }
                    joints.push(j.rect);
                    let pic = cue.effect.as_deref().and_then(|e| pics(&ctx, e));
                    let c = card(
                        ui,
                        i,
                        cue,
                        widths[i],
                        st,
                        sel == Some(i),
                        pic,
                        &tl.advance_mode,
                    );
                    if c.double_clicked() {
                        send(&ctx, "scene_jump_to_cue", i);
                    } else if c.clicked() {
                        select(&ctx, info, Some(i));
                    }
                    cards.push(c.rect);
                }
                let (gap, _) = ui.allocate_exact_size(vec2(JOINT_W, CARD_H), Sense::hover());
                joints.push(gap);
                add_card(ui, info);
            });
            follow_playhead(ui, tl, &cards, &joints);
        });
    drop_between(ui, out.inner_rect, &cards, &joints);
}

/// How far in from the view's left edge the playhead lands after the strip
/// turns a page.
const FOLLOW_LEAD: f32 = 40.0;

/// Keep what is playing in view, a page at a time: when the playhead, or a
/// cue with no hold time to run a playhead across, reaches the edge of the
/// view, the strip scrolls it back to near the left edge and it travels
/// across again. Scrolling it along continuously would take the strip away
/// from anyone looking elsewhere in it. Nothing moves while a card or a
/// preset is being dragged, so a drop lands where it was aimed.
fn follow_playhead(ui: &mut Ui, tl: &TimelineInfo, cards: &[Rect], joints: &[Rect]) {
    if egui::DragAndDrop::has_any_payload(ui.ctx()) {
        return;
    }
    let target = (0..cards.len()).find_map(|i| match play_state(tl, i) {
        Play::Holding(Some(frac)) => {
            let r = cards[i].shrink(PAD);
            let x = r.left() + frac * r.width();
            Some(Rect::from_x_y_ranges(x..=x, r.y_range()))
        }
        Play::Holding(None) => Some(cards[i]),
        Play::Arriving(progress) => {
            let j = joints[i];
            let x = j.left() + progress * j.width();
            Some(Rect::from_x_y_ranges(x..=x, j.y_range()))
        }
        Play::Leaving | Play::Not => None,
    });
    let Some(target) = target else {
        return;
    };
    let view = ui.clip_rect();
    let inside = target.left() >= view.left() && target.right() <= view.right() - FOLLOW_LEAD;
    if !inside {
        let lead = Rect::from_x_y_ranges(
            (target.left() - FOLLOW_LEAD)..=target.right(),
            target.y_range(),
        );
        ui.scroll_to_rect(lead, Some(Align::Min));
    }
}

/// Accept a cue dragged to a new place, or a preset dragged in, between
/// the cards.
fn drop_between(ui: &mut Ui, area: Rect, cards: &[Rect], joints: &[Rect]) {
    let ctx = ui.ctx().clone();
    let cue = egui::DragAndDrop::payload::<CueDrag>(&ctx);
    let preset = egui::DragAndDrop::payload::<PresetDrag>(&ctx);
    if cue.is_none() && preset.is_none() {
        return;
    }
    let Some(pos) = ctx.pointer_latest_pos().filter(|p| area.contains(*p)) else {
        return;
    };
    // The slot is the number of cards whose middle is left of the pointer.
    let slot = cards.iter().filter(|r| r.center().x < pos.x).count();
    // A cue dropped either side of itself stays where it is.
    let moves_to = cue.map(|c| (c.index, if slot > c.index { slot - 1 } else { slot }));
    if moves_to.is_some_and(|(from, to)| from == to) {
        return;
    }
    if let Some(j) = joints.get(slot) {
        let tc = theme_colors(&ctx);
        let layer = egui::LayerId::new(egui::Order::Foreground, Id::new("v2_cue_drop"));
        let p = ctx.layer_painter(layer);
        let x = j.center().x;
        p.line_segment(
            [pos2(x, j.top()), pos2(x, j.bottom())],
            Stroke::new(3.0_f32, tc.text_primary),
        );
    }
    if ctx.input(|i| i.pointer.any_released()) {
        if let Some((from, to)) = moves_to {
            if egui::DragAndDrop::take_payload::<CueDrag>(&ctx).is_some() {
                send(&ctx, "scene_move_cue", (from, to));
            }
        } else if let Some(p) = egui::DragAndDrop::take_payload::<PresetDrag>(&ctx) {
            send(&ctx, "scene_insert_cue", (p.name.clone(), slot));
        }
    }
}

/// The transition into cue `i`, drawn as the joint before its card: a bar
/// for a cut, a cross for a dissolve, a wave for a morph, each named
/// underneath. Its shape carries the type; no hue does.
fn joint(ui: &mut Ui, i: usize, cue: &CueDisplayInfo, st: Play, selected: bool) -> egui::Response {
    let tc = theme_colors(ui.ctx());
    let (rect, r) = ui.allocate_exact_size(vec2(JOINT_W, CARD_H), Sense::click());
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let color = if selected || r.hovered() {
            tc.text_primary
        } else {
            tc.text_secondary
        };
        let s = Stroke::new(if selected { 2.0_f32 } else { 1.5 }, color);
        let c = pos2(rect.center().x, rect.top() + PAD + PIC.y / 2.0);
        match cue.transition {
            TransitionType::Cut => {
                p.line_segment([c - vec2(0.0, 12.0), c + vec2(0.0, 12.0)], s);
            }
            TransitionType::Dissolve => {
                p.line_segment([c + vec2(-10.0, -10.0), c + vec2(10.0, 10.0)], s);
                p.line_segment([c + vec2(-10.0, 10.0), c + vec2(10.0, -10.0)], s);
            }
            TransitionType::ParamMorph => {
                let pts: Vec<_> = (0..=16)
                    .map(|k| {
                        let t = k as f32 / 16.0;
                        c + vec2(-12.0 + 24.0 * t, -5.0 * (t * std::f32::consts::TAU).sin())
                    })
                    .collect();
                p.add(egui::Shape::line(pts, s));
            }
        }
        let words = match cue.transition {
            TransitionType::Cut => "cut".to_string(),
            _ => format!("{} s", widgets::fmt_val(cue.transition_secs as f64)),
        };
        p.text(
            c + vec2(0.0, 22.0),
            egui::Align2::CENTER_TOP,
            words,
            egui::FontId::proportional(12.0),
            color,
        );
        // Through the glyph, stopping above the duration it would hide.
        if let Play::Arriving(progress) = st {
            let x = rect.left() + progress * rect.width();
            playhead(p, x, rect.top() + PAD, c.y + 18.0, &tc);
        }
    }
    r.on_hover_text(format!(
        "Into cue {}: {}. Click to edit.",
        i + 1,
        match cue.transition {
            TransitionType::Cut => "a cut".to_string(),
            t => format!(
                "{} over {} s",
                t.display_name(),
                widgets::fmt_val(cue.transition_secs as f64)
            ),
        }
    ))
}

/// One cue: its preset's picture, its name and how long it holds.
#[allow(clippy::too_many_arguments)]
fn card(
    ui: &mut Ui,
    i: usize,
    cue: &CueDisplayInfo,
    w: f32,
    st: Play,
    selected: bool,
    pic: Option<TextureId>,
    mode: &AdvanceMode,
) -> egui::Response {
    let tc = theme_colors(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(vec2(w, CARD_H), Sense::hover());
    let r = ui.interact(
        rect,
        Id::new("v2_cue_card").with(i),
        Sense::click_and_drag(),
    );
    r.dnd_set_drag_payload(CueDrag { index: i });
    if r.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    }
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        // On screen: a heavy outline, whether holding or fading out.
        let on_screen = matches!(st, Play::Holding(_) | Play::Leaving);
        let stroke = if on_screen {
            Stroke::new(2.5_f32, tc.text_primary)
        } else if r.hovered() {
            Stroke::new(1.5_f32, tc.hover_border)
        } else {
            Stroke::new(1.0_f32, tc.card_border)
        };
        p.rect(rect, 5.0, tc.card_bg, stroke, egui::StrokeKind::Inside);

        let pic_rect = Rect::from_min_size(rect.min + vec2(PAD, PAD), vec2(w - 2.0 * PAD, PIC.y));
        match pic {
            Some(tex) => {
                // Fill the width, cropping top and bottom to keep 16:9.
                let keep = ((16.0 / 9.0) / (pic_rect.width() / pic_rect.height())).min(1.0);
                let uv =
                    Rect::from_min_max(pos2(0.0, 0.5 - keep / 2.0), pos2(1.0, 0.5 + keep / 2.0));
                p.rect_filled(pic_rect, 3.0, Color32::BLACK);
                p.image(tex, pic_rect, uv, Color32::WHITE);
            }
            None => {
                p.rect_filled(pic_rect, 3.0, tc.widget_bg);
                p.text(
                    pic_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    widgets::truncate_chars(&cue.preset_name, 18),
                    egui::FontId::proportional(13.0),
                    tc.text_secondary,
                );
            }
        }
        if cue.layers > 1 {
            let chip =
                Rect::from_min_size(pic_rect.right_top() + vec2(-30.0, 4.0), vec2(26.0, 18.0));
            p.rect_filled(chip, 4.0, Color32::from_black_alpha(170));
            p.text(
                chip.center(),
                egui::Align2::CENTER_CENTER,
                format!("+{}", cue.layers - 1),
                egui::FontId::proportional(12.0),
                Color32::WHITE,
            );
        }
        let band = Rect::from_min_size(
            pos2(pic_rect.left(), pic_rect.bottom() + 4.0),
            vec2(pic_rect.width(), NAME_H),
        );
        let text_color = if selected {
            p.rect_filled(band, 3.0, tc.selection);
            tc.on_selection
        } else {
            tc.text_primary
        };
        let prefix = match st {
            Play::Holding(_) | Play::Leaving => "\u{25B6} ",
            Play::Arriving(_) => "\u{2192} ",
            Play::Not => "",
        };
        let name = cue.label.as_deref().unwrap_or(&cue.preset_name);
        let mut t = RichText::new(format!("{prefix}{}. {name}", i + 1)).size(13.0);
        if on_screen || selected {
            t = t.strong();
        }
        let g = egui::WidgetText::from(t).into_galley(
            ui,
            Some(egui::TextWrapMode::Truncate),
            band.width() - 8.0,
            egui::TextStyle::Body,
        );
        p.galley(
            pos2(band.left() + 4.0, band.center().y - g.size().y / 2.0),
            g,
            text_color,
        );
        let when = time_words(cue, mode);
        if !when.is_empty() {
            p.text(
                pos2(band.left() + 4.0, band.bottom() + 1.0),
                egui::Align2::LEFT_TOP,
                when,
                egui::FontId::proportional(12.0),
                tc.text_secondary,
            );
        }
        // Down the picture only, so it never crosses the name.
        if let Play::Holding(Some(frac)) = st {
            let x = pic_rect.left() + frac * pic_rect.width();
            playhead(p, x, pic_rect.top(), pic_rect.bottom(), &tc);
        }
    }
    let tip = match &cue.label {
        Some(l) => format!("{l} (preset {})", cue.preset_name),
        None => cue.preset_name.clone(),
    };
    r.on_hover_text(format!(
        "{tip}\nClick to edit, double-click to play from here, drag to move."
    ))
}

/// The last card: a dashed outline that adds a cue from a preset picker.
fn add_card(ui: &mut Ui, info: &SceneInfo) {
    let tc = theme_colors(ui.ctx());
    let (rect, r) = ui.allocate_exact_size(vec2(CARD_MIN_W, CARD_H), Sense::click());
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let color = if r.hovered() {
            tc.text_primary
        } else {
            tc.text_secondary
        };
        let s = Stroke::new(1.5_f32, color);
        let rr = rect.shrink(1.0);
        for (a, b) in [
            (rr.left_top(), rr.right_top()),
            (rr.right_top(), rr.right_bottom()),
            (rr.right_bottom(), rr.left_bottom()),
            (rr.left_bottom(), rr.left_top()),
        ] {
            p.extend(egui::Shape::dashed_line(&[a, b], s, 6.0, 4.0));
        }
        p.text(
            rect.center() - vec2(0.0, 10.0),
            egui::Align2::CENTER_CENTER,
            "+ Add preset",
            egui::FontId::proportional(14.0),
            color,
        );
        p.text(
            rect.center() + vec2(0.0, 12.0),
            egui::Align2::CENTER_CENTER,
            "or drag one here",
            egui::FontId::proportional(12.0),
            tc.text_secondary,
        );
    }
    egui::Popup::menu(&r)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.set_min_width(240.0);
            let q_id = Id::new("v2_add_cue_search");
            let mut q: String = ui.data(|d| d.get_temp(q_id)).unwrap_or_default();
            let edit = ui.add(egui::TextEdit::singleline(&mut q).hint_text("Search presets"));
            if !edit.has_focus() && q.is_empty() {
                edit.request_focus();
            }
            ui.data_mut(|d| d.insert_temp(q_id, q.clone()));
            let q = q.trim().to_lowercase();
            ScrollArea::vertical().max_height(280.0).show(ui, |ui| {
                let mut any = false;
                for name in info
                    .preset_names
                    .iter()
                    .filter(|n| q.is_empty() || n.to_lowercase().contains(&q))
                {
                    any = true;
                    if ui.button(RichText::new(name).size(13.0)).clicked() {
                        send(ui.ctx(), "scene_add_cue", name.clone());
                        ui.data_mut(|d| d.remove::<String>(q_id));
                        ui.close();
                    }
                }
                if !any {
                    ui.label(
                        RichText::new("No preset matches.")
                            .size(12.0)
                            .color(tc.text_secondary),
                    );
                }
            });
        });
}

// ── The selected cue ────────────────────────────────────────────────────

fn edit_row(ui: &mut Ui, info: &SceneInfo, tl: &TimelineInfo) {
    let tc = theme_colors(ui.ctx());
    let ctx = ui.ctx().clone();
    let Some(i) = selected(&ctx, info) else {
        let words = if info.cue_list.is_empty() {
            "Add presets to the strip, in the order they should play."
        } else {
            "Click a cue to edit its transition, timing and name. Double-click to play from it. \
             Drag cards to reorder them, or drag presets in from the preset tiles."
        };
        ui.label(RichText::new(words).size(12.0).color(tc.text_secondary));
        return;
    };
    let cue = &info.cue_list[i];
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.label(RichText::new(format!("Cue {}", i + 1)).size(13.0).strong());
        ui.label(
            RichText::new(&cue.preset_name)
                .size(13.0)
                .color(tc.text_secondary),
        );
        ui.separator();

        ui.label(RichText::new("In").size(13.0).color(tc.text_secondary));
        for &t in TransitionType::ALL {
            if ui
                .selectable_label(
                    cue.transition == t,
                    RichText::new(t.display_name()).size(13.0),
                )
                .clicked()
                && cue.transition != t
            {
                send(&ctx, "scene_set_cue_transition", (i, t));
            }
        }
        if cue.transition != TransitionType::Cut {
            if let Some(v) = live_drag(
                ui,
                Id::new("v2_cue_trans").with(i),
                cue.transition_secs,
                |v| {
                    egui::DragValue::new(v)
                        .range(0.1..=30.0)
                        .speed(0.05)
                        .suffix(" s")
                        .max_decimals(1)
                },
            ) {
                send(&ctx, "scene_set_cue_transition_secs", (i, v));
            }
        }

        if tl.advance_mode == AdvanceMode::Timer {
            ui.separator();
            let mut timed = cue.hold_secs.is_some();
            if ui
                .checkbox(&mut timed, RichText::new("Next cue after").size(13.0))
                .on_hover_text("Off: this cue waits for Go.")
                .changed()
            {
                if timed {
                    send(&ctx, "scene_set_cue_hold_secs", (i, 4.0f32));
                } else {
                    send(&ctx, "scene_clear_cue_hold", i);
                }
            }
            if let Some(hold) = cue.hold_secs {
                if let Some(v) = live_drag(ui, Id::new("v2_cue_hold").with(i), hold, |v| {
                    egui::DragValue::new(v)
                        .range(0.5..=600.0)
                        .speed(0.1)
                        .suffix(" s")
                        .max_decimals(1)
                }) {
                    send(&ctx, "scene_set_cue_hold_secs", (i, v));
                }
            }
        }

        ui.separator();
        ui.label(RichText::new("Name").size(13.0).color(tc.text_secondary));
        let name_id = Id::new("v2_cue_label").with((info.current_scene, i));
        let mut text: String = ctx
            .data(|d| d.get_temp(name_id))
            .unwrap_or_else(|| cue.label.clone().unwrap_or_default());
        let r = ui.add(
            egui::TextEdit::singleline(&mut text)
                .hint_text(cue.preset_name.as_str())
                .desired_width(150.0),
        );
        if r.lost_focus() {
            if text.trim() != cue.label.as_deref().unwrap_or("") {
                send(&ctx, "scene_set_cue_label", (i, text.clone()));
            }
            ctx.data_mut(|d| d.remove::<String>(name_id));
        } else if r.has_focus() {
            ctx.data_mut(|d| d.insert_temp(name_id, text));
        }

        ui.separator();
        if ui
            .button(RichText::new("\u{25B6}  Play from here").size(13.0))
            .clicked()
        {
            send(&ctx, "scene_jump_to_cue", i);
        }
        if ui.button(RichText::new("Remove").size(13.0)).clicked() {
            send(&ctx, "scene_remove_cue", i);
            select(&ctx, info, None);
        }
    });
}

/// A scene library of three, the first current, its three cues holding on
/// the second when `playing`.
#[cfg(test)]
pub(crate) fn sample(playing: bool) -> SceneInfo {
    let cue = |name: &str, transition| CueDisplayInfo {
        preset_name: name.to_string(),
        transition,
        transition_secs: 1.0,
        hold_secs: Some(4.0),
        label: None,
        effect: None,
        layers: 2,
    };
    SceneInfo {
        scene_store_names: vec!["Opener".into(), "Middle".into(), "Close".into()],
        current_scene: Some(0),
        timeline: Some(TimelineInfo {
            active: playing,
            cue_count: 3,
            current_cue: 1,
            state: if playing {
                TimelineInfoState::Holding {
                    elapsed: 1.0,
                    hold_secs: Some(4.0),
                }
            } else {
                TimelineInfoState::Idle
            },
            loop_mode: false,
            advance_mode: AdvanceMode::Timer,
        }),
        preset_names: vec!["Tide".into(), "Sumi".into()],
        cue_list: vec![
            cue("Tide", TransitionType::Cut),
            cue("Sumi", TransitionType::Dissolve),
            cue("Pegboard", TransitionType::ParamMorph),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Height of what `draw` puts in a 1000 x 2000 panel, after a few
    /// frames.
    fn height(mut draw: impl FnMut(&mut Ui)) -> f32 {
        let ctx = egui::Context::default();
        crate::ui::theme::colors::set_theme_colors(
            &ctx,
            crate::ui::theme::palette::Palette::GRAY.colors(),
        );
        let mut h = 0.0;
        for _ in 0..3 {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(1000.0, 2000.0))),
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    // Every cue selected in turn is the same; select the first.
                    select(ui.ctx(), &sample(false), Some(0));
                    h = ui.scope(|ui| draw(ui)).response.rect.height();
                });
            });
        }
        h
    }

    // Each piece is as tall as its rows in a panel with far more height
    // than it needs: a row laid out with a bare `with_layout` takes all the
    // height below it, and only a height assertion catches that.
    #[test]
    fn each_piece_is_as_tall_as_its_rows() {
        for playing in [false, true] {
            let info = sample(playing);
            let tl = info.timeline.clone().unwrap();
            let t = height(|ui| transport(ui, &tl, true));
            assert!(
                t <= MIN_INTERACT_HEIGHT + 1.0,
                "playing {playing}: transport {t}"
            );
            let s = height(|ui| strip(ui, &info, &tl, &mut |_, _| None));
            assert!(s < CARD_H + 20.0, "playing {playing}: strip {s}");
            let e = height(|ui| edit_row(ui, &info, &tl));
            assert!(
                e < 2.0 * MIN_INTERACT_HEIGHT + 12.0,
                "playing {playing}: edit row {e}"
            );
            let l = height(|ui| scene_list(ui, &info));
            assert!(
                l < 5.0 * (MIN_INTERACT_HEIGHT + 6.0),
                "playing {playing}: list {l}"
            );
        }
    }

    fn cue(hold: Option<f32>) -> CueDisplayInfo {
        CueDisplayInfo {
            preset_name: "Tide".into(),
            transition: TransitionType::Cut,
            transition_secs: 1.0,
            hold_secs: hold,
            label: None,
            effect: None,
            layers: 1,
        }
    }

    // In Timer mode the strip is a real timeline: a longer hold is a wider
    // card. Nothing is narrower than its picture, and a cue that waits for
    // Go has no length to show.
    #[test]
    fn timer_cards_are_as_wide_as_their_hold() {
        let cues = [cue(Some(4.0)), cue(Some(8.0)), cue(Some(16.0)), cue(None)];
        let w = card_widths(&cues, &AdvanceMode::Timer, 1600.0);
        assert!(w[0] < w[1] && w[1] < w[2], "{w:?}");
        assert!(w.iter().all(|&x| x >= CARD_MIN_W), "{w:?}");
        assert_eq!(w[3], CARD_MIN_W);
        for mode in [
            AdvanceMode::Manual,
            AdvanceMode::BeatSync { beats_per_cue: 8 },
        ] {
            let w = card_widths(&cues, &mode, 1600.0);
            assert!(w.iter().all(|&x| x == CARD_MIN_W), "{mode:?}: {w:?}");
        }
    }

    #[test]
    fn a_cue_plays_holding_leaving_or_arriving() {
        let tl = |state| TimelineInfo {
            active: true,
            cue_count: 3,
            current_cue: 1,
            state,
            loop_mode: false,
            advance_mode: AdvanceMode::Manual,
        };
        let holding = tl(TimelineInfoState::Holding {
            elapsed: 2.0,
            hold_secs: Some(8.0),
        });
        assert_eq!(play_state(&holding, 1), Play::Holding(Some(0.25)));
        assert_eq!(play_state(&holding, 0), Play::Not);
        let moving = tl(TimelineInfoState::Transitioning {
            from: 0,
            to: 1,
            progress: 0.5,
            transition_type: TransitionType::Dissolve,
        });
        assert_eq!(play_state(&moving, 0), Play::Leaving);
        assert_eq!(play_state(&moving, 1), Play::Arriving(0.5));
        assert_eq!(play_state(&moving, 2), Play::Not);
    }

    /// The strip alone, driven by real pointer events, one frame per call.
    struct Harness {
        ctx: egui::Context,
        info: SceneInfo,
        time: f64,
    }

    impl Harness {
        fn new() -> Self {
            Self::with(sample(false))
        }

        fn with(info: SceneInfo) -> Self {
            let ctx = egui::Context::default();
            crate::ui::theme::colors::set_theme_colors(
                &ctx,
                crate::ui::theme::palette::Palette::GRAY.colors(),
            );
            Self {
                ctx,
                info,
                time: 0.0,
            }
        }

        fn frame(&mut self, events: Vec<egui::Event>) {
            self.time += 1.0 / 60.0;
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(1400.0, 400.0))),
                time: Some(self.time),
                events,
                ..Default::default()
            };
            let info = &self.info;
            let _ = self.ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let tl = info.timeline.clone().unwrap();
                    strip(ui, info, &tl, &mut |_, _| None);
                });
            });
        }

        fn card(&self, i: usize) -> Rect {
            self.ctx
                .read_response(Id::new("v2_cue_card").with(i))
                .expect("card drawn")
                .rect
        }

        fn take<T: Clone + Send + Sync + 'static>(&self, key: &str) -> Option<T> {
            self.ctx.data_mut(|d| {
                let v = d.get_temp::<T>(Id::new(key));
                d.remove::<T>(Id::new(key));
                v
            })
        }

        /// Press at `from`, move to `to` in steps, release there.
        fn drag(&mut self, from: egui::Pos2, to: egui::Pos2) {
            use egui::{Event, PointerButton};
            let press = |pos, pressed| Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            };
            self.frame(vec![Event::PointerMoved(from), press(from, true)]);
            for k in 1..=8 {
                let p = from + (to - from) * (k as f32 / 8.0);
                self.frame(vec![Event::PointerMoved(p)]);
            }
            self.frame(vec![press(to, false)]);
        }
    }

    #[test]
    fn dragging_a_card_moves_its_cue() {
        let mut h = Harness::new();
        h.frame(vec![]);
        h.frame(vec![]);
        // Cue 1 past the middle of cue 3: it lands last.
        let (from, to) = (h.card(0).center(), h.card(2).center() + vec2(20.0, 0.0));
        h.drag(from, to);
        assert_eq!(h.take::<(usize, usize)>("scene_move_cue"), Some((0, 2)));
        // Dropped on its own place, it stays.
        let from = h.card(1).center();
        h.drag(from, from + vec2(10.0, 0.0));
        assert_eq!(h.take::<(usize, usize)>("scene_move_cue"), None);
        // Cue 3 before cue 1.
        let (from, to) = (h.card(2).center(), h.card(0).left_center() + vec2(4.0, 0.0));
        h.drag(from, to);
        assert_eq!(h.take::<(usize, usize)>("scene_move_cue"), Some((2, 0)));
    }

    #[test]
    fn a_preset_dropped_between_cards_is_inserted_there() {
        let mut h = Harness::new();
        h.frame(vec![]);
        h.frame(vec![]);
        let between = (h.card(0).right_center() + h.card(1).left_center().to_vec2()) / 2.0;
        egui::DragAndDrop::set_payload(
            &h.ctx,
            PresetDrag {
                name: "Sumi".into(),
            },
        );
        h.frame(vec![egui::Event::PointerMoved(between)]);
        h.frame(vec![egui::Event::PointerButton {
            pos: between,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Default::default(),
        }]);
        assert_eq!(
            h.take::<(String, usize)>("scene_insert_cue"),
            Some(("Sumi".to_string(), 1))
        );
    }

    // Editing a cue must never cut the show to it: a click selects, only a
    // double-click jumps.
    #[test]
    fn a_click_selects_a_cue_and_a_double_click_plays_it() {
        use egui::{Event, PointerButton};
        let mut h = Harness::new();
        h.frame(vec![]);
        h.frame(vec![]);
        let at = h.card(1).center();
        let press = |pressed| Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        h.frame(vec![Event::PointerMoved(at), press(true)]);
        h.frame(vec![press(false)]);
        assert_eq!(selected(&h.ctx, &h.info), Some(1));
        assert_eq!(h.take::<usize>("scene_jump_to_cue"), None);
        h.frame(vec![press(true)]);
        h.frame(vec![press(false)]);
        assert_eq!(h.take::<usize>("scene_jump_to_cue"), Some(1));
    }

    /// Twenty cues, far wider than the view, holding on `cue` with `frac`
    /// of its hold gone.
    fn long_scene(cue: usize, frac: f32) -> SceneInfo {
        let mut info = sample(true);
        let one = info.cue_list[0].clone();
        info.cue_list = vec![one; 20];
        let tl = info.timeline.as_mut().unwrap();
        tl.cue_count = 20;
        tl.current_cue = cue;
        tl.state = TimelineInfoState::Holding {
            elapsed: 4.0 * frac,
            hold_secs: Some(4.0),
        };
        info
    }

    fn settle(h: &mut Harness) {
        // Long enough for the scroll animation to finish.
        for _ in 0..60 {
            h.frame(vec![]);
        }
    }

    // The playhead went off the right of the view and the strip stayed put.
    // Now the strip turns a page and brings it back near the left edge.
    #[test]
    fn the_strip_follows_the_playhead_off_the_edge() {
        let mut h = Harness::with(long_scene(0, 0.5));
        settle(&mut h);
        let first = h.card(0);
        assert!(first.left() >= 0.0, "cue 1 plays and is in view: {first:?}");

        // Cue 8 starts past the view; there is strip enough beyond it to
        // turn a whole page (near the end, the page stops at the end).
        h.info = long_scene(7, 0.5);
        settle(&mut h);
        let c = h.card(7);
        let x = c.left() + PAD + 0.5 * (c.width() - 2.0 * PAD);
        assert!(
            (0.0..1400.0 - FOLLOW_LEAD).contains(&x),
            "the playhead of cue 8 is at {x:.0}, outside the view"
        );
        // Near the left edge, with the page ahead of it.
        assert!(x < 200.0, "the page turned only as far as {x:.0}");

        // And it holds still while the playhead crosses the page.
        h.info = long_scene(7, 0.9);
        settle(&mut h);
        assert!(
            (h.card(7).left() - c.left()).abs() < 1.0,
            "the strip moved mid-page"
        );
    }

    // A drag in progress keeps the strip still, so a drop lands where it
    // was aimed.
    #[test]
    fn the_strip_does_not_follow_during_a_drag() {
        let mut h = Harness::with(long_scene(0, 0.5));
        settle(&mut h);
        let before = h.card(0);
        egui::DragAndDrop::set_payload(
            &h.ctx,
            PresetDrag {
                name: "Sumi".into(),
            },
        );
        h.info = long_scene(10, 0.5);
        settle(&mut h);
        assert_eq!(h.card(0), before);
    }

    /// The lowest point of any vertical line the strip draws for `info`,
    /// relative to the strip's top.
    fn playhead_bottom(info: &SceneInfo) -> f32 {
        let ctx = egui::Context::default();
        crate::ui::theme::colors::set_theme_colors(
            &ctx,
            crate::ui::theme::palette::Palette::GRAY.colors(),
        );
        let mut top = 0.0;
        let mut out = None;
        for _ in 0..2 {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(1400.0, 400.0))),
                ..Default::default()
            };
            out = Some(ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    top = ui.max_rect().top();
                    let tl = info.timeline.clone().unwrap();
                    strip(ui, info, &tl, &mut |_, _| None);
                });
            }));
        }
        out.unwrap()
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                // The Cut glyph is a vertical line too; it ends at 12 px
                // below the glyph's middle, above any label.
                egui::Shape::LineSegment { points, stroke }
                    if points[0].x == points[1].x && stroke.width >= 2.0 =>
                {
                    Some(points[0].y.max(points[1].y) - top)
                }
                _ => None,
            })
            .fold(f32::MIN, f32::max)
    }

    // The playhead ran the card's full height through its name, and through
    // the joint's duration label, which then read "l s" for "1 s".
    #[test]
    fn the_playhead_never_crosses_text() {
        let holding = sample(true);
        let b = playhead_bottom(&holding);
        assert!(b > PAD, "no playhead drawn");
        assert!(
            b <= PAD + PIC.y + 0.5,
            "a holding playhead reaches {b}, below the picture"
        );

        let mut moving = sample(true);
        moving.timeline.as_mut().unwrap().state = TimelineInfoState::Transitioning {
            from: 0,
            to: 1,
            progress: 0.5,
            transition_type: TransitionType::Dissolve,
        };
        let label_top = PAD + PIC.y / 2.0 + 22.0;
        let b = playhead_bottom(&moving);
        assert!(b > PAD, "no playhead drawn");
        assert!(
            b < label_top,
            "a transition's playhead reaches {b}, into its label at {label_top}"
        );
    }
}
