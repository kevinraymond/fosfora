use egui::{RichText, Ui};

use crate::media::types::PlayDirection;
use crate::ui::theme::colors::theme_colors;
use crate::ui::theme::tokens::*;

/// Info about the active media layer, collected before UI borrow.
pub struct MediaInfo {
    pub file_name: String,
    pub media_width: u32,
    pub media_height: u32,
    pub frame_count: usize,
    pub is_animated: bool,
    pub is_video: bool,
    pub playing: bool,
    pub looping: bool,
    pub speed: f32,
    pub direction: PlayDirection,
    pub current_frame: usize,
    pub video_position_secs: f64,
    pub video_duration_secs: f64,
}

pub fn draw_media_panel(ui: &mut Ui, info: &MediaInfo) {
    let tc = theme_colors(ui.ctx());

    // File info
    ui.label(
        RichText::new(&info.file_name)
            .size(BODY_SIZE)
            .color(tc.text_primary),
    );
    ui.label(
        RichText::new(format!("{}x{}", info.media_width, info.media_height))
            .size(SMALL_SIZE)
            .color(tc.text_secondary),
    );

    if info.is_video {
        // While seek slider is being dragged, show drag position in time display
        let seek_id = egui::Id::new("media_seek_drag");
        let drag_pos: Option<f64> = ui.ctx().data(|d| d.get_temp(seek_id)).flatten();
        let display_pos = drag_pos.unwrap_or(info.video_position_secs);

        // Video-specific UI
        ui.label(
            RichText::new(format!(
                "{} / {}",
                format_time(display_pos),
                format_time(info.video_duration_secs),
            ))
            .size(SMALL_SIZE)
            .color(tc.text_secondary),
        );

        ui.add_space(4.0);

        // Play/Pause + Loop
        ui.horizontal(|ui| {
            let play_label = if info.playing { "Pause" } else { "Play" };
            if ui
                .button(RichText::new(play_label).size(SMALL_SIZE))
                .clicked()
            {
                ui.ctx().data_mut(|d| {
                    d.insert_temp(egui::Id::new("media_play_pause"), true);
                });
            }

            let mut looping = info.looping;
            if ui.checkbox(&mut looping, "Loop").changed() {
                ui.ctx().data_mut(|d| {
                    d.insert_temp(egui::Id::new("media_loop"), looping);
                });
            }
        });

        // Seek slider
        if info.video_duration_secs > 0.0 {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Seek")
                        .size(SMALL_SIZE)
                        .color(tc.text_secondary),
                );
                let mut pos = display_pos;
                let slider = ui.add(
                    egui::Slider::new(&mut pos, 0.0..=info.video_duration_secs)
                        .show_value(false)
                        .text(""),
                );
                if slider.changed() {
                    // Seek on every change (real-time scrubbing)
                    ui.ctx().data_mut(|d| {
                        d.insert_temp(seek_id, Some(pos));
                        d.insert_temp(egui::Id::new("media_seek"), pos);
                    });
                }
                if slider.drag_stopped() {
                    // Clear drag position override
                    ui.ctx().data_mut(|d| {
                        d.remove_temp::<Option<f64>>(seek_id);
                    });
                }
            });
        }

        // Speed slider
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Speed")
                    .size(SMALL_SIZE)
                    .color(tc.text_secondary),
            );
            let mut speed = info.speed;
            let slider = ui.add(
                egui::Slider::new(&mut speed, 0.1..=4.0)
                    .show_value(true)
                    .custom_formatter(|v, _| format!("{:.1}x", v))
                    .text(""),
            );
            if slider.changed() {
                ui.ctx().data_mut(|d| {
                    d.insert_temp(egui::Id::new("media_speed"), speed);
                });
            }
        });

        // No direction selector for video (forward only)
    } else if info.is_animated {
        ui.label(
            RichText::new(format!(
                "Frame {}/{}",
                info.current_frame + 1,
                info.frame_count
            ))
            .size(SMALL_SIZE)
            .color(tc.text_secondary),
        );

        ui.add_space(4.0);

        // Play/Pause
        ui.horizontal(|ui| {
            let play_label = if info.playing { "Pause" } else { "Play" };
            if ui
                .button(RichText::new(play_label).size(SMALL_SIZE))
                .clicked()
            {
                ui.ctx().data_mut(|d| {
                    d.insert_temp(egui::Id::new("media_play_pause"), true);
                });
            }

            // Loop toggle
            let mut looping = info.looping;
            if ui.checkbox(&mut looping, "Loop").changed() {
                ui.ctx().data_mut(|d| {
                    d.insert_temp(egui::Id::new("media_loop"), looping);
                });
            }
        });

        // Speed slider
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Speed")
                    .size(SMALL_SIZE)
                    .color(tc.text_secondary),
            );
            let mut speed = info.speed;
            let slider = ui.add(
                egui::Slider::new(&mut speed, 0.1..=4.0)
                    .show_value(true)
                    .custom_formatter(|v, _| format!("{:.1}x", v))
                    .text(""),
            );
            if slider.changed() {
                ui.ctx().data_mut(|d| {
                    d.insert_temp(egui::Id::new("media_speed"), speed);
                });
            }
        });

        // Direction
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Direction")
                    .size(SMALL_SIZE)
                    .color(tc.text_secondary),
            );
            let directions: [(PlayDirection, u8, &str); 3] = [
                (PlayDirection::Forward, 0, "Fwd"),
                (PlayDirection::Reverse, 1, "Rev"),
                (PlayDirection::PingPong, 2, "Ping-Pong"),
            ];
            for (dir, dir_u8, label) in &directions {
                let selected = info.direction == *dir;
                if ui
                    .selectable_label(selected, RichText::new(*label).size(SMALL_SIZE))
                    .clicked()
                    && !selected
                {
                    ui.ctx().data_mut(|d| {
                        d.insert_temp(egui::Id::new("media_direction"), *dir_u8);
                    });
                }
            }
        });
    } else {
        ui.label(
            RichText::new("Static image")
                .size(SMALL_SIZE)
                .color(tc.text_secondary),
        );
    }
}

