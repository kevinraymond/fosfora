//! Setup › Audio: the input and what keeps it working, the analysis live,
//! the tempo, and the detectors' tuning folded away.

use egui::{RichText, Ui};

use super::kit::{self, Status};
use crate::audio::input_level::{TRIM_MAX_DB, TRIM_MIN_DB};
use crate::audio::{AudioSystem, METER_FLOOR_DB};
use crate::settings::BandScale;
use crate::ui::panels::audio_panel as ap;
use crate::ui::shell::ShellState;
use crate::ui::theme::colors::theme_colors;
use crate::ui::widgets::Mark;

pub fn status(s: &ShellState<'_>) -> Status {
    if s.audio.active {
        Status::new(Mark::Active, "Listening")
    } else {
        Status::new(Mark::Fault, "No input")
    }
}

pub fn page(ui: &mut Ui, s: &mut ShellState<'_>) {
    kit::group_title(ui, "Input", None);
    let r = ui.scope(|ui| {
        kit::row(ui, "Device", |ui| {
            ap::device_combo(ui, s.audio, 440.0, kit::LABEL_SIZE);
        });
        let st = if s.audio.active {
            status(s)
        } else {
            match &s.audio.last_error {
                Some(e) => Status::new(Mark::Fault, format!("Not listening: {e}")),
                None => Status::new(Mark::Fault, "Not listening: no input device"),
            }
        };
        kit::tall_row(ui, "State", |ui| kit::status(ui, &st));
    });
    crate::ui::tour::anchor(ui, crate::ui::tour::Anchor::AudioInput, r.response.rect);

    if s.audio.active {
        kit::row(ui, "Level", |ui| level_meter(ui, s.audio));
    }
    kit::row(ui, "Input trim", |ui| {
        let mut db = s.audio.input_trim_db();
        let r = ui.add(
            egui::Slider::new(&mut db, TRIM_MIN_DB..=TRIM_MAX_DB)
                .step_by(0.5)
                .suffix(" dB"),
        );
        if r.changed() {
            s.audio.set_input_trim_db(db);
        }
        // Persist once per adjustment, not every drag frame.
        if r.drag_stopped() || (r.changed() && !r.dragged()) {
            kit::send(ui, "set_input_trim", db);
        }
    });
    kit::row_help(
        ui,
        "Lifts a quiet line or mic input off the silence gate, or tames a hot one. It changes \
         what the analysis hears; recordings keep the input as it arrives. The level meter \
         reads the input before the trim, since clipping happens at the source.",
    );

    kit::tall_row(ui, "If it goes quiet", |ui| {
        let mut on = s.settings.auto_reconnect;
        if ui
            .checkbox(
                &mut on,
                RichText::new("Reopen the device when it stops sending sound")
                    .size(kit::LABEL_SIZE),
            )
            .changed()
        {
            kit::send(ui, "set_auto_reconnect", on);
        }
        kit::help(
            ui,
            "An unplugged interface or a driver reset. It tries 5 times, waiting longer each \
             time, then leaves it to you.",
        );
    });
    ui.add_space(6.0);
    kit::row(ui, "Band scale", |ui| {
        let names: Vec<String> = BandScale::ALL
            .iter()
            .map(|b| b.display_name().to_string())
            .collect();
        let cur = BandScale::ALL
            .iter()
            .position(|b| *b == s.settings.band_scale);
        if let Some(i) = kit::pick(
            ui,
            "v2_band_scale",
            200.0,
            s.settings.band_scale.display_name(),
            &names,
            cur,
        ) {
            kit::send(ui, "set_band_scale", BandScale::ALL[i]);
        }
    });
    kit::row_help(
        ui,
        "Unified dB puts all seven bands on one decibel scale. Legacy keeps the older \
         mixed scaling, for presets tuned to it.",
    );

    ui.add_space(22.0);
    let count = format!("{} features, live", crate::audio::features::NUM_FEATURES);
    kit::group_title(ui, "What it hears", Some(&count));
    if !s.audio.active {
        kit::help(ui, "Nothing to show until an input is listening.");
    } else {
        hears(ui, s);
    }

    ui.add_space(22.0);
    kit::group_title(ui, "Tempo", None);
    ui.scope(|ui| {
        ui.set_max_width(520.0);
        ap::draw_tempo_body(ui, s.audio, s.uniforms);
    });

    ui.add_space(22.0);
    let id = ui.make_persistent_id("v2_setup_detection");
    egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false)
        .show_header(ui, |ui| {
            let tc = theme_colors(ui.ctx());
            ui.label(
                RichText::new("Build-up and drop detection")
                    .size(19.0)
                    .strong(),
            );
            ui.label(
                RichText::new("12 settings, for tuning the detectors")
                    .size(kit::HELP_SIZE)
                    .color(tc.text_secondary),
            );
        })
        .body(|ui| {
            ui.set_max_width(520.0);
            ap::draw_tuning_body(ui, s.audio);
        });
}

