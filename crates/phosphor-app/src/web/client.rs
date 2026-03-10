use std::io::{Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crossbeam_channel::{Receiver, Sender};
use tungstenite::WebSocket;
use tungstenite::protocol::Message;

use super::types::{SourceFieldInfo, WsInMessage};
use crate::midi::types::TriggerAction;

/// Run the per-client read/write loop.
/// Reads JSON from the client, sends outbound messages from the broadcast channel.
pub fn run_client<S: Read + Write>(
    mut ws: WebSocket<S>,
    inbound_tx: Sender<WsInMessage>,
    outbound_rx: Receiver<String>,
    initial_state: String,
    shutdown: Arc<AtomicBool>,
    client_id: usize,
) {
    log::info!("WebSocket client {} connected", client_id);

    // Send initial full state
    if !initial_state.is_empty() {
        if ws.send(Message::text(initial_state)).is_err() {
            log::info!(
                "WebSocket client {} disconnected on initial send",
                client_id
            );
            return;
        }
    }

    loop {
        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        // Try to read a message (50ms timeout)
        match ws.read() {
            Ok(Message::Text(text)) => {
                if let Some(msg) = parse_client_message(text.as_ref()) {
                    let _ = inbound_tx.try_send(msg);
                }
            }
            Ok(Message::Close(_)) => {
                log::info!("WebSocket client {} closed connection", client_id);
                break;
            }
            Ok(Message::Ping(data)) => {
                let _ = ws.send(Message::Pong(data));
            }
            Ok(Message::Binary(data)) => {
                if let Some(msg) = parse_binary_preview(&data) {
                    let _ = inbound_tx.try_send(msg);
                }
            }
            Ok(_) => {} // Pong, etc.
            Err(tungstenite::Error::Io(ref e))
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                // Timeout — expected, continue to drain outbound
            }
            Err(e) => {
                log::debug!("WebSocket client {} read error: {e}", client_id);
                break;
            }
        }

        // Drain outbound messages
        let mut sent_any = false;
        for msg in outbound_rx.try_iter() {
            if msg.is_empty() {
                continue;
            }
            match ws.send(Message::text(msg)) {
                Ok(_) => sent_any = true,
                Err(e) => {
                    log::debug!("WebSocket client {} write error: {e}", client_id);
                    return;
                }
            }
        }

        // Flush if we sent anything
        if sent_any {
            if ws.flush().is_err() {
                break;
            }
        }
    }

    let _ = ws.close(None);
    log::info!("WebSocket client {} disconnected", client_id);
}

/// Parse a JSON message from the client into a WsInMessage.
fn parse_client_message(text: &str) -> Option<WsInMessage> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let msg_type = v.get("type")?.as_str()?;

    match msg_type {
        "set_param" => {
            let name = v.get("name")?.as_str()?.to_string();
            let value = v.get("value")?.as_f64()? as f32;
            Some(WsInMessage::SetParam { name, value })
        }
        "set_layer_param" => {
            let layer = v.get("layer")?.as_u64()? as usize;
            let name = v.get("name")?.as_str()?.to_string();
            let value = v.get("value")?.as_f64()? as f32;
            Some(WsInMessage::SetLayerParam { layer, name, value })
        }
        "load_effect" => {
            let index = v.get("index")?.as_u64()? as usize;
            Some(WsInMessage::LoadEffect { index })
        }
        "select_layer" => {
            let index = v.get("index")?.as_u64()? as usize;
            Some(WsInMessage::SelectLayer { index })
        }
        "set_layer_opacity" => {
            let layer = v.get("layer")?.as_u64()? as usize;
            let value = v.get("value")?.as_f64()? as f32;
            Some(WsInMessage::SetLayerOpacity {
                layer,
                value: value.clamp(0.0, 1.0),
            })
        }
        "set_layer_blend" => {
            let layer = v.get("layer")?.as_u64()? as usize;
            let value = v.get("value")?.as_u64()? as u32;
            Some(WsInMessage::SetLayerBlend { layer, value })
        }
        "set_layer_enabled" => {
            let layer = v.get("layer")?.as_u64()? as usize;
            let value = v
                .get("value")?
                .as_bool()
                .or_else(|| v.get("value")?.as_f64().map(|f| f > 0.5))?;
            Some(WsInMessage::SetLayerEnabled { layer, value })
        }
        "trigger" => {
            let action_str = v.get("action")?.as_str()?;
            let action = match action_str {
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
                _ => return None,
            };
            Some(WsInMessage::Trigger(action))
        }
        "load_preset" => {
            let index = v.get("index")?.as_u64()? as usize;
            Some(WsInMessage::LoadPreset { index })
        }
        "set_postprocess_enabled" => {
            let value = v
                .get("value")?
                .as_bool()
                .or_else(|| v.get("value")?.as_f64().map(|f| f > 0.5))?;
            Some(WsInMessage::PostProcessEnabled(value))
        }
        "data" => {
            let source = v.get("source")?.as_str()?.to_string();
            let fields_obj = v.get("fields")?.as_object()?;
            let fields: Vec<(String, f32)> = fields_obj
                .iter()
                .filter_map(|(k, v)| Some((k.clone(), v.as_f64()? as f32)))
                .collect();
            Some(WsInMessage::BindData { source, fields })
        }
        "schema" => {
            let source = v.get("source")?.as_str()?.to_string();
            let fields_obj = v.get("fields")?.as_object()?;
            let fields: Vec<(String, SourceFieldInfo)> = fields_obj
                .iter()
                .filter_map(|(k, v)| {
                    let info: SourceFieldInfo = serde_json::from_value(v.clone()).ok()?;
                    Some((k.clone(), info))
                })
                .collect();
            Some(WsInMessage::BindSchema { source, fields })
        }
        _ => {
            log::debug!("Unknown WS message type: {msg_type}");
            None
        }
    }
}

