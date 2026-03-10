use super::persistence;
use super::types::*;

/// One-time migration of legacy MIDI/OSC param mappings to binding bus.
/// Called on first launch when global-bindings.json doesn't exist yet.
pub fn migrate_legacy_if_needed() {
    if persistence::global_exists() {
        return; // Already migrated or user has bindings
    }

    let mut bindings = Vec::new();
    let mut next_id: u64 = 1;

    // Migrate MIDI param mappings from midi.json
    let midi_config = crate::midi::mapping::MidiConfig::load();
    for (param_name, mapping) in &midi_config.params {
        let device = midi_config
            .port_name
            .as_deref()
            .unwrap_or("unknown");
        let device_sanitized: String = device
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '_' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let msg_prefix = match mapping.msg_type {
            crate::midi::types::MidiMsgType::Cc => "cc",
            crate::midi::types::MidiMsgType::Note => "note",
        };
        let source = format!(
            "midi.{}.{}.{}.{}",
            device_sanitized, msg_prefix, mapping.channel, mapping.cc
        );
        let target = format!("param.*.{param_name}"); // wildcard effect

        let mut transforms = Vec::new();
        if mapping.invert {
            transforms.push(TransformDef::Invert);
        }
        if mapping.min_val != 0.0 || mapping.max_val != 1.0 {
            transforms.push(TransformDef::Remap {
                in_lo: 0.0,
                in_hi: 1.0,
                out_lo: mapping.min_val,
                out_hi: mapping.max_val,
            });
        }

        bindings.push(Binding {
            id: format!("b_{:03}", next_id),
            name: format!("MIDI {} → {}", mapping.cc, param_name),
            enabled: true,
            scope: BindingScope::Global,
            source,
            target,
            transforms,
        });
        next_id += 1;
    }

    // Migrate OSC param mappings from osc.json
    let osc_config = crate::osc::types::OscConfig::load();
    for (param_name, mapping) in &osc_config.params {
        let source = format!("osc.{}", mapping.address);
        let target = format!("param.*.{param_name}");

        bindings.push(Binding {
            id: format!("b_{:03}", next_id),
            name: format!("OSC {} → {}", mapping.address, param_name),
            enabled: true,
            scope: BindingScope::Global,
            source,
            target,
            transforms: Vec::new(),
        });
        next_id += 1;
    }

    if !bindings.is_empty() {
        log::info!(
            "Migrated {} legacy MIDI/OSC param mappings to binding bus",
            bindings.len()
        );
        persistence::save_global(&bindings);
    } else {
        // Create empty file to mark migration complete
        persistence::save_global(&[]);
    }
}
