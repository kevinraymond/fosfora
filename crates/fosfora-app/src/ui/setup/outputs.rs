//! Setup › Outputs and streams: the second window, recording, what the black
//! parts become, and each stream to another app. Each stream's state comes
//! from the snapshot `main.rs` publishes every frame, and its settings go
//! back as requests `main.rs` handles.

// The stream helpers are used only by the streams a build has.
#![cfg_attr(
    not(any(
        feature = "ndi",
        feature = "v4l2",
        feature = "spout",
        feature = "syphon"
    )),
    allow(dead_code)
)]

use egui::{Context, RichText, Ui};

use super::kit::{self, Status};
use crate::gpu::types::OutputResolution;
use crate::recording::types::{Container, VideoCodec};
use crate::settings::AlphaOutputMode;
use crate::ui::panels::output_window_panel;
use crate::ui::panels::recording_panel::RecordingInfo;
use crate::ui::shell::ShellState;
use crate::ui::widgets::Mark;

fn info<T: 'static + Clone + Send + Sync>(ctx: &Context, key: &str) -> Option<T> {
    ctx.data(|d| d.get_temp::<T>(egui::Id::new(key)))
}

fn clock(secs: f64) -> String {
    format!("{:02}:{:02}", (secs / 60.0) as u32, (secs % 60.0) as u32)
}

/// A stream's state from what every sink reports.
fn stream_status(enabled: bool, running: bool, error: Option<&String>, sending: String) -> Status {
    match (enabled, running, error) {
        (false, _, _) => Status::off(),
        (true, true, _) => Status::new(Mark::Active, sending),
        (true, false, Some(_)) => Status::new(Mark::Fault, "Stopped: see below"),
        (true, false, None) => Status::new(Mark::Idle, "Starting"),
    }
}

/// Every stream built into this binary, with its state: for the page list.
fn stream_states(ctx: &Context) -> Vec<Status> {
    #[allow(unused_mut)]
    let mut v = Vec::new();
    #[cfg(feature = "ndi")]
    if let Some(i) = info::<crate::ui::panels::ndi_panel::NdiInfo>(ctx, "ndi_info")
        && i.ndi_available
    {
        v.push(stream_status(
            i.enabled,
            i.running,
            i.error.as_ref(),
            String::new(),
        ));
    }
    #[cfg(all(target_os = "linux", feature = "v4l2"))]
    if let Some(i) = info::<crate::ui::panels::v4l2_panel::V4l2Info>(ctx, "v4l2_info")
        && !i.devices.is_empty()
    {
        v.push(stream_status(
            i.enabled,
            i.running,
            i.error.as_ref(),
            String::new(),
        ));
    }
    #[cfg(all(target_os = "windows", feature = "spout"))]
    if let Some(i) = info::<crate::ui::panels::spout_panel::SpoutInfo>(ctx, "spout_info") {
        v.push(stream_status(
            i.enabled,
            i.running,
            i.error.as_ref(),
            String::new(),
        ));
    }
    #[cfg(all(target_os = "macos", feature = "syphon"))]
    if let Some(i) = info::<crate::ui::panels::syphon_panel::SyphonInfo>(ctx, "syphon_info")
        && i.available
    {
        v.push(stream_status(
            i.enabled,
            i.running,
            i.error.as_ref(),
            String::new(),
        ));
    }
    let _ = ctx;
    v
}

/// The page's state for the list: recording first, then how many outputs
/// are working.
pub fn summary(ctx: &Context) -> Status {
    let rec: Option<RecordingInfo> = info(ctx, "recording_info");
    if let Some(r) = &rec
        && r.recording
    {
        return Status::new(Mark::Active, format!("REC {}", clock(r.duration_secs)));
    }
    let window = output_window_panel::read(ctx).is_some_and(|w| w.open_on.is_some());
    let mut parts = stream_states(ctx);
    parts.push(if window {
        Status::new(Mark::Active, "")
    } else {
        Status::off()
    });
    if rec.is_some_and(|r| r.error.is_some()) {
        parts.push(Status::new(Mark::Fault, ""));
    }
    kit::summarize(&parts)
}