/// Parse a binary WebSocket message as a preview thumbnail.
/// Wire format: `[source_name_utf8] [0x00] [jpeg_bytes...]`
fn parse_binary_preview(data: &[u8]) -> Option<WsInMessage> {
    let sep = data.iter().position(|&b| b == 0)?;
    if sep == 0 || sep + 1 >= data.len() {
        return None;
    }
    let source = std::str::from_utf8(&data[..sep]).ok()?.to_string();
    let jpeg_data = data[sep + 1..].to_vec();
    // Quick JPEG magic check
    if jpeg_data.len() < 2 || jpeg_data[0] != 0xFF || jpeg_data[1] != 0xD8 {
        return None;
    }
    Some(WsInMessage::BindPreview { source, jpeg_data })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_set_param() {
        let json = r#"{"type":"set_param","name":"speed","value":0.75}"#;
        match parse_client_message(json) {
            Some(WsInMessage::SetParam { name, value }) => {
                assert_eq!(name, "speed");
                assert!((value - 0.75).abs() < 1e-6);
            }
            other => panic!("expected SetParam, got {:?}", other),
        }
    }

    #[test]
    fn parse_set_layer_param() {
        let json = r#"{"type":"set_layer_param","layer":1,"name":"intensity","value":0.5}"#;
        match parse_client_message(json) {
            Some(WsInMessage::SetLayerParam { layer, name, value }) => {
                assert_eq!(layer, 1);
                assert_eq!(name, "intensity");
                assert!((value - 0.5).abs() < 1e-6);
            }
            other => panic!("expected SetLayerParam, got {:?}", other),
        }
    }

    #[test]
    fn parse_load_effect() {
        let json = r#"{"type":"load_effect","index":3}"#;
        match parse_client_message(json) {
            Some(WsInMessage::LoadEffect { index }) => assert_eq!(index, 3),
            other => panic!("expected LoadEffect, got {:?}", other),
        }
    }

    #[test]
    fn parse_select_layer() {
        let json = r#"{"type":"select_layer","index":2}"#;
        match parse_client_message(json) {
            Some(WsInMessage::SelectLayer { index }) => assert_eq!(index, 2),
            other => panic!("expected SelectLayer, got {:?}", other),
        }
    }

    #[test]
    fn parse_set_layer_opacity_clamped() {
        let json = r#"{"type":"set_layer_opacity","layer":0,"value":1.5}"#;
        match parse_client_message(json) {
            Some(WsInMessage::SetLayerOpacity { layer, value }) => {
                assert_eq!(layer, 0);
                assert!((value - 1.0).abs() < 1e-6); // clamped
            }
            other => panic!("expected SetLayerOpacity, got {:?}", other),
        }
    }

    #[test]
    fn parse_set_layer_blend() {
        let json = r#"{"type":"set_layer_blend","layer":1,"value":3}"#;
        match parse_client_message(json) {
            Some(WsInMessage::SetLayerBlend { layer, value }) => {
                assert_eq!(layer, 1);
                assert_eq!(value, 3);
            }
            other => panic!("expected SetLayerBlend, got {:?}", other),
        }
    }

    #[test]
    fn parse_set_layer_enabled_bool() {
        let json = r#"{"type":"set_layer_enabled","layer":0,"value":true}"#;
        match parse_client_message(json) {
            Some(WsInMessage::SetLayerEnabled { layer, value }) => {
                assert_eq!(layer, 0);
                assert!(value);
            }
            other => panic!("expected SetLayerEnabled, got {:?}", other),
        }
    }

    #[test]
    fn parse_set_layer_enabled_float() {
        let json = r#"{"type":"set_layer_enabled","layer":0,"value":0.3}"#;
        match parse_client_message(json) {
            Some(WsInMessage::SetLayerEnabled { layer, value }) => {
                assert_eq!(layer, 0);
                assert!(!value); // 0.3 <= 0.5
            }
            other => panic!("expected SetLayerEnabled, got {:?}", other),
        }
    }

    #[test]
    fn parse_trigger() {
        for (action_str, expected) in [
            ("next_effect", TriggerAction::NextEffect),
            ("prev_effect", TriggerAction::PrevEffect),
            ("toggle_postprocess", TriggerAction::TogglePostProcess),
            ("toggle_overlay", TriggerAction::ToggleOverlay),
            ("next_preset", TriggerAction::NextPreset),
            ("prev_preset", TriggerAction::PrevPreset),
            ("next_layer", TriggerAction::NextLayer),
            ("prev_layer", TriggerAction::PrevLayer),
            ("scene_go_next", TriggerAction::SceneGoNext),
            ("scene_go_prev", TriggerAction::SceneGoPrev),
            ("toggle_timeline", TriggerAction::ToggleTimeline),
        ] {
            let json = format!(r#"{{"type":"trigger","action":"{action_str}"}}"#);
            match parse_client_message(&json) {
                Some(WsInMessage::Trigger(action)) => assert_eq!(action, expected),
                other => panic!("expected Trigger({:?}), got {:?}", expected, other),
            }
        }
    }

    #[test]
    fn parse_load_preset() {
        let json = r#"{"type":"load_preset","index":5}"#;
        match parse_client_message(json) {
            Some(WsInMessage::LoadPreset { index }) => assert_eq!(index, 5),
            other => panic!("expected LoadPreset, got {:?}", other),
        }
    }

    #[test]
    fn parse_postprocess_enabled() {
        let json = r#"{"type":"set_postprocess_enabled","value":true}"#;
        match parse_client_message(json) {
            Some(WsInMessage::PostProcessEnabled(v)) => assert!(v),
            other => panic!("expected PostProcessEnabled, got {:?}", other),
        }
    }

    #[test]
    fn parse_unknown_type_returns_none() {
        let json = r#"{"type":"invalid_type"}"#;
        assert!(parse_client_message(json).is_none());
    }

    #[test]
    fn parse_invalid_json_returns_none() {
        assert!(parse_client_message("not json").is_none());
    }

    // ---- Additional parse tests ----

    #[test]
    fn parse_set_layer_opacity_negative_clamped() {
        let json = r#"{"type":"set_layer_opacity","layer":0,"value":-0.5}"#;
        match parse_client_message(json) {
            Some(WsInMessage::SetLayerOpacity { value, .. }) => {
                assert!((value - 0.0).abs() < 1e-6); // clamped to 0.0
            }
            other => panic!("expected SetLayerOpacity, got {:?}", other),
        }
    }

    #[test]
    fn parse_set_param_missing_value_returns_none() {
        let json = r#"{"type":"set_param","name":"speed"}"#;
        assert!(parse_client_message(json).is_none());
    }

    #[test]
    fn parse_trigger_unknown_action_returns_none() {
        let json = r#"{"type":"trigger","action":"unknown_action"}"#;
        assert!(parse_client_message(json).is_none());
    }

    #[test]
    fn parse_set_layer_enabled_float_true() {
        let json = r#"{"type":"set_layer_enabled","layer":0,"value":0.9}"#;
        match parse_client_message(json) {
            Some(WsInMessage::SetLayerEnabled { value, .. }) => assert!(value),
            other => panic!("expected SetLayerEnabled, got {:?}", other),
        }
    }

    #[test]
    fn parse_postprocess_enabled_float() {
        let json = r#"{"type":"set_postprocess_enabled","value":0.8}"#;
        match parse_client_message(json) {
            Some(WsInMessage::PostProcessEnabled(v)) => assert!(v),
            other => panic!("expected PostProcessEnabled, got {:?}", other),
        }
    }

    #[test]
    fn parse_binary_preview_valid() {
        let data = b"test-cam\x00\xFF\xD8\xFF\xE0rest";
        match parse_binary_preview(data) {
            Some(WsInMessage::BindPreview { source, jpeg_data }) => {
                assert_eq!(source, "test-cam");
                assert_eq!(jpeg_data.len(), 8);
                assert_eq!(jpeg_data[0], 0xFF);
                assert_eq!(jpeg_data[1], 0xD8);
            }
            other => panic!("expected BindPreview, got {:?}", other),
        }
    }

    #[test]
    fn parse_binary_preview_no_separator() {
        assert!(parse_binary_preview(b"noseparator").is_none());
    }

    #[test]
    fn parse_binary_preview_empty_source() {
        assert!(parse_binary_preview(b"\x00\xFF\xD8data").is_none());
    }

    #[test]
    fn parse_binary_preview_bad_jpeg() {
        assert!(parse_binary_preview(b"src\x00notjpeg").is_none());
    }
}
