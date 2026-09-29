//! Setup › Control: MIDI, OSC and the web remote, then every trigger.

use egui::{RichText, Ui};

use super::kit::{self, Status};
use crate::midi::MidiSystem;
use crate::midi::types::{LearnTarget, MidiMsgType, TriggerAction};
use crate::osc::OscSystem;
use crate::osc::types::OscLearnTarget;
use crate::ui::shell::ShellState;
use crate::ui::theme::colors::theme_colors;
use crate::ui::widgets::Mark;
use crate::web::WebSystem;

pub fn midi_status(midi: &MidiSystem) -> Status {
    match (midi.config.enabled, midi.connected_port()) {
        (false, _) => Status::off(),
        (true, Some(port)) => Status::new(Mark::Active, format!("Connected to {port}")),
        (true, None) => Status::new(Mark::Idle, "On, no port chosen"),
    }
}

pub fn osc_status(osc: &OscSystem) -> Status {
    let port = osc.config.rx_port;
    match (osc.config.enabled, osc.is_recently_active()) {
        (false, _) => Status::off(),
        (true, true) => Status::new(Mark::Active, format!("Receiving on port {port}")),
        (true, false) => Status::new(Mark::Idle, format!("Listening on port {port}")),
    }
}

fn tx_status(osc: &OscSystem) -> Status {
    if osc.config.tx_enabled {
        Status::new(
            Mark::Active,
            format!(
                "Sending to {}:{}, {} a second",
                osc.config.tx_host, osc.config.tx_port, osc.config.tx_rate_hz
            ),
        )
    } else {
        Status::new(Mark::Off, "Not sending")
    }
}

pub fn web_status(web: &WebSystem) -> Status {
    match (web.config.enabled, web.is_running(), web.client_count) {
        (false, _, _) => Status::off(),
        (true, false, _) => Status::new(Mark::Fault, "On, but the server is not running"),
        (true, true, 0) => Status::new(Mark::Idle, "Running, nothing connected"),
        (true, true, 1) => Status::new(Mark::Active, "Running · 1 connected"),
        (true, true, n) => Status::new(Mark::Active, format!("Running · {n} connected")),
    }
}

/// The Control page's state for the list: what is switched on, and how
/// much of it is waiting.
pub fn summary(s: &ShellState<'_>) -> Status {
    kit::summarize(&[
        midi_status(s.midi),
        osc_status(s.osc),
        tx_status(s.osc),
        web_status(s.web),
    ])
}

pub fn page(ui: &mut Ui, s: &mut ShellState<'_>) {
    midi_block(ui, s.midi);
    osc_block(ui, s.osc);
    web_block(ui, s.web);
    triggers_block(ui, s.midi, s.osc);
}

/// A learn in progress, said where the thing learned is set up.
fn learning(ui: &mut Ui, text: &str) {
    ui.add_space(6.0);
    kit::status_wrapped(ui, &Status::new(Mark::Warn, text));
    ui.ctx().request_repaint();
}

fn midi_block(ui: &mut Ui, midi: &mut MidiSystem) {
    let st = midi_status(midi);
    let on = midi.config.enabled;
    let flip = kit::block(
        ui,
        "MIDI",
        Some(&st),
        |ui| kit::switch(ui, on, "MIDI").clicked(),
        |ui| {
            kit::row(ui, "Port", |ui| {
                let mut names = vec!["Not connected".to_string()];
                names.extend(midi.available_ports.iter().cloned());
                let cur = match midi.connected_port() {
                    None => Some(0),
                    Some(p) => midi
                        .available_ports
                        .iter()
                        .position(|a| a == p)
                        .map(|i| i + 1),
                };
                let shown = midi.connected_port().unwrap_or("Not connected").to_string();
                match kit::pick(ui, "v2_midi_port", 360.0, &shown, &names, cur) {
                    Some(0) => midi.disconnect(),
                    Some(i) => {
                        let port = names[i].clone();
                        midi.connect(&port);
                    }
                    None => {}
                }
            });
            if midi.available_ports.is_empty() {
                kit::row_help(
                    ui,
                    "No MIDI ports found. Plug in a controller and it appears here.",
                );
            }
            kit::tall_row(ui, "Learning", |ui| {
                kit::help(
                    ui,
                    "M beside any control learns a knob for it. Triggers, below, learn a pad \
                     or a key for an action.",
                );
            });
            if let Some(t) = &midi.learn_target {
                let what = match t {
                    LearnTarget::Param(name) => format!("Move a knob for {name}"),
                    LearnTarget::Trigger(a) => format!("Press a pad for {}", action_name(*a)),
                };
                learning(ui, &what);
            }
        },
    );
    if flip {
        midi.set_enabled(!on);
    }
}

