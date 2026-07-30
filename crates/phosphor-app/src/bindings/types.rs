use serde::{Deserialize, Serialize};

pub type BindingId = String;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum BindingScope {
    Preset,
    Global,
}

/// Which field of a layer a `layer.{n}.{field}` target drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LayerField {
    Opacity,
    Blend,
    Displace,
    Enabled,
}

impl LayerField {
    /// Every variant, so callers that need to enumerate the set (the target
    /// catalog, the schema dump) cannot silently miss one: adding a variant
    /// changes the array length and breaks the build here.
    ///
    /// Gated with the catalog that reads it — see `bindings/catalog.rs`.
    #[cfg(any(feature = "analyze", test))]
    pub const ALL: [LayerField; 4] = [
        LayerField::Opacity,
        LayerField::Blend,
        LayerField::Displace,
        LayerField::Enabled,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            LayerField::Opacity => "opacity",
            LayerField::Blend => "blend",
            LayerField::Displace => "displace",
            LayerField::Enabled => "enabled",
        }
    }

    pub(crate) fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "opacity" => LayerField::Opacity,
            "blend" => LayerField::Blend,
            "displace" => LayerField::Displace,
            "enabled" => LayerField::Enabled,
            _ => return None,
        })
    }
}

/// Where a binding sends its value.
///
/// This was a free-form dotted `String`, re-parsed at every use — and that is
/// what let three bugs through in a row: reordering layers, deleting a layer, and
/// changing a layer's effect all left targets pointing at the wrong thing,
/// because nothing in the type said "there is a layer index in here". The layer
/// is now a real field, so [`BindingTarget::layer`] is exhaustive and any new
/// layer-bearing variant is a compile error until the remap handles it.
///
/// Serialized as the same dotted string it always was (see `Display`/`FromStr`),
/// so binding files written by earlier versions keep loading untouched.
/// `Ord` is by the serialized form, so any sort stays stable and human-sensible.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub enum BindingTarget {
    /// Nothing chosen yet — a freshly created binding.
    #[default]
    Unset,
    Param {
        layer: usize,
        effect: String,
        param: String,
    },
    /// The pre-#1792 indexless form, kept so older files load. Resolved against
    /// whichever layer runs that effect, and upgraded to `Param` on preset load.
    LegacyParam {
        effect: String,
        param: String,
    },
    Layer {
        layer: usize,
        field: LayerField,
    },
    PostFx(String),
    Particle(String),
    Uniform(String),
    SceneTransport(String),
    GlobalMasterOpacity,
    /// Anything that does not parse, kept verbatim.
    ///
    /// A hand-edited file or one written by a newer version round-trips instead
    /// of silently losing the binding — dropping a VJ's mapping because we did
    /// not recognise it would be worse than carrying it inert.
    Unknown(String),
}

impl BindingTarget {
    /// The layer this target drives, if it names one.
    ///
    /// Exhaustive by construction: add a layer-bearing variant and this stops
    /// compiling until it is handled here and in [`BindingTarget::with_layer`].
    pub fn layer(&self) -> Option<usize> {
        match self {
            BindingTarget::Param { layer, .. } | BindingTarget::Layer { layer, .. } => Some(*layer),
            BindingTarget::Unset
            | BindingTarget::LegacyParam { .. }
            | BindingTarget::PostFx(_)
            | BindingTarget::Particle(_)
            | BindingTarget::Uniform(_)
            | BindingTarget::SceneTransport(_)
            | BindingTarget::GlobalMasterOpacity
            | BindingTarget::Unknown(_) => None,
        }
    }

    /// The same target pointed at a different layer. Unchanged when it names none.
    pub fn with_layer(&self, new_layer: usize) -> Self {
        match self {
            BindingTarget::Param { effect, param, .. } => BindingTarget::Param {
                layer: new_layer,
                effect: effect.clone(),
                param: param.clone(),
            },
            BindingTarget::Layer { field, .. } => BindingTarget::Layer {
                layer: new_layer,
                field: *field,
            },
            // Spelled out rather than a catch-all: a future layer-bearing variant
            // must fail to compile here too, not silently return itself unmoved.
            // `layer()` catching it while this quietly no-opped would be a
            // half-fix, and the half that reports would look correct.
            BindingTarget::Unset
            | BindingTarget::LegacyParam { .. }
            | BindingTarget::PostFx(_)
            | BindingTarget::Particle(_)
            | BindingTarget::Uniform(_)
            | BindingTarget::SceneTransport(_)
            | BindingTarget::GlobalMasterOpacity
            | BindingTarget::Unknown(_) => self.clone(),
        }
    }

