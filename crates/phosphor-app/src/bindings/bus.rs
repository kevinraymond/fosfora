use std::collections::HashMap;
use std::time::Instant;

use crate::audio::features::AudioFeatures;
use crate::midi::MidiSystem;
use crate::osc::OscSystem;

use super::persistence;
use super::sources::{self, SourceSnapshot};
use super::transforms;
use super::types::*;

/// Central binding bus: owns bindings, runtime state, and evaluates each frame.
pub struct BindingBus {
    pub bindings: Vec<Binding>,
    pub(crate) runtimes: HashMap<BindingId, BindingRuntime>,
    /// WebSocket binding data values (accumulated from WS data frames).
    pub ws_bind_values: HashMap<String, f32>,
    /// Preview thumbnail JPEG data from WS bridge sources.
    pub ws_preview_images: HashMap<String, Vec<u8>>,
    /// Last time each WS field was updated (for per-field expiry).
    pub(crate) ws_field_last_seen: HashMap<String, Instant>,
    pub(crate) next_id_counter: u64,
    pub(crate) dirty: bool,
    /// Debounce: when the dirty flag was first set (save after 1s of no changes).
    pub(crate) dirty_since: Option<Instant>,
    pub learn_target: Option<LearnState>,
    /// Last frame's source snapshot (for UI diagnostics / templates).
    pub last_snapshot: SourceSnapshot,
    /// Scene transport triggers pending drain by the main loop.
    pub pending_triggers: Vec<String>,
}

impl BindingBus {
    pub fn new() -> Self {
        let global = persistence::load_global();
        let max_id = global
            .iter()
            .filter_map(|b| b.id.strip_prefix("b_").and_then(|s| s.parse::<u64>().ok()))
            .max()
            .unwrap_or(0);

        let mut runtimes = HashMap::new();
        for b in &global {
            runtimes.insert(b.id.clone(), BindingRuntime::new());
        }

        Self {
            bindings: global,
            runtimes,
            ws_bind_values: HashMap::new(),
            ws_preview_images: HashMap::new(),
            ws_field_last_seen: HashMap::new(),
            next_id_counter: max_id + 1,
            dirty: false,
            dirty_since: None,
            learn_target: None,
            last_snapshot: HashMap::new(),
            pending_triggers: Vec::new(),
        }
    }

    /// Create a new binding with default settings.
    pub fn add_binding(
        &mut self,
        source: String,
        target: String,
        scope: BindingScope,
    ) -> BindingId {
        let id = format!("b_{:03}", self.next_id_counter);
        self.next_id_counter += 1;

        // Name left empty — UI will show auto-derived name from source+target
        let binding = Binding {
            id: id.clone(),
            name: String::new(),
            enabled: true,
            scope,
            source,
            target,
            transforms: Vec::new(),
        };

        self.runtimes.insert(id.clone(), BindingRuntime::new());
        self.bindings.push(binding);
        self.mark_dirty();
        id
    }

    /// Remove a binding by ID. Saves immediately.
    pub fn remove_binding(&mut self, id: &str) {
        self.bindings.retain(|b| b.id != id);
        self.runtimes.remove(id);
        self.save_global();
    }

    /// Get a binding by ID.
    pub fn get_binding(&self, id: &str) -> Option<&Binding> {
        self.bindings.iter().find(|b| b.id == id)
    }

    /// Get a mutable binding by ID. Marks as dirty for debounced save.
    pub fn get_binding_mut(&mut self, id: &str) -> Option<&mut Binding> {
        self.mark_dirty();
        self.bindings.iter_mut().find(|b| b.id == id)
    }

    fn mark_dirty(&mut self) {
        self.dirty = true;
        if self.dirty_since.is_none() {
            self.dirty_since = Some(Instant::now());
        }
    }

    /// Find all bindings targeting a given target string.
    #[allow(dead_code)]
    pub fn bindings_for_target(&self, target: &str) -> Vec<&Binding> {
        self.bindings
            .iter()
            .filter(|b| b.target == target)
            .collect()
    }

    /// Get runtime state for a binding (for UI diagnostics).
    pub fn runtime(&self, id: &str) -> Option<&BindingRuntime> {
        self.runtimes.get(id)
    }

