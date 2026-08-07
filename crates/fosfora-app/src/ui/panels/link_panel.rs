use egui::{RichText, Ui};

use crate::link::LinkMode;
use crate::ui::theme::colors::theme_colors;
use crate::ui::theme::tokens::*;

/// Snapshot of Link state for UI (avoids passing &mut LinkSystem through draw_panels).
#[derive(Clone, Default)]
pub struct LinkInfo {
    pub enabled: bool,
    pub mode: LinkMode,
    pub quantum: f64,
    pub start_stop_sync: bool,
    pub peers: u64,
    pub session_tempo: f64,
    /// Position on the session grid, `0..quantum` beats.
    pub quantum_phase: f64,
    pub playing: bool,
}

const QUANTUM_CHOICES: [f64; 7] = [1.0, 2.0, 3.0, 4.0, 6.0, 8.0, 16.0];

pub fn draw_link_panel(ui: &mut Ui, info: &LinkInfo) {
    let tc = theme_colors(ui.ctx());

    let mut enabled = info.enabled;
    if ui
        .checkbox(&mut enabled, RichText::new("Enable Link").size(SMALL_SIZE))
        .changed()
    {
        ui.ctx()
            .data_mut(|d| d.insert_temp(egui::Id::new("link_set_enabled"), enabled));
    }

    // Tempo direction (one way at a time — see link::types::LinkMode)
    ui.horizontal(|ui| {
        ui.label(RichText::new("Mode").size(SMALL_SIZE));
        let label = |m: LinkMode| match m {
            LinkMode::Follow => "Follow session",
            LinkMode::Lead => "Lead session",
        };
        egui::ComboBox::from_id_salt("link_mode")
            .selected_text(label(info.mode))
            .width(130.0)
            .show_ui(ui, |ui| {
                for (i, mode) in [LinkMode::Follow, LinkMode::Lead].into_iter().enumerate() {
                    if ui
                        .selectable_label(info.mode == mode, label(mode))
                        .clicked()
                    {
                        ui.ctx()
                            .data_mut(|d| d.insert_temp(egui::Id::new("link_set_mode"), i as u8));
                    }
                }
            });
    });

    // Quantum (bar length peers phase-align to)
    ui.horizontal(|ui| {
        ui.label(RichText::new("Quantum").size(SMALL_SIZE));
        egui::ComboBox::from_id_salt("link_quantum")
            .selected_text(format!("{} beats", info.quantum))
            .width(130.0)
            .show_ui(ui, |ui| {
                for &q in &QUANTUM_CHOICES {
                    if ui
                        .selectable_label(info.quantum == q, format!("{q} beats"))
                        .clicked()
                    {
                        ui.ctx()
                            .data_mut(|d| d.insert_temp(egui::Id::new("link_set_quantum"), q));
                    }
                }
            });
    });

    let mut sss = info.start_stop_sync;
    if ui
        .checkbox(
            &mut sss,
            RichText::new("Start/stop sync (drives timeline)").size(SMALL_SIZE),
        )
        .changed()
    {
        ui.ctx()
            .data_mut(|d| d.insert_temp(egui::Id::new("link_set_start_stop"), sss));
    }

    if info.enabled {
        let mut status = format!(
            "{} peer{} · session {:.1} BPM · beat {:.1}/{}",
            info.peers,
            if info.peers == 1 { "" } else { "s" },
            info.session_tempo,
            info.quantum_phase,
            info.quantum
        );
        if info.start_stop_sync {
            status.push_str(if info.playing {
                " · playing"
            } else {
                " · stopped"
            });
        }
        ui.label(
            RichText::new(status)
                .size(SMALL_SIZE)
                .color(tc.text_secondary),
        );
        let hint = if info.peers == 0 {
            "No peers yet — the beat tracker runs free until one joins."
        } else if info.mode == LinkMode::Follow {
            "Session tempo pins the tracker; beat phase still follows the audio."
        } else {
            "Detected BPM is committed to the session once it holds stable."
        };
        ui.label(
            RichText::new(hint)
                .size(SMALL_SIZE - 1.0)
                .color(tc.text_secondary),
        );
    }
}