    /// The parameter this target drives, in either param form.
    ///
    /// Only the tests consume it today — they assert on which param a template
    /// picked, which is the question this answers exactly.
    #[allow(dead_code)]
    pub fn param(&self) -> Option<&str> {
        match self {
            BindingTarget::Param { param, .. } | BindingTarget::LegacyParam { param, .. } => {
                Some(param)
            }
            _ => None,
        }
    }

    pub fn is_unset(&self) -> bool {
        matches!(self, BindingTarget::Unset)
    }

    /// Whether this and `other` denote the same destination, treating the legacy
    /// indexless param form as equal to an indexed one naming the same effect
    /// and param.
    ///
    /// The UI used to do this by building a string reverse-map from every
    /// four-part target to its three-part equivalent, rebuilt each frame in three
    /// separate places. One method, one definition.
    pub fn same_destination(&self, other: &BindingTarget) -> bool {
        if self == other {
            return true;
        }
        let as_pair = |t: &BindingTarget| match t {
            BindingTarget::Param { effect, param, .. }
            | BindingTarget::LegacyParam { effect, param } => Some((effect.clone(), param.clone())),
            _ => None,
        };
        // Only bridge across the two param forms; two different indexed params
        // are still different destinations.
        let one_is_legacy = matches!(self, BindingTarget::LegacyParam { .. })
            || matches!(other, BindingTarget::LegacyParam { .. });
        one_is_legacy && as_pair(self).is_some() && as_pair(self) == as_pair(other)
    }
}

impl PartialOrd for BindingTarget {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for BindingTarget {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.to_string().cmp(&other.to_string())
    }
}

impl std::fmt::Display for BindingTarget {
    /// The wire format, unchanged from when this was a bare `String`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BindingTarget::Unset => Ok(()),
            BindingTarget::Param {
                layer,
                effect,
                param,
            } => write!(f, "param.{layer}.{effect}.{param}"),
            BindingTarget::LegacyParam { effect, param } => write!(f, "param.{effect}.{param}"),
            BindingTarget::Layer { layer, field } => {
                write!(f, "layer.{layer}.{}", field.as_str())
            }
            BindingTarget::PostFx(field) => write!(f, "postfx.{field}"),
            BindingTarget::Particle(field) => write!(f, "particle.{field}"),
            BindingTarget::Uniform(field) => write!(f, "uniform.{field}"),
            BindingTarget::SceneTransport(action) => write!(f, "scene.transport.{action}"),
            BindingTarget::GlobalMasterOpacity => write!(f, "global.master_opacity"),
            BindingTarget::Unknown(raw) => write!(f, "{raw}"),
        }
    }
}

impl std::str::FromStr for BindingTarget {
    type Err = std::convert::Infallible;

    /// Never fails: an unrecognised string becomes [`BindingTarget::Unknown`]
    /// rather than an error, so one bad line cannot fail a whole bindings file.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(parse_target(s))
    }
}

fn parse_target(s: &str) -> BindingTarget {
    if s.is_empty() {
        return BindingTarget::Unset;
    }
    let parts: Vec<&str> = s.split('.').collect();
    // No shipped effect or param name contains a dot, so segment counts are
    // unambiguous — guarded by a test over every .pfx.
    match parts.as_slice() {
        ["param", idx, effect, param] => match idx.parse::<usize>() {
            Ok(layer) => BindingTarget::Param {
                layer,
                effect: (*effect).to_string(),
                param: (*param).to_string(),
            },
            Err(_) => BindingTarget::Unknown(s.to_string()),
        },
        ["param", effect, param] => BindingTarget::LegacyParam {
            effect: (*effect).to_string(),
            param: (*param).to_string(),
        },
        ["layer", idx, field] => match (idx.parse::<usize>(), LayerField::parse(field)) {
            (Ok(layer), Some(field)) => BindingTarget::Layer { layer, field },
            _ => BindingTarget::Unknown(s.to_string()),
        },
        ["postfx", field] => BindingTarget::PostFx((*field).to_string()),
        ["particle", field] => BindingTarget::Particle((*field).to_string()),
        ["uniform", field] => BindingTarget::Uniform((*field).to_string()),
        ["scene", "transport", action] => BindingTarget::SceneTransport((*action).to_string()),
        ["global", "master_opacity"] => BindingTarget::GlobalMasterOpacity,
        _ => BindingTarget::Unknown(s.to_string()),
    }
}