/// Peak meter for the raw input with a clip light (#84).
fn level_meter(ui: &mut Ui, audio: &mut AudioSystem) {
    let tc = theme_colors(ui.ctx());
    let m = audio.input_meter();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(240.0, 10.0), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, tc.meter_bg);
    let frac = ((m.peak_dbfs - METER_FLOOR_DB) / -METER_FLOOR_DB).clamp(0.0, 1.0);
    let color = if m.clipping {
        tc.error
    } else if m.peak_dbfs > -6.0 {
        tc.warning
    } else {
        tc.success
    };
    let mut fill = rect;
    fill.set_width(rect.width() * frac);
    painter.rect_filled(fill, 2.0, color);
    let readout = if m.peak_dbfs <= METER_FLOOR_DB {
        "silent".to_owned()
    } else {
        format!("{:.0} dB", m.peak_dbfs)
    };
    ui.label(RichText::new(readout).monospace().size(13.0));
    if m.clipping {
        ui.label(
            RichText::new("CLIP")
                .strong()
                .size(kit::LABEL_SIZE)
                .color(tc.error),
        );
    }
    ui.ctx().request_repaint();
}

/// A card of the analysis: its title, a note, and what draws it.
type Card<'a> = (&'a str, &'a str, &'a dyn Fn(&mut Ui));

/// The analysis as cards, two across where there is room.
fn hears(ui: &mut Ui, s: &ShellState<'_>) {
    let u = s.uniforms;
    let bands = [
        u.sub_bass,
        u.bass,
        u.low_mid,
        u.mid,
        u.upper_mid,
        u.presence,
        u.brilliance,
    ];
    let two = ui.available_width() >= 640.0;
    let cards: [Card<'_>; 5] = [
        ("Spectrum", "7 bands", &|ui| {
            ap::draw_spectrum_bars(ui, &bands);
        }),
        ("Chroma and key", "12 pitch classes", &|ui| {
            ui.vertical_centered(|ui| ap::draw_chroma_wheel(ui, &u.chroma));
            ap::draw_key_readout(ui, u);
        }),
        ("Dynamics", "7 features", &|ui| {
            ap::draw_dynamics_rows(ui, u);
        }),
        ("Structure", "loudness · build · drop", &|ui| {
            ap::draw_structure_rows(ui, u);
        }),
        ("Timbre · MFCC", "13 coefficients", &|ui| {
            ap::draw_mfcc_heatmap(ui, &u.mfcc);
        }),
    ];
    let card = |ui: &mut Ui, title: &str, note: &str, body: &dyn Fn(&mut Ui)| {
        let tc = theme_colors(ui.ctx());
        egui::Frame::new()
            .stroke(egui::Stroke::new(1.0_f32, tc.card_border))
            .corner_radius(6.0)
            .inner_margin(egui::Margin::symmetric(14, 12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(title.to_uppercase())
                            .size(12.0)
                            .strong()
                            .color(tc.text_secondary),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(note).size(12.0).color(tc.text_secondary));
                    });
                });
                ui.add_space(6.0);
                body(ui);
            });
    };
    if two {
        // Left: spectrum, dynamics, MFCC. Right: chroma, structure.
        ui.columns(2, |cols| {
            for (i, (t, n, b)) in cards.iter().enumerate() {
                let c = if matches!(i, 1 | 3) {
                    &mut cols[1]
                } else {
                    &mut cols[0]
                };
                card(c, t, n, *b);
                c.add_space(12.0);
            }
        });
    } else {
        for (t, n, b) in cards {
            card(ui, t, n, b);
            ui.add_space(12.0);
        }
    }
}
