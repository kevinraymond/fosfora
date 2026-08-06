use std::net::UdpSocket;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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
                    Ok((size, _addr)) => match rosc::decoder::decode_udp(&buf[..size]) {
                        Ok((_, packet)) => {
                            process_packet(&packet, &tx);
                        }
                        Err(e) => {
                            log::debug!("OSC decode error: {e}");
                        }
                    },
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

/// Extract the first string value from OSC args.
fn first_string(args: &[OscType]) -> Option<String> {
    args.first().and_then(|a| match a {
        OscType::String(s) => Some(s.clone()),
        _ => None,
    })
}

fn parse_osc_message(msg: &OscMessage) -> Option<OscInMessage> {
    let addr = &msg.addr;
    let parts: Vec<&str> = addr.split('/').collect();
    // parts[0] is always "" (leading slash)

    // All our addresses start with /fosfora/; the pre-rename /phosphor/ prefix is
    // accepted forever so existing rigs and saved controller patches keep working.
    if parts.len() < 3 || (parts[1] != "fosfora" && parts[1] != "phosphor") {
        // Not our namespace — capture as Raw for learn mode
        let value = first_float(&msg.args).unwrap_or(1.0);
        return Some(OscInMessage::Raw {
            address: addr.clone(),
            value,
        });
    }

    match parts[2] {
        // /fosfora/param/{name}
        "param" if parts.len() >= 4 => {
            let name = parts[3..].join("/"); // handle nested names
            let value = first_float(&msg.args)?;
            Some(OscInMessage::Param { name, value })
        }

        // /fosfora/layer/{n}/...
        "layer" if parts.len() >= 5 => {
            let layer: usize = parts[3].parse().ok()?;
            match parts[4] {
                // /fosfora/layer/{n}/param/{name}
                "param" if parts.len() >= 6 => {
                    let name = parts[5..].join("/");
                    let value = first_float(&msg.args)?;
                    Some(OscInMessage::LayerParam { layer, name, value })
                }
                // /fosfora/layer/{n}/opacity
                "opacity" => {
                    let value = first_float(&msg.args)?;
                    Some(OscInMessage::LayerOpacity {
                        layer,
                        value: value.clamp(0.0, 1.0),
                    })
                }
                // /fosfora/layer/{n}/blend
                "blend" => {
                    let value = first_float(&msg.args)? as u32;
                    Some(OscInMessage::LayerBlend { layer, value })
                }
                // /fosfora/layer/{n}/displace
                "displace" => {
                    let value = first_float(&msg.args)?;
                    Some(OscInMessage::LayerDisplace {
                        layer,
                        value: value.clamp(0.0, 1.0),
                    })
                }
                // /fosfora/layer/{n}/enabled
                "enabled" => {
                    let value = first_float(&msg.args)?;
                    Some(OscInMessage::LayerEnabled {
                        layer,
                        value: value > 0.5,
                    })
                }
                // /fosfora/layer/{n}/obstacle/{field}
                "obstacle" if parts.len() >= 6 => match parts[5] {
                    "enabled" => {
                        let value = first_float(&msg.args)?;
                        Some(OscInMessage::LayerObstacleEnabled {
                            layer,
                            value: value > 0.5,
                        })
                    }
                    "mode" => {
                        let value = first_float(&msg.args)? as u32;
                        Some(OscInMessage::LayerObstacleMode { layer, value })
                    }
                    "threshold" => {
                        let value = first_float(&msg.args)?;
                        Some(OscInMessage::LayerObstacleThreshold {
                            layer,
                            value: value.clamp(0.0, 1.0),
                        })
                    }
                    "elasticity" => {
                        let value = first_float(&msg.args)?;
                        Some(OscInMessage::LayerObstacleElasticity {
                            layer,
                            value: value.clamp(0.0, 1.0),
                        })
                    }
                    _ => {
                        let value = first_float(&msg.args).unwrap_or(1.0);
                        Some(OscInMessage::Raw {
                            address: addr.clone(),
                            value,
                        })
                    }
                },
                _ => {
                    let value = first_float(&msg.args).unwrap_or(1.0);
                    Some(OscInMessage::Raw {
                        address: addr.clone(),
                        value,
                    })
                }
            }
        }

        // /fosfora/trigger/{action_name}
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
                "scene_go_next" => TriggerAction::SceneGoNext,
                "scene_go_prev" => TriggerAction::SceneGoPrev,
                "toggle_timeline" => TriggerAction::ToggleTimeline,
                "tempo_half" => TriggerAction::TempoHalf,
                "tempo_double" => TriggerAction::TempoDouble,
                "tempo_tap" => TriggerAction::TempoTap,
                _ => {
                    let value = first_float(&msg.args).unwrap_or(1.0);
                    return Some(OscInMessage::Raw {
                        address: addr.clone(),
                        value,
                    });
                }
            };
            Some(OscInMessage::Trigger(action))
        }

        // /fosfora/postprocess/enabled
        "postprocess" if parts.len() >= 4 && parts[3] == "enabled" => {
            let value = first_float(&msg.args)?;
            Some(OscInMessage::PostProcessEnabled(value > 0.5))
        }

        // /fosfora/volumetric/enabled  or  /fosfora/volumetric/{param}
        "volumetric" if parts.len() >= 4 => {
            let value = first_float(&msg.args)?;
            if parts[3] == "enabled" {
                Some(OscInMessage::VolumetricEnabled(value > 0.5))
            } else {
                Some(OscInMessage::VolumetricParam {
                    name: parts[3..].join("/"),
                    value,
                })
            }
        }

        // /fosfora/scene/...
        "scene" if parts.len() >= 4 => {
            match parts[3] {
                // /fosfora/scene/goto_cue i
                "goto_cue" => {
                    let value = first_float(&msg.args)? as usize;
                    Some(OscInMessage::SceneGotoCue(value))
                }
                // /fosfora/scene/load i (int) or s (string)
                "load" => {
                    // Try string first, fall back to int
                    if let Some(name) = first_string(&msg.args) {
                        Some(OscInMessage::SceneLoadName(name))
                    } else {
                        let value = first_float(&msg.args)? as usize;
                        Some(OscInMessage::SceneLoadIndex(value))
                    }
                }
                // /fosfora/scene/loop_mode f (0/1)
                "loop_mode" => {
                    let value = first_float(&msg.args)?;
                    Some(OscInMessage::SceneLoopMode(value > 0.5))
                }
                // /fosfora/scene/advance_mode i (0=Manual, 1=Timer, 2=BeatSync)
                "advance_mode" => {
                    let value = first_float(&msg.args)? as u8;
                    Some(OscInMessage::SceneAdvanceMode(value))
                }
                _ => {
                    let value = first_float(&msg.args).unwrap_or(1.0);
                    Some(OscInMessage::Raw {
                        address: addr.clone(),
                        value,
                    })
                }
            }
        }

        // Unknown /fosfora/... address — capture as Raw
        _ => {
            let value = first_float(&msg.args).unwrap_or(1.0);
            Some(OscInMessage::Raw {
                address: addr.clone(),
                value,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rosc::OscType;

    #[test]
    fn first_float_from_float() {
        assert_eq!(first_float(&[OscType::Float(0.5)]), Some(0.5));
    }

    #[test]
    fn first_float_from_int() {
        assert_eq!(first_float(&[OscType::Int(42)]), Some(42.0));
    }

    #[test]
    fn first_float_from_double() {
        let v = first_float(&[OscType::Double(4.567)]).unwrap();
        assert!((v - 4.567).abs() < 0.01);
    }

    #[test]
    fn first_float_from_bool() {
        assert_eq!(first_float(&[OscType::Bool(true)]), Some(1.0));
        assert_eq!(first_float(&[OscType::Bool(false)]), Some(0.0));
    }

    #[test]
    fn first_float_from_long() {
        assert_eq!(first_float(&[OscType::Long(100)]), Some(100.0));
    }

    #[test]
    fn first_float_empty_args() {
        assert_eq!(first_float(&[]), None);
    }

    #[test]
    fn parse_param() {
        let msg = OscMessage {
            addr: "/fosfora/param/speed".into(),
            args: vec![OscType::Float(0.75)],
        };
        match parse_osc_message(&msg) {
            Some(OscInMessage::Param { name, value }) => {
                assert_eq!(name, "speed");
                assert!((value - 0.75).abs() < 1e-6);
            }
            other => panic!("expected Param, got {:?}", other),
        }
    }

    #[test]
    fn parse_layer_param() {
        let msg = OscMessage {
            addr: "/fosfora/layer/2/param/intensity".into(),
            args: vec![OscType::Float(0.5)],
        };
        match parse_osc_message(&msg) {
            Some(OscInMessage::LayerParam { layer, name, value }) => {
                assert_eq!(layer, 2);
                assert_eq!(name, "intensity");
                assert!((value - 0.5).abs() < 1e-6);
            }
            other => panic!("expected LayerParam, got {:?}", other),
        }
    }

    #[test]
    fn parse_layer_opacity_clamped() {
        let msg = OscMessage {
            addr: "/fosfora/layer/0/opacity".into(),
            args: vec![OscType::Float(1.5)],
        };
        match parse_osc_message(&msg) {
            Some(OscInMessage::LayerOpacity { layer, value }) => {
                assert_eq!(layer, 0);
                assert!((value - 1.0).abs() < 1e-6); // clamped to 1.0
            }
            other => panic!("expected LayerOpacity, got {:?}", other),
        }
    }

    #[test]
    fn parse_trigger() {
        let msg = OscMessage {
            addr: "/fosfora/trigger/next_effect".into(),
            args: vec![OscType::Float(1.0)],
        };
        match parse_osc_message(&msg) {
            Some(OscInMessage::Trigger(TriggerAction::NextEffect)) => {}
            other => panic!("expected Trigger(NextEffect), got {:?}", other),
        }
    }

    #[test]
    fn parse_postprocess_enabled() {
        let msg = OscMessage {
            addr: "/fosfora/postprocess/enabled".into(),
            args: vec![OscType::Float(1.0)],
        };
        match parse_osc_message(&msg) {
            Some(OscInMessage::PostProcessEnabled(v)) => assert!(v),
            other => panic!("expected PostProcessEnabled, got {:?}", other),
        }
    }

    #[test]
    fn parse_foreign_namespace_returns_raw() {
        let msg = OscMessage {
            addr: "/other/thing".into(),
            args: vec![OscType::Float(0.5)],
        };
        match parse_osc_message(&msg) {
            Some(OscInMessage::Raw { address, value }) => {
                assert_eq!(address, "/other/thing");
                assert!((value - 0.5).abs() < 1e-6);
            }
            other => panic!("expected Raw, got {:?}", other),
        }
    }

    /// The pre-rename namespace stays a first-class citizen: a `/phosphor/...` message
    /// must parse to the same structured variant as its `/fosfora/...` twin, not fall
    /// through to Raw — old rigs and saved controller patches depend on it.
    #[test]
    fn parse_legacy_phosphor_prefix_matches_fosfora() {
        for prefix in ["fosfora", "phosphor"] {
            let msg = OscMessage {
                addr: format!("/{prefix}/param/speed"),
                args: vec![OscType::Float(0.7)],
            };
            match parse_osc_message(&msg) {
                Some(OscInMessage::Param { name, value }) => {
                    assert_eq!(name, "speed");
                    assert!((value - 0.7).abs() < 1e-6);
                }
                other => panic!("expected Param via /{prefix}/, got {:?}", other),
            }
        }
    }

    // ---- Additional parse branch tests ----

    #[test]
    fn parse_layer_blend() {
        let msg = OscMessage {
            addr: "/fosfora/layer/1/blend".into(),
            args: vec![OscType::Int(3)],
        };
        match parse_osc_message(&msg) {
            Some(OscInMessage::LayerBlend { layer, value }) => {
                assert_eq!(layer, 1);
                assert_eq!(value, 3);
            }
            other => panic!("expected LayerBlend, got {:?}", other),
        }
    }

    #[test]
    fn parse_layer_displace_clamped() {
        let msg = OscMessage {
            addr: "/fosfora/layer/2/displace".into(),
            args: vec![OscType::Float(1.7)],
        };
        match parse_osc_message(&msg) {
            Some(OscInMessage::LayerDisplace { layer, value }) => {
                assert_eq!(layer, 2);
                assert!((value - 1.0).abs() < 1e-6, "got {value}");
            }
            other => panic!("expected LayerDisplace, got {:?}", other),
        }
    }

    #[test]
    fn parse_layer_enabled_true() {
        let msg = OscMessage {
            addr: "/fosfora/layer/0/enabled".into(),
            args: vec![OscType::Float(0.8)],
        };
        match parse_osc_message(&msg) {
            Some(OscInMessage::LayerEnabled { layer, value }) => {
                assert_eq!(layer, 0);
                assert!(value); // 0.8 > 0.5
            }
            other => panic!("expected LayerEnabled, got {:?}", other),
        }
    }

    #[test]
    fn parse_layer_enabled_false() {
        let msg = OscMessage {
            addr: "/fosfora/layer/0/enabled".into(),
            args: vec![OscType::Float(0.3)],
        };
        match parse_osc_message(&msg) {
            Some(OscInMessage::LayerEnabled { layer, value }) => {
                assert_eq!(layer, 0);
                assert!(!value); // 0.3 <= 0.5
            }
            other => panic!("expected LayerEnabled, got {:?}", other),
        }
    }

    // ---- Obstacle parse tests (#1793) ----

    #[test]
    fn parse_layer_obstacle_enabled_true() {
        let msg = OscMessage {
            addr: "/fosfora/layer/0/obstacle/enabled".into(),
            args: vec![OscType::Float(0.8)],
        };
        match parse_osc_message(&msg) {
            Some(OscInMessage::LayerObstacleEnabled { layer, value }) => {
                assert_eq!(layer, 0);
                assert!(value); // 0.8 > 0.5
            }
            other => panic!("expected LayerObstacleEnabled, got {:?}", other),
        }
    }

    #[test]
    fn parse_layer_obstacle_enabled_false() {
        let msg = OscMessage {
            addr: "/fosfora/layer/0/obstacle/enabled".into(),
            args: vec![OscType::Float(0.3)],
        };
        match parse_osc_message(&msg) {
            Some(OscInMessage::LayerObstacleEnabled { layer: 0, value }) => assert!(!value),
            other => panic!("expected LayerObstacleEnabled, got {:?}", other),
        }
    }

    #[test]
    fn parse_layer_obstacle_mode() {
        let msg = OscMessage {
            addr: "/fosfora/layer/1/obstacle/mode".into(),
            args: vec![OscType::Int(3)],
        };
        match parse_osc_message(&msg) {
            Some(OscInMessage::LayerObstacleMode { layer, value }) => {
                assert_eq!(layer, 1);
                assert_eq!(value, 3);
            }
            other => panic!("expected LayerObstacleMode, got {:?}", other),
        }
    }

    #[test]
    fn parse_layer_obstacle_threshold_clamped() {
        let msg = OscMessage {
            addr: "/fosfora/layer/0/obstacle/threshold".into(),
            args: vec![OscType::Float(1.5)],
        };
        match parse_osc_message(&msg) {
            Some(OscInMessage::LayerObstacleThreshold { layer: 0, value }) => {
                assert!((value - 1.0).abs() < 1e-6);
            }
            other => panic!("expected LayerObstacleThreshold, got {:?}", other),
        }
    }

    #[test]
    fn parse_layer_obstacle_elasticity() {
        let msg = OscMessage {
            addr: "/fosfora/layer/2/obstacle/elasticity".into(),
            args: vec![OscType::Float(0.25)],
        };
        match parse_osc_message(&msg) {
            Some(OscInMessage::LayerObstacleElasticity { layer, value }) => {
                assert_eq!(layer, 2);
                assert!((value - 0.25).abs() < 1e-6);
            }
            other => panic!("expected LayerObstacleElasticity, got {:?}", other),
        }
    }

    #[test]
    fn parse_unknown_obstacle_field_returns_raw() {
        let msg = OscMessage {
            addr: "/fosfora/layer/0/obstacle/bogus".into(),
            args: vec![OscType::Float(1.0)],
        };
        match parse_osc_message(&msg) {
            Some(OscInMessage::Raw { address, .. }) => {
                assert_eq!(address, "/fosfora/layer/0/obstacle/bogus");
            }
            other => panic!("expected Raw, got {:?}", other),
        }
    }

    #[test]
    fn parse_bare_obstacle_returns_raw() {
        // No field segment -> fails the parts.len() >= 6 guard -> Raw (learn mode).
        let msg = OscMessage {
            addr: "/fosfora/layer/0/obstacle".into(),
            args: vec![OscType::Float(1.0)],
        };
        match parse_osc_message(&msg) {
            Some(OscInMessage::Raw { address, .. }) => {
                assert_eq!(address, "/fosfora/layer/0/obstacle");
            }
            other => panic!("expected Raw, got {:?}", other),
        }
    }

    #[test]
    fn parse_obstacle_missing_args_returns_none() {
        let msg = OscMessage {
            addr: "/fosfora/layer/0/obstacle/threshold".into(),
            args: vec![],
        };
        assert!(parse_osc_message(&msg).is_none());
    }

    #[test]
    fn parse_unknown_trigger_returns_raw() {
        let msg = OscMessage {
            addr: "/fosfora/trigger/unknown_action".into(),
            args: vec![OscType::Float(1.0)],
        };
        match parse_osc_message(&msg) {
            Some(OscInMessage::Raw { address, .. }) => {
                assert_eq!(address, "/fosfora/trigger/unknown_action");
            }
            other => panic!("expected Raw, got {:?}", other),
        }
    }

    #[test]
    fn parse_unknown_layer_subpath_returns_raw() {
        let msg = OscMessage {
            addr: "/fosfora/layer/0/unknown".into(),
            args: vec![OscType::Float(1.0)],
        };
        match parse_osc_message(&msg) {
            Some(OscInMessage::Raw { address, .. }) => {
                assert_eq!(address, "/fosfora/layer/0/unknown");
            }
            other => panic!("expected Raw, got {:?}", other),
        }
    }

    #[test]
    fn parse_nested_param_name() {
        let msg = OscMessage {
            addr: "/fosfora/param/group/sub".into(),
            args: vec![OscType::Float(0.5)],
        };
        match parse_osc_message(&msg) {
            Some(OscInMessage::Param { name, .. }) => {
                assert_eq!(name, "group/sub");
            }
            other => panic!("expected Param, got {:?}", other),
        }
    }

    #[test]
    fn parse_param_missing_float_returns_none() {
        let msg = OscMessage {
            addr: "/fosfora/param/speed".into(),
            args: vec![],
        };
        assert!(parse_osc_message(&msg).is_none());
    }

    #[test]
    fn first_float_string_returns_none() {
        assert_eq!(first_float(&[OscType::String("hello".into())]), None);
    }

    // ---- Scene control parse tests ----

    #[test]
    fn parse_scene_goto_cue() {
        let msg = OscMessage {
            addr: "/fosfora/scene/goto_cue".into(),
            args: vec![OscType::Int(2)],
        };
        match parse_osc_message(&msg) {
            Some(OscInMessage::SceneGotoCue(idx)) => assert_eq!(idx, 2),
            other => panic!("expected SceneGotoCue, got {:?}", other),
        }
    }

    #[test]
    fn parse_scene_load_int() {
        let msg = OscMessage {
            addr: "/fosfora/scene/load".into(),
            args: vec![OscType::Int(1)],
        };
        match parse_osc_message(&msg) {
            Some(OscInMessage::SceneLoadIndex(idx)) => assert_eq!(idx, 1),
            other => panic!("expected SceneLoadIndex, got {:?}", other),
        }
    }

    #[test]
    fn parse_scene_load_string() {
        let msg = OscMessage {
            addr: "/fosfora/scene/load".into(),
            args: vec![OscType::String("My Scene".into())],
        };
        match parse_osc_message(&msg) {
            Some(OscInMessage::SceneLoadName(name)) => assert_eq!(name, "My Scene"),
            other => panic!("expected SceneLoadName, got {:?}", other),
        }
    }

    #[test]
    fn parse_scene_loop_mode() {
        let msg = OscMessage {
            addr: "/fosfora/scene/loop_mode".into(),
            args: vec![OscType::Float(1.0)],
        };
        match parse_osc_message(&msg) {
            Some(OscInMessage::SceneLoopMode(v)) => assert!(v),
            other => panic!("expected SceneLoopMode, got {:?}", other),
        }
        let msg = OscMessage {
            addr: "/fosfora/scene/loop_mode".into(),
            args: vec![OscType::Float(0.0)],
        };
        match parse_osc_message(&msg) {
            Some(OscInMessage::SceneLoopMode(v)) => assert!(!v),
            other => panic!("expected SceneLoopMode, got {:?}", other),
        }
    }

    #[test]
    fn parse_scene_advance_mode() {
        let msg = OscMessage {
            addr: "/fosfora/scene/advance_mode".into(),
            args: vec![OscType::Int(2)],
        };
        match parse_osc_message(&msg) {
            Some(OscInMessage::SceneAdvanceMode(m)) => assert_eq!(m, 2),
            other => panic!("expected SceneAdvanceMode, got {:?}", other),
        }
    }

    #[test]
    fn parse_scene_unknown_returns_raw() {
        let msg = OscMessage {
            addr: "/fosfora/scene/unknown".into(),
            args: vec![OscType::Float(1.0)],
        };
        match parse_osc_message(&msg) {
            Some(OscInMessage::Raw { address, .. }) => {
                assert_eq!(address, "/fosfora/scene/unknown");
            }
            other => panic!("expected Raw, got {:?}", other),
        }
    }
}
