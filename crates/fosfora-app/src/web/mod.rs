pub mod client;
pub mod server;
#[allow(dead_code)]
pub mod state;
pub mod types;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Instant;

use crossbeam_channel::{Receiver, Sender};

use self::types::{WebConfig, WebFrameResult, WsInMessage};
use crate::audio::features::AudioFeatures;
use crate::inbound::DropOldestSender;
use std::collections::HashMap;

use crate::params::{ParamDef, ParamValue};

/// Central WebSocket system: owns accept thread, client channels, config.
pub struct WebSystem {
    inbound_rx: Option<Receiver<WsInMessage>>,
    inbound_tx: DropOldestSender<WsInMessage>,
    clients: Arc<Mutex<Vec<Sender<String>>>>,
    shutdown: Option<Arc<AtomicBool>>,
    accept_handle: Option<JoinHandle<()>>,
    latest_state: Arc<Mutex<String>>,
    pub config: WebConfig,
    pub client_count: usize,
    pub last_activity: Option<Instant>,
    last_audio_broadcast: Instant,
    /// When full state was last stored/broadcast; `None` until the first time.
    last_state_update: Option<Instant>,
    /// Accumulated binding data values from WebSocket clients.
    pub bind_values: std::collections::HashMap<String, f32>,
    /// Preview thumbnail JPEG data from bridge sources.
    pub preview_images: std::collections::HashMap<String, Vec<u8>>,
}

impl WebSystem {
    /// The system with `config` and nothing opened: no server started.
    fn unconnected(config: WebConfig) -> Self {
        let (inbound_tx, inbound_rx) = crate::inbound::bounded(64);

        Self {
            inbound_rx: Some(inbound_rx),
            inbound_tx,
            clients: Arc::new(Mutex::new(Vec::new())),
            shutdown: None,
            accept_handle: None,
            latest_state: Arc::new(Mutex::new(String::new())),
            config,
            client_count: 0,
            last_activity: None,
            last_audio_broadcast: Instant::now(),
            last_state_update: None,
            bind_values: std::collections::HashMap::new(),
            preview_images: std::collections::HashMap::new(),
        }
    }

    /// For tests: default settings and nothing opened, so nothing on the
    /// machine running them is read or touched.
    #[cfg(test)]
    pub(crate) fn offline() -> Self {
        Self::unconnected(WebConfig::default())
    }

    pub fn new() -> Self {
        let mut sys = Self::unconnected(WebConfig::load());
        if sys.config.access_key.is_empty() {
            sys.config.access_key = types::new_access_key();
            sys.config.save();
        }

        if sys.config.enabled {
            sys.start_server();
        }

        sys
    }

    /// Start the WebSocket server.
    pub fn start_server(&mut self) {
        self.stop_server();
        let shutdown = Arc::new(AtomicBool::new(false));
        let clients = Arc::new(Mutex::new(Vec::new()));
        let (tx, rx) = crate::inbound::bounded(64);

        match server::spawn_accept_loop(
            self.config.port,
            self.config.lan,
            self.config.access_key.clone(),
            tx.clone(),
            clients.clone(),
            self.latest_state.clone(),
            shutdown.clone(),
        ) {
            Ok(handle) => {
                self.inbound_tx = tx;
                self.inbound_rx = Some(rx);
                self.clients = clients;
                self.shutdown = Some(shutdown);
                self.accept_handle = Some(handle);
            }
            Err(e) => {
                log::error!(
                    "Failed to start web server on port {}: {e}",
                    self.config.port
                );
            }
        }
    }

    /// Stop the WebSocket server.
    pub fn stop_server(&mut self) {
        if let Some(ref shutdown) = self.shutdown {
            shutdown.store(true, Ordering::Relaxed);
        }
        if let Some(handle) = self.accept_handle.take() {
            let _ = handle.join();
        }
        self.shutdown = None;
        // Clear client list
        if let Ok(mut clients) = self.clients.lock() {
            clients.clear();
        }
        self.client_count = 0;
    }

    /// Restart server (e.g., after port change).
    pub fn restart_server(&mut self) {
        self.stop_server();
        if self.config.enabled {
            self.start_server();
        }
    }