fn osc_block(ui: &mut Ui, osc: &mut OscSystem) {
    let st = osc_status(osc);
    let on = osc.config.enabled;
    let flip = kit::block(
        ui,
        "OSC",
        Some(&st),
        |ui| kit::switch(ui, on, "Receive OSC").clicked(),
        |ui| {
            kit::row(ui, "Listen on port", |ui| {
                let mut port = osc.config.rx_port;
                if ui
                    .add(
                        egui::DragValue::new(&mut port)
                            .range(1024..=65535)
                            .speed(1.0),
                    )
                    .changed()
                {
                    osc.config.rx_port = port;
                    osc.config.save();
                    osc.restart_receiver();
                }
                kit::help(ui, "UDP. Changing it restarts the listener.");
            });
            let lan = osc.config.rx_lan;
            kit::row(ui, "Other devices", |ui| {
                if kit::switch(ui, lan, "Receive OSC from other devices").clicked() {
                    osc.set_rx_lan(!lan);
                }
            });
            kit::row_help(
                ui,
                &network_help(lan, "a controller on a phone or another computer"),
            );
            if let Some(t) = &osc.learn_target {
                let what = match t {
                    OscLearnTarget::Param(name) => format!("Send an OSC message for {name}"),
                    OscLearnTarget::Trigger(a) => {
                        format!("Send an OSC message for {}", action_name(*a))
                    }
                };
                learning(ui, &what);
            }

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(4.0);
            let tx = osc.config.tx_enabled;
            ui.horizontal(|ui| {
                ui.set_min_height(kit::ROW_H + 2.0);
                ui.label(
                    RichText::new("Send analysis to another app")
                        .size(15.0)
                        .strong(),
                );
                ui.add_space(10.0);
                kit::status(ui, &tx_status(osc));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if kit::switch(ui, tx, "Send OSC").clicked() {
                        osc.set_tx_enabled(!tx);
                    }
                });
            });
            ui.add_space(6.0);
            kit::row(ui, "To", |ui| {
                let mut host = osc.config.tx_host.clone();
                let host_changed = ui
                    .add(egui::TextEdit::singleline(&mut host).desired_width(170.0))
                    .changed();
                ui.label(RichText::new("port").size(kit::LABEL_SIZE));
                let mut port = osc.config.tx_port;
                let port_changed = ui
                    .add(
                        egui::DragValue::new(&mut port)
                            .range(1024..=65535)
                            .speed(1.0),
                    )
                    .changed();
                if host_changed || port_changed {
                    osc.config.tx_host = host;
                    osc.config.tx_port = port;
                    osc.config.save();
                    if osc.config.tx_enabled {
                        osc.sender
                            .configure(&osc.config.tx_host, osc.config.tx_port);
                    }
                }
            });
            kit::row(ui, "Rate", |ui| {
                let mut rate = osc.config.tx_rate_hz;
                if ui
                    .add(egui::DragValue::new(&mut rate).range(1..=120).speed(1.0))
                    .changed()
                {
                    osc.config.tx_rate_hz = rate;
                    osc.config.save();
                }
                ui.label(RichText::new("times a second").size(kit::LABEL_SIZE));
            });
            kit::row(ui, "Addresses start", |ui| {
                let names = [
                    "/fosfora/…".to_string(),
                    "/phosphor/… (older name)".to_string(),
                ];
                let cur = usize::from(osc.config.tx_prefix == "phosphor");
                if let Some(i) =
                    kit::pick(ui, "v2_osc_prefix", 240.0, &names[cur], &names, Some(cur))
                {
                    osc.set_tx_prefix(["fosfora", "phosphor"][i]);
                }
            });
            kit::row_help(
                ui,
                "Sends the audio features and state, separately from what OSC receives. \
                 The older name keeps rigs made before the rename working.",
            );
        },
    );
    if flip {
        osc.set_enabled(!on);
    }
}

