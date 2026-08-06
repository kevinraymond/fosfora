use egui::{RichText, Ui};

use crate::gpu::types::OutputResolution;
use crate::recording::encoder::EncoderInfo;
use crate::recording::types::{Container, RecordingConfig, VideoCodec};
use crate::ui::theme::colors::theme_colors;
use crate::ui::theme::tokens::*;

/// Snapshot of recording state for UI (avoids passing &mut RecordingSystem through draw_panels).
#[derive(Clone)]
pub struct RecordingInfo {
    pub recording: bool,
    pub has_audio: bool,
    pub ffmpeg_found: bool,
    pub encoder_info: EncoderInfo,
    pub config: RecordingConfig,
    pub duration_secs: f64,
    pub frames_encoded: u64,
    pub bytes_written: u64,
    pub output_width: u32,
    pub output_height: u32,
    pub encoder_name: String,
    pub error: Option<String>,
    pub audio_active: bool,
}

impl Default for RecordingInfo {
    fn default() -> Self {
        Self {
            recording: false,
            has_audio: false,
            ffmpeg_found: false,
            encoder_info: EncoderInfo::default(),
            config: RecordingConfig::default(),
            duration_secs: 0.0,
            frames_encoded: 0,
            bytes_written: 0,
            output_width: 0,
            output_height: 0,
            encoder_name: String::new(),
            error: None,
            audio_active: false,
        }
    }
}

