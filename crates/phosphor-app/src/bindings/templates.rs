use super::bus::BindingBus;
use super::types::{BindingScope, BindingTarget, TransformDef};

/// A built-in binding template.
#[derive(Debug)]
pub struct BindingTemplate {
    pub name: &'static str,
    pub description: &'static str,
    pub entries: &'static [TemplateEntry],
}

/// A single entry in a template.
#[derive(Debug)]
pub struct TemplateEntry {
    pub source: &'static str,
    /// Target pattern. Three placeholders are substituted by [`BindingBus::apply_template`]:
    ///
    /// * `{layer}` — the index of the layer the template is applied to.
    /// * `{effect}` — that layer's effect name.
    /// * `{param_N}` — the Nth param of that effect, positionally. Unresolved → entry skipped.
    /// * `{param: a|b|*}` — the first of `a`, `b`, … the effect actually has, else (for `*`)
    ///   the next param no other entry in this template has claimed. Unresolved → skipped.
    pub target_pattern: &'static str,
    pub transforms: fn() -> Vec<TransformDef>,
    pub scope: BindingScope,
}

/// Params the `{param: …|*}` fallback must not grab.
///
/// Sweeping one of these re-seeds or flattens the effect rather than modulating it: a
/// kick-driven `trail_decay` strobes the whole feedback buffer, and the mode/preset-style
/// selectors snap between discrete looks instead of moving. They stay reachable *by name* —
/// `centroid → color_mode` is exactly what drift.pfx's own `audio_mappings` asks for.
/// Ignored entirely when nothing else is free, because a jarring binding still beats a dead one.
const FALLBACK_SKIP: &[&str] = &[
    "trail_decay",
    "trail_length",
    "color_mode",
    "mode",
    "preset",
    "init_pattern",
    "attractor",
    "projection",
    "layout",
    "bass_mode",
    "dipole_mode",
    "num_species",
    "symmetry",
    "feed_rate",
    "kill_rate",
];

/// "The param that reads as impact" — kick and beat.
const T_PUNCH: &str = "param.{layer}.{effect}.{param: warp_intensity|audio_drive\
|audio_reactivity|shard_force|burst_power|burst_force|flash_power|field_strength|beat_pulse\
|intensity|flow_intensity|spring_k|*}";

/// "The param that changes the colour" — centroid.
const T_COLOUR: &str = "param.{layer}.{effect}.{param: color_shift|color_mode|hue|ice_hue\
|water_hue|saturation|brightness|exposure|edge_glow|emitter_glow|glow_width|glow|*}";

/// "The param that turns or advances" — beat phase.
const T_MOTION: &str = "param.{layer}.{effect}.{param: rotation|rotation_speed|field_rotation\
|orbit_speed|swirl|twist_amount|drift_speed|curtain_speed|expansion_speed|turn_speed\
|growth_speed|sim_speed|speed_mult|inward_speed|flow_speed|dt_scale|camera_pitch|ribbon_drift\
|speed|*}";

/// "The param that makes it bigger or denser" — sustained energy.
///
/// Added because every template had only three slots to aim at, so a fourth
/// source had nowhere to go but a positional guess. 30 of the 48 shipped effects
/// name one of these outright.
const T_SCALE: &str = "param.{layer}.{effect}.{param: zoom|spread|density|radius|ring_radius\
|orbit_radius|tunnel_radius|max_radius|thickness|arc_thickness|ring_width|band_spread\
|cell_scale|pattern_scale|splat_scale|splat_radius|height_scale|explode_amount|fill_amount\
|target_density|rib_density|scatter_amount|*}";

/// All available built-in templates.
pub fn builtin_templates() -> &'static [&'static BindingTemplate] {
    static ALL: &[&BindingTemplate] = &[
        &AUDIO_REACTIVE,
        &BEAT_SYNC,
        &SPECTRAL_BANDS,
        &AMBIENT,
        &MIDI_FADERS,
    ];
    ALL
}

/// The entry every audio template ends with, so none of them is a silent no-op.
///
/// The eight Lattice rules declare no params at all, so on a sixth of the shipped
/// effects this is the ONLY entry that resolves — a template that bound nothing
/// there would look like the feature being broken.
///
/// A gentle breathe rather than a fade: the floor is 0.72, not the 0.3 this used
/// to sit at, because bottoming out through a breakdown reads as the app dropping
/// out rather than as the music moving.
macro_rules! opacity_breathe {
    ($source:expr) => {
        TemplateEntry {
            source: $source,
            target_pattern: "layer.{layer}.opacity",
            transforms: || {
                vec![
                    TransformDef::Remap {
                        in_lo: 0.0,
                        in_hi: 0.7,
                        out_lo: 0.72,
                        out_hi: 1.0,
                    },
                    TransformDef::Smooth { factor: 0.85 },
                ]
            },
            scope: BindingScope::Preset,
        }
    };
}