pub fn page(ui: &mut Ui, s: &mut ShellState<'_>) {
    let ctx = ui.ctx().clone();
    window_block(ui);
    if let Some(r) = info::<RecordingInfo>(&ctx, "recording_info") {
        recording_block(ui, &r);
    }

    ui.add_space(10.0);
    kit::group_title(
        ui,
        "Streams",
        Some("The output as a live video source for other apps"),
    );
    kit::block(
        ui,
        "Transparency",
        None,
        |_| {},
        |ui| {
            kit::row(ui, "Black becomes", |ui| {
                let cur = s.settings.output_alpha;
                let names: Vec<String> = AlphaOutputMode::ALL
                    .iter()
                    .map(|m| m.display_name().to_string())
                    .collect();
                let at = AlphaOutputMode::ALL.iter().position(|m| *m == cur);
                if let Some(i) =
                    kit::pick(ui, "v2_output_alpha", 200.0, cur.display_name(), &names, at)
                {
                    kit::send(ui, "set_output_alpha", AlphaOutputMode::ALL[i]);
                }
            });
            kit::row_help(
                ui,
                "What the alpha channel carries, on screen and in every stream. Auto passes \
                 transparency through when the scene is all overlays, else follows NDI's \
                 Alpha from brightness, else stays opaque. Passthrough sends the scene's \
                 real transparency.",
            );
        },
    );

    #[cfg(feature = "ndi")]
    if let Some(i) = info::<crate::ui::panels::ndi_panel::NdiInfo>(&ctx, "ndi_info") {
        ndi_block(ui, &i);
    }
    #[cfg(all(target_os = "linux", feature = "v4l2"))]
    if let Some(i) = info::<crate::ui::panels::v4l2_panel::V4l2Info>(&ctx, "v4l2_info") {
        v4l2_block(ui, &i);
    }
    #[cfg(all(target_os = "windows", feature = "spout"))]
    if let Some(i) = info::<crate::ui::panels::spout_panel::SpoutInfo>(&ctx, "spout_info") {
        named_stream_block(
            ui,
            "Spout",
            NamedStream {
                enabled: i.enabled,
                running: i.running,
                name: &i.sender_name,
                name_label: "Sender name",
                prefix: "spout",
                name_key: "spout_sender_name",
                resolution: i.resolution,
                sent: (
                    i.output_width,
                    i.output_height,
                    i.frames_sent,
                    i.frames_dropped,
                ),
                error: i.error.as_deref(),
            },
        );
    }
    #[cfg(all(target_os = "macos", feature = "syphon"))]
    if let Some(i) = info::<crate::ui::panels::syphon_panel::SyphonInfo>(&ctx, "syphon_info") {
        syphon_block(ui, &i);
    }
    kit::help(ui, &not_here());
}

/// The streams this binary cannot offer, and why, so none seems missing.
fn not_here() -> String {
    let mut missing: Vec<&str> = Vec::new();
    if !cfg!(feature = "ndi") {
        missing.push("NDI® (build with the ndi feature)");
    }
    if cfg!(target_os = "linux") && !cfg!(feature = "v4l2") {
        missing.push("the virtual camera (build with the v4l2 feature)");
    }
    if cfg!(target_os = "windows") && !cfg!(feature = "spout") {
        missing.push("Spout (build with the spout feature)");
    }
    if cfg!(target_os = "macos") && !cfg!(feature = "syphon") {
        missing.push("Syphon (build with the syphon feature)");
    }
    let elsewhere = if cfg!(target_os = "linux") {
        "Spout is on Windows and Syphon on macOS."
    } else if cfg!(target_os = "windows") {
        "The virtual camera is on Linux and Syphon on macOS."
    } else {
        "The virtual camera is on Linux and Spout on Windows."
    };
    if missing.is_empty() {
        elsewhere.to_string()
    } else {
        format!("Not in this build: {}. {elsewhere}", missing.join(", "))
    }
}

fn window_block(ui: &mut Ui) {
    let ow = output_window_panel::read(ui.ctx()).unwrap_or_default();
    let st = match &ow.open_on {
        Some(name) => Status::new(Mark::Active, format!("On {name}")),
        None => Status::new(Mark::Off, "Closed"),
    };
    kit::block(
        ui,
        "Second window",
        Some(&st),
        |_| {},
        |ui| {
            kit::tall_row(ui, "Display", |ui| {
                ui.set_max_width(440.0);
                output_window_panel::draw(ui, &ow);
            });
            kit::row_help(
                ui,
                "A borderless window with only the output on it, for a projector or a second \
                 screen. Esc closes it from either window. The same control sits under the \
                 output preview in every workspace.",
            );
        },
    );
}