pub fn draw_recording_panel(ui: &mut Ui, info: &RecordingInfo) {
    let tc = theme_colors(ui.ctx());

    if !info.ffmpeg_found {
        ui.label(
            RichText::new("ffmpeg not found on PATH. Install ffmpeg to enable recording.")
                .size(SMALL_SIZE)
                .color(tc.text_secondary),
        );
        return;
    }

    // Record button
    let (btn_text, btn_color) = if info.recording {
        (
            "\u{23F9} Stop Recording",
            egui::Color32::from_rgb(0xE0, 0x40, 0x40),
        )
    } else {
        ("\u{23FA} Record", egui::Color32::from_rgb(0xE0, 0x40, 0x40))
    };

    let btn = egui::Button::new(RichText::new(btn_text).size(SMALL_SIZE).color(
        if info.recording {
            egui::Color32::WHITE
        } else {
            tc.text_primary
        },
    ))
    .min_size(egui::vec2(120.0, 22.0));

    let btn = if info.recording {
        btn.fill(btn_color)
    } else {
        btn
    };

    if ui.add(btn).clicked() {
        ui.ctx().data_mut(|d| {
            d.insert_temp(egui::Id::new("recording_toggle"), true);
        });
    }

    // Show error if any
    if let Some(ref err) = info.error {
        ui.label(RichText::new(err).size(SMALL_SIZE).color(tc.error));
    }

    // Recording status
    if info.recording {
        let dur = info.duration_secs;
        let mins = (dur / 60.0) as u32;
        let secs = (dur % 60.0) as u32;

        let size_mb = info.bytes_written as f64 / (1024.0 * 1024.0);
        let size_str = if size_mb >= 1024.0 {
            format!("{:.1} GB", size_mb / 1024.0)
        } else {
            format!("{:.1} MB", size_mb)
        };

        ui.label(
            RichText::new(format!(
                "{:02}:{:02}  {}  {} frames",
                mins, secs, size_str, info.frames_encoded
            ))
            .size(SMALL_SIZE)
            .color(egui::Color32::from_rgb(0xE0, 0x60, 0x60)),
        );

        if !info.encoder_name.is_empty() {
            let audio_tag = if info.has_audio { " +audio" } else { "" };
            ui.label(
                RichText::new(format!(
                    "{}x{} @ {} • {}{}",
                    info.output_width,
                    info.output_height,
                    info.config.fps,
                    info.encoder_name,
                    audio_tag
                ))
                .size(SMALL_SIZE)
                .color(tc.text_secondary),
            );
        }

        return; // Don't show settings while recording
    }

    ui.add_space(2.0);

    // Codec dropdown
    ui.horizontal(|ui| {
        ui.label(RichText::new("Codec").size(SMALL_SIZE));
        let current = info.config.codec;
        egui::ComboBox::from_id_salt("rec_codec")
            .selected_text(format!(
                "{} ({})",
                current.display_name(),
                info.encoder_info.encoder_label(current)
            ))
            .width(140.0)
            .show_ui(ui, |ui| {
                for (i, &codec) in VideoCodec::ALL.iter().enumerate() {
                    let label = format!(
                        "{} ({})",
                        codec.display_name(),
                        info.encoder_info.encoder_label(codec)
                    );
                    let available = info.encoder_info.has_any(codec);
                    let resp = ui.add_enabled(
                        available,
                        egui::Button::new(label).selected(current == codec),
                    );
                    if resp.clicked() {
                        ui.ctx().data_mut(|d| {
                            d.insert_temp(egui::Id::new("rec_codec_change"), i as u8);
                        });
                    }
                }
            });
    });

    // Resolution dropdown
    ui.horizontal(|ui| {
        ui.label(RichText::new("Resolution").size(SMALL_SIZE));
        let current = info.config.resolution;
        egui::ComboBox::from_id_salt("rec_resolution")
            .selected_text(current.display_name())
            .width(120.0)
            .show_ui(ui, |ui| {
                for (i, &res) in OutputResolution::ALL.iter().enumerate() {
                    if ui
                        .selectable_label(current == res, res.display_name())
                        .clicked()
                    {
                        ui.ctx().data_mut(|d| {
                            d.insert_temp(egui::Id::new("rec_resolution_change"), i as u8);
                        });
                    }
                }
            });
    });

    // FPS dropdown
    ui.horizontal(|ui| {
        ui.label(RichText::new("FPS").size(SMALL_SIZE));
        let current = info.config.fps;
        egui::ComboBox::from_id_salt("rec_fps")
            .selected_text(format!("{}", current))
            .width(60.0)
            .show_ui(ui, |ui| {
                for &fps in &[30u32, 60] {
                    if ui
                        .selectable_label(current == fps, format!("{fps}"))
                        .clicked()
                    {
                        ui.ctx().data_mut(|d| {
                            d.insert_temp(egui::Id::new("rec_fps_change"), fps);
                        });
                    }
                }
            });
    });

    // Quality slider
    ui.horizontal(|ui| {
        ui.label(RichText::new("Quality").size(SMALL_SIZE));
        let mut quality = info.config.quality;
        let resp = ui.add(
            egui::Slider::new(&mut quality, 15..=35)
                .show_value(true)
                .text(RichText::new("CQ").size(SMALL_SIZE - 1.0)),
        );
        if resp.changed() {
            ui.ctx().data_mut(|d| {
                d.insert_temp(egui::Id::new("rec_quality_change"), quality);
            });
        }
    });

    // Container toggle
    ui.horizontal(|ui| {
        ui.label(RichText::new("Container").size(SMALL_SIZE));
        let current = info.config.container;
        for (i, &cont) in Container::ALL.iter().enumerate() {
            if ui
                .selectable_label(current == cont, cont.display_name())
                .clicked()
            {
                ui.ctx().data_mut(|d| {
                    d.insert_temp(egui::Id::new("rec_container_change"), i as u8);
                });
            }
        }
    });

    // Hardware encoder toggle
    ui.horizontal(|ui| {
        let mut use_hw = info.config.use_hw_encoder;
        if ui
            .checkbox(
                &mut use_hw,
                RichText::new("Prefer hardware encoder").size(SMALL_SIZE),
            )
            .changed()
        {
            ui.ctx().data_mut(|d| {
                d.insert_temp(egui::Id::new("rec_hw_toggle"), use_hw);
            });
        }
    });

    // Record audio toggle
    ui.horizontal(|ui| {
        let mut rec_audio = info.config.record_audio;
        let resp = ui.checkbox(
            &mut rec_audio,
            RichText::new("Record audio").size(SMALL_SIZE),
        );
        if !info.audio_active {
            ui.label(
                RichText::new("(no audio)")
                    .size(SMALL_SIZE - 1.0)
                    .color(tc.text_secondary),
            );
        }
        if resp.changed() {
            ui.ctx().data_mut(|d| {
                d.insert_temp(egui::Id::new("rec_audio_toggle"), rec_audio);
            });
        }
    });

    // Output directory
    ui.horizontal(|ui| {
        ui.label(RichText::new("Output").size(SMALL_SIZE));
        let dir_display = info
            .config
            .output_dir
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_else(|| info.config.output_dir.to_string_lossy().to_string());
        ui.label(
            RichText::new(format!("~/{dir_display}"))
                .size(SMALL_SIZE)
                .color(tc.text_secondary),
        );
    });
}
