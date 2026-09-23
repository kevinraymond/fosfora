use std::collections::HashMap;
use std::time::Duration;

use super::types::Palette;
use crate::params::ParamValue;
use crate::show::definition::{scheduled_index, PaletteCue};

#[derive(Debug, Clone, Default)]
pub struct PaletteScheduler {
    pub last_processed_scheduled_index: Option<usize>,
    pub active_palette_id: Option<String>,
}

impl PaletteScheduler {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn manual_set(&mut self, id: String) {
        self.active_palette_id = Some(id);
    }

    pub fn sync_index(&mut self, cues: &[PaletteCue], elapsed: Duration) {
        let secs = elapsed.as_secs().min(u64::from(u32::MAX)) as u32;
        self.last_processed_scheduled_index =
            scheduled_index(cues.iter().map(|c| c.at_secs), secs);
        if let Some(i) = self.last_processed_scheduled_index {
            self.active_palette_id = cues.get(i).map(|c| c.palette_id.clone());
        }
    }

    pub fn tick(
        &mut self,
        cues: &[PaletteCue],
        palettes: &HashMap<String, Palette>,
        elapsed: Duration,
    ) -> Option<HashMap<String, ParamValue>> {
        let secs = elapsed.as_secs().min(u64::from(u32::MAX)) as u32;
        let idx = scheduled_index(cues.iter().map(|c| c.at_secs), secs);
        if idx != self.last_processed_scheduled_index {
            self.last_processed_scheduled_index = idx;
            if let Some(i) = idx {
                self.active_palette_id = cues.get(i).map(|c| c.palette_id.clone());
            }
        }
        palette_colors_at(cues, palettes, elapsed)
    }
}

/// Slot-ID-keyed interpolation. A mid-transition scene change does not reset this.
pub fn palette_colors_at(
    cues: &[PaletteCue],
    palettes: &HashMap<String, Palette>,
    elapsed: Duration,
) -> Option<HashMap<String, ParamValue>> {
    if cues.is_empty() {
        return None;
    }
    let secs_f = elapsed.as_secs_f64();
    let secs = elapsed.as_secs().min(u64::from(u32::MAX)) as u32;
    let idx = scheduled_index(cues.iter().map(|c| c.at_secs), secs)?;
    let to_cue = &cues[idx];
    let to = palettes.get(&to_cue.palette_id)?;
    if idx == 0 || to_cue.transition_ms == 0 {
        return Some(to.as_param_map());
    }
    let from_cue = &cues[idx - 1];
    let from = palettes.get(&from_cue.palette_id)?;
    let start = f64::from(to_cue.at_secs);
    let dur = f64::from(to_cue.transition_ms) / 1000.0;
    let t = if dur <= 0.0 {
        1.0
    } else {
        ((secs_f - start) / dur).clamp(0.0, 1.0) as f32
    };
    Some(lerp_palettes(from, to, t))
}

pub fn lerp_palettes(from: &Palette, to: &Palette, t: f32) -> HashMap<String, ParamValue> {
    let mut out = HashMap::new();
    for swatch in &to.swatches {
        let Some(a) = from.color(&swatch.slot) else {
            continue;
        };
        let av = ParamValue::Color(a);
        let bv = ParamValue::Color(swatch.rgba);
        out.insert(swatch.slot.clone(), av.lerp(&bv, t));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::show::definition::PaletteCue;

    fn pal(id: &str, v: f32) -> Palette {
        let mut p = Palette::solid_test(id, id);
        for s in &mut p.swatches {
            s.rgba = [v, v, v, 1.0];
        }
        p
    }

    #[test]
    fn mid_transition_survives_independent_of_scene() {
        let cues = vec![
            PaletteCue {
                at_secs: 0,
                palette_id: "a".into(),
                transition_ms: 0,
            },
            PaletteCue {
                at_secs: 10,
                palette_id: "b".into(),
                transition_ms: 100_000,
            },
        ];
        let mut map = HashMap::new();
        map.insert("a".into(), pal("a", 0.0));
        map.insert("b".into(), pal("b", 1.0));
        // 37% of 100s transition: t=10+37 = 47s
        let colors = palette_colors_at(&cues, &map, Duration::from_secs(47)).unwrap();
        match colors.get("color-1") {
            Some(ParamValue::Color(c)) => assert!((c[0] - 0.37).abs() < 0.02, "{c:?}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn stall_jump_to_latest_palette() {
        let cues = vec![
            PaletteCue {
                at_secs: 0,
                palette_id: "a".into(),
                transition_ms: 0,
            },
            PaletteCue {
                at_secs: 10,
                palette_id: "b".into(),
                transition_ms: 1000,
            },
        ];
        let mut map = HashMap::new();
        map.insert("a".into(), pal("a", 0.0));
        map.insert("b".into(), pal("b", 1.0));
        let colors = palette_colors_at(&cues, &map, Duration::from_secs(60)).unwrap();
        match colors.get("color-1") {
            Some(ParamValue::Color(c)) => assert!((c[0] - 1.0).abs() < 1e-5),
            other => panic!("{other:?}"),
        }
    }
}