fn recording_block(ui: &mut Ui, r: &RecordingInfo) {
    let st = if !r.ffmpeg_found {
        Status::new(Mark::Warn, "Needs ffmpeg")
    } else if r.recording {
        Status::new(
            Mark::Active,
            format!("Recording {}", clock(r.duration_secs)),
        )
    } else if r.error.is_some() {
        Status::new(Mark::Fault, "Stopped: see below")
    } else {
        Status::new(Mark::Off, "Ready")
    };
    let can = r.ffmpeg_found;
    kit::block(
        ui,
        "Recording",
        Some(&st),
        |ui| {
            let label = if r.recording {
                "■  Stop recording"
            } else {
                "●  Record"
            };
            let b = egui::Button::new(RichText::new(label).size(14.0).strong())
                .min_size(egui::vec2(150.0, kit::ROW_H))
                .selected(r.recording);
            if ui.add_enabled(can, b).clicked() {
                kit::send(ui, "recording_toggle", true);
            }
        },
        |ui| {
            if !r.ffmpeg_found {
                kit::help(
                    ui,
                    "Recording uses ffmpeg. Install it so it is on your PATH, then restart Fosfora.",
                );
                return;
            }
            if let Some(e) = &r.error {
                kit::status_wrapped(ui, &Status::new(Mark::Fault, e.clone()));
                ui.add_space(6.0);
            }
            if r.recording {
                let mb = r.bytes_written as f64 / (1024.0 * 1024.0);
                let size = if mb >= 1024.0 {
                    format!("{:.1} GB", mb / 1024.0)
                } else {
                    format!("{mb:.1} MB")
                };
                kit::row(ui, "So far", |ui| {
                    ui.label(
                        RichText::new(format!(
                            "{}   {size}   {} frames",
                            clock(r.duration_secs),
                            r.frames_encoded
                        ))
                        .monospace()
                        .size(kit::LABEL_SIZE),
                    );
                });
                if !r.encoder_name.is_empty() {
                    kit::row(ui, "As", |ui| {
                        ui.label(
                            RichText::new(format!(
                                "{}×{} at {} fps · {}{}",
                                r.output_width,
                                r.output_height,
                                r.config.fps,
                                r.encoder_name,
                                if r.has_audio { " + audio" } else { "" }
                            ))
                            .size(kit::LABEL_SIZE),
                        );
                    });
                }
                kit::row_help(ui, "The settings wait until the recording stops.");
                return;
            }
            let c = &r.config;
            kit::row(ui, "Codec", |ui| {
                let label = |codec: VideoCodec| {
                    let how = match r.encoder_info.encoder_label(codec) {
                        "HW" => "hardware",
                        "SW" => "software",
                        _ => "not available",
                    };
                    format!("{} ({how})", codec.display_name())
                };
                let mut picked = None;
                egui::ComboBox::from_id_salt("v2_rec_codec")
                    .width(220.0)
                    .selected_text(RichText::new(label(c.codec)).size(kit::LABEL_SIZE))
                    .show_ui(ui, |ui| {
                        for (i, &codec) in VideoCodec::ALL.iter().enumerate() {
                            let b = egui::Button::new(
                                RichText::new(label(codec)).size(kit::LABEL_SIZE),
                            )
                            .selected(codec == c.codec);
                            if ui.add_enabled(r.encoder_info.has_any(codec), b).clicked() {
                                picked = Some(i);
                            }
                        }
                    });
                if let Some(i) = picked {
                    kit::send(ui, "rec_codec_change", i as u8);
                }
                let mut hw = c.use_hw_encoder;
                if ui
                    .checkbox(
                        &mut hw,
                        RichText::new("Prefer hardware").size(kit::LABEL_SIZE),
                    )
                    .changed()
                {
                    kit::send(ui, "rec_hw_toggle", hw);
                }
            });
            kit::row(ui, "Size", |ui| {
                if let Some(i) = resolution_pick(ui, "v2_rec_res", c.resolution) {
                    kit::send(ui, "rec_resolution_change", i as u8);
                }
                let rates = ["30 fps".to_string(), "60 fps".to_string()];
                let at = [30u32, 60].iter().position(|f| *f == c.fps);
                if let Some(i) = kit::pick(
                    ui,
                    "v2_rec_fps",
                    100.0,
                    &format!("{} fps", c.fps),
                    &rates,
                    at,
                ) {
                    kit::send(ui, "rec_fps_change", [30u32, 60][i]);
                }
            });
            kit::row(ui, "Quality", |ui| {
                ui.label(RichText::new("Better").size(12.0));
                let mut q = c.quality;
                // Lower CQ is the better picture: the slider runs that way,
                // with the words at its ends saying so.
                if ui
                    .add(egui::Slider::new(&mut q, 15..=35).show_value(false))
                    .changed()
                {
                    kit::send(ui, "rec_quality_change", q);
                }
                ui.label(RichText::new("Smaller").size(12.0));
                ui.label(RichText::new(format!("CQ {q}")).monospace().size(13.0));
            });
            kit::row(ui, "File", |ui| {
                for (i, &cont) in Container::ALL.iter().enumerate() {
                    if ui
                        .selectable_label(
                            cont == c.container,
                            RichText::new(cont.display_name()).size(kit::LABEL_SIZE),
                        )
                        .clicked()
                        && cont != c.container
                    {
                        kit::send(ui, "rec_container_change", i as u8);
                    }
                }
                ui.add_space(8.0);
                ui.label(
                    RichText::new(format!("in {}", c.output_dir.display()))
                        .monospace()
                        .size(13.0),
                );
            });
            kit::row(ui, "Sound", |ui| {
                let mut a = c.record_audio;
                if ui
                    .checkbox(
                        &mut a,
                        RichText::new("Record the audio input too").size(kit::LABEL_SIZE),
                    )
                    .changed()
                {
                    kit::send(ui, "rec_audio_toggle", a);
                }
                if !r.audio_active {
                    kit::help(ui, "(no input is listening)");
                }
            });
        },
    );
}

