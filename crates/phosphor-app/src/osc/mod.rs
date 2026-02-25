pub mod receiver;
pub mod sender;
pub mod types;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Instant;

use crossbeam_channel::Receiver;

use self::sender::OscSender;
use self::types::{OscConfig, OscInMessage, OscLearnTarget, OscMapping};
use crate::audio::features::AudioFeatures;
use crate::midi::types::TriggerAction;
use crate::params::{ParamDef, ParamStore, ParamValue};

/// Result of a single OscSystem::update() call.
pub struct OscFrameResult {
    pub triggers: Vec<TriggerAction>,
    pub layer_params: Vec<(usize, String, f32)>,
    pub layer_opacity: Vec<(usize, f32)>,
    pub layer_blend: Vec<(usize, u32)>,
    pub layer_enabled: Vec<(usize, bool)>,
    pub postprocess_enabled: Option<bool>,
}

impl OscFrameResult {
    fn empty() -> Self {
        Self {
            triggers: Vec::new(),
            layer_params: Vec::new(),
            layer_opacity: Vec::new(),
            layer_blend: Vec::new(),
            layer_enabled: Vec::new(),
            postprocess_enabled: None,
        }
    }
}

/// Central OSC system: owns receiver thread, sender, config, learn state.
pub struct OscSystem {
    receiver: Option<Receiver<OscInMessage>>,
    shutdown: Option<Arc<AtomicBool>>,
    thread_handle: Option<JoinHandle<()>>,
    pub sender: OscSender,
    pub config: OscConfig,
    pub learn_target: Option<OscLearnTarget>,
    pub last_activity: Option<Instant>,
    pub last_address: Option<String>,
    last_tx_time: Instant,
}

impl OscSystem {
    pub fn new() -> Self {
        let config = OscConfig::load();
        let mut sys = Self {
            receiver: None,
            shutdown: None,
            thread_handle: None,
            sender: OscSender::new(),
            config,
            learn_target: None,
            last_activity: None,
            last_address: None,
            last_tx_time: Instant::now(),
        };

        // Start receiver if enabled
        if sys.config.enabled {
            sys.start_receiver();
        }

        // Configure sender if TX enabled
        if sys.config.tx_enabled {
            sys.sender.configure(&sys.config.tx_host, sys.config.tx_port);
        }

        sys
    }

    /// Start the OSC receiver thread.
    pub fn start_receiver(&mut self) {
        self.stop_receiver();
        let (tx, rx) = crossbeam_channel::bounded(64);
        match receiver::spawn_receiver(self.config.rx_port, tx) {
            Ok((shutdown, handle)) => {
                self.receiver = Some(rx);
                self.shutdown = Some(shutdown);
                self.thread_handle = Some(handle);
            }
            Err(e) => {
                log::error!("Failed to start OSC receiver on port {}: {e}", self.config.rx_port);
            }
        }
    }