fn format_time(secs: f64) -> String {
    let total_secs = secs.max(0.0) as u64;
    let mins = total_secs / 60;
    let s = total_secs % 60;
    format!("{:02}:{:02}", mins, s)
}

/// A media file still decoding for a new layer, as the panels show it.
#[derive(Clone)]
pub struct MediaLoading {
    pub file_name: String,
    /// Frames decoded, and how many the probe expects (0 = not known yet).
    pub done: u32,
    pub total: u32,
    pub secs: f32,
}

impl MediaLoading {
    pub fn of(load: &crate::app::MediaLoad) -> Self {
        use std::sync::atomic::Ordering;
        Self {
            file_name: load.file_name.clone(),
            done: load.progress.done.load(Ordering::Relaxed),
            total: load.progress.total.load(Ordering::Relaxed),
            secs: load.started.elapsed().as_secs_f32(),
        }
    }

    /// `"212 of 450 frames"`, or what is happening before frames arrive.
    fn words(&self) -> String {
        if self.total > 0 {
            format!("{} of {} frames", self.done.min(self.total), self.total)
        } else {
            "reading the file".to_string()
        }
    }
}

/// One line per file still loading, where the new layer will appear: what,
/// how far, and a way to stop it. A video decodes every frame up front, and
/// before this the app froze for the whole of it with nothing on screen.
pub fn draw_loading(ui: &mut Ui) {
    let loads: Option<std::sync::Arc<Vec<MediaLoading>>> = ui
        .ctx()
        .data(|d| d.get_temp(egui::Id::new("media_loading")));
    let Some(loads) = loads.filter(|l| !l.is_empty()) else {
        return;
    };
    let tc = theme_colors(ui.ctx());
    for (i, l) in loads.iter().enumerate() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(RichText::new(format!("Loading {}", l.file_name)).size(13.0));
            let fraction = if l.total > 0 {
                l.done as f32 / l.total as f32
            } else {
                0.0
            };
            ui.add(
                egui::ProgressBar::new(fraction.clamp(0.0, 1.0))
                    .desired_width(160.0)
                    .text(RichText::new(l.words()).size(11.0)),
            );
            ui.label(
                RichText::new(format!("{:.0} s", l.secs))
                    .size(11.0)
                    .color(tc.text_secondary),
            );
            if ui.small_button("Cancel").clicked() {
                ui.ctx()
                    .data_mut(|d| d.insert_temp(egui::Id::new("cancel_media_load"), i));
            }
        });
    }
    ui.label(
        RichText::new(
            "A video decodes every frame before it plays; the layer appears when it is done.",
        )
        .size(11.0)
        .color(tc.text_secondary),
    );
    ui.ctx().request_repaint();
}
