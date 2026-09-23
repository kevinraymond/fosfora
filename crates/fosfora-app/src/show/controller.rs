use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::clock::{ClockTransport, ShowClock};
use super::definition::ShowDefinition;
use super::scheduler::{SceneScheduler, ScheduledSceneAction};
use super::validation::{
    hibernation_acceptance, validate_show, MediaEstimate, ValidationReport,
};
use crate::palette::bindings::PaletteBindingSet;
use crate::palette::controller::{palette_colors_at, PaletteScheduler};
use crate::palette::types::Palette;
use crate::params::ParamValue;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShowCommand {
    Start,
    Pause,
    Resume,
    Reset,
    Seek { secs: u32 },
    StopAuto,
    NextVisual,
    PrevVisual,
    NextPalette,
    PrevPalette,
    ToggleBlackout,
}

#[derive(Debug, Clone)]
pub enum ShowVisualEvent {
    Scene(ScheduledSceneAction),
    Palette(HashMap<String, ParamValue>),
}

pub struct ShowController {
    pub definition: ShowDefinition,
    pub pack_root: PathBuf,
    pub clock: ShowClock,
    pub auto_enabled: bool,
    pub scene: SceneScheduler,
    pub palette: PaletteScheduler,
    pub palettes: HashMap<String, Palette>,
    pub bindings: HashMap<String, PaletteBindingSet>,
    pub preset_ids: Vec<String>,
    pub last_report: ValidationReport,
}

impl ShowController {
    pub fn new(
        definition: ShowDefinition,
        pack_root: PathBuf,
        palettes: HashMap<String, Palette>,
        bindings: HashMap<String, PaletteBindingSet>,
        preset_ids: Vec<String>,
    ) -> Self {
        Self {
            definition,
            pack_root,
            clock: ShowClock::new(),
            auto_enabled: false,
            scene: SceneScheduler::default(),
            palette: PaletteScheduler::default(),
            palettes,
            bindings,
            preset_ids,
            last_report: ValidationReport { issues: Vec::new() },
        }
    }