static AUDIO_REACTIVE: BindingTemplate = BindingTemplate {
    name: "Audio Reactive",
    description: "Hits, brightness, beat and energy on four axes \u{2014} the general-purpose one",
    entries: &[
        // percussive_energy, NOT kick: the kick band false-fires 78-84% of the time
        // on a sustained bass note (#1836), so a kick-driven template pulses all the
        // way through a pad and reads as broken. HPSS reads ~0 on the same drone.
        TemplateEntry {
            source: "audio.percussive_energy",
            target_pattern: T_PUNCH,
            transforms: || {
                vec![
                    // Percussive energy sits low even on busy material; open it up
                    // or the punch never reaches the top of the param's range.
                    TransformDef::Remap {
                        in_lo: 0.05,
                        in_hi: 0.6,
                        out_lo: 0.0,
                        out_hi: 1.0,
                    },
                    TransformDef::Smooth { factor: 0.55 },
                ]
            },
            scope: BindingScope::Preset,
        },
        TemplateEntry {
            source: "audio.centroid",
            target_pattern: T_COLOUR,
            // Colour should drift with the mix, not flicker per frame.
            transforms: || vec![TransformDef::Smooth { factor: 0.9 }],
            scope: BindingScope::Preset,
        },
        TemplateEntry {
            source: "audio.beat_phase",
            target_pattern: T_MOTION,
            // Deliberately unsmoothed: it is a sawtooth, and smoothing rounds off
            // the reset that makes the motion land on the beat.
            transforms: || vec![],
            scope: BindingScope::Preset,
        },
        // Energy drives SIZE, which is the visible one.
        TemplateEntry {
            source: "audio.rms",
            target_pattern: T_SCALE,
            transforms: || {
                vec![
                    TransformDef::Remap {
                        in_lo: 0.0,
                        in_hi: 0.7,
                        out_lo: 0.25,
                        out_hi: 1.0,
                    },
                    TransformDef::Smooth { factor: 0.85 },
                ]
            },
            scope: BindingScope::Preset,
        },
        opacity_breathe!("audio.rms"),
    ],
};

static BEAT_SYNC: BindingTemplate = BindingTemplate {
    name: "Beat Sync",
    description: "Locked to the grid \u{2014} hits on the beat, sweeps across the bar",
    entries: &[
        TemplateEntry {
            source: "audio.beat",
            target_pattern: T_PUNCH,
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
            target_pattern: T_MOTION,
            transforms: || vec![],
            scope: BindingScope::Preset,
        },
        // The bar clock is what makes this read as musical rather than merely
        // periodic: a four-beat sweep the eye can predict.
        TemplateEntry {
            source: "audio.bar_phase",
            target_pattern: T_SCALE,
            transforms: || vec![],
            scope: BindingScope::Preset,
        },
        // Colour turns over on the "1", so the phrase has a visible downbeat.
        TemplateEntry {
            source: "audio.downbeat",
            target_pattern: T_COLOUR,
            transforms: || {
                vec![
                    TransformDef::Gate { threshold: 0.5 },
                    TransformDef::Smooth { factor: 0.9 },
                ]
            },
            scope: BindingScope::Preset,
        },
        // Beat strength rather than rms here, so this one still pulses on the grid
        // when it is the only entry that resolves.
        opacity_breathe!("audio.beat_strength"),
    ],
};

static SPECTRAL_BANDS: BindingTemplate = BindingTemplate {
    name: "Spectral Bands",
    description: "Sub, bass, mid and air each drive a different axis",
    // Was seven bands onto the first seven params BY POSITION, which is close to
    // random: param 0 is `trail_decay` on 11 of the 40 effects that have params,
    // and sweeping that strobes the whole feedback buffer. 13% of the bindings it
    // produced landed on params the semantic slots deliberately refuse, and ten
    // effects had entries silently dropped for having fewer than seven params.
    entries: &[
        TemplateEntry {
            source: "audio.band.1",
            target_pattern: T_PUNCH,
            transforms: || vec![TransformDef::Smooth { factor: 0.6 }],
            scope: BindingScope::Preset,
        },
        TemplateEntry {
            source: "audio.band.6",
            target_pattern: T_COLOUR,
            transforms: || vec![TransformDef::Smooth { factor: 0.85 }],
            scope: BindingScope::Preset,
        },
        TemplateEntry {
            source: "audio.band.3",
            target_pattern: T_MOTION,
            transforms: || vec![TransformDef::Smooth { factor: 0.75 }],
            scope: BindingScope::Preset,
        },
        TemplateEntry {
            source: "audio.band.0",
            target_pattern: T_SCALE,
            transforms: || {
                vec![
                    TransformDef::Remap {
                        in_lo: 0.0,
                        in_hi: 0.8,
                        out_lo: 0.2,
                        out_hi: 1.0,
                    },
                    TransformDef::Smooth { factor: 0.8 },
                ]
            },
            scope: BindingScope::Preset,
        },
        opacity_breathe!("audio.rms"),
    ],
};

