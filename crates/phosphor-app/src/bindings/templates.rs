use super::bus::BindingBus;
use super::types::{BindingScope, TransformDef};

/// A built-in binding template.
pub struct BindingTemplate {
    pub name: &'static str,
    pub description: &'static str,
    pub entries: &'static [TemplateEntry],
}

/// A single entry in a template.
pub struct TemplateEntry {
    pub source: &'static str,
    /// Target pattern - "{effect}" is replaced with the current effect name.
    pub target_pattern: &'static str,
    pub transforms: fn() -> Vec<TransformDef>,
    pub scope: BindingScope,
}

/// All available built-in templates.
pub fn builtin_templates() -> &'static [&'static BindingTemplate] {
    static ALL: &[&BindingTemplate] =
        &[&AUDIO_REACTIVE, &BEAT_SYNC, &SPECTRAL_BANDS, &MIDI_FADERS];
    ALL
}

static AUDIO_REACTIVE: BindingTemplate = BindingTemplate {
    name: "Audio Reactive",
    description: "Kick, RMS, centroid, and beat phase mapped to common params",
    entries: &[
        TemplateEntry {
            source: "audio.kick",
            target_pattern: "param.{effect}.warp_intensity",
            transforms: || vec![TransformDef::Smooth { factor: 0.7 }],
            scope: BindingScope::Preset,
        },
        TemplateEntry {
            source: "audio.rms",
            target_pattern: "layer.0.opacity",
            transforms: || {
                vec![
                    TransformDef::Remap {
                        in_lo: 0.0,
                        in_hi: 0.8,
                        out_lo: 0.3,
                        out_hi: 1.0,
                    },
                    TransformDef::Smooth { factor: 0.85 },
                ]
            },
            scope: BindingScope::Preset,
        },
        TemplateEntry {
            source: "audio.centroid",
            target_pattern: "param.{effect}.color_shift",
            transforms: || vec![TransformDef::Smooth { factor: 0.9 }],
            scope: BindingScope::Preset,
        },
        TemplateEntry {
            source: "audio.beat_phase",
            target_pattern: "param.{effect}.rotation",
            transforms: || vec![],
            scope: BindingScope::Preset,
        },
    ],
};

static BEAT_SYNC: BindingTemplate = BindingTemplate {
    name: "Beat Sync",
    description: "Beat gate and smooth phase for rhythmic control",
    entries: &[
        TemplateEntry {
            source: "audio.beat",
            target_pattern: "param.{effect}.warp_intensity",
            transforms: || {
                vec![
                    TransformDef::Gate { threshold: 0.5 },
                    TransformDef::Smooth { factor: 0.6 },
                ]
            },
            scope: BindingScope::Preset,
        },
        TemplateEntry {
            source: "audio.beat_phase",
            target_pattern: "param.{effect}.rotation",
            transforms: || vec![TransformDef::Smooth { factor: 0.8 }],
            scope: BindingScope::Preset,
        },
    ],
};

static SPECTRAL_BANDS: BindingTemplate = BindingTemplate {
    name: "Spectral Bands",
    description: "7 frequency bands mapped to first 7 params",
    entries: &[
        TemplateEntry {
            source: "audio.band.0",
            target_pattern: "param.{effect}.{param_0}",
            transforms: || vec![TransformDef::Smooth { factor: 0.8 }],
            scope: BindingScope::Preset,
        },
        TemplateEntry {
            source: "audio.band.1",
            target_pattern: "param.{effect}.{param_1}",
            transforms: || vec![TransformDef::Smooth { factor: 0.8 }],
            scope: BindingScope::Preset,
        },
        TemplateEntry {
            source: "audio.band.2",
            target_pattern: "param.{effect}.{param_2}",
            transforms: || vec![TransformDef::Smooth { factor: 0.8 }],
            scope: BindingScope::Preset,
        },
        TemplateEntry {
            source: "audio.band.3",
            target_pattern: "param.{effect}.{param_3}",
            transforms: || vec![TransformDef::Smooth { factor: 0.8 }],
            scope: BindingScope::Preset,
        },
        TemplateEntry {
            source: "audio.band.4",
            target_pattern: "param.{effect}.{param_4}",
            transforms: || vec![TransformDef::Smooth { factor: 0.8 }],
            scope: BindingScope::Preset,
        },
        TemplateEntry {
            source: "audio.band.5",
            target_pattern: "param.{effect}.{param_5}",
            transforms: || vec![TransformDef::Smooth { factor: 0.8 }],
            scope: BindingScope::Preset,
        },
        TemplateEntry {
            source: "audio.band.6",
            target_pattern: "param.{effect}.{param_6}",
            transforms: || vec![TransformDef::Smooth { factor: 0.8 }],
            scope: BindingScope::Preset,
        },
    ],
};

