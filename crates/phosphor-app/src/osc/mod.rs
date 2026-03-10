pub mod receiver;
pub mod sender;
pub mod types;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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
    // Scene control
    pub scene_goto_cue: Option<usize>,
    pub scene_load_index: Option<usize>,
    pub scene_load_name: Option<String>,
    pub scene_loop_mode: Option<bool>,
    pub scene_advance_mode: Option<u8>,
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
            scene_goto_cue: None,
            scene_load_index: None,
            scene_load_name: None,
            scene_loop_mode: None,
            scene_advance_mode: None,
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
    /// Last raw OSC values for binding bus: address -> last value.
    pub last_raw_values: std::collections::HashMap<String, f32>,
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
            last_raw_values: std::collections::HashMap::new(),
        };

        // Start receiver if enabled
        if sys.config.enabled {
            sys.start_receiver();
        }

        // Configure sender if TX enabled
        if sys.config.tx_enabled {
            sys.sender
                .configure(&sys.config.tx_host, sys.config.tx_port);
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
                log::error!(
                    "Failed to start OSC receiver on port {}: {e}",
                    self.config.rx_port
                );
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
    #[allow(dead_code)]
    pub fn set_tx_enabled(&mut self, enabled: bool) {
        self.config.tx_enabled = enabled;
        if enabled {
            self.sender
                .configure(&self.config.tx_host, self.config.tx_port);
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
                let mapping = OscMapping {
                    address: address.clone(),
                };
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

            // Accumulate all messages for binding bus
            if let Some(value) = msg_value(&msg) {
                self.last_raw_values.insert(address.clone(), value);
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
                OscInMessage::SceneGotoCue(index) => {
                    result.scene_goto_cue = Some(index);
                }
                OscInMessage::SceneLoadIndex(index) => {
                    result.scene_load_index = Some(index);
                }
                OscInMessage::SceneLoadName(name) => {
                    result.scene_load_name = Some(name);
                }
                OscInMessage::SceneLoopMode(enabled) => {
                    result.scene_loop_mode = Some(enabled);
                }
                OscInMessage::SceneAdvanceMode(mode) => {
                    result.scene_advance_mode = Some(mode);
                }
                OscInMessage::Raw { ref address, value } => {
                    // Accumulate for binding bus
                    self.last_raw_values.insert(address.clone(), value);
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
                let mapping = OscMapping {
                    address: address.clone(),
                };
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
                OscInMessage::SceneGotoCue(index) => {
                    result.scene_goto_cue = Some(index);
                }
                OscInMessage::SceneLoadIndex(index) => {
                    result.scene_load_index = Some(index);
                }
                OscInMessage::SceneLoadName(name) => {
                    result.scene_load_name = Some(name);
                }
                OscInMessage::SceneLoopMode(enabled) => {
                    result.scene_loop_mode = Some(enabled);
                }
                OscInMessage::SceneAdvanceMode(mode) => {
                    result.scene_advance_mode = Some(mode);
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
    pub fn send_state(
        &mut self,
        features: &AudioFeatures,
        active_layer: usize,
        effect_name: &str,
        timeline_active: bool,
        timeline_cue_index: usize,
        timeline_cue_count: usize,
        timeline_transition_progress: f32,
    ) {
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
        self.sender.send_timeline(
            timeline_active,
            timeline_cue_index,
            timeline_cue_count,
            timeline_transition_progress,
        );
    }
}

impl Drop for OscSystem {
    fn drop(&mut self) {
        self.stop_receiver();
    }
}

/// Extract address string from any OscInMessage variant.
/// Extract the float value from an OSC message (for binding bus accumulation).
fn msg_value(msg: &OscInMessage) -> Option<f32> {
    match msg {
        OscInMessage::Param { value, .. }
        | OscInMessage::LayerParam { value, .. }
        | OscInMessage::LayerOpacity { value, .. }
        | OscInMessage::Raw { value, .. } => Some(*value),
        OscInMessage::LayerBlend { value, .. } => Some(*value as f32),
        OscInMessage::LayerEnabled { value, .. }
        | OscInMessage::PostProcessEnabled(value)
        | OscInMessage::SceneLoopMode(value) => Some(if *value { 1.0 } else { 0.0 }),
        OscInMessage::SceneGotoCue(v) | OscInMessage::SceneLoadIndex(v) => Some(*v as f32),
        OscInMessage::SceneAdvanceMode(v) => Some(*v as f32),
        OscInMessage::Trigger(_) | OscInMessage::SceneLoadName(_) => None,
    }
}

fn msg_address(msg: &OscInMessage) -> String {
    match msg {
        OscInMessage::Param { name, .. } => format!("/phosphor/param/{name}"),
        OscInMessage::LayerParam { layer, name, .. } => {
            format!("/phosphor/layer/{layer}/param/{name}")
        }
        OscInMessage::Trigger(action) => format!("/phosphor/trigger/{}", trigger_slug(action)),
        OscInMessage::LayerOpacity { layer, .. } => format!("/phosphor/layer/{layer}/opacity"),
        OscInMessage::LayerBlend { layer, .. } => format!("/phosphor/layer/{layer}/blend"),
        OscInMessage::LayerEnabled { layer, .. } => format!("/phosphor/layer/{layer}/enabled"),
        OscInMessage::PostProcessEnabled(_) => "/phosphor/postprocess/enabled".to_string(),
        OscInMessage::SceneGotoCue(_) => "/phosphor/scene/goto_cue".to_string(),
        OscInMessage::SceneLoadIndex(_) => "/phosphor/scene/load".to_string(),
        OscInMessage::SceneLoadName(_) => "/phosphor/scene/load".to_string(),
        OscInMessage::SceneLoopMode(_) => "/phosphor/scene/loop_mode".to_string(),
        OscInMessage::SceneAdvanceMode(_) => "/phosphor/scene/advance_mode".to_string(),
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
        TriggerAction::SceneGoNext => "scene_go_next",
        TriggerAction::SceneGoPrev => "scene_go_prev",
        TriggerAction::ToggleTimeline => "toggle_timeline",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trigger_slug_all_actions() {
        let expected = [
            (TriggerAction::NextEffect, "next_effect"),
            (TriggerAction::PrevEffect, "prev_effect"),
            (TriggerAction::TogglePostProcess, "toggle_postprocess"),
            (TriggerAction::ToggleOverlay, "toggle_overlay"),
            (TriggerAction::NextPreset, "next_preset"),
            (TriggerAction::PrevPreset, "prev_preset"),
            (TriggerAction::NextLayer, "next_layer"),
            (TriggerAction::PrevLayer, "prev_layer"),
            (TriggerAction::SceneGoNext, "scene_go_next"),
            (TriggerAction::SceneGoPrev, "scene_go_prev"),
            (TriggerAction::ToggleTimeline, "toggle_timeline"),
        ];
        for (action, slug) in expected {
            assert_eq!(trigger_slug(&action), slug);
        }
    }

    #[test]
    fn msg_address_param() {
        let msg = OscInMessage::Param {
            name: "speed".into(),
            value: 0.5,
        };
        assert_eq!(msg_address(&msg), "/phosphor/param/speed");
    }

    #[test]
    fn msg_address_layer_param() {
        let msg = OscInMessage::LayerParam {
            layer: 2,
            name: "intensity".into(),
            value: 0.5,
        };
        assert_eq!(msg_address(&msg), "/phosphor/layer/2/param/intensity");
    }

    #[test]
    fn msg_address_trigger() {
        let msg = OscInMessage::Trigger(TriggerAction::NextEffect);
        assert_eq!(msg_address(&msg), "/phosphor/trigger/next_effect");
    }

    // ---- Additional msg_address tests ----

    #[test]
    fn msg_address_layer_opacity() {
        let msg = OscInMessage::LayerOpacity {
            layer: 3,
            value: 0.5,
        };
        assert_eq!(msg_address(&msg), "/phosphor/layer/3/opacity");
    }

    #[test]
    fn msg_address_layer_blend() {
        let msg = OscInMessage::LayerBlend { layer: 1, value: 2 };
        assert_eq!(msg_address(&msg), "/phosphor/layer/1/blend");
    }

    #[test]
    fn msg_address_layer_enabled() {
        let msg = OscInMessage::LayerEnabled {
            layer: 0,
            value: true,
        };
        assert_eq!(msg_address(&msg), "/phosphor/layer/0/enabled");
    }

    #[test]
    fn msg_address_postprocess_enabled() {
        let msg = OscInMessage::PostProcessEnabled(true);
        assert_eq!(msg_address(&msg), "/phosphor/postprocess/enabled");
    }

    #[test]
    fn msg_address_raw() {
        let msg = OscInMessage::Raw {
            address: "/custom/addr".into(),
            value: 1.0,
        };
        assert_eq!(msg_address(&msg), "/custom/addr");
    }

    #[test]
    fn osc_frame_result_empty_fields() {
        let r = OscFrameResult::empty();
        assert!(r.triggers.is_empty());
        assert!(r.layer_params.is_empty());
        assert!(r.layer_opacity.is_empty());
        assert!(r.layer_blend.is_empty());
        assert!(r.layer_enabled.is_empty());
        assert!(r.postprocess_enabled.is_none());
        assert!(r.scene_goto_cue.is_none());
        assert!(r.scene_load_index.is_none());
        assert!(r.scene_load_name.is_none());
        assert!(r.scene_loop_mode.is_none());
        assert!(r.scene_advance_mode.is_none());
    }

    // ---- Scene msg_address tests ----

    #[test]
    fn msg_address_scene_goto_cue() {
        let msg = OscInMessage::SceneGotoCue(3);
        assert_eq!(msg_address(&msg), "/phosphor/scene/goto_cue");
    }

    #[test]
    fn msg_address_scene_load_index() {
        let msg = OscInMessage::SceneLoadIndex(0);
        assert_eq!(msg_address(&msg), "/phosphor/scene/load");
    }

    #[test]
    fn msg_address_scene_load_name() {
        let msg = OscInMessage::SceneLoadName("Test".into());
        assert_eq!(msg_address(&msg), "/phosphor/scene/load");
    }

    #[test]
    fn msg_address_scene_loop_mode() {
        let msg = OscInMessage::SceneLoopMode(true);
        assert_eq!(msg_address(&msg), "/phosphor/scene/loop_mode");
    }

    #[test]
    fn msg_address_scene_advance_mode() {
        let msg = OscInMessage::SceneAdvanceMode(1);
        assert_eq!(msg_address(&msg), "/phosphor/scene/advance_mode");
    }
}
