//! Setup's smaller pages: Sync, Appearance, Tutorials and General.

use egui::{Context, RichText, Ui};

use super::kit::{self, Status};
use crate::settings::{FlashLimit, ParticleQuality};
use crate::ui::shell::ShellState;
use crate::ui::widgets::Mark;

#[cfg(feature = "link")]
fn link_info(ctx: &Context) -> Option<crate::ui::panels::link_panel::LinkInfo> {
    ctx.data(|d| d.get_temp(egui::Id::new("link_info")))
}

#[cfg(feature = "link")]
fn link_status(i: &crate::ui::panels::link_panel::LinkInfo) -> Status {
    match (i.enabled, i.peers) {
        (false, _) => Status::off(),
        (true, 0) => Status::new(Mark::Idle, "On, no peers yet"),
        (true, 1) => Status::new(Mark::Active, format!("1 peer · {:.1} BPM", i.session_tempo)),
        (true, n) => Status::new(
            Mark::Active,
            format!("{n} peers · {:.1} BPM", i.session_tempo),
        ),
    }
}

pub fn sync_status(ctx: &Context) -> Status {
    #[cfg(feature = "link")]
    {
        link_info(ctx).map_or_else(Status::off, |i| link_status(&i))
    }
    #[cfg(not(feature = "link"))]
    {
        let _ = ctx;
        Status::new(Mark::Off, "Not in this build")
    }
}

pub fn sync_page(ui: &mut Ui) {
    #[cfg(feature = "link")]
    {
        // Published every frame by main.rs; before the first, Link is off.
        link_block(ui, &link_info(ui.ctx()).unwrap_or_default());
    }
    #[cfg(not(feature = "link"))]
    kit::block(
        ui,
        "Ableton Link",
        Some(&Status::new(Mark::Off, "Not in this build")),
        |_| {},
        |ui| {
            kit::help(
                ui,
                "Link shares tempo and beat with other apps on the network. Its library is \
                 GPL-licensed, so the downloads leave it out; a build of your own can have \
                 it:",
            );
            ui.add_space(4.0);
            kit::code(ui, "cargo build --release --features release,link");
        },
    );
}

#[cfg(feature = "link")]
fn link_block(ui: &mut Ui, i: &crate::ui::panels::link_panel::LinkInfo) {
    use crate::link::LinkMode;
    kit::block(
        ui,
        "Ableton Link",
        Some(&link_status(i)),
        |ui| {
            if kit::switch(ui, i.enabled, "Ableton Link").clicked() {
                kit::send(ui, "link_set_enabled", !i.enabled);
            }
        },
        |ui| {
            kit::help(
                ui,
                "Shares tempo and beat with other apps on the network: Ableton Live, \
                 Traktor, phones and other Link apps.",
            );
            ui.add_space(6.0);
            kit::row(ui, "Role", |ui| {
                for (k, (mode, name)) in [
                    (LinkMode::Follow, "Follow the session"),
                    (LinkMode::Lead, "Lead the session"),
                ]
                .into_iter()
                .enumerate()
                {
                    if ui
                        .selectable_label(i.mode == mode, RichText::new(name).size(kit::LABEL_SIZE))
                        .clicked()
                        && i.mode != mode
                    {
                        kit::send(ui, "link_set_mode", k as u8);
                    }
                }
            });
            kit::row_help(
                ui,
                if i.mode == LinkMode::Follow {
                    "The session's tempo pins the beat tracker; the beat's phase still \
                     follows the audio. With no peers the tracker runs free."
                } else {
                    "Once the detected tempo holds steady, it becomes the session's tempo."
                },
            );
            kit::row(ui, "Bar length", |ui| {
                const Q: [f64; 7] = [1.0, 2.0, 3.0, 4.0, 6.0, 8.0, 16.0];
                let beats = |q: f64| {
                    if q == 1.0 {
                        "1 beat".to_string()
                    } else {
                        format!("{q} beats")
                    }
                };
                let names: Vec<String> = Q.iter().map(|q| beats(*q)).collect();
                let at = Q.iter().position(|q| *q == i.quantum);
                if let Some(k) = kit::pick(ui, "v2_link_q", 140.0, &beats(i.quantum), &names, at) {
                    kit::send(ui, "link_set_quantum", Q[k]);
                }
            });
            kit::row(ui, "Transport", |ui| {
                let mut on = i.start_stop_sync;
                if ui
                    .checkbox(
                        &mut on,
                        RichText::new("Start and stop the scene timeline with the session")
                            .size(kit::LABEL_SIZE),
                    )
                    .changed()
                {
                    kit::send(ui, "link_set_start_stop", on);
                }
            });
            if i.enabled {
                kit::row(ui, "Session", |ui| {
                    ui.label(
                        RichText::new(format!("{:.1} BPM", i.session_tempo))
                            .monospace()
                            .size(18.0),
                    );
                    ui.add_space(8.0);
                    let mut words = format!("beat {:.1} of {}", i.quantum_phase + 1.0, i.quantum);
                    if i.start_stop_sync {
                        words.push_str(if i.playing {
                            " · playing"
                        } else {
                            " · stopped"
                        });
                    }
                    ui.label(RichText::new(words).monospace().size(13.0));
                });
            }
        },
    );
}