    /// Ingest WS bind values, tracking per-field timestamps and expiring
    /// individual fields that haven't been updated in 5 seconds.
    pub fn ingest_ws_values(&mut self, incoming: &HashMap<String, f32>) {
        const WS_FIELD_TIMEOUT_SECS: f64 = 5.0;
        let now = Instant::now();

        // Update values and per-field timestamps
        for (key, &value) in incoming {
            self.ws_bind_values.insert(key.clone(), value);
            self.ws_field_last_seen.insert(key.clone(), now);
        }

        // Prune expired fields — but keep (zeroed) any field that has an
        // active binding, so wired-up dynamic sources survive expiry.
        let expired: Vec<String> = self
            .ws_field_last_seen
            .iter()
            .filter(|(_, last)| now.duration_since(**last).as_secs_f64() > WS_FIELD_TIMEOUT_SECS)
            .map(|(k, _)| k.clone())
            .collect();

        for key in &expired {
            let ws_source_key = format!("ws.{key}");
            let is_bound = self
                .bindings
                .iter()
                .any(|b| b.enabled && b.source == ws_source_key);

            if is_bound {
                // Keep the field alive at zero so the binding stays wired
                self.ws_bind_values.insert(key.clone(), 0.0);
            } else {
                self.ws_field_last_seen.remove(key);
                self.ws_bind_values.remove(key);
            }
        }

        // Clean up preview images for sources with no remaining fields
        self.ws_preview_images.retain(|source, _| {
            let prefix = format!("{source}.");
            self.ws_bind_values.keys().any(|k| k.starts_with(&prefix))
        });
    }

    /// Count of enabled bindings.
    pub fn active_count(&self) -> usize {
        self.bindings.iter().filter(|b| b.enabled).count()
    }

    /// Evaluate all enabled bindings for one frame.
    /// Returns (target_id, value) pairs for the app to apply.
    pub fn evaluate(
        &mut self,
        audio: Option<&AudioFeatures>,
        mel: &[f32],
        dmfcc: &[f32; 13],
        midi: &MidiSystem,
        osc: &OscSystem,
    ) -> Vec<(String, f32)> {
        // Collect source snapshots (always, even with no bindings — needed for matrix meters)
        let mut snapshot = HashMap::with_capacity(128);

        if let Some(features) = audio {
            snapshot.extend(sources::collect_audio(features));
        }
        // Mel bands (A1b `audio.mel.N`, #1512) — empty until the first audio frame arrives.
        if !mel.is_empty() {
            snapshot.extend(sources::collect_mel_bands(mel));
        }
        // Delta-MFCC slopes (A16 `audio.dmfcc.N`, #1467) — bindings-only timbre-motion sources;
        // zeros until the first audio frame arrives.
        snapshot.extend(sources::collect_dmfcc_bands(dmfcc));
        snapshot.extend(sources::collect_midi(midi));
        snapshot.extend(sources::collect_osc(osc));
        snapshot.extend(sources::collect_websocket(&self.ws_bind_values));

        if self.bindings.is_empty() {
            self.last_snapshot = snapshot;
            return Vec::new();
        }

        // Check learn mode: if waiting for source, grab first new value
        if let Some(ref learn) = self.learn_target {
            if learn.field == LearnField::Source {
                // For MIDI/OSC learn, find first key that starts with midi. or osc.
                // that has a non-zero value (indicates new activity)
                let midi_source = snapshot
                    .iter()
                    .find(|(k, (v, _))| k.starts_with("midi.") && *v > 0.0)
                    .map(|(k, _)| k.clone());
                let osc_source = snapshot
                    .iter()
                    .find(|(k, _)| k.starts_with("osc."))
                    .map(|(k, _)| k.clone());

                if let Some(source_key) = midi_source.or(osc_source) {
                    let binding_id = learn.binding_id.clone();
                    self.learn_target = None;
                    if let Some(b) = self.bindings.iter_mut().find(|b| b.id == binding_id) {
                        b.source = source_key;
                        self.dirty = true;
                    }
                }
            }
        }

        // Evaluate each enabled binding
        let mut results = Vec::with_capacity(self.bindings.len());

        for binding in &self.bindings {
            if !binding.enabled {
                continue;
            }

            let Some((value, raw)) = snapshot.get(&binding.source) else {
                continue;
            };

            let runtime = match self.runtimes.get_mut(binding.id.as_str()) {
                Some(r) => r,
                None => self.runtimes.entry(binding.id.clone()).or_default(),
            };

            runtime.last_input = Some(*value);
            runtime.last_raw = Some(raw.clone());

            let output = transforms::apply_chain(*value, &binding.transforms, runtime);
            runtime.last_output = Some(output);

            results.push((binding.target.clone(), output));
        }

        self.last_snapshot = snapshot;

        results
    }