    /// Enable or disable the web server.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.config.enabled = enabled;
        if enabled {
            self.start_server();
        } else {
            self.stop_server();
        }
        self.config.save();
    }

    /// Allow or refuse connections from other devices; restarts the server.
    pub fn set_lan(&mut self, lan: bool) {
        self.config.lan = lan;
        self.config.save();
        self.restart_server();
    }

    /// Replace the access key. Restarts the server, so every connection drops
    /// and only devices given the new key get back in.
    pub fn replace_access_key(&mut self) {
        self.config.access_key = types::new_access_key();
        self.config.save();
        self.restart_server();
    }

    /// Whether the server is running.
    pub fn is_running(&self) -> bool {
        self.shutdown
            .as_ref()
            .map_or(false, |s| !s.load(Ordering::Relaxed))
    }

    /// Main per-frame update. Drains WS messages, returns structured results.
    /// Accepts split-borrowed ParamStore fields to avoid cloning defs.
    pub fn update(
        &mut self,
        param_values: &mut HashMap<String, ParamValue>,
        param_changed: &mut bool,
        param_defs: &[ParamDef],
    ) -> WebFrameResult {
        let mut result = WebFrameResult::empty();

        self.refresh_client_count();

        let Some(ref rx) = self.inbound_rx else {
            return result;
        };

        let messages: Vec<WsInMessage> = rx.try_iter().collect();
        if messages.is_empty() {
            return result;
        }

        self.last_activity = Some(Instant::now());

        for msg in messages {
            match msg {
                WsInMessage::SetParam { name, value } => {
                    apply_param(param_values, param_changed, param_defs, &name, value);
                }
                WsInMessage::SetLayerParam { layer, name, value } => {
                    result.layer_params.push((layer, name, value));
                }
                WsInMessage::LoadEffect { index } => {
                    result.effect_load = Some(index);
                }
                WsInMessage::SelectLayer { index } => {
                    result.select_layer = Some(index);
                }
                WsInMessage::SetLayerOpacity { layer, value } => {
                    result.layer_opacity.push((layer, value));
                }
                WsInMessage::SetLayerBlend { layer, value } => {
                    result.layer_blend.push((layer, value));
                }
                WsInMessage::SetLayerEnabled { layer, value } => {
                    result.layer_enabled.push((layer, value));
                }
                WsInMessage::Trigger(action) => {
                    result.triggers.push(action);
                }
                WsInMessage::LoadPreset { index } => {
                    result.preset_load = Some(index);
                }
                WsInMessage::PostProcessEnabled(enabled) => {
                    result.postprocess_enabled = Some(enabled);
                }
                WsInMessage::BindData { source, fields } => {
                    for (field, value) in fields {
                        self.bind_values.insert(format!("{source}.{field}"), value);
                    }
                }
                WsInMessage::BindSchema { .. } => {
                    // Schema is informational only — could be stored for UI auto-discovery
                }
                WsInMessage::BindPreview { source, jpeg_data } => {
                    self.preview_images.insert(source, jpeg_data);
                }
            }
        }

        result
    }

    /// Drain messages but only process triggers (skip params). Used when active layer is locked.
    pub fn update_triggers_only(&mut self) -> WebFrameResult {
        let mut result = WebFrameResult::empty();

        self.refresh_client_count();

        let Some(ref rx) = self.inbound_rx else {
            return result;
        };

        let messages: Vec<WsInMessage> = rx.try_iter().collect();
        if messages.is_empty() {
            return result;
        }

        self.last_activity = Some(Instant::now());

        for msg in messages {
            match msg {
                WsInMessage::Trigger(action) => {
                    result.triggers.push(action);
                }
                WsInMessage::SetLayerParam { layer, name, value } => {
                    result.layer_params.push((layer, name, value));
                }
                WsInMessage::SetLayerOpacity { layer, value } => {
                    result.layer_opacity.push((layer, value));
                }
                WsInMessage::SetLayerBlend { layer, value } => {
                    result.layer_blend.push((layer, value));
                }
                WsInMessage::SetLayerEnabled { layer, value } => {
                    result.layer_enabled.push((layer, value));
                }
                WsInMessage::PostProcessEnabled(enabled) => {
                    result.postprocess_enabled = Some(enabled);
                }
                WsInMessage::LoadEffect { index } => {
                    result.effect_load = Some(index);
                }
                WsInMessage::SelectLayer { index } => {
                    result.select_layer = Some(index);
                }
                WsInMessage::LoadPreset { index } => {
                    result.preset_load = Some(index);
                }
                WsInMessage::BindPreview { source, jpeg_data } => {
                    self.preview_images.insert(source, jpeg_data);
                }
                _ => {} // Skip active-layer param application
            }
        }

        result
    }

    /// Broadcast a JSON string to all connected clients. Prunes disconnected senders.
    pub fn broadcast_json(&self, json: &str) {
        if let Ok(mut clients) = self.clients.lock() {
            clients.retain(|tx| {
                match tx.try_send(json.to_string()) {
                    Ok(_) => true,
                    Err(crossbeam_channel::TrySendError::Full(_)) => true, // backpressure, keep
                    Err(crossbeam_channel::TrySendError::Disconnected(_)) => false, // dead
                }
            });
        }
    }

    /// Broadcast audio features at 10Hz.
    pub fn broadcast_audio(&mut self, features: &AudioFeatures) {
        if self.client_count == 0 {
            return;
        }
        // 10Hz = 100ms interval
        if self.last_audio_broadcast.elapsed().as_millis() < 100 {
            return;
        }
        self.last_audio_broadcast = Instant::now();

        let json = state::build_audio_snapshot(features);
        self.broadcast_json(&json);
    }

    /// Whether the caller should build full state this frame: the server is up
    /// and either `changed` forces it or the 10 Hz refresh is due. Building it
    /// serialises every layer's parameters, so it is not done every frame.
    pub fn state_due(&self, changed: bool) -> bool {
        (self.client_count > 0 || self.is_running())
            && (changed
                || self
                    .last_state_update
                    .is_none_or(|t| t.elapsed().as_millis() >= 100))
    }

    /// Store the latest full state for initial sync on new connections and
    /// broadcast it to existing clients, so MIDI/OSC/egui changes are
    /// reflected. Called when [`Self::state_due`] says so.
    pub fn update_latest_state(&mut self, state_json: String) {
        self.last_state_update = Some(Instant::now());
        if self.client_count > 0 {
            self.broadcast_json(&state_json);
        }
        if let Ok(mut state) = self.latest_state.lock() {
            *state = state_json;
        }
    }

    /// Update client count after broadcast_json prunes disconnected senders.
    fn refresh_client_count(&mut self) {
        if let Ok(clients) = self.clients.lock() {
            self.client_count = clients.len();
        }
    }
}