static AMBIENT: BindingTemplate = BindingTemplate {
    name: "Ambient",
    description: "For pads and drones \u{2014} texture and stereo, no beat needed",
    // The other three templates all rest on transients, so on ambient material they
    // sit almost still and the app looks dead. Every source here responds to
    // sustained sound: harmonic_ratio is level-invariant and neutral at 0.5 on
    // silence, so it moves with the material rather than with the fader.
    entries: &[
        TemplateEntry {
            source: "audio.harmonic_energy",
            target_pattern: T_PUNCH,
            transforms: || {
                vec![
                    TransformDef::Remap {
                        in_lo: 0.0,
                        in_hi: 0.7,
                        out_lo: 0.0,
                        out_hi: 0.8,
                    },
                    TransformDef::Smooth { factor: 0.9 },
                ]
            },
            scope: BindingScope::Preset,
        },
        TemplateEntry {
            source: "audio.harmonic_ratio",
            target_pattern: T_COLOUR,
            transforms: || vec![TransformDef::Smooth { factor: 0.92 }],
            scope: BindingScope::Preset,
        },
        TemplateEntry {
            source: "audio.stereo_width",
            target_pattern: T_MOTION,
            transforms: || vec![TransformDef::Smooth { factor: 0.9 }],
            scope: BindingScope::Preset,
        },
        TemplateEntry {
            source: "audio.rms",
            target_pattern: T_SCALE,
            transforms: || {
                vec![
                    TransformDef::Remap {
                        in_lo: 0.0,
                        in_hi: 0.5,
                        out_lo: 0.3,
                        out_hi: 1.0,
                    },
                    TransformDef::Smooth { factor: 0.93 },
                ]
            },
            scope: BindingScope::Preset,
        },
        opacity_breathe!("audio.harmonic_energy"),
    ],
};

static MIDI_FADERS: BindingTemplate = BindingTemplate {
    name: "MIDI Faders",
    description: "CC 1-8 onto the first eight params, in order",
    // Positional here is the RIGHT call, unlike the audio templates: the user is
    // driving, so landing on a mode selector is immediately visible and immediately
    // undone. Smoothing is light for the same reason — a fader that lags reads as
    // broken hardware.
    entries: &[
        TemplateEntry {
            source: "midi.*.cc.0.1",
            target_pattern: "param.{layer}.{effect}.{param_0}",
            transforms: || vec![TransformDef::Smooth { factor: 0.45 }],
            scope: BindingScope::Preset,
        },
        TemplateEntry {
            source: "midi.*.cc.0.2",
            target_pattern: "param.{layer}.{effect}.{param_1}",
            transforms: || vec![TransformDef::Smooth { factor: 0.45 }],
            scope: BindingScope::Preset,
        },
        TemplateEntry {
            source: "midi.*.cc.0.3",
            target_pattern: "param.{layer}.{effect}.{param_2}",
            transforms: || vec![TransformDef::Smooth { factor: 0.45 }],
            scope: BindingScope::Preset,
        },
        TemplateEntry {
            source: "midi.*.cc.0.4",
            target_pattern: "param.{layer}.{effect}.{param_3}",
            transforms: || vec![TransformDef::Smooth { factor: 0.45 }],
            scope: BindingScope::Preset,
        },
        TemplateEntry {
            source: "midi.*.cc.0.5",
            target_pattern: "param.{layer}.{effect}.{param_4}",
            transforms: || vec![TransformDef::Smooth { factor: 0.45 }],
            scope: BindingScope::Preset,
        },
        TemplateEntry {
            source: "midi.*.cc.0.6",
            target_pattern: "param.{layer}.{effect}.{param_5}",
            transforms: || vec![TransformDef::Smooth { factor: 0.45 }],
            scope: BindingScope::Preset,
        },
        TemplateEntry {
            source: "midi.*.cc.0.7",
            target_pattern: "param.{layer}.{effect}.{param_6}",
            transforms: || vec![TransformDef::Smooth { factor: 0.45 }],
            scope: BindingScope::Preset,
        },
        TemplateEntry {
            source: "midi.*.cc.0.8",
            target_pattern: "param.{layer}.{effect}.{param_7}",
            transforms: || vec![TransformDef::Smooth { factor: 0.45 }],
            scope: BindingScope::Preset,
        },
    ],
};

/// Byte span of a `{param: …}` placeholder, if the pattern has one.
fn param_span(pattern: &str) -> Option<(usize, usize)> {
    let open = pattern.find("{param:")?;
    let close = pattern[open..].find('}')? + open;
    Some((open, close))
}

