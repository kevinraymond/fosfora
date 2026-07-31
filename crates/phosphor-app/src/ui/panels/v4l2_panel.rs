use egui::{RichText, Ui};

use crate::ui::theme::colors::theme_colors;
use crate::ui::theme::tokens::*;
use crate::v4l2::types::{OutputResolution, V4l2PixelFormat};

/// Snapshot of v4l2 state for UI (avoids passing &mut V4l2System through draw_panels).
#[derive(Clone, Default)]
pub struct V4l2Info {
    pub enabled: bool,
    pub running: bool,
    /// (path, card label) for each detected loopback device.
    pub devices: Vec<(String, String)>,
    /// Configured device; `None` = auto (first loopback).
    pub device_path: Option<String>,
    /// What auto-selection actually opened, when running.
    pub resolved_path: Option<String>,
    pub resolution: OutputResolution,
    pub pixel_format: V4l2PixelFormat,
    pub frames_sent: u64,
    pub frames_dropped: u64,
    pub output_width: u32,
    pub output_height: u32,
    pub error: Option<String>,
}

pub fn draw_v4l2_panel(ui: &mut Ui, info: &V4l2Info) {
    let tc = theme_colors(ui.ctx());

    if info.devices.is_empty() {
        ui.label(
            RichText::new("No v4l2loopback device found. Install and load the kernel module:")
                .size(SMALL_SIZE)
                .color(tc.text_secondary),
        );
        ui.label(
            RichText::new("sudo apt install v4l2loopback-dkms")
                .size(SMALL_SIZE - 1.0)
                .monospace(),
        );
        ui.label(
            RichText::new(
                "sudo modprobe v4l2loopback devices=1 video_nr=10 \\\n  card_label=\"Phosphor\" exclusive_caps=1",
            )
            .size(SMALL_SIZE - 1.0)
            .monospace(),
        );
        ui.label(
            RichText::new("(exclusive_caps=1 is required for Chrome to list the camera)")
                .size(SMALL_SIZE - 1.0)
                .color(tc.text_secondary),
        );
        ui.hyperlink_to(
            RichText::new("github.com/umlaeute/v4l2loopback →").size(SMALL_SIZE),
            "https://github.com/umlaeute/v4l2loopback",
        );
        if ui
            .button(RichText::new("Refresh").size(SMALL_SIZE))
            .clicked()
        {
            ui.ctx().data_mut(|d| {
                d.insert_temp(egui::Id::new("v4l2_refresh_devices"), true);
            });
        }
        return;
    }

    // Enable checkbox
    let mut enabled = info.enabled;
    if ui
        .checkbox(
            &mut enabled,
            RichText::new("Enable virtual camera").size(SMALL_SIZE),
        )
        .changed()
    {
        ui.ctx().data_mut(|d| {
            d.insert_temp(egui::Id::new("v4l2_set_enabled"), enabled);
        });
    }

    // Device dropdown (Auto + detected loopbacks)
    ui.horizontal(|ui| {
        ui.label(RichText::new("Device").size(SMALL_SIZE));
        let selected_text = match &info.device_path {
            Some(p) => p.clone(),
            None => "Auto".to_string(),
        };
        egui::ComboBox::from_id_salt("v4l2_device")
            .selected_text(RichText::new(selected_text).size(SMALL_SIZE))
            .width(150.0)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(info.device_path.is_none(), "Auto (first loopback)")
                    .clicked()
                {
                    ui.ctx().data_mut(|d| {
                        d.insert_temp(egui::Id::new("v4l2_device_path"), None::<String>);
                    });
                }
                for (path, name) in &info.devices {
                    let label = format!("{} — {name}", path.trim_start_matches("/dev/"));
                    let is_sel = info.device_path.as_deref() == Some(path.as_str());
                    if ui.selectable_label(is_sel, label).clicked() {
                        ui.ctx().data_mut(|d| {
                            d.insert_temp(egui::Id::new("v4l2_device_path"), Some(path.clone()));
                        });
                    }
                }
            });
        if ui
            .small_button(RichText::new("⟳").size(SMALL_SIZE))
            .on_hover_text("Rescan loopback devices")
            .clicked()
        {
            ui.ctx().data_mut(|d| {
                d.insert_temp(egui::Id::new("v4l2_refresh_devices"), true);
            });
        }
    });

    // Resolution dropdown
    ui.horizontal(|ui| {
        ui.label(RichText::new("Resolution").size(SMALL_SIZE));
        let current = info.resolution;
        egui::ComboBox::from_id_salt("v4l2_resolution")
            .selected_text(current.display_name())
            .width(120.0)
            .show_ui(ui, |ui| {
                for (i, &res) in OutputResolution::ALL.iter().enumerate() {
                    if ui
                        .selectable_label(current == res, res.display_name())
                        .clicked()
                    {
                        ui.ctx().data_mut(|d| {
                            d.insert_temp(egui::Id::new("v4l2_resolution_change"), i as u8);
                        });
                    }
                }
            });
    });

    // Pixel format dropdown
    ui.horizontal(|ui| {
        ui.label(RichText::new("Format").size(SMALL_SIZE));
        let current = info.pixel_format;
        egui::ComboBox::from_id_salt("v4l2_pixel_format")
            .selected_text(current.display_name())
            .width(150.0)
            .show_ui(ui, |ui| {
                for (i, &f) in V4l2PixelFormat::ALL.iter().enumerate() {
                    if ui
                        .selectable_label(current == f, f.display_name())
                        .clicked()
                    {
                        ui.ctx().data_mut(|d| {
                            d.insert_temp(egui::Id::new("v4l2_pixel_format"), i as u8);
                        });
                    }
                }
            });
    });

    // Show failure if any (device vanished, permission, format refused, ...)
    if let Some(ref err) = info.error {
        ui.label(RichText::new(err).size(SMALL_SIZE).color(tc.error));
    }

    // Status when running
    if info.running && info.output_width > 0 {
        let dev = info
            .resolved_path
            .as_deref()
            .unwrap_or("?")
            .trim_start_matches("/dev/");
        ui.label(
            RichText::new(format!(
                "{dev}: {}x{} (locked while streaming) · sent {} · dropped {}",
                info.output_width, info.output_height, info.frames_sent, info.frames_dropped
            ))
            .size(SMALL_SIZE)
            .color(tc.text_secondary),
        );
    }
}