impl Drop for WebSystem {
    fn drop(&mut self) {
        self.stop_server();
    }
}

/// Apply a normalized (0-1) float value to a param, scaling to its defined range.
fn apply_param(
    values: &mut HashMap<String, ParamValue>,
    changed: &mut bool,
    defs: &[ParamDef],
    name: &str,
    value: f32,
) {
    if let Some(def) = defs.iter().find(|d| d.name() == name) {
        match def {
            ParamDef::Float { min, max, .. } => {
                let val = min + (max - min) * value.clamp(0.0, 1.0);
                values.insert(name.to_string(), ParamValue::Float(val));
                *changed = true;
            }
            ParamDef::Bool { .. } => {
                values.insert(name.to_string(), ParamValue::Bool(value > 0.5));
                *changed = true;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A burst of loads compiles only the last effect and preset (#44).
    #[test]
    fn loads_in_one_frame_coalesce_to_the_last() {
        let mut web = WebSystem::offline();
        for index in [1, 2, 3] {
            web.inbound_tx.send(WsInMessage::LoadEffect { index });
        }
        for index in [4, 5] {
            web.inbound_tx.send(WsInMessage::LoadPreset { index });
        }
        let mut changed = false;
        let r = web.update(&mut HashMap::new(), &mut changed, &[]);
        assert_eq!(r.effect_load, Some(3));
        assert_eq!(r.preset_load, Some(5));

        web.inbound_tx.send(WsInMessage::LoadEffect { index: 7 });
        web.inbound_tx.send(WsInMessage::LoadEffect { index: 8 });
        let r = web.update_triggers_only();
        assert_eq!(r.effect_load, Some(8));
    }

    /// Full state is built at 10 Hz or on a change, and never with no server.
    #[test]
    fn full_state_is_due_on_change_or_refresh_only() {
        let mut web = WebSystem::offline();
        assert!(!web.state_due(true), "no server, no state");

        web.shutdown = Some(Arc::new(AtomicBool::new(false)));
        assert!(web.state_due(false), "first state is due at once");
        web.update_latest_state("{\"a\":1}".to_owned());
        assert_eq!(*web.latest_state.lock().unwrap(), "{\"a\":1}");
        assert!(!web.state_due(false), "refresh waits 100 ms");
        assert!(web.state_due(true), "a change does not wait");

        web.last_state_update = Instant::now().checked_sub(std::time::Duration::from_millis(100));
        assert!(web.state_due(false));
    }
}