    /// Load preset-scoped bindings (called on preset load).
    pub fn load_preset_bindings(&mut self, preset_name: &str) {
        // Remove existing preset-scoped bindings
        self.bindings.retain(|b| b.scope != BindingScope::Preset);
        self.merge_preset_bindings(persistence::load_preset(preset_name));
    }

    /// Merge preset-scoped bindings into the bus, re-assigning any id that
    /// collides with an existing (global) binding. The `b_{n}` counter is
    /// per-file, so a preset authored in another session can reuse an id a
    /// global binding holds — the runtimes map would silently collide and
    /// `remove_binding` (retain by id) would drop both. The sidecar file
    /// self-heals with the new ids on the next preset save.
    fn merge_preset_bindings(&mut self, mut incoming: Vec<Binding>) {
        let parse_id = |id: &str| id.strip_prefix("b_").and_then(|s| s.parse::<u64>().ok());
        let max_id = incoming.iter().filter_map(|b| parse_id(&b.id)).max();
        if let Some(max_id) = max_id {
            if max_id >= self.next_id_counter {
                self.next_id_counter = max_id + 1;
            }
        }

        let existing: std::collections::HashSet<String> =
            self.bindings.iter().map(|b| b.id.clone()).collect();
        for b in &mut incoming {
            if existing.contains(&b.id) {
                b.id = format!("b_{:03}", self.next_id_counter);
                self.next_id_counter += 1;
            }
            self.runtimes.insert(b.id.clone(), BindingRuntime::new());
        }
        self.bindings.extend(incoming);
    }

    /// Save preset-scoped bindings (called on preset save).
    pub fn save_preset_bindings(&self, preset_name: &str) {
        let preset_bindings: Vec<&Binding> = self
            .bindings
            .iter()
            .filter(|b| b.scope == BindingScope::Preset)
            .collect();
        let owned: Vec<Binding> = preset_bindings.into_iter().cloned().collect();
        persistence::save_preset(preset_name, &owned);
    }

    /// Save global-scoped bindings.
    pub fn save_global(&self) {
        let global_bindings: Vec<&Binding> = self
            .bindings
            .iter()
            .filter(|b| b.scope == BindingScope::Global)
            .collect();
        let owned: Vec<Binding> = global_bindings.into_iter().cloned().collect();
        persistence::save_global(&owned);
    }

    /// Auto-save if dirty (debounced: waits 1s after last change).
    pub fn save_if_dirty(&mut self) {
        if self.dirty {
            if let Some(since) = self.dirty_since {
                if since.elapsed().as_secs_f32() >= 1.0 {
                    self.save_global();
                    self.dirty = false;
                    self.dirty_since = None;
                }
            }
        }
    }

