use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransitionKind {
    Cut,
    Dissolve,
    ParamMorph,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SceneTransition {
    pub kind: TransitionKind,
    pub duration_ms: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SceneCue {
    pub at_secs: u32,
    pub preset_id: String,
    pub transition: SceneTransition,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaletteCue {
    pub at_secs: u32,
    pub palette_id: String,
    #[serde(default)]
    pub transition_ms: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShowDefinition {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    pub duration_secs: u32,
    #[serde(default)]
    pub scene_track: Vec<SceneCue>,
    #[serde(default)]
    pub palette_track: Vec<PaletteCue>,
}

impl ShowDefinition {
    pub fn scene_index_at(&self, elapsed_secs: u32) -> Option<usize> {
        scheduled_index(self.scene_track.iter().map(|c| c.at_secs), elapsed_secs)
    }

    pub fn palette_index_at(&self, elapsed_secs: u32) -> Option<usize> {
        scheduled_index(self.palette_track.iter().map(|c| c.at_secs), elapsed_secs)
    }
}

/// Latest cue whose `at_secs <= elapsed`.
pub fn scheduled_index(at_secs: impl IntoIterator<Item = u32>, elapsed_secs: u32) -> Option<usize> {
    let mut last = None;
    for (i, at) in at_secs.into_iter().enumerate() {
        if at <= elapsed_secs {
            last = Some(i);
        } else {
            break;
        }
    }
    last
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheduled_index_boundaries() {
        let times = [0, 1200, 3600];
        assert_eq!(scheduled_index(times, 0), Some(0));
        assert_eq!(scheduled_index(times, 1199), Some(0));
        assert_eq!(scheduled_index(times, 1200), Some(1));
        assert_eq!(scheduled_index(times, 3600), Some(2));
        assert_eq!(scheduled_index(times, 10_000), Some(2));
    }

    #[test]
    fn empty_track() {
        assert_eq!(scheduled_index([], 10), None);
    }
}