pub fn appearance_page(ui: &mut Ui, s: &mut ShellState<'_>) {
    crate::ui::panels::appearance_panel::draw_appearance_panel(
        ui,
        &s.settings.theme,
        s.settings.ui_scale,
    );
}

pub fn tutorials_page(ui: &mut Ui, s: &mut ShellState<'_>) {
    use crate::ui::tour::{self, Tour};
    for &t in Tour::ALL {
        let done = s.settings.tours_done.iter().any(|k| k == t.key());
        let st = Status::new(
            if done { Mark::Active } else { Mark::Off },
            format!(
                "{} steps{}",
                t.steps().len(),
                if done { " · taken" } else { "" }
            ),
        );
        kit::block(
            ui,
            t.name(),
            Some(&st),
            |ui| {
                if ui
                    .add(
                        egui::Button::new(RichText::new("Start").size(kit::LABEL_SIZE))
                            .min_size(egui::vec2(80.0, kit::ROW_H)),
                    )
                    .clicked()
                {
                    tour::start(ui.ctx(), t);
                }
            },
            |ui| kit::help(ui, t.description()),
        );
    }
}

pub fn general_page(ui: &mut Ui, s: &mut ShellState<'_>) {
    kit::block(
        ui,
        "Performance",
        None,
        |_| {},
        |ui| {
            kit::row(ui, "Particle quality", |ui| {
                let cur = s.settings.particle_quality;
                let names: Vec<String> = ParticleQuality::ALL
                    .iter()
                    .map(|q| q.display_name().to_string())
                    .collect();
                let at = ParticleQuality::ALL.iter().position(|q| *q == cur);
                if let Some(i) = kit::pick(
                    ui,
                    "v2_particle_quality",
                    200.0,
                    cur.display_name(),
                    &names,
                    at,
                ) {
                    kit::send(ui, "set_particle_quality", ParticleQuality::ALL[i]);
                }
            });
            kit::row_help(
                ui,
                "How many particles the particle effects run, against each effect's own \
                 count. Lower it if the frame rate drops.",
            );
        },
    );
    kit::block(
        ui,
        "Safety",
        None,
        |_| {},
        |ui| {
            kit::row(ui, "Flash limiter", |ui| {
                let cur = s.settings.flash_limit;
                let names: Vec<String> = FlashLimit::ALL
                    .iter()
                    .map(|l| l.display_name().to_string())
                    .collect();
                let at = FlashLimit::ALL.iter().position(|l| *l == cur);
                if let Some(i) =
                    kit::pick(ui, "v2_flash_limit", 200.0, cur.display_name(), &names, at)
                {
                    kit::send(ui, "set_flash_limit", FlashLimit::ALL[i]);
                }
            });
            kit::row_help(
                ui,
                "Holds flashing to a rate that is safer for people with photosensitive \
                 epilepsy, on screen and in every output and recording: bright hits \
                 beyond it are dimmed. Auto is Standard, or Strict when the system asks \
                 for reduced motion. Switch it off only if your venue has decided to.",
            );
        },
    );
    #[cfg(feature = "webcam")]
    kit::block(
        ui,
        "Cameras",
        None,
        |_| {},
        |ui| {
            kit::tall_row(ui, "Virtual webcams", |ui| {
                let mut on = s.settings.use_ffmpeg_webcam;
                if ui
                    .checkbox(
                        &mut on,
                        RichText::new("Open cameras through FFmpeg").size(kit::LABEL_SIZE),
                    )
                    .changed()
                {
                    kit::send(ui, "set_ffmpeg_webcam", on);
                }
                kit::help(
                    ui,
                    "For virtual cameras such as Iriun or DroidCam, which the built-in reader \
                     can't open. Needs FFmpeg.",
                );
            });
        },
    );
    kit::block(
        ui,
        "Layout",
        None,
        |_| {},
        |ui| {
            kit::tall_row(ui, "Classic layout", |ui| {
                let mut on = s.settings.classic_layout;
                if ui
                    .checkbox(
                        &mut on,
                        RichText::new(
                            "Use the two side panels instead of Perform, Build and Setup",
                        )
                        .size(kit::LABEL_SIZE),
                    )
                    .changed()
                {
                    kit::send(ui, "set_classic_layout", on);
                }
                kit::help(ui, "The Classic layout goes away in v2.1.");
            });
        },
    );
}