/// Infallible by construction — an unrecognised string becomes
/// [`BindingTarget::Unknown`]. Convenient at the string boundaries (config
/// files, tests) without weakening the type: everything still goes through the
/// parse rather than being treated as text.
impl From<&str> for BindingTarget {
    fn from(s: &str) -> Self {
        parse_target(s)
    }
}

impl From<String> for BindingTarget {
    fn from(s: String) -> Self {
        parse_target(&s)
    }
}

impl Serialize for BindingTarget {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for BindingTarget {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(parse_target(&s))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Binding {
    pub id: BindingId,
    pub name: String,
    pub enabled: bool,
    pub scope: BindingScope,
    /// Source identifier, e.g. "audio.kick", "midi.MPD218.cc.0.42", "osc./foo", "ws.mediapipe.left_thumb_y"
    pub source: String,
    pub target: BindingTarget,
    pub transforms: Vec<TransformDef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum TransformDef {
    #[serde(rename = "remap")]
    Remap {
        in_lo: f32,
        in_hi: f32,
        out_lo: f32,
        out_hi: f32,
    },
    #[serde(rename = "smooth")]
    Smooth { factor: f32 },
    #[serde(rename = "invert")]
    Invert,
    #[serde(rename = "quantize")]
    Quantize { steps: u32 },
    #[serde(rename = "deadzone")]
    Deadzone { lo: f32, hi: f32 },
    #[serde(rename = "curve")]
    Curve { curve_type: String },
    #[serde(rename = "gate")]
    Gate { threshold: f32 },
    #[serde(rename = "scale")]
    Scale { factor: f32 },
    #[serde(rename = "offset")]
    Offset { value: f32 },
    #[serde(rename = "clamp")]
    Clamp { lo: f32, hi: f32 },
}

/// Per-binding runtime state (not serialized).
#[derive(Debug, Default, Clone)]
pub struct BindingRuntime {
    pub smooth_state: f32,
    pub last_input: Option<f32>,
    pub last_output: Option<f32>,
    pub last_raw: Option<SourceRaw>,
    /// Rising-edge latch (#1791): whether last frame's post-transform output
    /// was above the 0.5 trigger threshold. Deliberately freezes while the
    /// binding is disabled or its source is absent from the snapshot, so a
    /// briefly missing held-high source can suppress a trigger but never
    /// fire a spurious one.
    pub prev_above_threshold: bool,
}

impl BindingRuntime {
    pub fn new() -> Self {
        Self {
            smooth_state: 0.0,
            last_input: None,
            last_output: None,
            last_raw: None,
            prev_above_threshold: false,
        }
    }
}

/// One evaluated binding result for the app to apply this frame.
#[derive(Debug, Clone)]
pub struct BindingOutput {
    pub target: BindingTarget,
    pub value: f32,
    /// True only on the frame `value` crossed above 0.5. Trigger-style
    /// targets (scene.transport.*) fire on this; continuous targets ignore
    /// it and stay level-driven.
    pub rising: bool,
}

/// Original value before normalization (UI diagnostics only).
#[derive(Debug, Clone)]
pub struct SourceRaw {
    pub display: String,
    #[allow(dead_code)]
    pub numeric: f64,
}

/// What the binding bus is currently learning.
#[derive(Debug, Clone)]
pub struct LearnState {
    pub binding_id: BindingId,
    pub field: LearnField,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LearnField {
    Source,
    #[allow(dead_code)]
    Target,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binding_serde_roundtrip() {
        let b = Binding {
            id: "b_001".into(),
            name: "Kick to warp".into(),
            enabled: true,
            scope: BindingScope::Global,
            source: "audio.kick".into(),
            target: "param.Phosphor.warp_intensity".into(),
            transforms: vec![
                TransformDef::Gate { threshold: 0.5 },
                TransformDef::Smooth { factor: 0.8 },
                TransformDef::Remap {
                    in_lo: 0.0,
                    in_hi: 1.0,
                    out_lo: 0.2,
                    out_hi: 1.0,
                },
            ],
        };

        let json = serde_json::to_string_pretty(&b).unwrap();
        let b2: Binding = serde_json::from_str(&json).unwrap();

        assert_eq!(b2.id, "b_001");
        assert_eq!(b2.name, "Kick to warp");
        assert_eq!(b2.scope, BindingScope::Global);
        assert_eq!(b2.transforms.len(), 3);
    }

    #[test]
    fn transform_serde_all_variants() {
        let transforms = vec![
            TransformDef::Remap {
                in_lo: 0.0,
                in_hi: 1.0,
                out_lo: 0.0,
                out_hi: 1.0,
            },
            TransformDef::Smooth { factor: 0.9 },
            TransformDef::Invert,
            TransformDef::Quantize { steps: 8 },
            TransformDef::Deadzone { lo: 0.1, hi: 0.9 },
            TransformDef::Curve {
                curve_type: "ease_in".into(),
            },
            TransformDef::Gate { threshold: 0.5 },
            TransformDef::Scale { factor: 2.0 },
            TransformDef::Offset { value: -0.5 },
            TransformDef::Clamp { lo: 0.0, hi: 1.0 },
        ];

        let json = serde_json::to_string(&transforms).unwrap();
        let parsed: Vec<TransformDef> = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.len(), 10);
    }