/// Resolve every `{param: a|b|*}` in a template against one effect's params, returning
/// the chosen name per entry (`None` = drop that entry).
///
/// Two passes, and the order matters: a single left-to-right pass lets an earlier
/// entry's `*` grab a param a later entry names outright. On Tunnel that put centroid
/// on `speed` and left beat_phase — which names `speed` — with `tunnel_radius`.
///
/// Nothing is ever claimed twice, which is what stops three sources piling onto a
/// one-param effect's single slider, where they would fight and last write would win.
fn resolve_template_params(
    template: &BindingTemplate,
    param_names: &[String],
) -> Vec<Option<String>> {
    let bodies: Vec<Option<&str>> = template
        .entries
        .iter()
        .map(|e| param_span(e.target_pattern).map(|(o, c)| &e.target_pattern[o + 7..c]))
        .collect();
    let mut out: Vec<Option<String>> = vec![None; template.entries.len()];
    let mut claimed: Vec<String> = Vec::new();

    // Pass 1 — named candidates only, in each entry's own preference order.
    for (i, body) in bodies.iter().enumerate() {
        let Some(body) = body else { continue };
        out[i] = body
            .split('|')
            .map(str::trim)
            .filter(|c| *c != "*")
            .find(|c| param_names.iter().any(|n| n == c) && !claimed.iter().any(|n| n == c))
            .map(|c| {
                claimed.push(c.to_string());
                c.to_string()
            });
    }

    // Pass 2 — wildcards take whatever is left, preferring a param that is safe to
    // sweep. A denied param is taken only when the template would otherwise bind
    // NOTHING at all: a jarring binding beats a dead one, but only as a last
    // resort. Without that condition the templates scrape a small effect down to
    // its leftovers — Beam declares [mode, trail_decay, beam_focus, intensity], and
    // with four slots competing the tail ones landed beat_phase on `mode` and rms
    // on `trail_decay`, which snaps between looks and strobes the feedback buffer
    // respectively, on top of two slots that had already resolved cleanly.
    for (i, body) in bodies.iter().enumerate() {
        let Some(body) = body else { continue };
        if out[i].is_some() || !body.split('|').any(|c| c.trim() == "*") {
            continue;
        }
        let free = || param_names.iter().filter(|n| !claimed.contains(n));
        let last_resort = claimed.is_empty();
        let pick = free()
            .find(|n| !FALLBACK_SKIP.contains(&n.as_str()))
            .or_else(|| if last_resort { free().next() } else { None })
            .cloned();
        if let Some(name) = pick {
            claimed.push(name.clone());
            out[i] = Some(name);
        }
    }
    out
}

impl BindingBus {
    /// Apply a template to `layer_idx`, creating bindings with resolved target patterns.
    ///
    /// Emits the 4-part `param.{layer}.{effect}.{name}` target — the form
    /// `build_target_options` produces and `apply_binding_target` resolves without
    /// consulting the active layer. The old 3-part `param.{effect}.{name}` worked only
    /// while the layer it was applied to happened to still be selected; pick another
    /// layer and every template binding went quietly dead. Reading 3-part is unchanged,
    /// so saved bindings keep working.
    pub fn apply_template(
        &mut self,
        template: &BindingTemplate,
        layer_idx: usize,
        effect_name: &str,
        param_names: &[String],
    ) {
        // Media and webcam layers report an empty effect name; substituting it would
        // emit the unresolvable target `param.0..warp_intensity`.
        if effect_name.is_empty() {
            return;
        }

        // Substitute a live MIDI device for `*` source placeholders: evaluation
        // is exact-match on snapshot keys, so a literal `midi.*.cc.…` binding
        // can never fire. If no MIDI source has been seen yet, the `*` stays —
        // the card then shows the no-signal chip until the user re-Learns it.
        let midi_device: Option<String> = self
            .last_snapshot
            .keys()
            .find(|k| k.starts_with("midi."))
            .and_then(|k| k.split('.').nth(1))
            .map(str::to_string);

        // `{param: a|b|*}` — a preference list, not a hardcoded name. The old patterns
        // named warp_intensity / color_shift / rotation, which each exist on exactly one
        // of the 40 shipped effects, so "Audio Reactive" produced three dead bindings on
        // everything but Drift, Flux and Cymatics. Resolved for the whole template at
        // once so entries cannot claim the same param.
        let chosen = resolve_template_params(template, param_names);

        for (entry, choice) in template.entries.iter().zip(&chosen) {
            let mut target = entry
                .target_pattern
                .replace("{effect}", effect_name)
                .replace("{layer}", &layer_idx.to_string());

            if let (Some((open, close)), Some(name)) = (param_span(&target), choice) {
                target.replace_range(open..=close, name);
            }

            // Replace {param_N} placeholders with actual param names
            for (i, name) in param_names.iter().enumerate() {
                target = target.replace(&format!("{{param_{i}}}"), name);
            }

            // Skip if we still have unresolved placeholders. Broadened from "{param_":
            // `{param:` has no underscore, so the narrow check let an unresolved
            // candidate list through and emitted it as a literal target.
            if target.contains("{param") {
                continue;
            }

            // Patterns are authored as strings for readability, so parse once
            // here; an unresolvable one becomes Unknown rather than reaching the
            // bus as a plausible-looking dotted string that drives nothing.
            let target: BindingTarget = target.parse().unwrap_or_default();

            let mut source = entry.source.to_string();
            if let (true, Some(dev)) = (source.contains('*'), midi_device.as_deref()) {
                source = source.replace('*', dev);
            }

            // Replace any preset-scope binding already driving this exact target.
            //
            // Templates used to append unconditionally, so trying a second one left
            // both sets live and two sources fighting over the same param, last
            // write per frame winning — which looks like the app ignoring the
            // template rather than like a conflict. Scoped to this target and to
            // preset scope, so hand-made and global bindings elsewhere survive.
            let replaced: Vec<String> = self
                .bindings
                .iter()
                .filter(|b| b.target == target && b.scope == BindingScope::Preset)
                .map(|b| b.id.clone())
                .collect();
            for id in replaced {
                self.remove_binding(&id);
            }

            let id = self.add_binding(source, target, entry.scope.clone());

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
            preset_scope_dirty: false,
            learn_target: None,
            last_snapshot: HashMap::new(),
            pending_triggers: Vec::new(),
        }
    }

