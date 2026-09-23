use std::collections::{BTreeSet, HashMap};

use serde::{Deserialize, Serialize};

use crate::params::ParamValue;

pub const DEFAULT_SLOTS: [&str; 6] = [
    "color-1", "color-2", "color-3", "color-4", "color-5", "color-6",
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Swatch {
    pub slot: String,
    pub rgba: [f32; 4],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Palette {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    pub swatches: Vec<Swatch>,
}

impl Palette {
    pub fn slot_ids(&self) -> BTreeSet<String> {
        self.swatches.iter().map(|s| s.slot.clone()).collect()
    }

    pub fn color(&self, slot: &str) -> Option<[f32; 4]> {
        self.swatches
            .iter()
            .find(|s| s.slot == slot)
            .map(|s| s.rgba)
    }

    pub fn set_color(&mut self, slot: &str, rgba: [f32; 4]) {
        if let Some(s) = self.swatches.iter_mut().find(|s| s.slot == slot) {
            s.rgba = rgba;
        } else {
            self.swatches.push(Swatch {
                slot: slot.to_string(),
                rgba,
            });
        }
    }

    pub fn as_param_map(&self) -> HashMap<String, ParamValue> {
        self.swatches
            .iter()
            .map(|s| (s.slot.clone(), ParamValue::Color(s.rgba)))
            .collect()
    }

    pub fn solid_test(id: &str, name: &str) -> Self {
        let mut swatches = Vec::new();
        for (i, slot) in DEFAULT_SLOTS.iter().enumerate() {
            let t = i as f32 / 5.0;
            swatches.push(Swatch {
                slot: (*slot).to_string(),
                rgba: [0.1 + t * 0.2, 0.15, 0.2 + t * 0.1, 1.0],
            });
        }
        Self {
            schema_version: 1,
            id: id.into(),
            name: name.into(),
            swatches,
        }
    }
}

pub fn required_slot_ids(palette: &Palette) -> BTreeSet<String> {
    palette.slot_ids()
}

pub fn rgba_to_hex(rgba: [f32; 4]) -> String {
    format!(
        "#{:02X}{:02X}{:02X}{:02X}",
        (rgba[0].clamp(0.0, 1.0) * 255.0).round() as u8,
        (rgba[1].clamp(0.0, 1.0) * 255.0).round() as u8,
        (rgba[2].clamp(0.0, 1.0) * 255.0).round() as u8,
        (rgba[3].clamp(0.0, 1.0) * 255.0).round() as u8,
    )
}

pub fn hex_to_rgba(hex: &str) -> Option<[f32; 4]> {
    let h = hex.trim().trim_start_matches('#');
    let bytes = match h.len() {
        6 => {
            let n = u32::from_str_radix(h, 16).ok()?;
            [
                ((n >> 16) & 0xFF) as f32 / 255.0,
                ((n >> 8) & 0xFF) as f32 / 255.0,
                (n & 0xFF) as f32 / 255.0,
                1.0,
            ]
        }
        8 => {
            let n = u32::from_str_radix(h, 16).ok()?;
            [
                ((n >> 24) & 0xFF) as f32 / 255.0,
                ((n >> 16) & 0xFF) as f32 / 255.0,
                ((n >> 8) & 0xFF) as f32 / 255.0,
                (n & 0xFF) as f32 / 255.0,
            ]
        }
        _ => return None,
    };
    Some(bytes)
}

pub fn rgba_to_hsl(rgba: [f32; 4]) -> [f32; 4] {
    let (r, g, b, a) = (rgba[0], rgba[1], rgba[2], rgba[3]);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let d = max - min;
    if d < 1e-6 {
        return [0.0, 0.0, l, a];
    }
    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };
    let h = if (max - r).abs() < 1e-6 {
        ((g - b) / d + if g < b { 6.0 } else { 0.0 }) / 6.0
    } else if (max - g).abs() < 1e-6 {
        ((b - r) / d + 2.0) / 6.0
    } else {
        ((r - g) / d + 4.0) / 6.0
    };
    [h, s, l, a]
}

pub fn hsl_to_rgba(hsla: [f32; 4]) -> [f32; 4] {
    let (h, s, l, a) = (hsla[0], hsla[1], hsla[2], hsla[3]);
    if s < 1e-6 {
        return [l, l, l, a];
    }
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let hue2rgb = |p: f32, q: f32, mut t: f32| {
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    [
        hue2rgb(p, q, h + 1.0 / 3.0),
        hue2rgb(p, q, h),
        hue2rgb(p, q, h - 1.0 / 3.0),
        a,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_rgba_roundtrip() {
        let c = [0.3, 0.22, 0.18, 1.0];
        let hex = rgba_to_hex(c);
        let back = hex_to_rgba(&hex).unwrap();
        for i in 0..4 {
            assert!((c[i] - back[i]).abs() < 0.01);
        }
    }

    #[test]
    fn hsl_rgba_roundtrip() {
        let c = [0.2, 0.4, 0.8, 0.5];
        let hsl = rgba_to_hsl(c);
        let back = hsl_to_rgba(hsl);
        for i in 0..3 {
            assert!((c[i] - back[i]).abs() < 0.02, "{c:?} vs {back:?}");
        }
        assert!((c[3] - back[3]).abs() < 1e-6);
    }

    #[test]
    fn reorder_keeps_slot_ids() {
        let mut p = Palette::solid_test("earth", "Earth");
        p.swatches.reverse();
        assert!(p.color("color-1").is_some());
        assert_eq!(p.slot_ids().len(), 6);
    }
}