static MIDI_FADERS: BindingTemplate = BindingTemplate {
    name: "MIDI Faders",
    description: "CC 1-8 mapped to first 8 params",
    entries: &[
        TemplateEntry {
            source: "midi.*.cc.0.1",
            target_pattern: "param.{effect}.{param_0}",
            transforms: || vec![TransformDef::Smooth { factor: 0.9 }],
            scope: BindingScope::Preset,
        },
        TemplateEntry {
            source: "midi.*.cc.0.2",
            target_pattern: "param.{effect}.{param_1}",
            transforms: || vec![TransformDef::Smooth { factor: 0.9 }],
            scope: BindingScope::Preset,
        },
        TemplateEntry {
            source: "midi.*.cc.0.3",
            target_pattern: "param.{effect}.{param_2}",
            transforms: || vec![TransformDef::Smooth { factor: 0.9 }],
            scope: BindingScope::Preset,
        },
        TemplateEntry {
            source: "midi.*.cc.0.4",
            target_pattern: "param.{effect}.{param_3}",
            transforms: || vec![TransformDef::Smooth { factor: 0.9 }],
            scope: BindingScope::Preset,
        },
        TemplateEntry {
            source: "midi.*.cc.0.5",
            target_pattern: "param.{effect}.{param_4}",
            transforms: || vec![TransformDef::Smooth { factor: 0.9 }],
            scope: BindingScope::Preset,
        },
        TemplateEntry {
            source: "midi.*.cc.0.6",
            target_pattern: "param.{effect}.{param_5}",
            transforms: || vec![TransformDef::Smooth { factor: 0.9 }],
            scope: BindingScope::Preset,
        },
        TemplateEntry {
            source: "midi.*.cc.0.7",
            target_pattern: "param.{effect}.{param_6}",
            transforms: || vec![TransformDef::Smooth { factor: 0.9 }],
            scope: BindingScope::Preset,
        },
        TemplateEntry {
            source: "midi.*.cc.0.8",
            target_pattern: "param.{effect}.{param_7}",
            transforms: || vec![TransformDef::Smooth { factor: 0.9 }],
            scope: BindingScope::Preset,
        },
    ],
};

impl BindingBus {
    /// Apply a template, creating bindings with resolved target patterns.
    /// `effect_name` replaces `{effect}` in target patterns.
    /// `param_names` replaces `{param_N}` placeholders.
    pub fn apply_template(
        &mut self,
        template: &BindingTemplate,
        effect_name: &str,
        param_names: &[String],
    ) {
        for entry in template.entries {
            let mut target = entry.target_pattern.replace("{effect}", effect_name);

            // Replace {param_N} placeholders with actual param names
            for (i, name) in param_names.iter().enumerate() {
                target = target.replace(&format!("{{param_{i}}}"), name);
            }

            // Skip if we still have unresolved placeholders
            if target.contains("{param_") {
                continue;
            }

            let id = self.add_binding(entry.source.to_string(), target, entry.scope.clone());

            // Apply transforms
            if let Some(b) = self.get_binding_mut(&id) {
                b.transforms = (entry.transforms)();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn test_bus() -> BindingBus {
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

    #[test]
    fn apply_audio_reactive_template() {
        let mut bus = test_bus();
        let params = vec![
            "warp_intensity".into(),
            "color_shift".into(),
            "rotation".into(),
        ];
        bus.apply_template(&AUDIO_REACTIVE, "Phosphor", &params);
        // Should create 4 bindings (kick, rms, centroid, beat_phase)
        assert_eq!(bus.bindings.len(), 4);
        assert_eq!(bus.bindings[0].source, "audio.kick");
        assert_eq!(bus.bindings[0].target, "param.Phosphor.warp_intensity");
    }

    #[test]
    fn spectral_skips_missing_params() {
        let mut bus = test_bus();
        let params = vec!["warp".into(), "color".into()]; // Only 2 params
        bus.apply_template(&SPECTRAL_BANDS, "Phosphor", &params);
        // Should only create 2 bindings (bands 0 and 1), skip 2-6
        assert_eq!(bus.bindings.len(), 2);
    }

    #[test]
    fn builtin_templates_available() {
        let templates = builtin_templates();
        assert_eq!(templates.len(), 4);
        assert_eq!(templates[0].name, "Audio Reactive");
        assert_eq!(templates[1].name, "Beat Sync");
        assert_eq!(templates[2].name, "Spectral Bands");
        assert_eq!(templates[3].name, "MIDI Faders");
    }
}
