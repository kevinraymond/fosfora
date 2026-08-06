use egui::{RichText, Ui};

use crate::settings::{AlphaOutputMode, BandScale, ParticleQuality};
use crate::ui::theme::ThemeMode;
use crate::ui::theme::tokens::*;
use crate::ui::widgets::rows;

pub fn draw_settings_panel(
    ui: &mut Ui,
    current_theme: ThemeMode,
    current_quality: ParticleQuality,
    current_band_scale: BandScale,
    use_ffmpeg_webcam: bool,
    auto_reconnect: bool,
    output_alpha: AlphaOutputMode,
) {
    rows::combo_row(
        ui,
        "theme_selector",
        "Theme",
        None,
        current_theme.display_name(),
        |ui| {
            for &mode in ThemeMode::ALL {
                let r = ui.selectable_label(
                    mode == current_theme,
                    RichText::new(mode.display_name()).size(SMALL_SIZE),
                );
                if r.clicked() && mode != current_theme {
                    ui.ctx().data_mut(|d| {
                        d.insert_temp(egui::Id::new("set_theme"), mode);
                    });
                }
            }
        },
    );

    rows::combo_row(
        ui,
        "particle_quality_selector",
        "Particle quality",
        None,
        current_quality.display_name(),
        |ui| {
            for &q in ParticleQuality::ALL {
                let r = ui.selectable_label(
                    q == current_quality,
                    RichText::new(q.display_name()).size(SMALL_SIZE),
                );
                if r.clicked() && q != current_quality {
                    ui.ctx().data_mut(|d| {
                        d.insert_temp(egui::Id::new("set_particle_quality"), q);
                    });
                }
            }
        },
    );

    rows::combo_row(
        ui,
        "output_alpha_selector",
        "Output alpha",
        Some(
            "What the alpha channel of the output carries — on screen and into every \
             sink (NDI/Spout/Syphon). Auto: passthrough when the scene is all overlay \
             effects, else the NDI luma-key checkbox, else opaque. Passthrough sends \
             the scene's real (premultiplied) transparency.",
        ),
        output_alpha.display_name(),
        |ui| {
            for &m in AlphaOutputMode::ALL {
                let r = ui.selectable_label(
                    m == output_alpha,
                    RichText::new(m.display_name()).size(SMALL_SIZE),
                );
                if r.clicked() && m != output_alpha {
                    ui.ctx().data_mut(|d| {
                        d.insert_temp(egui::Id::new("set_output_alpha"), m);
                    });
                }
            }
        },
    );

    rows::combo_row(
        ui,
        "band_scale_selector",
        "Band scale",
        Some(
            "How the 7 frequency bands are scaled. Unified dB makes all bands comparable \
             (recommended); Legacy is the old linear/dB split, for presets tuned to it.",
        ),
        current_band_scale.display_name(),
        |ui| {
            for &bs in BandScale::ALL {
                let r = ui.selectable_label(
                    bs == current_band_scale,
                    RichText::new(bs.display_name()).size(SMALL_SIZE),
                );
                if r.clicked() && bs != current_band_scale {
                    ui.ctx().data_mut(|d| {
                        d.insert_temp(egui::Id::new("set_band_scale"), bs);
                    });
                }
            }
        },
    );

    // A9 (#1460): auto-reconnect the capture device after a confirmed loss.
    let mut reconnect = auto_reconnect;
    let resp = rows::checkbox_row(
        ui,
        &mut reconnect,
        "Auto-reconnect",
        Some(
            "Reopen the audio device automatically when it stops delivering data — an \
             unplugged interface, a driver reset. Retries 5 times with backoff, then leaves \
             it to you.",
        ),
    );
    if resp.changed() {
        ui.ctx().data_mut(|d| {
            d.insert_temp(egui::Id::new("set_auto_reconnect"), reconnect);
        });
    }

    // FFmpeg webcam backend (webcam feature only)
    #[cfg(feature = "webcam")]
    {
        let mut ffmpeg = use_ffmpeg_webcam;
        let resp = rows::checkbox_row(
            ui,
            &mut ffmpeg,
            "FFmpeg webcam",
            Some(
                "Use FFmpeg for webcam capture. Enable if your camera isn't detected \
                 (e.g. virtual cameras like Irium or DroidCam). Requires FFmpeg installed.",
            ),
        );
        if resp.changed() {
            ui.ctx().data_mut(|d| {
                d.insert_temp(egui::Id::new("set_ffmpeg_webcam"), ffmpeg);
            });
        }
    }
    #[cfg(not(feature = "webcam"))]
    let _ = use_ffmpeg_webcam;
}
