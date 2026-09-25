use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;

use super::definition::ShowDefinition;
use crate::palette::bindings::PaletteBindingSet;
use crate::palette::types::{required_slot_ids, Palette};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationIssue {
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationReport {
    pub issues: Vec<ValidationIssue>,
}

impl ValidationReport {
    pub fn ready(&self) -> bool {
        self.issues.is_empty()
    }

    pub fn summary(&self) -> String {
        if self.ready() {
            "READY".into()
        } else {
            format!("{} ISSUES", self.issues.len())
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct MediaEstimate {
    pub path: String,
    pub exists: bool,
    pub estimated_ram_bytes: u64,
}

/// Generic show-pack checks. Hibernation-specific duration lives in [`hibernation_acceptance`].
pub fn validate_show(
    def: &ShowDefinition,
    palettes: &HashMap<String, Palette>,
    bindings: &HashMap<String, PaletteBindingSet>,
    preset_ids: &HashSet<String>,
    media: &[MediaEstimate],
    color_params: &HashSet<(usize, String)>,
    conflicting_owners: &[(String, usize, String)],
) -> ValidationReport {
    let mut issues = Vec::new();

    if def.schema_version != 1 {
        issues.push(issue(format!(
            "Unsupported show schema_version {}",
            def.schema_version
        )));
    }
    if def.duration_secs == 0 {
        issues.push(issue("duration_secs must be > 0"));
    }

    validate_track_sorted(
        &mut issues,
        "scene_track",
        def.scene_track.iter().map(|c| c.at_secs),
        def.duration_secs,
    );
    validate_track_sorted(
        &mut issues,
        "palette_track",
        def.palette_track.iter().map(|c| c.at_secs),
        def.duration_secs,
    );

    for cue in &def.scene_track {
        if !preset_ids.contains(&cue.preset_id) {
            issues.push(issue(format!(
                "Unknown preset_id '{}' at {}s",
                cue.preset_id, cue.at_secs
            )));
        }
    }
    for cue in &def.palette_track {
        if !palettes.contains_key(&cue.palette_id) {
            issues.push(issue(format!(
                "Unknown palette_id '{}' at {}s",
                cue.palette_id, cue.at_secs
            )));
        }
    }

    let referenced: Vec<&Palette> = def
        .palette_track
        .iter()
        .filter_map(|c| palettes.get(&c.palette_id))
        .collect();
    if !referenced.is_empty() {
        let schema = required_slot_ids(referenced[0]);
        for palette in &referenced {
            for slot in &schema {
                if !palette.swatches.iter().any(|s| &s.slot == slot) {
                    issues.push(issue(format!(
                        "Palette \"{}\" missing slot \"{slot}\"",
                        palette.name
                    )));
                }
            }
        }
    }

    for (preset_id, set) in bindings {
        if !preset_ids.contains(preset_id) && !preset_ids.contains(&set.preset) {
            issues.push(issue(format!(
                "Unknown preset '{preset_id}' in palette bindings"
            )));
        }
        for b in &set.bindings {
            if !color_params.is_empty()
                && !color_params.contains(&(b.layer, b.parameter.clone()))
            {
                issues.push(issue(format!(
                    "Binding {preset_id} slot {} → layer {} / {} is not a Color param",
                    b.slot, b.layer, b.parameter
                )));
            }
        }
        // Delete-bound-swatch: every bound slot must still exist on at least
        // one palette on the track (slot IDs are stable; array position is not).
        let live_slots: BTreeSet<&str> = def
            .palette_track
            .iter()
            .filter_map(|c| palettes.get(&c.palette_id))
            .flat_map(|p| p.swatches.iter().map(|s| s.slot.as_str()))
            .collect();
        if !live_slots.is_empty() {
            for b in &set.bindings {
                if !live_slots.contains(b.slot.as_str()) {
                    issues.push(issue(format!(
                        "Binding {preset_id} slot {} no longer exists on any track palette",
                        b.slot
                    )));
                }
            }
        }
    }

    for (owner, layer, param) in conflicting_owners {
        issues.push(issue(format!(
            "layer {layer} / {param} controlled by both palette and {owner}"
        )));
    }

    for m in media {
        if !m.exists {
            issues.push(issue(format!("Missing media '{}'", m.path)));
        }
    }

    ValidationReport { issues }
}

pub fn hibernation_acceptance(def: &ShowDefinition) -> Vec<ValidationIssue> {
    if def.id == "hibernation" && def.duration_secs != 10800 {
        vec![issue(format!(
            "Hibernation duration_secs must be 10800 (got {})",
            def.duration_secs
        ))]
    } else {
        vec![]
    }
}

pub fn estimate_rgba_ram(width: u32, height: u32, fps: f64, duration_secs: f64) -> u64 {
    let frames = (duration_secs * fps).ceil().max(0.0) as u64;
    frames.saturating_mul(u64::from(width) * u64::from(height) * 4)
}

pub fn media_exists(root: &Path, stored: &str) -> bool {
    let p = Path::new(stored);
    if p.is_absolute() {
        p.exists()
    } else {
        root.join(p).exists()
    }
}

fn validate_track_sorted(
    issues: &mut Vec<ValidationIssue>,
    name: &str,
    times: impl IntoIterator<Item = u32>,
    duration: u32,
) {
    let mut prev: Option<u32> = None;
    for at in times {
        if at > duration {
            issues.push(issue(format!("{name} cue at {at}s exceeds duration {duration}s")));
        }
        if let Some(p) = prev {
            if at < p {
                issues.push(issue(format!("{name} is not sorted ({at}s after {p}s)")));
            }
        }
        prev = Some(at);
    }
}

fn issue(message: impl Into<String>) -> ValidationIssue {
    ValidationIssue {
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::show::definition::{PaletteCue, SceneCue, SceneTransition, TransitionKind};

    fn def() -> ShowDefinition {
        ShowDefinition {
            schema_version: 1,
            id: "hibernation".into(),
            name: "Hibernation".into(),
            duration_secs: 10800,
            scene_track: vec![SceneCue {
                at_secs: 0,
                preset_id: "deep-sleep".into(),
                transition: SceneTransition {
                    kind: TransitionKind::Cut,
                    duration_ms: 0,
                },
            }],
            palette_track: vec![PaletteCue {
                at_secs: 0,
                palette_id: "earth".into(),
                transition_ms: 0,
            }],
        }
    }

    #[test]
    fn ready_when_clean() {
        let mut palettes = HashMap::new();
        palettes.insert("earth".into(), crate::palette::types::Palette::solid_test("earth", "Earth"));
        let mut presets = HashSet::new();
        presets.insert("deep-sleep".into());
        let report = validate_show(
            &def(),
            &palettes,
            &HashMap::new(),
            &presets,
            &[],
            &HashSet::new(),
            &[],
        );
        assert!(report.ready(), "{:?}", report.issues);
        assert_eq!(report.summary(), "READY");
        assert!(hibernation_acceptance(&def()).is_empty());
    }

    #[test]
    fn duration_and_unknown_refs() {
        let mut d = def();
        d.duration_secs = 0;
        d.scene_track[0].preset_id = "missing".into();
        let report = validate_show(
            &d,
            &HashMap::new(),
            &HashMap::new(),
            &HashSet::new(),
            &[MediaEstimate {
                path: "gone.mp4".into(),
                exists: false,
                estimated_ram_bytes: 0,
            }],
            &HashSet::new(),
            &[],
        );
        assert!(!report.ready());
        assert!(report.summary().contains("ISSUES"));
    }

    #[test]
    fn ram_estimate_720p_8s_15fps() {
        let bytes = estimate_rgba_ram(1280, 720, 15.0, 8.0);
        // 120 frames * 1280 * 720 * 4 ≈ 442 MB
        assert!(bytes > 400_000_000 && bytes < 500_000_000);
    }

    #[test]
    fn delete_bound_swatch_is_a_validation_error() {
        use crate::palette::bindings::{PaletteBinding, PaletteBindingSet};
        let mut palettes = HashMap::new();
        let mut earth = crate::palette::types::Palette::solid_test("earth", "Earth");
        earth.swatches.retain(|s| s.slot != "color-1");
        palettes.insert("earth".into(), earth);
        let mut presets = HashSet::new();
        presets.insert("deep-sleep".into());
        let mut bindings = HashMap::new();
        bindings.insert(
            "deep-sleep".into(),
            PaletteBindingSet {
                schema_version: 1,
                preset: "deep-sleep".into(),
                bindings: vec![PaletteBinding {
                    slot: "color-1".into(),
                    layer: 1,
                    parameter: "tint".into(),
                }],
            },
        );
        let report = validate_show(
            &def(),
            &palettes,
            &bindings,
            &presets,
            &[],
            &HashSet::new(),
            &[],
        );
        assert!(!report.ready());
        assert!(report.issues.iter().any(|i| i.message.contains("color-1")));
    }
}