    #[test]
    fn scope_serde() {
        let s = BindingScope::Global;
        let json = serde_json::to_string(&s).unwrap();
        let s2: BindingScope = serde_json::from_str(&json).unwrap();
        assert_eq!(s2, BindingScope::Global);

        let s = BindingScope::Preset;
        let json = serde_json::to_string(&s).unwrap();
        let s2: BindingScope = serde_json::from_str(&json).unwrap();
        assert_eq!(s2, BindingScope::Preset);
    }
}

#[cfg(test)]
mod target_tests {
    use super::*;

    /// The wire format is unchanged, so binding files written by every earlier
    /// version keep loading. This is the whole compatibility contract in one
    /// test: parse each form, and get the identical string back.
    #[test]
    fn every_target_form_round_trips_through_its_string() {
        for raw in [
            "param.0.Raster.warp_intensity",
            "param.12.Frost.bite",
            "param.Raster.warp_intensity", // pre-#1792 indexless form
            "param.*.warp_intensity",      // the migration's wildcard
            "layer.0.opacity",
            "layer.3.blend",
            "layer.1.displace",
            "layer.2.enabled",
            "postfx.bloom_threshold",
            "particle.emit_rate",
            "uniform.kick",
            "scene.transport.go",
            "global.master_opacity",
        ] {
            let parsed = BindingTarget::from(raw);
            assert!(
                !matches!(parsed, BindingTarget::Unknown(_)),
                "{raw} fell through to Unknown"
            );
            assert_eq!(parsed.to_string(), raw, "{raw} did not round-trip");
        }
        assert_eq!(BindingTarget::from("").to_string(), "");
        assert!(BindingTarget::from("").is_unset());
    }

    /// An unrecognised target is carried verbatim rather than dropped — losing a
    /// VJ's mapping because we did not recognise it would be worse than carrying
    /// it inert, and it lets a file written by a newer version survive a
    /// round-trip through an older one.
    #[test]
    fn an_unknown_target_survives_verbatim() {
        for raw in [
            "something.we.have.not.invented.yet",
            "layer.notanumber.opacity",
            "param.notanumber.Raster.warp",
            "layer.0.no_such_field",
        ] {
            let parsed = BindingTarget::from(raw);
            assert!(matches!(parsed, BindingTarget::Unknown(_)), "{raw}");
            assert_eq!(parsed.to_string(), raw);
        }
    }

    #[test]
    fn serde_uses_the_same_string_as_display() {
        let t = BindingTarget::Param {
            layer: 2,
            effect: "Frost".into(),
            param: "bite".into(),
        };
        let json = serde_json::to_string(&t).expect("serializable");
        assert_eq!(json, "\"param.2.Frost.bite\"");
        let back: BindingTarget = serde_json::from_str(&json).expect("deserializable");
        assert_eq!(back, t);
    }

    /// A whole binding still serializes to the shape already on disk.
    #[test]
    fn a_binding_serializes_to_the_shape_already_on_disk() {
        let json = r#"{
            "id": "b_001",
            "name": "",
            "enabled": true,
            "scope": "Preset",
            "source": "audio.rms",
            "target": "layer.0.opacity",
            "transforms": []
        }"#;
        let b: Binding = serde_json::from_str(json).expect("loads an existing file");
        assert_eq!(
            b.target,
            BindingTarget::Layer {
                layer: 0,
                field: LayerField::Opacity
            }
        );
        let out = serde_json::to_value(&b).expect("serializable");
        assert_eq!(out["target"], serde_json::json!("layer.0.opacity"));
    }

