use egui::{Color32, CornerRadius, Rect, StrokeKind, Ui, Vec2};

use crate::scene::timeline::{TimelineInfo, TimelineInfoState};
use crate::scene::types::TransitionType;
use crate::ui::theme::colors::theme_colors;
use crate::ui::theme::tokens::*;
use crate::ui::widgets;

/// Draw a horizontal timeline bar showing cue blocks and playhead.
/// Returns true if the timeline bar is visible and was drawn.
pub fn draw_timeline_bar(ui: &mut Ui, timeline: &TimelineInfo, cue_names: &[String]) -> bool {
    if !timeline.active || timeline.cue_count == 0 {
        return false;
    }

    let tc = theme_colors(ui.ctx());
    let available_width = ui.available_width();
    let bar_height = 32.0;

    // Extract transition state once for use below
    let transition_state = match &timeline.state {
        TimelineInfoState::Transitioning {
            from,
            to,
            progress,
            transition_type,
        } => Some((*from, *to, *progress, *transition_type)),
        _ => None,
    };

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;

        let block_width = if timeline.cue_count > 0 {
            available_width / timeline.cue_count as f32
        } else {
            available_width
        };

        for i in 0..timeline.cue_count {
            let is_current = i == timeline.current_cue;
            let is_from = transition_state
                .as_ref()
                .map_or(false, |(from, _, _, _)| *from == i);
            let is_to = transition_state
                .as_ref()
                .map_or(false, |(_, to, _, _)| *to == i);

            // The playing cue is outlined, not filled: the playhead crosses
            // it and was lost on a solid selection fill (#3173).
            let playing = is_current && transition_state.is_none();
            let bg = if playing {
                tc.card_bg
            } else if is_to {
                tc.accent.linear_multiply(0.10)
            } else {
                tc.card_bg
            };

            let (rect, response) =
                ui.allocate_exact_size(Vec2::new(block_width, bar_height), egui::Sense::click());

            let painter = ui.painter();

            // Draw block background
            painter.rect_filled(rect, CornerRadius::same(2), bg);

            // A heavy outline on the playing cue, a hairline on the rest.
            let border_color = if playing {
                egui::Stroke::new(2.0_f32, tc.text_primary)
            } else {
                egui::Stroke::new(1.0_f32, tc.card_border)
            };
            painter.rect_stroke(
                rect,
                CornerRadius::same(2),
                border_color,
                StrokeKind::Inside,
            );

            // From-cue during transition: striped overlay
            if is_from {
                if let Some((_, _, _, transition_type)) = &transition_state {
                    let stripe_color = match transition_type {
                        TransitionType::Cut => Color32::TRANSPARENT,
                        TransitionType::Dissolve => Color32::from_rgba_unmultiplied(
                            tc.accent.r(),
                            tc.accent.g(),
                            tc.accent.b(),
                            25,
                        ),
                        TransitionType::ParamMorph => Color32::from_rgba_unmultiplied(
                            tc.success.r(),
                            tc.success.g(),
                            tc.success.b(),
                            25,
                        ),
                    };
                    widgets::draw_diagonal_stripes(painter, rect, stripe_color, 6.0);
                }
            }

            // Transition progress overlay on target cue
            if is_to {
                if let Some((_, _, progress, transition_type)) = &transition_state {
                    let overlay_color = match transition_type {
                        TransitionType::Cut => Color32::TRANSPARENT,
                        TransitionType::Dissolve => tc.accent.linear_multiply(0.2 * progress),
                        TransitionType::ParamMorph => Color32::from_rgba_unmultiplied(
                            tc.success.r(),
                            tc.success.g(),
                            tc.success.b(),
                            (40.0 * progress) as u8,
                        ),
                    };
                    let progress_rect = Rect::from_min_size(
                        rect.min,
                        Vec2::new(rect.width() * progress, rect.height()),
                    );
                    painter.rect_filled(progress_rect, CornerRadius::same(2), overlay_color);
                }
            }

            // Cue label — two lines for target cue during transition
            let cue_label = cue_names.get(i).map(|s| s.as_str()).unwrap_or("?");
            if is_to {
                if let Some((_, _, progress, transition_type)) = &transition_state {
                    // Top line: cue name
                    let name_galley = painter.layout_no_wrap(
                        cue_label.to_string(),
                        egui::FontId::proportional(SMALL_SIZE),
                        tc.text_primary,
                    );
                    let trans_label = format!(
                        "{} {:.0}%",
                        transition_type.display_name(),
                        progress * 100.0,
                    );
                    let trans_color = match transition_type {
                        TransitionType::Cut => tc.text_secondary,
                        TransitionType::Dissolve => tc.accent,
                        TransitionType::ParamMorph => tc.success,
                    };
                    let trans_galley = painter.layout_no_wrap(
                        trans_label,
                        egui::FontId::proportional(SMALL_SIZE),
                        trans_color,
                    );
                    let total_h = name_galley.size().y + trans_galley.size().y + 1.0;
                    let top_y = rect.center().y - total_h * 0.5;
                    let name_x = rect.center().x - name_galley.size().x * 0.5;
                    let trans_x = rect.center().x - trans_galley.size().x * 0.5;
                    painter.galley(egui::pos2(name_x, top_y), name_galley, tc.text_primary);
                    painter.galley(
                        egui::pos2(trans_x, top_y + total_h - trans_galley.size().y),
                        trans_galley,
                        trans_color,
                    );
                }
            } else {
                // Single-line cue name
                let label_color = if playing {
                    tc.text_primary
                } else {
                    tc.text_secondary
                };
                let galley = painter.layout_no_wrap(
                    cue_label.to_string(),
                    egui::FontId::proportional(SMALL_SIZE),
                    label_color,
                );
                let text_pos = egui::pos2(
                    rect.center().x - galley.size().x * 0.5,
                    rect.center().y - galley.size().y * 0.5,
                );
                painter.galley(text_pos, galley, label_color);
            }

            // Click to jump
            if response.clicked() {
                ui.ctx().data_mut(|d| {
                    d.insert_temp(egui::Id::new("scene_jump_to_cue"), i);
                });
            }
        }

        // Playhead indicator, in the text color so it shows on any cue.
        // Measured from the bar's own left edge: from x = 0 it sat the
        // panel's margin short of where it belonged.
        let base_x = ui.min_rect().min.x;
        let base_y = ui.min_rect().min.y;
        match &timeline.state {
            TimelineInfoState::Transitioning { to, progress, .. } => {
                let cue_x = base_x + *to as f32 * block_width;
                let playhead_x = cue_x + progress * block_width;
                let top = egui::pos2(playhead_x, base_y);
                let bottom = egui::pos2(playhead_x, base_y + bar_height);
                ui.painter()
                    .line_segment([top, bottom], egui::Stroke::new(2.0_f32, tc.text_primary));
            }
            TimelineInfoState::Holding { elapsed, hold_secs } => {
                let cue_x = base_x + timeline.current_cue as f32 * block_width;
                let playhead_x = if let Some(hold) = hold_secs {
                    let frac = (elapsed / hold).min(1.0);
                    cue_x + frac * block_width
                } else {
                    cue_x + block_width * 0.5
                };
                let top = egui::pos2(playhead_x, base_y);
                let bottom = egui::pos2(playhead_x, base_y + bar_height);
                ui.painter()
                    .line_segment([top, bottom], egui::Stroke::new(2.0_f32, tc.text_primary));
            }
            _ => {}
        }
    });

    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::types::AdvanceMode;

    /// Draw a playing three-cue bar `left` points in from the window's edge,
    /// holding cue 1 a quarter of the way through. Returns the playhead's x
    /// and the bar's left edge.
    fn playhead_x(left: f32) -> (f32, f32) {
        let ctx = egui::Context::default();
        crate::ui::theme::colors::set_theme_colors(
            &ctx,
            crate::ui::theme::palette::Palette::GRAY.colors(),
        );
        let tl = TimelineInfo {
            active: true,
            cue_count: 3,
            current_cue: 1,
            state: TimelineInfoState::Holding {
                elapsed: 1.0,
                hold_secs: Some(4.0),
            },
            loop_mode: false,
            advance_mode: AdvanceMode::Manual,
        };
        let mut bar_left = 0.0;
        let mut out = None;
        for _ in 0..2 {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1200.0, 200.0),
                )),
                ..Default::default()
            };
            out = Some(ctx.run(input, |ctx| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(left as i8, 4)))
                    .show(ctx, |ui| {
                        bar_left = ui.max_rect().left();
                        draw_timeline_bar(ui, &tl, &[]);
                    });
            }));
        }
        // The playhead is the only vertical line segment drawn.
        let x = out
            .unwrap()
            .shapes
            .iter()
            .find_map(|c| match &c.shape {
                egui::Shape::LineSegment { points, .. } if points[0].x == points[1].x => {
                    Some(points[0].x)
                }
                _ => None,
            })
            .expect("a playhead");
        (x, bar_left)
    }

    // The playhead was placed from x = 0 rather than from the bar's left
    // edge: in Classic's 2 px margin nobody saw it, and in the workspace
    // drawer it sat the margin short of the playing cue.
    #[test]
    fn the_playhead_is_inside_the_playing_cue_wherever_the_bar_starts() {
        for left in [0.0, 10.0, 60.0] {
            let (x, bar_left) = playhead_x(left);
            let block = (1200.0 - 2.0 * left) / 3.0;
            let want = bar_left + block * 1.25;
            assert!(
                (x - want).abs() < 1.0,
                "margin {left}: the playhead is at {x:.1}, a quarter into cue 2 is {want:.1}"
            );
        }
    }
}
