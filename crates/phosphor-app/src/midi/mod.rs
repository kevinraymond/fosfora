pub mod input;
pub mod mapping;
pub mod types;

use std::collections::HashMap;
use std::time::Instant;

use crossbeam_channel::Receiver;

use self::input::MidiPort;
use self::mapping::{MidiConfig, MidiMapping};
use self::types::{LearnTarget, MidiMessage, MidiMsgType, TriggerAction};
use crate::params::{ParamDef, ParamStore, ParamValue};

/// Result of a single MidiSystem::update() call.
pub struct MidiFrameResult {
    pub triggers: Vec<TriggerAction>,
}

/// Central MIDI system: owns connection, config, learn state.
pub struct MidiSystem {
    receiver: Option<Receiver<MidiMessage>>,
    connection: Option<MidiPort>,
    pub config: MidiConfig,
    pub learn_target: Option<LearnTarget>,
    trigger_prev_values: HashMap<(TriggerAction, u8, u8), u8>, // (action, cc, channel) -> last value
    pub last_activity: Option<Instant>,
    pub last_message: Option<MidiMessage>,
    last_port_poll: Instant,
    pub available_ports: Vec<String>,
}

impl MidiSystem {
    pub fn new() -> Self {
        let config = MidiConfig::load();
        let mut sys = Self {
            receiver: None,
            connection: None,
            config,
            learn_target: None,
            trigger_prev_values: HashMap::new(),
            last_activity: None,
            last_message: None,
            last_port_poll: Instant::now(),
            available_ports: Vec::new(),
        };

        // Initial port scan
        sys.available_ports = MidiPort::list_ports();

        // Auto-connect to saved port or first available
        if let Some(ref saved) = sys.config.port_name.clone() {
            if sys.available_ports.contains(saved) {
                sys.connect(saved);
            } else if let Some(first) = sys.available_ports.first().cloned() {
                log::info!("Saved MIDI port '{}' not found, trying '{}'", saved, first);
                sys.connect(&first);
            }
        } else if let Some(first) = sys.available_ports.first().cloned() {
            sys.connect(&first);
        }

        sys
    }

    /// Connect to a MIDI port by name.
    pub fn connect(&mut self, port_name: &str) {
        // Disconnect existing
        self.disconnect();

        match MidiPort::open(port_name) {
            Ok((port, rx)) => {
                log::info!("Connected to MIDI port: {}", port_name);
                self.connection = Some(port);
                self.receiver = Some(rx);
                self.config.port_name = Some(port_name.to_string());
                self.config.save();
            }
            Err(e) => {
                log::error!("Failed to connect MIDI port '{}': {e}", port_name);
            }
        }
    }

    /// Disconnect from the current MIDI port.
    pub fn disconnect(&mut self) {
        if self.connection.is_some() {
            log::info!("Disconnecting MIDI");
        }
        self.connection = None;
        self.receiver = None;
        self.last_message = None;
        self.trigger_prev_values.clear();
    }

    /// Name of the currently connected port, if any.
    pub fn connected_port(&self) -> Option<&str> {
        self.connection.as_ref().map(|c| c.port_name.as_str())
    }

    /// Whether we've received MIDI activity within the last 300ms.
    pub fn is_recently_active(&self) -> bool {
        self.last_activity
            .map(|t| t.elapsed().as_millis() < 300)
            .unwrap_or(false)
    }

    /// Start MIDI learn for a parameter or trigger.
    pub fn start_learn(&mut self, target: LearnTarget) {
        self.learn_target = Some(target);
    }

    /// Cancel MIDI learn.
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