    /// Check if any source type has active bindings (for UI indicators).
    #[allow(dead_code)]
    pub fn has_source_type(&self, prefix: &str) -> bool {
        self.bindings
            .iter()
            .any(|b| b.enabled && b.source.starts_with(prefix))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_bus() -> BindingBus {
        BindingBus {
            bindings: Vec::new(),
            runtimes: HashMap::new(),
            ws_bind_values: HashMap::new(),
            ws_preview_images: HashMap::new(),
            ws_field_last_seen: HashMap::new(),
            next_id_counter: 1,
            dirty: false,
            dirty_since: None,
            learn_target: None,
            last_snapshot: HashMap::new(),
            pending_triggers: Vec::new(),
        }
    }

    fn preset_binding(id: &str, source: &str) -> Binding {
        Binding {
            id: id.into(),
            name: String::new(),
            enabled: true,
            scope: BindingScope::Preset,
            source: source.into(),
            target: "layer.0.opacity".into(),
            transforms: Vec::new(),
        }
    }

    #[test]
    fn merge_reassigns_colliding_preset_ids() {
        let mut bus = empty_bus();
        let global_id = bus.add_binding(
            "audio.kick".into(),
            "param.Phosphor.warp".into(),
            BindingScope::Global,
        );
        assert_eq!(global_id, "b_001");

        // A preset file authored elsewhere reuses b_001.
        bus.merge_preset_bindings(vec![
            preset_binding("b_001", "audio.rms"),
            preset_binding("b_007", "audio.flux"),
        ]);

        assert_eq!(bus.bindings.len(), 3);
        // The colliding preset binding got a fresh id; the global one is intact.
        let ids: Vec<&str> = bus.bindings.iter().map(|b| b.id.as_str()).collect();
        assert_eq!(ids.iter().filter(|i| **i == "b_001").count(), 1);
        let preset_ids: Vec<&str> = bus
            .bindings
            .iter()
            .filter(|b| b.scope == BindingScope::Preset)
            .map(|b| b.id.as_str())
            .collect();
        assert!(!preset_ids.contains(&"b_001"));
        assert!(preset_ids.contains(&"b_007"));
        // Every binding has a runtime under its FINAL id.
        for b in &bus.bindings {
            assert!(bus.runtimes.contains_key(&b.id), "no runtime for {}", b.id);
        }
        // Counter advanced past everything now in the bus.
        let max = bus
            .bindings
            .iter()
            .filter_map(|b| b.id.strip_prefix("b_").and_then(|s| s.parse::<u64>().ok()))
            .max()
            .unwrap();
        assert!(bus.next_id_counter > max);
    }

    #[test]
    fn remove_after_merge_removes_exactly_one() {
        let mut bus = empty_bus();
        bus.add_binding(
            "audio.kick".into(),
            "param.Phosphor.warp".into(),
            BindingScope::Global,
        );
        bus.merge_preset_bindings(vec![preset_binding("b_001", "audio.rms")]);
        assert_eq!(bus.bindings.len(), 2);

        // Deleting the global b_001 must not take the (re-id'd) preset one along.
        bus.remove_binding("b_001");
        assert_eq!(bus.bindings.len(), 1);
        assert_eq!(bus.bindings[0].scope, BindingScope::Preset);
        assert_eq!(bus.bindings[0].source, "audio.rms");
    }

    #[test]
    fn test_add_remove_binding() {
        let mut bus = BindingBus {
            bindings: Vec::new(),
            runtimes: HashMap::new(),
            ws_bind_values: HashMap::new(),
            ws_preview_images: HashMap::new(),
            ws_field_last_seen: HashMap::new(),
            next_id_counter: 1,
            dirty: false,
            dirty_since: None,
            learn_target: None,
            last_snapshot: HashMap::new(),
            pending_triggers: Vec::new(),
        };

        let id = bus.add_binding(
            "audio.kick".into(),
            "param.Phosphor.warp".into(),
            BindingScope::Global,
        );
        assert_eq!(id, "b_001");
        assert_eq!(bus.bindings.len(), 1);
        assert!(bus.dirty);

        bus.remove_binding(&id);
        assert_eq!(bus.bindings.len(), 0);
    }

    #[test]
    fn test_bindings_for_target() {
        let mut bus = BindingBus {
            bindings: Vec::new(),
            runtimes: HashMap::new(),
            ws_bind_values: HashMap::new(),
            ws_preview_images: HashMap::new(),
            ws_field_last_seen: HashMap::new(),
            next_id_counter: 1,
            dirty: false,
            dirty_since: None,
            learn_target: None,
            last_snapshot: HashMap::new(),
            pending_triggers: Vec::new(),
        };

        bus.add_binding(
            "audio.kick".into(),
            "param.Phosphor.warp".into(),
            BindingScope::Global,
        );
        bus.add_binding(
            "audio.rms".into(),
            "param.Phosphor.warp".into(),
            BindingScope::Global,
        );
        bus.add_binding(
            "audio.beat".into(),
            "layer.0.opacity".into(),
            BindingScope::Global,
        );

        let warp_bindings = bus.bindings_for_target("param.Phosphor.warp");
        assert_eq!(warp_bindings.len(), 2);

        let opacity_bindings = bus.bindings_for_target("layer.0.opacity");
        assert_eq!(opacity_bindings.len(), 1);
    }

    #[test]
    fn test_active_count() {
        let mut bus = BindingBus {
            bindings: Vec::new(),
            runtimes: HashMap::new(),
            ws_bind_values: HashMap::new(),
            ws_preview_images: HashMap::new(),
            ws_field_last_seen: HashMap::new(),
            next_id_counter: 1,
            dirty: false,
            dirty_since: None,
            learn_target: None,
            last_snapshot: HashMap::new(),
            pending_triggers: Vec::new(),
        };

        bus.add_binding(
            "audio.kick".into(),
            "param.P.w".into(),
            BindingScope::Global,
        );
        let id2 = bus.add_binding("audio.rms".into(), "param.P.x".into(), BindingScope::Global);

        assert_eq!(bus.active_count(), 2);

        if let Some(b) = bus.get_binding_mut(&id2) {
            b.enabled = false;
        }
        assert_eq!(bus.active_count(), 1);
    }
}