fn resolution_pick(ui: &mut Ui, id: &str, cur: OutputResolution) -> Option<usize> {
    let names: Vec<String> = OutputResolution::ALL
        .iter()
        .map(|r| r.display_name().to_string())
        .collect();
    let at = OutputResolution::ALL.iter().position(|r| *r == cur);
    kit::pick(ui, id, 170.0, cur.display_name(), &names, at)
}

/// "1920×1080 · sent 12,480 · dropped 0" for a running stream.
fn sending_line(ui: &mut Ui, what: &str, (w, h, sent, dropped): (u32, u32, u64, u64)) {
    if w == 0 {
        return;
    }
    kit::row(ui, "Sending", |ui| {
        ui.label(
            RichText::new(format!("{what}{w}×{h} · sent {sent} · dropped {dropped}"))
                .monospace()
                .size(13.0),
        );
    });
}

/// A stream's failure, when it has one.
fn error_line(ui: &mut Ui, error: Option<&str>) {
    if let Some(e) = error {
        ui.add_space(4.0);
        kit::status_wrapped(ui, &Status::new(Mark::Fault, e));
    }
}

/// A block for a stream that needs something installed first.
fn missing_block(ui: &mut Ui, title: &str, needs: &str, body: impl FnOnce(&mut Ui)) {
    kit::block(
        ui,
        title,
        Some(&Status::new(Mark::Warn, needs)),
        |_| {},
        body,
    );
}