    /// Main per-frame update. Call from App::update().
    pub fn update(
        &mut self,
        param_store: &mut ParamStore,
        param_defs: &[ParamDef],
    ) -> MidiFrameResult {
        let mut result = MidiFrameResult {
            triggers: Vec::new(),
        };

        // Hot-plug detection: poll ports every 2 seconds
        if self.last_port_poll.elapsed().as_secs() >= 2 {
            self.last_port_poll = Instant::now();
            self.available_ports = MidiPort::list_ports();

            if let Some(ref port) = self.connection {
                // Check if our connected port disappeared
                if !self.available_ports.contains(&port.port_name) {
                    log::warn!("MIDI port '{}' disconnected", port.port_name);
                    self.disconnect();
                }
            } else if let Some(ref saved) = self.config.port_name.clone() {
                // Try to reconnect to saved port
                if self.available_ports.contains(saved) {
                    log::info!("MIDI port '{}' reappeared, reconnecting", saved);
                    self.connect(saved);
                }
            }
        }

        // Drain messages from channel
        let Some(ref rx) = self.receiver else {
            return result;
        };

        let messages: Vec<MidiMessage> = rx.try_iter().collect();
        if messages.is_empty() {
            return result;
        }

        self.last_activity = Some(Instant::now());

        for msg in messages {
            self.last_message = Some(msg);

            // MIDI Learn mode: bind first meaningful message
            if let Some(ref target) = self.learn_target.clone() {
                // Skip Note Off (value 0) during learn
                if msg.msg_type == MidiMsgType::Note && msg.value == 0 {
                    continue;
                }

                let mapping = MidiMapping::from_learn(msg.number, msg.channel, msg.msg_type);
                match target {
                    LearnTarget::Param(name) => {
                        self.config.params.insert(name.clone(), mapping);
                    }
                    LearnTarget::Trigger(action) => {
                        self.config.triggers.insert(*action, mapping);
                    }
                }
                self.learn_target = None;
                self.config.save();
                log::info!(
                    "MIDI learned: {:?} ch{} #{} → {:?}",
                    msg.msg_type,
                    msg.channel,
                    msg.number,
                    target
                );
                continue; // Don't apply the learn message as a value
            }

            // Apply param mappings
            if let Some(param_name) =
                self.config
                    .find_param(msg.number, msg.channel, msg.msg_type)
            {
                let param_name = param_name.to_string();
                let mapping = &self.config.params[&param_name];
                let scaled = mapping.scale(msg.value);

                // Find the param def to know the type
                if let Some(def) = param_defs.iter().find(|d| d.name() == param_name) {
                    match def {
                        ParamDef::Float { min, max, .. } => {
                            let val = min + (max - min) * scaled;
                            param_store.set(&param_name, ParamValue::Float(val));
                        }
                        ParamDef::Bool { .. } => {
                            param_store.set(&param_name, ParamValue::Bool(scaled > 0.5));
                        }
                        _ => {} // Color/Point2D not mappable via single CC
                    }
                }
            }

            // Apply trigger mappings (rising edge detection)
            if let Some(action) =
                self.config
                    .find_trigger(msg.number, msg.channel, msg.msg_type)
            {
                let key = (action, msg.number, msg.channel);
                let prev = self.trigger_prev_values.get(&key).copied().unwrap_or(0);
                let threshold = 64u8; // ~0.5 in MIDI range
                if msg.value >= threshold && prev < threshold {
                    result.triggers.push(action);
                }
                self.trigger_prev_values.insert(key, msg.value);
            }
        }

        result
    }

    /// Drain MIDI messages but only process triggers (skip param CC). Used when active layer is locked.
    pub fn update_triggers_only(&mut self) -> MidiFrameResult {
        let mut result = MidiFrameResult {
            triggers: Vec::new(),
        };

        // Hot-plug detection (same as update)
        if self.last_port_poll.elapsed().as_secs() >= 2 {
            self.last_port_poll = Instant::now();
            self.available_ports = MidiPort::list_ports();

            if let Some(ref port) = self.connection {
                if !self.available_ports.contains(&port.port_name) {
                    log::warn!("MIDI port '{}' disconnected", port.port_name);
                    self.disconnect();
                }
            } else if let Some(ref saved) = self.config.port_name.clone() {
                if self.available_ports.contains(saved) {
                    log::info!("MIDI port '{}' reappeared, reconnecting", saved);
                    self.connect(saved);
                }
            }
        }

        let Some(ref rx) = self.receiver else {
            return result;
        };

        let messages: Vec<MidiMessage> = rx.try_iter().collect();
        if messages.is_empty() {
            return result;
        }

        self.last_activity = Some(Instant::now());

        for msg in messages {
            self.last_message = Some(msg);

            // MIDI Learn still works (so you can bind triggers while locked)
            if let Some(ref target) = self.learn_target.clone() {
                if msg.msg_type == MidiMsgType::Note && msg.value == 0 {
                    continue;
                }
                let mapping = MidiMapping::from_learn(msg.number, msg.channel, msg.msg_type);
                match target {
                    LearnTarget::Param(name) => {
                        self.config.params.insert(name.clone(), mapping);
                    }
                    LearnTarget::Trigger(action) => {
                        self.config.triggers.insert(*action, mapping);
                    }
                }
                self.learn_target = None;
                self.config.save();
                continue;
            }

            // Skip param mappings — only process triggers
            if let Some(action) =
                self.config
                    .find_trigger(msg.number, msg.channel, msg.msg_type)
            {
                let key = (action, msg.number, msg.channel);
                let prev = self.trigger_prev_values.get(&key).copied().unwrap_or(0);
                let threshold = 64u8;
                if msg.value >= threshold && prev < threshold {
                    result.triggers.push(action);
                }
                self.trigger_prev_values.insert(key, msg.value);
            }
        }

        result
    }
}