    pub fn resolve_path(&self, stored: &str) -> PathBuf {
        let p = Path::new(stored);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.pack_root.join(p)
        }
    }

    pub fn validate(
        &mut self,
        media: &[MediaEstimate],
        color_params: &std::collections::HashSet<(usize, String)>,
        conflicts: &[(String, usize, String)],
    ) -> ValidationReport {
        let preset_set = self.preset_ids.iter().cloned().collect();
        let mut report = validate_show(
            &self.definition,
            &self.palettes,
            &self.bindings,
            &preset_set,
            media,
            color_params,
            conflicts,
        );
        report
            .issues
            .extend(hibernation_acceptance(&self.definition));
        self.last_report = report.clone();
        report
    }

    pub fn elapsed_ms_at(&self, now: Instant) -> u128 {
        self.clock.elapsed_at(now).as_millis()
    }

    pub fn handle(&mut self, now: Instant, cmd: ShowCommand) -> Vec<ShowVisualEvent> {
        match cmd {
            ShowCommand::Start => {
                self.auto_enabled = true;
                self.clock.start(now);
                let mut out = Vec::new();
                if let Some(scene) = self.scene.start_cut(&self.definition.scene_track) {
                    out.push(ShowVisualEvent::Scene(scene));
                }
                self.palette.reset();
                if let Some(colors) = self.palette_at(Duration::ZERO) {
                    out.push(ShowVisualEvent::Palette(colors));
                }
                out
            }
            ShowCommand::Pause => {
                self.clock.pause(now);
                Vec::new()
            }
            ShowCommand::Resume => {
                self.clock.resume(now);
                Vec::new()
            }
            ShowCommand::Reset => {
                self.clock.reset();
                self.scene = SceneScheduler::default();
                self.palette.reset();
                Vec::new()
            }
            ShowCommand::Seek { secs } => {
                let pos = Duration::from_secs(u64::from(secs));
                self.clock.seek(now, pos);
                self.seek_events(secs)
            }
            ShowCommand::StopAuto => {
                self.auto_enabled = false;
                Vec::new()
            }
            ShowCommand::NextVisual => {
                if let Some(id) = self.step_preset(1) {
                    self.scene.manual_set_preset(id.clone());
                    vec![ShowVisualEvent::Scene(ScheduledSceneAction {
                        preset_id: id,
                        kind: crate::show::definition::TransitionKind::Cut,
                        duration_ms: 0,
                        snap: true,
                    })]
                } else {
                    Vec::new()
                }
            }
            ShowCommand::PrevVisual => {
                if let Some(id) = self.step_preset(-1) {
                    self.scene.manual_set_preset(id.clone());
                    vec![ShowVisualEvent::Scene(ScheduledSceneAction {
                        preset_id: id,
                        kind: crate::show::definition::TransitionKind::Cut,
                        duration_ms: 0,
                        snap: true,
                    })]
                } else {
                    Vec::new()
                }
            }
            ShowCommand::NextPalette => self.manual_palette(1),
            ShowCommand::PrevPalette => self.manual_palette(-1),
            ShowCommand::ToggleBlackout => Vec::new(),
        }
    }

    pub fn tick(&mut self, now: Instant) -> Vec<ShowVisualEvent> {
        if !self.auto_enabled || self.clock.transport() != ClockTransport::Running {
            return Vec::new();
        }
        let elapsed = self.clock.elapsed_at(now);
        let secs = elapsed.as_secs().min(u64::from(u32::MAX)) as u32;
        let mut out = Vec::new();
        if let Some(scene) = self.scene.tick(&self.definition.scene_track, secs) {
            out.push(ShowVisualEvent::Scene(scene));
        }
        if let Some(colors) = self.palette.tick(
            &self.definition.palette_track,
            &self.palettes,
            elapsed,
        ) {
            out.push(ShowVisualEvent::Palette(colors));
        }
        out
    }

    fn seek_events(&mut self, secs: u32) -> Vec<ShowVisualEvent> {
        let mut out = Vec::new();
        if let Some(scene) = self.scene.seek_snap(&self.definition.scene_track, secs) {
            out.push(ShowVisualEvent::Scene(scene));
        }
        if let Some(colors) = self.palette_at(Duration::from_secs(u64::from(secs))) {
            out.push(ShowVisualEvent::Palette(colors));
        }
        out
    }

    fn palette_at(&mut self, elapsed: Duration) -> Option<HashMap<String, ParamValue>> {
        self.palette.sync_index(&self.definition.palette_track, elapsed);
        palette_colors_at(&self.definition.palette_track, &self.palettes, elapsed)
    }

    fn step_preset(&self, delta: i32) -> Option<String> {
        if self.preset_ids.is_empty() {
            return None;
        }
        let current = self.scene.active_preset_id.as_deref().unwrap_or("");
        let idx = self
            .preset_ids
            .iter()
            .position(|id| id == current)
            .unwrap_or(0);
        let next = (idx as i32 + delta).rem_euclid(self.preset_ids.len() as i32) as usize;
        Some(self.preset_ids[next].clone())
    }

    fn manual_palette(&mut self, delta: i32) -> Vec<ShowVisualEvent> {
        let ids: Vec<String> = self.definition.palette_track.iter().map(|c| c.palette_id.clone()).collect();
        if ids.is_empty() {
            return Vec::new();
        }
        let current = self
            .palette
            .active_palette_id
            .clone()
            .or_else(|| ids.first().cloned())
            .unwrap_or_default();
        let idx = ids.iter().position(|id| id == &current).unwrap_or(0);
        let next = (idx as i32 + delta).rem_euclid(ids.len() as i32) as usize;
        let id = ids[next].clone();
        self.palette.manual_set(id.clone());
        if let Some(p) = self.palettes.get(&id) {
            vec![ShowVisualEvent::Palette(p.as_param_map())]
        } else {
            Vec::new()
        }
    }

    pub fn snapshot(&self, now: Instant, blackout: bool) -> ShowSnapshot {
        let elapsed = self.clock.elapsed_at(now);
        let secs = elapsed.as_secs().min(u64::from(u32::MAX)) as u32;
        let scene_idx = self.definition.scene_index_at(secs);
        let next_scene = scene_idx.and_then(|i| self.definition.scene_track.get(i + 1));
        let pal_idx = self.definition.palette_index_at(secs);
        let next_pal = pal_idx.and_then(|i| self.definition.palette_track.get(i + 1));
        ShowSnapshot {
            loaded: true,
            auto_enabled: self.auto_enabled,
            running: self.clock.transport() == ClockTransport::Running,
            elapsed_ms: elapsed.as_millis() as u64,
            scene: self.scene.active_preset_id.clone(),
            next_scene: next_scene.map(|c| c.preset_id.clone()),
            palette: self.palette.active_palette_id.clone(),
            next_palette: next_pal.map(|c| c.palette_id.clone()),
            blackout,
            validation: self.last_report.summary(),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ShowSnapshot {
    pub loaded: bool,
    pub auto_enabled: bool,
    pub running: bool,
    pub elapsed_ms: u64,
    pub scene: Option<String>,
    pub next_scene: Option<String>,
    pub palette: Option<String>,
    pub next_palette: Option<String>,
    pub blackout: bool,
    pub validation: String,
}

impl ShowSnapshot {
    pub fn empty(blackout: bool) -> Self {
        Self {
            loaded: false,
            auto_enabled: false,
            running: false,
            elapsed_ms: 0,
            scene: None,
            next_scene: None,
            palette: None,
            next_palette: None,
            blackout,
            validation: "NO SHOW".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::show::definition::{PaletteCue, SceneCue, SceneTransition, TransitionKind};

    fn controller() -> ShowController {
        let def = ShowDefinition {
            schema_version: 1,
            id: "test".into(),
            name: "Test".into(),
            duration_secs: 10800,
            scene_track: vec![
                SceneCue {
                    at_secs: 0,
                    preset_id: "A".into(),
                    transition: SceneTransition {
                        kind: TransitionKind::Cut,
                        duration_ms: 0,
                    },
                },
                SceneCue {
                    at_secs: 1200,
                    preset_id: "B".into(),
                    transition: SceneTransition {
                        kind: TransitionKind::Dissolve,
                        duration_ms: 8000,
                    },
                },
            ],
            palette_track: vec![
                PaletteCue {
                    at_secs: 0,
                    palette_id: "earth".into(),
                    transition_ms: 0,
                },
                PaletteCue {
                    at_secs: 3500,
                    palette_id: "deep-blue".into(),
                    transition_ms: 60_000,
                },
            ],
        };
        let mut palettes = HashMap::new();
        palettes.insert(
            "earth".into(),
            Palette::solid_test("earth", "Earth"),
        );
        palettes.insert(
            "deep-blue".into(),
            Palette::solid_test("deep-blue", "Deep Blue"),
        );
        ShowController::new(
            def,
            PathBuf::from("/tmp"),
            palettes,
            HashMap::new(),
            vec!["A".into(), "B".into()],
        )
    }

    #[test]
    fn start_is_cut_zero() {
        let mut c = controller();
        let t0 = Instant::now();
        let events = c.handle(t0, ShowCommand::Start);
        let scene = events.iter().find_map(|e| match e {
            ShowVisualEvent::Scene(s) => Some(s),
            _ => None,
        });
        let scene = scene.expect("scene");
        assert_eq!(scene.preset_id, "A");
        assert_eq!(scene.duration_ms, 0);
        assert!(scene.snap);
    }

    #[test]
    fn stop_auto_does_not_reset_clock() {
        let mut c = controller();
        let t0 = Instant::now();
        c.handle(t0, ShowCommand::Start);
        c.handle(t0 + Duration::from_secs(30), ShowCommand::StopAuto);
        assert!(!c.auto_enabled);
        assert_eq!(
            c.clock.elapsed_at(t0 + Duration::from_secs(30)),
            Duration::from_secs(30)
        );
    }
}