    /// Param names of the given effect, straight from the shipped `.pfx` files.
    /// CARGO_MANIFEST_DIR, not assets_dir(): the latter resolves CWD-relative, and
    /// `cargo test` runs with CWD = crates/phosphor-app, which has no assets/.
    fn shipped_effects() -> Vec<(String, Vec<String>)> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/effects");
        let mut out: Vec<(String, Vec<String>)> = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "pfx"))
            .filter_map(|p| std::fs::read_to_string(p).ok())
            .filter_map(|j| serde_json::from_str::<crate::effect::format::PfxEffect>(&j).ok())
            .map(|e| {
                let params = e
                    .inputs
                    .iter()
                    .filter(|d| {
                        matches!(
                            d,
                            crate::params::ParamDef::Float { .. }
                                | crate::params::ParamDef::Bool { .. }
                        )
                    })
                    .map(|d| d.name().to_string())
                    .collect();
                (e.name, params)
            })
            .collect();
        out.sort();
        assert!(!out.is_empty(), "no .pfx files found in {}", dir.display());
        out
    }

    #[test]
    fn audio_reactive_prefers_named_params() {
        let mut bus = test_bus();
        // Drift's real inputs.
        let params: Vec<String> = ["warp_intensity", "flow_speed", "color_mode", "density"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        bus.apply_template(&AUDIO_REACTIVE, 0, "Drift", &params);
        assert_eq!(bus.bindings.len(), 5);
        // percussive_energy, not kick — the kick band false-fires on sustained bass (#1836).
        assert_eq!(bus.bindings[0].source, "audio.percussive_energy");
        assert_eq!(
            bus.bindings[0].target.to_string(),
            "param.0.Drift.warp_intensity"
        );
        // color_mode is in FALLBACK_SKIP but named in the colour list, so it still wins —
        // drift.pfx's own audio_mappings ask for exactly centroid -> palette colour.
        assert_eq!(
            bus.bindings[1].target.to_string(),
            "param.0.Drift.color_mode"
        );
        assert_eq!(
            bus.bindings[2].target.to_string(),
            "param.0.Drift.flow_speed"
        );
        // The SCALE slot takes density outright.
        assert_eq!(bus.bindings[3].target.to_string(), "param.0.Drift.density");
        assert_eq!(bus.bindings[4].target.to_string(), "layer.0.opacity");
    }

    #[test]
    fn audio_reactive_falls_back_when_no_name_matches() {
        let mut bus = test_bus();
        // Strata shares no vocabulary with any candidate list.
        let params: Vec<String> = [
            "height_scale",
            "draw_distance",
            "camera_pitch",
            "rock_detail",
            "snow_line",
            "zoom",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        bus.apply_template(&AUDIO_REACTIVE, 0, "Strata", &params);
        assert_eq!(bus.bindings.len(), 5);
        let hit: Vec<&str> = bus
            .bindings
            .iter()
            .filter_map(|b| b.target.param())
            .collect();
        assert_eq!(hit.len(), 4, "every param entry should resolve: {hit:?}");
        let mut sorted = hit.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 4, "a param was bound twice: {hit:?}");
    }

    #[test]
    fn audio_reactive_never_double_binds_one_param() {
        // Phosphor declares a single param. Without the `claimed` set, kick, centroid
        // and beat_phase would all land on it and fight, last write winning.
        let mut bus = test_bus();
        let params = vec!["trail_decay".to_string()];
        bus.apply_template(&AUDIO_REACTIVE, 0, "Phosphor", &params);
        assert_eq!(bus.bindings.len(), 2); // one param + layer opacity
        assert_eq!(
            bus.bindings[0].target.to_string(),
            "param.0.Phosphor.trail_decay"
        );
        assert_eq!(bus.bindings[1].target.to_string(), "layer.0.opacity");
    }

    #[test]
    fn audio_reactive_on_a_paramless_effect_still_binds_opacity() {
        // The 8 Lattice rules and the hidden stress effect declare no inputs at all.
        let mut bus = test_bus();
        bus.apply_template(&AUDIO_REACTIVE, 2, "Lattice Clouds", &[]);
        assert_eq!(bus.bindings.len(), 1);
        assert_eq!(bus.bindings[0].target.to_string(), "layer.2.opacity");
    }

    #[test]
    fn fallback_avoids_simulation_resetting_params() {
        // Sweeping trail_decay strobes the whole feedback buffer rather than modulating,
        // so the kick wildcard must reach past it.
        let mut bus = test_bus();
        let params = vec!["trail_decay".to_string(), "curl_scale".to_string()];
        bus.apply_template(&AUDIO_REACTIVE, 0, "X", &params);
        assert_eq!(bus.bindings[0].source, "audio.percussive_energy");
        assert_eq!(bus.bindings[0].target.to_string(), "param.0.X.curl_scale");
    }

    #[test]
    fn fallback_uses_a_denied_param_rather_than_nothing() {
        let mut bus = test_bus();
        let params = vec!["trail_decay".to_string()];
        bus.apply_template(&AUDIO_REACTIVE, 0, "X", &params);
        assert_eq!(bus.bindings[0].target.to_string(), "param.0.X.trail_decay");
    }

    #[test]
    fn a_wildcard_does_not_steal_a_param_a_later_entry_names() {
        // Tunnel names two motion params and nothing the colour list knows. A single
        // left-to-right pass let centroid's wildcard take `speed` first and left
        // beat_phase — the entry that actually names it — with `tunnel_radius`.
        let mut bus = test_bus();
        let params: Vec<String> = [
            "twist_amount",
            "speed",
            "tunnel_radius",
            "rib_density",
            "pinch_h",
            "pinch_v",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        bus.apply_template(&AUDIO_REACTIVE, 0, "Tunnel", &params);
        let phase = bus
            .bindings
            .iter()
            .find(|b| b.source == "audio.beat_phase")
            .expect("beat_phase entry should resolve");
        assert_eq!(phase.target.to_string(), "param.0.Tunnel.twist_amount");
        // …and the wildcards then take what the named passes left.
        let centroid = bus
            .bindings
            .iter()
            .find(|b| b.source == "audio.centroid")
            .expect("centroid entry should resolve");
        assert_ne!(centroid.target.to_string(), "param.0.Tunnel.twist_amount");
    }

    #[test]
    fn unresolved_candidate_list_is_skipped_not_emitted_literally() {
        // The trap: the skip guard tested for "{param_", and `{param:` has no
        // underscore, so an unresolved list would sail through as a literal target.
        let mut bus = test_bus();
        bus.apply_template(&AUDIO_REACTIVE, 0, "Nothing", &[]);
        assert!(
            !bus.bindings
                .iter()
                .any(|b| matches!(b.target, BindingTarget::Unknown(_))),
            "emitted an unresolved placeholder: {:?}",
            bus.bindings.iter().map(|b| &b.target).collect::<Vec<_>>()
        );
    }

    #[test]
    fn audio_reactive_moves_something_on_every_shipped_effect() {
        // The bug this replaces: the template's targets were only ever verified
        // against a hand-fed param list that no shipped effect actually has.
        for (name, params) in shipped_effects() {
            if params.is_empty() {
                continue; // 8 Lattice rules + the hidden stress effect
            }
            let mut bus = test_bus();
            bus.apply_template(&AUDIO_REACTIVE, 0, &name, &params);
            let hit: Vec<&str> = bus
                .bindings
                .iter()
                .filter_map(|b| b.target.param())
                .collect();
            assert!(!hit.is_empty(), "'{name}' gets no param binding at all");
            for p in &hit {
                assert!(
                    params.iter().any(|have| have == p),
                    "'{name}' bound to '{p}', which it does not have"
                );
            }
            let mut sorted = hit.clone();
            sorted.sort_unstable();
            sorted.dedup();
            assert_eq!(sorted.len(), hit.len(), "'{name}' double-bound a param");
        }
    }

    #[test]
    fn template_targets_the_layer_it_was_applied_to() {
        // Pre-fix this emitted param.{effect}.{name}, which apply_binding_target only
        // honours while that effect is on the *active* layer.
        let mut bus = test_bus();
        let params: Vec<String> = ["warp_intensity", "flow_speed", "color_mode"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        bus.apply_template(&AUDIO_REACTIVE, 3, "Drift", &params);
        for b in &bus.bindings {
            if matches!(b.target, BindingTarget::Param { .. }) {
                assert_eq!(
                    b.target.layer(),
                    Some(3),
                    "{} landed on the wrong layer",
                    b.target
                );
            } else {
                assert_eq!(b.target.to_string(), "layer.3.opacity");
            }
        }
    }

    #[test]
    fn template_is_a_noop_on_a_layer_with_no_effect() {
        // Media and webcam layers report an empty effect name.
        let mut bus = test_bus();
        bus.apply_template(&AUDIO_REACTIVE, 0, "", &[]);
        assert!(bus.bindings.is_empty());
    }

    #[test]
    fn spectral_skips_missing_params() {
        // Two params and four semantic slots: the two that resolve get one each,
        // the rest drop, and the opacity entry always lands. Previously this
        // template was seven bands onto param_0..param_6 by POSITION, so the same
        // effect dropped five entries and the two it kept were whatever happened
        // to be declared first.
        let mut bus = test_bus();
        let params = vec!["warp".into(), "color".into()];
        bus.apply_template(&SPECTRAL_BANDS, 0, "Phosphor", &params);
        assert_eq!(bus.bindings.len(), 3);

        let mut bound: Vec<&str> = bus
            .bindings
            .iter()
            .filter_map(|b| b.target.param())
            .collect();
        bound.sort_unstable();
        assert_eq!(bound, vec!["color", "warp"], "each param taken once");
        assert!(
            bus.bindings
                .iter()
                .any(|b| b.target == BindingTarget::from("layer.0.opacity"))
        );
    }

    #[test]
    fn midi_faders_substitutes_live_device() {
        use crate::bindings::types::SourceRaw;
        let mut bus = test_bus();
        bus.last_snapshot.insert(
            "midi.MPD218.cc.0.1".to_string(),
            (
                0.5,
                SourceRaw {
                    display: "0.5".into(),
                    numeric: 0.5,
                },
            ),
        );
        let params: Vec<String> = (0..8).map(|i| format!("p{i}")).collect();
        bus.apply_template(&MIDI_FADERS, 0, "Phosphor", &params);
        assert_eq!(bus.bindings.len(), 8);
        for (i, b) in bus.bindings.iter().enumerate() {
            assert_eq!(b.source, format!("midi.MPD218.cc.0.{}", i + 1));
        }
    }

    #[test]
    fn midi_faders_keeps_wildcard_without_live_device() {
        let mut bus = test_bus();
        let params: Vec<String> = (0..8).map(|i| format!("p{i}")).collect();
        bus.apply_template(&MIDI_FADERS, 0, "Phosphor", &params);
        // No live MIDI source: the placeholder survives so the UI can flag it.
        assert_eq!(bus.bindings[0].source, "midi.*.cc.0.1");
    }

    #[test]
    fn builtin_templates_available() {
        let templates = builtin_templates();
        assert_eq!(templates.len(), 5);
        assert_eq!(templates[0].name, "Audio Reactive");
        assert_eq!(templates[1].name, "Beat Sync");
        assert_eq!(templates[2].name, "Spectral Bands");
        assert_eq!(templates[3].name, "Ambient");
        assert_eq!(templates[4].name, "MIDI Faders");
    }

    /// Every audio template must move something on every shipped effect.
    ///
    /// A template that resolves nothing is indistinguishable from a broken
    /// feature, and a sixth of the shipped effects (the Lattice rules) declare no
    /// params at all — which is exactly where a param-only template goes silent.
    #[test]
    fn every_audio_template_binds_something_on_every_effect() {
        for template in builtin_templates() {
            if template.name == "MIDI Faders" {
                continue; // hardware-only, and positional by design
            }
            for (effect, params) in shipped_effects() {
                let mut bus = test_bus();
                bus.apply_template(template, 0, &effect, &params);
                assert!(
                    !bus.bindings.is_empty(),
                    "template '{}' bound nothing on '{effect}' ({} params)",
                    template.name,
                    params.len()
                );
            }
        }
    }

    /// No audio template may drive a param through a source the audio side refuses.
    ///
    /// The old Spectral Bands mapped seven bands onto the first seven params BY
    /// POSITION, and `param_0` is `trail_decay` on 11 of the 40 effects that have
    /// params — so sub-bass strobed the whole feedback buffer. 13% of the bindings
    /// it produced landed on a param the semantic slots deliberately refuse.
    #[test]
    fn no_audio_template_sweeps_a_simulation_resetting_param() {
        for template in builtin_templates() {
            if template.name == "MIDI Faders" {
                continue; // user-driven, and immediately undone if it lands badly
            }
            for (effect, params) in shipped_effects() {
                // An effect whose ONLY params are denied has to use one anyway —
                // a jarring binding still beats a dead one.
                if params.iter().all(|p| FALLBACK_SKIP.contains(&p.as_str())) {
                    continue;
                }
                let mut bus = test_bus();
                bus.apply_template(template, 0, &effect, &params);
                for b in &bus.bindings {
                    let Some(param) = b.target.param() else {
                        continue;
                    };
                    // Named candidates may claim a denied param on purpose; only the
                    // `*` fallback is forbidden from reaching one. color_mode is the
                    // documented case — drift.pfx asks for centroid -> palette.
                    if param == "color_mode" {
                        continue;
                    }
                    assert!(
                        !FALLBACK_SKIP.contains(&param),
                        "template '{}' put {} on '{effect}.{param}', which resets \
                         rather than modulates",
                        template.name,
                        b.source
                    );
                }
            }
        }
    }

    /// Applying a second template must replace the first on any shared target,
    /// not stack a second source onto it where the two fight and the last write
    /// each frame wins.
    #[test]
    fn a_second_template_replaces_rather_than_stacks() {
        let params: Vec<String> = ["warp_intensity", "flow_speed", "hue", "density"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let mut bus = test_bus();
        bus.apply_template(&AUDIO_REACTIVE, 0, "Drift", &params);
        let after_first = bus.bindings.len();
        bus.apply_template(&BEAT_SYNC, 0, "Drift", &params);

        let mut targets: Vec<&BindingTarget> = bus.bindings.iter().map(|b| &b.target).collect();
        targets.sort_unstable();
        let before_dedup = targets.len();
        targets.dedup();
        assert_eq!(
            before_dedup,
            targets.len(),
            "two bindings share a target after applying a second template"
        );
        assert!(
            bus.bindings.len() <= after_first + 1,
            "second template stacked instead of replacing: {} -> {}",
            after_first,
            bus.bindings.len()
        );
        // ...and the second template is the one now driving.
        let punch = bus
            .bindings
            .iter()
            .find(|b| b.target == BindingTarget::from("param.0.Drift.warp_intensity"))
            .expect("punch slot resolves on Drift");
        assert_eq!(punch.source, "audio.beat");
    }

    /// A hand-made binding on an unrelated target survives a template.
    #[test]
    fn applying_a_template_leaves_unrelated_bindings_alone() {
        let params: Vec<String> = ["warp_intensity", "flow_speed", "hue", "density"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let mut bus = test_bus();
        let mine = bus.add_binding(
            "midi.MPD218.cc.0.42".to_string(),
            "postfx.vignette".into(),
            BindingScope::Preset,
        );
        bus.apply_template(&AUDIO_REACTIVE, 0, "Drift", &params);
        assert!(
            bus.bindings.iter().any(|b| b.id == mine),
            "a template removed a hand-made binding on an unrelated target"
        );
    }

    /// Every source a template names must be one the bus actually collects.
    ///
    /// A mistyped id binds to nothing and reads a permanent 0.0 — the failure
    /// mode that left seven band-pan sources dead for several releases (#1801).
    #[test]
    fn every_template_source_is_really_collected() {
        use crate::bindings::sources;
        let mut collected: std::collections::HashSet<String> =
            sources::collect_audio(&Default::default())
                .into_keys()
                .collect();
        collected.extend(sources::collect_mel_bands(&[0.0; 64]).into_keys());
        collected.extend(sources::collect_dmfcc_bands(&[0.0; 13]).into_keys());

        for template in builtin_templates() {
            for entry in template.entries {
                if !entry.source.starts_with("audio.") {
                    continue; // midi.*/osc./ws. are discovered at runtime
                }
                assert!(
                    collected.contains(entry.source),
                    "template '{}' names '{}', which no collector publishes",
                    template.name,
                    entry.source
                );
            }
        }
    }

    /// Not an assertion — prints what each template actually produces on every
    /// shipped effect, so the mapping can be reviewed rather than assumed.
    /// Run: cargo test -p phosphor-app -- --ignored template_coverage_report --nocapture
    #[test]
    #[ignore = "reporting probe, not an assertion"]
    fn template_coverage_report() {
        for template in builtin_templates() {
            if template.name == "MIDI Faders" {
                continue;
            }
            let mut denied = 0usize;
            let mut total = 0usize;
            let mut only_opacity = Vec::new();
            for (effect, params) in shipped_effects() {
                let mut bus = test_bus();
                bus.apply_template(template, 0, &effect, &params);
                let param_hits: Vec<&str> = bus
                    .bindings
                    .iter()
                    .filter_map(|b| b.target.param())
                    .collect();
                total += param_hits.len();
                denied += param_hits
                    .iter()
                    .filter(|p| FALLBACK_SKIP.contains(p) && **p != "color_mode")
                    .count();
                if param_hits.is_empty() {
                    only_opacity.push(effect.clone());
                }
            }
            println!(
                "{:<16} {total:3} param bindings, {denied} on a denied param, \
                 {} effects opacity-only",
                template.name,
                only_opacity.len()
            );
        }
    }
}