    /// Stop the OSC receiver thread.
    pub fn stop_receiver(&mut self) {
        if let Some(ref shutdown) = self.shutdown {
            shutdown.store(true, Ordering::Relaxed);
        }
        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }
        self.receiver = None;
        self.shutdown = None;
        self.last_address = None;
    }

    /// Restart receiver (e.g., after port change).
    pub fn restart_receiver(&mut self) {
        self.stop_receiver();
        if self.config.enabled {
            self.start_receiver();
        }
    }

    /// Whether we've received OSC activity within the last 300ms.
    pub fn is_recently_active(&self) -> bool {
        self.last_activity
            .map(|t| t.elapsed().as_millis() < 300)
            .unwrap_or(false)
    }

    /// Start OSC learn for a parameter or trigger.
    pub fn start_learn(&mut self, target: OscLearnTarget) {
        self.learn_target = Some(target);
    }

    /// Cancel OSC learn.
    pub fn cancel_learn(&mut self) {
        self.learn_target = None;
    }

    /// Clear a param mapping.
    pub fn clear_param_mapping(&mut self, name: &str) {
        self.config.params.remove(name);
        self.config.save();
    }

    /// Clear a trigger mapping.
    pub fn clear_trigger_mapping(&mut self, action: TriggerAction) {
        self.config.triggers.remove(&action);
        self.config.save();
    }

    /// Enable or disable OSC.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.config.enabled = enabled;
        if enabled {
            self.start_receiver();
        } else {
            self.stop_receiver();
        }
        self.config.save();
    }

    /// Enable or disable TX.
    pub fn set_tx_enabled(&mut self, enabled: bool) {
        self.config.tx_enabled = enabled;
        if enabled {
            self.sender.configure(&self.config.tx_host, self.config.tx_port);
        } else {
            self.sender.disable();
        }
        self.config.save();
    }

    /// Main per-frame update. Drains OSC messages, applies to active layer params, returns structured results.
    pub fn update(
        &mut self,
        param_store: &mut ParamStore,
        param_defs: &[ParamDef],
    ) -> OscFrameResult {
        let mut result = OscFrameResult::empty();

        let Some(ref rx) = self.receiver else {
            return result;
        };

        let messages: Vec<OscInMessage> = rx.try_iter().collect();
        if messages.is_empty() {
            return result;
        }

        self.last_activity = Some(Instant::now());

        if !self.config.enabled {
            // Drain but don't process
            if let Some(msg) = messages.last() {
                self.last_address = Some(msg_address(msg));
            }
            return result;
        }

        for msg in messages {
            let address = msg_address(&msg);
            self.last_address = Some(address.clone());

            // OSC Learn mode
            if let Some(ref target) = self.learn_target.clone() {
                let mapping = OscMapping { address: address.clone() };
                match target {
                    OscLearnTarget::Param(name) => {
                        self.config.params.insert(name.clone(), mapping);
                    }
                    OscLearnTarget::Trigger(action) => {
                        self.config.triggers.insert(*action, mapping);
                    }
                }
                self.learn_target = None;
                self.config.save();
                log::info!("OSC learned: {} → {:?}", address, target);
                continue;
            }

            match msg {
                OscInMessage::Param { name, value } => {
                    apply_param(param_store, param_defs, &name, value);
                }
                OscInMessage::LayerParam { layer, name, value } => {
                    result.layer_params.push((layer, name, value));
                }
                OscInMessage::Trigger(action) => {
                    result.triggers.push(action);
                }
                OscInMessage::LayerOpacity { layer, value } => {
                    result.layer_opacity.push((layer, value));
                }
                OscInMessage::LayerBlend { layer, value } => {
                    result.layer_blend.push((layer, value));
                }
                OscInMessage::LayerEnabled { layer, value } => {
                    result.layer_enabled.push((layer, value));
                }
                OscInMessage::PostProcessEnabled(enabled) => {
                    result.postprocess_enabled = Some(enabled);
                }
                OscInMessage::Raw { ref address, value } => {
                    // Check learned param mappings
                    if let Some(param_name) = self.config.find_param(address) {
                        let param_name = param_name.to_string();
                        apply_param(param_store, param_defs, &param_name, value);
                    }
                    // Check learned trigger mappings
                    else if let Some(action) = self.config.find_trigger(address) {
                        if value > 0.5 {
                            result.triggers.push(action);
                        }
                    }
                }
            }
        }

        result
    }

    /// Drain OSC messages but only process triggers (skip param application). Used when active layer is locked.
    pub fn update_triggers_only(&mut self) -> OscFrameResult {
        let mut result = OscFrameResult::empty();

        let Some(ref rx) = self.receiver else {
            return result;
        };

        let messages: Vec<OscInMessage> = rx.try_iter().collect();
        if messages.is_empty() {
            return result;
        }

        self.last_activity = Some(Instant::now());

        if !self.config.enabled {
            if let Some(msg) = messages.last() {
                self.last_address = Some(msg_address(msg));
            }
            return result;
        }

        for msg in messages {
            let address = msg_address(&msg);
            self.last_address = Some(address.clone());

            // Learn still works while locked
            if let Some(ref target) = self.learn_target.clone() {
                let mapping = OscMapping { address: address.clone() };
                match target {
                    OscLearnTarget::Param(name) => {
                        self.config.params.insert(name.clone(), mapping);
                    }
                    OscLearnTarget::Trigger(action) => {
                        self.config.triggers.insert(*action, mapping);
                    }
                }
                self.learn_target = None;
                self.config.save();
                continue;
            }

            // Only process triggers and layer-targeted messages
            match msg {
                OscInMessage::Trigger(action) => {
                    result.triggers.push(action);
                }
                OscInMessage::LayerOpacity { layer, value } => {
                    result.layer_opacity.push((layer, value));
                }
                OscInMessage::LayerBlend { layer, value } => {
                    result.layer_blend.push((layer, value));
                }
                OscInMessage::LayerEnabled { layer, value } => {
                    result.layer_enabled.push((layer, value));
                }
                OscInMessage::LayerParam { layer, name, value } => {
                    result.layer_params.push((layer, name, value));
                }
                OscInMessage::PostProcessEnabled(enabled) => {
                    result.postprocess_enabled = Some(enabled);
                }
                OscInMessage::Raw { ref address, value } => {
                    if let Some(action) = self.config.find_trigger(address) {
                        if value > 0.5 {
                            result.triggers.push(action);
                        }
                    }
                }
                _ => {} // Skip active-layer param application
            }
        }

        result
    }

    /// Send outbound state if TX is enabled and rate-limited.
    pub fn send_state(&mut self, features: &AudioFeatures, active_layer: usize, effect_name: &str) {
        if !self.config.tx_enabled {
            return;
        }
        let interval_ms = 1000 / self.config.tx_rate_hz.max(1);
        if self.last_tx_time.elapsed().as_millis() < interval_ms as u128 {
            return;
        }
        self.last_tx_time = Instant::now();
        self.sender.send_audio(features);
        self.sender.send_state(active_layer, effect_name);
    }
}

