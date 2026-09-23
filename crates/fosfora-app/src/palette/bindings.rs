use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaletteBinding {
    pub slot: String,
    pub layer: usize,
    pub parameter: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaletteBindingSet {
    #[serde(default = "schema_v1")]
    pub schema_version: u32,
    pub preset: String,
    pub bindings: Vec<PaletteBinding>,
}

fn schema_v1() -> u32 {
    1
}

impl PaletteBindingSet {
    pub fn uses_slot(&self, slot: &str) -> bool {
        self.bindings.iter().any(|b| b.slot == slot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delete_bound_swatch_detected() {
        let set = PaletteBindingSet {
            schema_version: 1,
            preset: "deep-sleep".into(),
            bindings: vec![PaletteBinding {
                slot: "color-1".into(),
                layer: 0,
                parameter: "tint".into(),
            }],
        };
        assert!(set.uses_slot("color-1"));
        assert!(!set.uses_slot("color-6"));
    }
}