fn web_block(ui: &mut Ui, web: &mut WebSystem) {
    let st = web_status(web);
    let on = web.config.enabled;
    let flip = kit::block(
        ui,
        "Web remote",
        Some(&st),
        |ui| kit::switch(ui, on, "Web remote").clicked(),
        |ui| {
            kit::row(ui, "Port", |ui| {
                let mut port = web.config.port;
                if ui
                    .add(
                        egui::DragValue::new(&mut port)
                            .range(1024..=65535)
                            .speed(1.0),
                    )
                    .changed()
                {
                    web.config.port = port;
                    web.config.save();
                    web.restart_server();
                }
                kit::help(ui, "Changing it restarts the server.");
            });
            let lan = web.config.lan;
            kit::row(ui, "Other devices", |ui| {
                if kit::switch(ui, lan, "Web remote from other devices").clicked() {
                    web.set_lan(!lan);
                }
            });
            if lan {
                kit::row_help(
                    ui,
                    "Other devices on the same network can connect with the access key, which the \
                     network link below includes. Use it on a network you trust.",
                );
            } else {
                kit::row_help(
                    ui,
                    &network_help(lan, "a phone, a tablet or a bridge in Docker"),
                );
            }
            if web.is_running() {
                let port = web.config.port;
                address_row(ui, "On this computer", &format!("http://localhost:{port}"));
                if lan {
                    let key = web.config.access_key.clone();
                    if let Some(ip) = lan_ip(ui.ctx()) {
                        address_row(
                            ui,
                            "On your network",
                            &format!("http://{ip}:{port}/?key={key}"),
                        );
                    }
                    kit::row(ui, "Access key", |ui| {
                        ui.label(RichText::new(&key).monospace().size(kit::LABEL_SIZE));
                        if ui.button("Copy").clicked() {
                            ui.ctx().copy_text(key.clone());
                        }
                        if ui
                            .button("New key")
                            .on_hover_text("Disconnects every device using the current key")
                            .clicked()
                        {
                            web.replace_access_key();
                        }
                    });
                    kit::row_help(ui, "Bridges take it as --key or FOSFORA_KEY.");
                }
            }
        },
    );
    if flip {
        web.set_enabled(!on);
    }
}

/// What the Other devices switch means, in its current position.
fn network_help(lan: bool, for_what: &str) -> String {
    if lan {
        "Anything on the same network can control Fosfora. Use it on a network you trust."
            .to_string()
    } else {
        format!("Only this computer can connect. Switch on for {for_what}.")
    }
}

fn address_row(ui: &mut Ui, label: &str, url: &str) {
    kit::row(ui, label, |ui| {
        ui.label(RichText::new(url).monospace().size(kit::LABEL_SIZE));
        if ui.button("Copy").clicked() {
            ui.ctx().copy_text(url.to_string());
        }
    });
}

/// This machine's address on the network, looked up once a second at most:
/// finding it opens a socket.
fn lan_ip(ctx: &egui::Context) -> Option<String> {
    let id = egui::Id::new("v2_setup_lan_ip");
    let now = ctx.input(|i| i.time);
    if let Some((at, ip)) = ctx.data(|d| d.get_temp::<(f64, Option<String>)>(id))
        && now - at < 1.0
    {
        return ip;
    }
    let ip = (|| {
        let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
        socket.connect("8.8.8.8:80").ok()?;
        let ip = socket.local_addr().ok()?.ip();
        (!ip.is_loopback()).then(|| ip.to_string())
    })();
    ctx.data_mut(|d| d.insert_temp(id, (now, ip.clone())));
    ip
}

/// Every action a trigger can fire, in the order the table lists them.
pub const ACTIONS: [TriggerAction; 14] = [
    TriggerAction::NextEffect,
    TriggerAction::PrevEffect,
    TriggerAction::NextPreset,
    TriggerAction::PrevPreset,
    TriggerAction::NextLayer,
    TriggerAction::PrevLayer,
    TriggerAction::SceneGoNext,
    TriggerAction::SceneGoPrev,
    TriggerAction::ToggleTimeline,
    TriggerAction::TogglePostProcess,
    TriggerAction::ToggleOverlay,
    TriggerAction::TempoHalf,
    TriggerAction::TempoDouble,
    TriggerAction::TempoTap,
];

/// An action, in the words the table uses.
pub fn action_name(a: TriggerAction) -> &'static str {
    match a {
        TriggerAction::NextEffect => "Next effect",
        TriggerAction::PrevEffect => "Previous effect",
        TriggerAction::NextPreset => "Next preset",
        TriggerAction::PrevPreset => "Previous preset",
        TriggerAction::NextLayer => "Next layer",
        TriggerAction::PrevLayer => "Previous layer",
        TriggerAction::SceneGoNext => "Next scene",
        TriggerAction::SceneGoPrev => "Previous scene",
        TriggerAction::ToggleTimeline => "Timeline play and stop",
        TriggerAction::TogglePostProcess => "Post-processing on and off",
        TriggerAction::ToggleOverlay => "Hide and show the interface",
        TriggerAction::TempoHalf => "Tempo ÷2",
        TriggerAction::TempoDouble => "Tempo ×2",
        TriggerAction::TempoTap => "Tap tempo",
    }
}