    /// Only the layer-bearing forms report one — this is what `remap_layer_targets`
    /// walks, and it is exhaustive rather than a string sniff.
    #[test]
    fn only_layer_bearing_targets_report_a_layer() {
        assert_eq!(BindingTarget::from("layer.2.opacity").layer(), Some(2));
        assert_eq!(BindingTarget::from("layer.11.blend").layer(), Some(11));
        assert_eq!(BindingTarget::from("param.3.Raster.warp").layer(), Some(3));
        for raw in [
            "param.Raster.warp", // legacy: resolved against the active layer
            "postfx.vignette",
            "particle.emit_rate",
            "uniform.kick",
            "scene.transport.go",
            "global.master_opacity",
            "",
        ] {
            assert_eq!(
                BindingTarget::from(raw).layer(),
                None,
                "{raw} should carry no layer"
            );
        }
    }

    #[test]
    fn with_layer_changes_only_the_index() {
        assert_eq!(
            BindingTarget::from("layer.0.opacity")
                .with_layer(4)
                .to_string(),
            "layer.4.opacity"
        );
        assert_eq!(
            BindingTarget::from("param.0.Raster.warp")
                .with_layer(2)
                .to_string(),
            "param.2.Raster.warp"
        );
        // Layerless targets are returned unchanged rather than mangled.
        for raw in [
            "postfx.vignette",
            "global.master_opacity",
            "param.Raster.warp",
        ] {
            assert_eq!(BindingTarget::from(raw).with_layer(7).to_string(), raw);
        }
    }

    /// The legacy indexless form names the same destination as an indexed one on
    /// the same effect+param, which is what the UI's string reverse-map used to do.
    #[test]
    fn same_destination_bridges_the_two_param_forms() {
        let indexed = BindingTarget::from("param.0.Raster.warp");
        let legacy = BindingTarget::from("param.Raster.warp");
        assert!(indexed.same_destination(&legacy));
        assert!(legacy.same_destination(&indexed));
        // ...but two different layers are still two different destinations.
        let other = BindingTarget::from("param.1.Raster.warp");
        assert!(!indexed.same_destination(&other));
        // ...and a different param never matches.
        assert!(!legacy.same_destination(&BindingTarget::from("param.0.Raster.flow")));
        // Non-param targets compare by plain equality.
        let vig = BindingTarget::from("postfx.vignette");
        assert!(vig.same_destination(&BindingTarget::from("postfx.vignette")));
        assert!(!vig.same_destination(&BindingTarget::from("postfx.grain_rate")));
    }

    /// Round-trips every binding file in the user's real config through the
    /// parser, asserting the serialized form comes back byte-identical.
    /// Run: cargo test -p phosphor-app -- --ignored real_binding_files_round_trip --nocapture
    #[test]
    #[ignore = "reads the operator's ~/.config/phosphor; not hermetic"]
    fn real_binding_files_round_trip() {
        let dir = dirs::config_dir().expect("config dir").join("phosphor");
        let mut files: Vec<std::path::PathBuf> = vec![dir.join("global-bindings.json")];
        if let Ok(rd) = std::fs::read_dir(dir.join("presets")) {
            for e in rd.flatten() {
                if e.file_name().to_string_lossy().ends_with(".bindings.json") {
                    files.push(e.path());
                }
            }
        }
        let mut checked = 0usize;
        for f in files {
            let Ok(text) = std::fs::read_to_string(&f) else {
                continue;
            };
            let v: serde_json::Value = serde_json::from_str(&text).expect("valid json");
            for b in v["bindings"].as_array().into_iter().flatten() {
                let raw = b["target"].as_str().unwrap_or_default();
                let parsed = BindingTarget::from(raw);
                assert_eq!(parsed.to_string(), raw, "{} in {}", raw, f.display());
                assert!(
                    !matches!(parsed, BindingTarget::Unknown(_)),
                    "{} in {} did not parse",
                    raw,
                    f.display()
                );
                checked += 1;
            }
        }
        println!("round-tripped {checked} real targets");
        assert!(checked > 0, "no real bindings found to check");
    }
}