#[cfg(feature = "ndi")]
fn ndi_block(ui: &mut Ui, i: &crate::ui::panels::ndi_panel::NdiInfo) {
    if !i.ndi_available {
        missing_block(ui, "NDI®", "Needs the NDI® Runtime", |ui| {
            kit::help(ui, "Install the free NDI® Runtime, then restart Fosfora.");
            ui.hyperlink_to(
                RichText::new("ndi.video").size(kit::LABEL_SIZE),
                "https://ndi.video",
            );
            let paths = crate::ndi::ffi::ndi_search_diagnostics();
            if !paths.is_empty() {
                ui.collapsing(RichText::new("Where it looked").size(13.0), |ui| {
                    kit::code(ui, &paths.join("\n"));
                });
            }
        });
        return;
    }
    let st = stream_status(
        i.enabled,
        i.running,
        i.error.as_ref(),
        format!("Sending as {}", i.source_name),
    );
    kit::block(
        ui,
        "NDI®",
        Some(&st),
        |ui| {
            if kit::switch(ui, i.enabled, "NDI").clicked() {
                kit::send(ui, "ndi_set_enabled", !i.enabled);
            }
        },
        |ui| {
            kit::row(ui, "Source name", |ui| {
                kit::name_field(ui, "v2_ndi_name", &i.source_name, "ndi_source_name");
            });
            kit::row(ui, "Size", |ui| {
                if let Some(k) = resolution_pick(ui, "v2_ndi_res", i.resolution) {
                    kit::send(ui, "ndi_resolution_change", k as u8);
                }
            });
            kit::row(ui, "Alpha", |ui| {
                let mut a = i.alpha_from_luma;
                if ui
                    .checkbox(
                        &mut a,
                        RichText::new("Alpha from brightness").size(kit::LABEL_SIZE),
                    )
                    .changed()
                {
                    kit::send(ui, "ndi_alpha_from_luma", a);
                }
            });
            if i.running {
                sending_line(
                    ui,
                    "",
                    (
                        i.output_width,
                        i.output_height,
                        i.frames_sent,
                        i.frames_dropped,
                    ),
                );
            }
            error_line(ui, i.error.as_deref());
            ui.add_space(4.0);
            kit::help(ui, "NDI® is a registered trademark of Vizrt NDI AB.");
        },
    );
}

#[cfg(all(target_os = "linux", feature = "v4l2"))]
fn v4l2_block(ui: &mut Ui, i: &crate::ui::panels::v4l2_panel::V4l2Info) {
    use crate::v4l2::types::V4l2PixelFormat;
    if i.devices.is_empty() {
        missing_block(ui, "Virtual camera", "Needs v4l2loopback", |ui| {
            kit::help(
                ui,
                "Install and load the kernel module, then press Look again. exclusive_caps=1 \
                 is what lets Chrome list the camera.",
            );
            ui.add_space(4.0);
            kit::code(
                ui,
                "sudo apt install v4l2loopback-dkms\nsudo modprobe v4l2loopback devices=1 \
                 video_nr=10 card_label=\"Fosfora\" exclusive_caps=1",
            );
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.button("Look again").clicked() {
                    kit::send(ui, "v4l2_refresh_devices", true);
                }
                ui.hyperlink_to(
                    RichText::new("github.com/umlaeute/v4l2loopback").size(13.0),
                    "https://github.com/umlaeute/v4l2loopback",
                );
            });
        });
        return;
    }
    let dev = i
        .resolved_path
        .clone()
        .unwrap_or_else(|| "the loopback device".into());
    let st = stream_status(
        i.enabled,
        i.running,
        i.error.as_ref(),
        format!("Sending to {dev}"),
    );
    kit::block(
        ui,
        "Virtual camera",
        Some(&st),
        |ui| {
            if kit::switch(ui, i.enabled, "Virtual camera").clicked() {
                kit::send(ui, "v4l2_set_enabled", !i.enabled);
            }
        },
        |ui| {
            kit::help(
                ui,
                "Shows the output as a webcam in browsers and video calls.",
            );
            ui.add_space(6.0);
            kit::row(ui, "Device", |ui| {
                let mut names = vec!["Auto (first loopback)".to_string()];
                names.extend(
                    i.devices
                        .iter()
                        .map(|(p, n)| format!("{} — {n}", p.trim_start_matches("/dev/"))),
                );
                let at = match &i.device_path {
                    None => Some(0),
                    Some(p) => i.devices.iter().position(|(d, _)| d == p).map(|k| k + 1),
                };
                let shown = at.map_or_else(
                    || i.device_path.clone().unwrap_or_default(),
                    |k| names[k].clone(),
                );
                if let Some(k) = kit::pick(ui, "v2_v4l2_dev", 280.0, &shown, &names, at) {
                    let path = (k > 0).then(|| i.devices[k - 1].0.clone());
                    kit::send(ui, "v4l2_device_path", path);
                }
                if ui.button("Rescan").clicked() {
                    kit::send(ui, "v4l2_refresh_devices", true);
                }
            });
            kit::row(ui, "Size", |ui| {
                ui.add_enabled_ui(!i.running, |ui| {
                    if let Some(k) = resolution_pick(ui, "v2_v4l2_res", i.resolution) {
                        kit::send(ui, "v4l2_resolution_change", k as u8);
                    }
                });
                if i.running {
                    kit::help(ui, "Fixed while it streams.");
                }
            });
            kit::row(ui, "Format", |ui| {
                let names: Vec<String> = V4l2PixelFormat::ALL
                    .iter()
                    .map(|f| f.display_name().to_string())
                    .collect();
                let at = V4l2PixelFormat::ALL
                    .iter()
                    .position(|f| *f == i.pixel_format);
                if let Some(k) = kit::pick(
                    ui,
                    "v2_v4l2_fmt",
                    280.0,
                    i.pixel_format.display_name(),
                    &names,
                    at,
                ) {
                    kit::send(ui, "v4l2_pixel_format", k as u8);
                }
            });
            if i.running {
                sending_line(
                    ui,
                    "",
                    (
                        i.output_width,
                        i.output_height,
                        i.frames_sent,
                        i.frames_dropped,
                    ),
                );
            }
            error_line(ui, i.error.as_deref());
        },
    );
}

