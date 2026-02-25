use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crossbeam_channel::Sender;
use rosc::{OscMessage, OscPacket, OscType};

use super::types::OscInMessage;
use crate::midi::types::TriggerAction;

/// Spawn a UDP receiver thread that decodes OSC and sends parsed messages.
pub fn spawn_receiver(
    port: u16,
    tx: Sender<OscInMessage>,
) -> anyhow::Result<(Arc<AtomicBool>, JoinHandle<()>)> {
    let addr = format!("0.0.0.0:{port}");
    let socket = UdpSocket::bind(&addr)?;
    socket.set_read_timeout(Some(Duration::from_millis(100)))?;
    log::info!("OSC receiver listening on {addr}");

    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_flag = shutdown.clone();

    let handle = thread::Builder::new()
        .name("phosphor-osc-rx".into())
        .spawn(move || {
            let mut buf = [0u8; 4096];
            while !shutdown_flag.load(Ordering::Relaxed) {
                match socket.recv_from(&mut buf) {
                    Ok((size, _addr)) => {
                        match rosc::decoder::decode_udp(&buf[..size]) {
                            Ok((_, packet)) => {
                                process_packet(&packet, &tx);
                            }
                            Err(e) => {
                                log::debug!("OSC decode error: {e}");
                            }
                        }
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        // Timeout — loop back and check shutdown flag
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => {
                        // Windows-style timeout
                    }
                    Err(e) => {
                        log::error!("OSC recv error: {e}");
                        break;
                    }
                }
            }
            log::info!("OSC receiver thread shutting down");
        })?;

    Ok((shutdown, handle))
}

fn process_packet(packet: &OscPacket, tx: &Sender<OscInMessage>) {
    match packet {
        OscPacket::Message(msg) => {
            if let Some(parsed) = parse_osc_message(msg) {
                let _ = tx.try_send(parsed);
            }
        }
        OscPacket::Bundle(bundle) => {
            for p in &bundle.content {
                process_packet(p, tx);
            }
        }
    }
}

/// Extract the first float-ish value from OSC args.
fn first_float(args: &[OscType]) -> Option<f32> {
    args.first().and_then(|a| match a {
        OscType::Float(f) => Some(*f),
        OscType::Double(d) => Some(*d as f32),
        OscType::Int(i) => Some(*i as f32),
        OscType::Long(l) => Some(*l as f32),
        OscType::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        _ => None,
    })
}

fn parse_osc_message(msg: &OscMessage) -> Option<OscInMessage> {
    let addr = &msg.addr;
    let parts: Vec<&str> = addr.split('/').collect();
    // parts[0] is always "" (leading slash)

    // All our addresses start with /phosphor/
    if parts.len() < 3 || parts[1] != "phosphor" {
        // Not our namespace — capture as Raw for learn mode
        let value = first_float(&msg.args).unwrap_or(1.0);
        return Some(OscInMessage::Raw {
            address: addr.clone(),
            value,
        });
    }

    match parts[2] {
        // /phosphor/param/{name}
        "param" if parts.len() >= 4 => {
            let name = parts[3..].join("/"); // handle nested names
            let value = first_float(&msg.args)?;
            Some(OscInMessage::Param { name, value })
        }

        // /phosphor/layer/{n}/...
        "layer" if parts.len() >= 5 => {
            let layer: usize = parts[3].parse().ok()?;
            match parts[4] {
                // /phosphor/layer/{n}/param/{name}
                "param" if parts.len() >= 6 => {
                    let name = parts[5..].join("/");
                    let value = first_float(&msg.args)?;
                    Some(OscInMessage::LayerParam { layer, name, value })
                }
                // /phosphor/layer/{n}/opacity
                "opacity" => {
                    let value = first_float(&msg.args)?;
                    Some(OscInMessage::LayerOpacity { layer, value: value.clamp(0.0, 1.0) })
                }
                // /phosphor/layer/{n}/blend
                "blend" => {
                    let value = first_float(&msg.args)? as u32;
                    Some(OscInMessage::LayerBlend { layer, value })
                }
                // /phosphor/layer/{n}/enabled
                "enabled" => {
                    let value = first_float(&msg.args)?;
                    Some(OscInMessage::LayerEnabled { layer, value: value > 0.5 })
                }
                _ => {
                    let value = first_float(&msg.args).unwrap_or(1.0);
                    Some(OscInMessage::Raw { address: addr.clone(), value })
                }
            }
        }

        // /phosphor/trigger/{action_name}
        "trigger" if parts.len() >= 4 => {
            let action = match parts[3] {
                "next_effect" => TriggerAction::NextEffect,
                "prev_effect" => TriggerAction::PrevEffect,
                "toggle_postprocess" => TriggerAction::TogglePostProcess,
                "toggle_overlay" => TriggerAction::ToggleOverlay,
                "next_preset" => TriggerAction::NextPreset,
                "prev_preset" => TriggerAction::PrevPreset,
                "next_layer" => TriggerAction::NextLayer,
                "prev_layer" => TriggerAction::PrevLayer,
                _ => {
                    let value = first_float(&msg.args).unwrap_or(1.0);
                    return Some(OscInMessage::Raw { address: addr.clone(), value });
                }
            };
            Some(OscInMessage::Trigger(action))
        }

        // /phosphor/postprocess/enabled
        "postprocess" if parts.len() >= 4 && parts[3] == "enabled" => {
            let value = first_float(&msg.args)?;
            Some(OscInMessage::PostProcessEnabled(value > 0.5))
        }

        // Unknown /phosphor/... address — capture as Raw
        _ => {
            let value = first_float(&msg.args).unwrap_or(1.0);
            Some(OscInMessage::Raw { address: addr.clone(), value })
        }
    }
}