fn triggers_block(ui: &mut Ui, midi: &mut MidiSystem, osc: &mut OscSystem) {
    let mapped = midi.config.triggers.len() + osc.config.triggers.len();
    let st = Status::new(
        if mapped > 0 { Mark::Active } else { Mark::Off },
        format!("{mapped} mapped"),
    );
    kit::block(
        ui,
        "Triggers",
        Some(&st),
        |_| {},
        |ui| {
            kit::help(
                ui,
                "An action on a MIDI pad or an OSC address. Press Learn, then the pad, or \
                 send the message. Click a mapping to learn it again; right-click it to \
                 clear it.",
            );
            ui.add_space(8.0);
            let tc = theme_colors(ui.ctx());
            let w = ui.available_width();
            let (name_w, midi_w) = ((w - 200.0 - 260.0).max(180.0), 200.0);
            egui::Grid::new("v2_triggers")
                .num_columns(3)
                .min_col_width(0.0)
                .min_row_height(kit::ROW_H + 4.0)
                .spacing([12.0, 2.0])
                .striped(true)
                .show(ui, |ui| {
                    for h in ["ACTION", "MIDI", "OSC"] {
                        ui.label(
                            RichText::new(h)
                                .size(12.0)
                                .strong()
                                .color(tc.text_secondary),
                        );
                    }
                    ui.end_row();
                    for a in ACTIONS {
                        ui.allocate_ui(egui::vec2(name_w, kit::ROW_H), |ui| {
                            ui.set_min_width(name_w);
                            ui.label(RichText::new(action_name(a)).size(kit::LABEL_SIZE));
                        });
                        ui.allocate_ui(egui::vec2(midi_w, kit::ROW_H), |ui| {
                            ui.set_min_width(midi_w);
                            midi_cell(ui, midi, a);
                        });
                        osc_cell(ui, osc, a);
                        ui.end_row();
                    }
                });
        },
    );
}

/// A mapping as a button: what it is, or Learn, or the wait for one.
fn mapping_button(ui: &mut Ui, text: Option<&str>, waiting: bool, full: &str) -> egui::Response {
    if waiting {
        ui.ctx().request_repaint();
        ui.add(egui::Button::new(RichText::new("Waiting… click to stop").size(13.0)).selected(true))
    } else if let Some(t) = text {
        ui.add(egui::Button::new(RichText::new(t).monospace().size(13.0)))
            .on_hover_text(format!(
                "{full}\nClick to learn again, right-click to clear"
            ))
    } else {
        ui.add(egui::Button::new(RichText::new("Learn").size(13.0)))
    }
}

fn midi_cell(ui: &mut Ui, midi: &mut MidiSystem, a: TriggerAction) {
    let waiting = midi.learn_target == Some(LearnTarget::Trigger(a));
    let text = midi.config.triggers.get(&a).map(|m| match m.msg_type {
        MidiMsgType::Cc => format!("CC {}", m.cc),
        MidiMsgType::Note => format!("Note {}", m.cc),
    });
    let r = mapping_button(ui, text.as_deref(), waiting, text.as_deref().unwrap_or(""));
    if r.clicked() {
        if waiting {
            midi.cancel_learn();
        } else {
            midi.start_learn(LearnTarget::Trigger(a));
        }
    }
    if r.secondary_clicked() && text.is_some() {
        midi.clear_trigger_mapping(a);
    }
}

fn osc_cell(ui: &mut Ui, osc: &mut OscSystem, a: TriggerAction) {
    let waiting = osc.learn_target == Some(OscLearnTarget::Trigger(a));
    let addr = osc.config.triggers.get(&a).map(|m| m.address.clone());
    let short = addr.as_deref().map(|s| shorten(s, 28));
    let r = mapping_button(ui, short.as_deref(), waiting, addr.as_deref().unwrap_or(""));
    if r.clicked() {
        if waiting {
            osc.cancel_learn();
        } else {
            osc.start_learn(OscLearnTarget::Trigger(a));
        }
    }
    if r.secondary_clicked() && addr.is_some() {
        osc.clear_trigger_mapping(a);
    }
}

/// An address cut from the front to `max` characters, keeping its end.
fn shorten(addr: &str, max: usize) -> String {
    let n = addr.chars().count();
    if n <= max {
        addr.to_string()
    } else {
        let tail: String = addr.chars().skip(n - (max - 1)).collect();
        format!("…{tail}")
    }
}