#[cfg(all(target_os = "macos", feature = "syphon"))]
fn syphon_block(ui: &mut Ui, i: &crate::ui::panels::syphon_panel::SyphonInfo) {
    if !i.available {
        missing_block(ui, "Syphon", "Syphon framework not found", |ui| {
            kit::help(
                ui,
                "The release app carries it. For a build from source, put Syphon.framework \
                 in ~/Library/Frameworks or set SYPHON_FRAMEWORK_PATH to its folder.",
            );
            let paths = crate::syphon::ffi::syphon_search_diagnostics();
            if !paths.is_empty() {
                ui.collapsing(RichText::new("Where it looked").size(13.0), |ui| {
                    kit::code(ui, &paths.join("\n"));
                });
            }
        });
        return;
    }
    named_stream_block(
        ui,
        "Syphon",
        NamedStream {
            enabled: i.enabled,
            running: i.running,
            name: &i.server_name,
            name_label: "Server name",
            prefix: "syphon",
            name_key: "syphon_server_name",
            resolution: i.resolution,
            sent: (
                i.output_width,
                i.output_height,
                i.frames_sent,
                i.frames_dropped,
            ),
            error: i.error.as_deref(),
        },
    );
}

/// Spout and Syphon: a name, a size, and what is being sent.
// Built everywhere so every platform's compiler checks it; only Windows
// (Spout) and macOS (Syphon) call it.
#[cfg_attr(
    not(any(
        all(target_os = "windows", feature = "spout"),
        all(target_os = "macos", feature = "syphon")
    )),
    allow(dead_code)
)]
struct NamedStream<'a> {
    enabled: bool,
    running: bool,
    name: &'a str,
    name_label: &'a str,
    /// The requests' prefix: `spout` or `syphon`.
    prefix: &'a str,
    name_key: &'a str,
    resolution: OutputResolution,
    sent: (u32, u32, u64, u64),
    error: Option<&'a str>,
}

// Built everywhere so every platform's compiler checks it; only Windows
// (Spout) and macOS (Syphon) call it.
#[cfg_attr(
    not(any(
        all(target_os = "windows", feature = "spout"),
        all(target_os = "macos", feature = "syphon")
    )),
    allow(dead_code)
)]
fn named_stream_block(ui: &mut Ui, title: &str, n: NamedStream<'_>) {
    let st = stream_status(
        n.enabled,
        n.running,
        n.error.map(str::to_string).as_ref(),
        format!("Sending as {}", n.name),
    );
    kit::block(
        ui,
        title,
        Some(&st),
        |ui| {
            if kit::switch(ui, n.enabled, title).clicked() {
                kit::send(ui, &format!("{}_set_enabled", n.prefix), !n.enabled);
            }
        },
        |ui| {
            kit::row(ui, n.name_label, |ui| {
                kit::name_field(ui, &format!("v2_{}_name", n.prefix), n.name, n.name_key);
            });
            kit::row(ui, "Size", |ui| {
                if let Some(k) = resolution_pick(ui, &format!("v2_{}_res", n.prefix), n.resolution)
                {
                    kit::send(ui, &format!("{}_resolution_change", n.prefix), k as u8);
                }
            });
            if n.running {
                sending_line(ui, "", n.sent);
            }
            error_line(ui, n.error);
        },
    );
}