impl Drop for OscSystem {
    fn drop(&mut self) {
        self.stop_receiver();
    }
}

/// Extract address string from any OscInMessage variant.
fn msg_address(msg: &OscInMessage) -> String {
    match msg {
        OscInMessage::Param { name, .. } => format!("/phosphor/param/{name}"),
        OscInMessage::LayerParam { layer, name, .. } => format!("/phosphor/layer/{layer}/param/{name}"),
        OscInMessage::Trigger(action) => format!("/phosphor/trigger/{}", trigger_slug(action)),
        OscInMessage::LayerOpacity { layer, .. } => format!("/phosphor/layer/{layer}/opacity"),
        OscInMessage::LayerBlend { layer, .. } => format!("/phosphor/layer/{layer}/blend"),
        OscInMessage::LayerEnabled { layer, .. } => format!("/phosphor/layer/{layer}/enabled"),
        OscInMessage::PostProcessEnabled(_) => "/phosphor/postprocess/enabled".to_string(),
        OscInMessage::Raw { address, .. } => address.clone(),
    }
}

fn trigger_slug(action: &TriggerAction) -> &'static str {
    match action {
        TriggerAction::NextEffect => "next_effect",
        TriggerAction::PrevEffect => "prev_effect",
        TriggerAction::TogglePostProcess => "toggle_postprocess",
        TriggerAction::ToggleOverlay => "toggle_overlay",
        TriggerAction::NextPreset => "next_preset",
        TriggerAction::PrevPreset => "prev_preset",
        TriggerAction::NextLayer => "next_layer",
        TriggerAction::PrevLayer => "prev_layer",
    }
}

/// Apply a float value to a param, scaling to its defined range.
fn apply_param(store: &mut ParamStore, defs: &[ParamDef], name: &str, value: f32) {
    if let Some(def) = defs.iter().find(|d| d.name() == name) {
        match def {
            ParamDef::Float { min, max, .. } => {
                // OSC value is treated as 0-1 normalized, scaled to param range
                let val = min + (max - min) * value.clamp(0.0, 1.0);
                store.set(name, ParamValue::Float(val));
            }
            ParamDef::Bool { .. } => {
                store.set(name, ParamValue::Bool(value > 0.5));
            }
            _ => {}
        }
    }
}
