//! The v2 workspace shell (#3122, GH #32).
//!
//! The v1 layout puts every control into two fixed 315 px side panels drawn
//! over a full-window render. This draws the alternative: a top bar that
//! switches between three workspaces, the output as a preview rather than the
//! backdrop, and each setting at one level of the Preset → Layers → Effect
//! hierarchy.
//!
//! It ships behind `SettingsConfig::classic_layout` for one release, so the
//! old panels stay reachable while this settles. Both layouts call the same
//! per-section draw functions in [`super::panels`] — only the arrangement
//! differs, so a control fixed in one is fixed in both.

use crate::ui::theme::tokens::{MIN_INTERACT_HEIGHT, SMALL_SIZE};
use egui::{Context, Frame, Margin, ScrollArea};

use super::panels::{
    audio_panel, catalog_panel, cue_strip, layer_panel, media_panel, output_window_panel,
    param_panel, postfx_panel, preset_panel, scene_panel, stack_panel, status_bar,
    volumetric_panel,
};
use super::{tour, widgets};
use crate::audio::AudioSystem;
use crate::effect::EffectLoader;
use crate::effect::format::PostProcessDef;
use crate::gpu::ShaderUniforms;
use crate::gpu::layer::LayerInfo;
use crate::gpu::volumetric::VolumetricParams;
use crate::midi::MidiSystem;
use crate::osc::OscSystem;
use crate::params::ParamStore;
use crate::preset::PresetStore;
use crate::settings::SettingsConfig;
use crate::ui::theme::colors::theme_colors;
use crate::web::WebSystem;

/// Which workspace is open. Held in egui data, not in settings: it is where
/// you are right now, not a preference, and a fresh launch should open on
/// Build rather than wherever the last session ended.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Workspace {
    Perform,
    Build,
    Setup,
}

impl Workspace {
    const ALL: [(Workspace, &'static str); 3] = [
        (Workspace::Perform, "Perform"),
        (Workspace::Build, "Build"),
        (Workspace::Setup, "Setup"),
    ];

    pub fn read(ctx: &Context) -> Self {
        ctx.data_mut(|d| match d.get_temp::<u8>(egui::Id::new("v2_workspace")) {
            Some(0) => Workspace::Perform,
            Some(2) => Workspace::Setup,
            // Nothing stored yet, or a value from an older build: open on Build.
            _ => Workspace::Build,
        })
    }

    pub fn write(self, ctx: &Context) {
        let v: u8 = match self {
            Workspace::Perform => 0,
            Workspace::Build => 1,
            Workspace::Setup => 2,
        };
        ctx.data_mut(|d| d.insert_temp(egui::Id::new("v2_workspace"), v));
    }
}

/// Everything the shell draws. One struct rather than thirty arguments: the
/// v1 `draw_panels` takes them positionally and adding a section there means
/// threading another parameter through every call site.
pub struct ShellState<'a> {
    pub audio: &'a mut AudioSystem,
    pub params: &'a mut ParamStore,
    pub shader_error: &'a Option<String>,
    pub uniforms: &'a ShaderUniforms,
    pub effect_loader: &'a EffectLoader,
    pub postprocess: &'a mut PostProcessDef,
    /// Master's post-processing before its last reset, if it has one.
    pub postprocess_previous: Option<&'a PostProcessDef>,
    pub volumetric_enabled: &'a mut bool,
    pub volumetric_params: &'a mut VolumetricParams,
    pub particle_count: Option<u32>,
    pub midi: &'a mut MidiSystem,
    pub osc: &'a mut OscSystem,
    pub web: &'a mut WebSystem,
    pub preset_store: &'a PresetStore,
    pub layers: &'a [LayerInfo],
    pub active_layer: usize,
    pub master_chain: Option<crate::gpu::layer::ChainBadge>,
    pub media_info: Option<media_panel::MediaInfo>,
    pub webcam_info: Option<super::panels::webcam_panel::WebcamInfo>,
    pub particle_info: Option<super::panels::particle_panel::ParticleInfo>,
    /// The active layer's obstacle, lattice and helix settings: each `Some`
    /// only for an effect that has them (#3238).
    pub obstacle_info: Option<super::panels::obstacle_panel::ObstacleInfo>,
    pub lattice_info: Option<super::panels::lattice_panel::LatticeInfo>,
    pub helix_info: Option<super::panels::helix_panel::HelixInfo>,
    pub status_error: &'a Option<(String, std::time::Instant)>,
    pub settings: &'a SettingsConfig,
    /// The layer rows' pictures (#3123). `None` where there is no GPU: the
    /// rows then draw without them.
    pub layer_thumbs: Option<&'a crate::gpu::layer_thumbs::LayerThumbs>,
    /// Per layer: does its effect's own post-processing equal Master's?
    pub postfx_matches: &'a [bool],
    /// The finished frame and its aspect ratio, drawn as the output preview.
    pub display: Option<(egui::TextureId, f32)>,
    /// The catalog's pictures (#3124).
    pub catalog_thumbs: &'a mut crate::ui::catalog_thumbs::CatalogThumbs,
    /// Scenes, the current cue list and where the timeline is (#3173).
    pub scene: &'a scene_panel::SceneInfo,
    /// The bindings, read for what drives each control (#3127). Made and
    /// edited in the binding matrix, which has the bus to itself.
    pub bindings: &'a crate::bindings::bus::BindingBus,
}

/// Draw the workspace shell. Returns without drawing when the overlay is
/// hidden, exactly like the v1 panels — F still gives a clean full-window
/// output with no interface over it.
pub fn draw_shell(ctx: &Context, visible: bool, s: &mut ShellState<'_>) {
    if !visible {
        return;
    }
    let tc = theme_colors(ctx);
    let ws = Workspace::read(ctx);

    // A tour step that needs a layer with controls (#3127).
    if ctx
        .data_mut(|d| d.remove_temp::<bool>(egui::Id::new(tour::SELECT_EFFECT_LAYER)))
        .is_some()
        && let Some(i) = s
            .layers
            .iter()
            .position(|l| l.effect_index.is_some() && !l.is_media)
    {
        stack_panel::select_layer(ctx, i);
    }
    // A Layers tour step: the layer whose blend it explains (#3129).
    if ctx
        .data_mut(|d| d.remove_temp::<bool>(egui::Id::new(tour::SELECT_BLEND_LAYER)))
        .is_some()
    {
        let starter = s
            .effect_loader
            .effects
            .iter()
            .any(|e| e.name == tour::STARTER_EFFECT);
        match tour::blend_layer(s.layers, s.active_layer) {
            Some(tour::BlendLayer::Explain(i)) => stack_panel::select_layer(ctx, i),
            Some(tour::BlendLayer::Start(i)) if starter => tour::start_layer(ctx, i),
            Some(tour::BlendLayer::Start(i)) => stack_panel::select_layer(ctx, i),
            None => {}
        }
    }

    let top = egui::TopBottomPanel::top("v2_top")
        .exact_height(40.0)
        .frame(Frame {
            fill: tc.panel,
            inner_margin: Margin::symmetric(10, 4),
            ..Default::default()
        })
        .show(ctx, |ui| {
            ui.horizontal_centered(|ui| {
                ui.label(egui::RichText::new("fosfora").size(15.0).strong());
                ui.separator();

                let preset_name = s
                    .preset_store
                    .current_preset
                    .and_then(|i| s.preset_store.presets.get(i))
                    .map(|(n, _)| n.as_str())
                    .unwrap_or("Unsaved preset");
                ui.label(egui::RichText::new(preset_name).size(13.0));
                if s.preset_store.dirty {
                    ui.label(
                        egui::RichText::new("edited")
                            .size(SMALL_SIZE)
                            .color(tc.text_secondary),
                    );
                }

                ui.separator();
                for (w, label) in Workspace::ALL {
                    if ui
                        .selectable_label(w == ws, egui::RichText::new(label).size(13.0))
                        .clicked()
                    {
                        w.write(ui.ctx());
                    }
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.menu_button(egui::RichText::new("Tours").size(12.0), |ui| {
                        for &t in crate::ui::tour::Tour::ALL {
                            if ui.button(t.name()).on_hover_text(t.description()).clicked() {
                                crate::ui::tour::start(ui.ctx(), t);
                                ui.close();
                            }
                        }
                    });
                    if ui
                        .button(egui::RichText::new("Full output").size(12.0))
                        .on_hover_text("Hide the interface — Esc or D brings it back")
                        .clicked()
                    {
                        ui.ctx().data_mut(|d| {
                            d.insert_temp(egui::Id::new("request_hide_overlay"), true);
                        });
                    }
                    let bpm = s.uniforms.bpm * 300.0;
                    if bpm > 1.0 {
                        ui.label(
                            egui::RichText::new(format!("{bpm:.0} BPM"))
                                .size(12.0)
                                .color(tc.text_secondary),
                        );
                    }
                });
            });
        });

    let playing = s.scene.timeline.as_ref().filter(|t| t.active);
    let status = egui::TopBottomPanel::bottom("v2_status").show(ctx, |ui| {
        status_bar::draw_status_bar(
            ui,
            s.shader_error,
            s.uniforms,
            s.particle_count,
            s.midi.config.enabled,
            s.midi.is_recently_active(),
            s.osc.config.enabled,
            s.osc.is_recently_active(),
            s.web.config.enabled,
            s.web.client_count,
            false,
            false,
            false,
            false,
            playing.is_some(),
            playing.map(|t| (t.current_cue, t.cue_count)),
            s.status_error,
            None,
            s.audio.indicator(),
        );
    });

    // Each returns where its output column starts, if it has one.
    let output_left = match ws {
        Workspace::Build => Some(build_workspace(ctx, s, tc.panel)),
        Workspace::Perform => {
            perform_workspace(ctx, s, tc.panel);
            None
        }
        Workspace::Setup => Some(setup_workspace(ctx, s, tc.panel)),
    };
    // The modals cover everything left of the output column, between the
    // bars, so the output stays in view under them.
    if let Some(right) = output_left {
        crate::ui::modal::set_bounds(
            ctx,
            egui::Rect::from_min_max(
                egui::pos2(ctx.content_rect().left(), top.response.rect.bottom()),
                egui::pos2(right, status.response.rect.top()),
            ),
        );
    }
}

/// The output: the picture, and under it the control that sends it to a second
/// display.
///
/// One function rather than a section per workspace, because the two belong
/// together — the picker first went only into Setup, and from Build or Perform
/// there was no sign the feature existed.
fn output_section(ui: &mut egui::Ui, id: &str, display: Option<(egui::TextureId, f32)>) {
    let ow = output_window_panel::read(ui.ctx()).unwrap_or_default();
    let badge = ow.open_on.is_some().then_some("2nd window");
    widgets::section(ui, id, "Output", badge, true, |ui| {
        let r = ui.scope(|ui| preview(ui, display));
        tour::anchor(ui, tour::Anchor::Output, r.response.rect);
        ui.add_space(8.0);
        output_window_panel::draw(ui, &ow);
    });
}

/// The output as a picture, sized to the width it is given. Until the first
/// frame registers it, an empty frame of the same size, so nothing jumps
/// when it arrives.
fn preview(ui: &mut egui::Ui, display: Option<(egui::TextureId, f32)>) {
    let w = ui.available_width();
    let aspect = display.map_or(16.0 / 9.0, |(_, a)| a);
    let size = egui::vec2(w, (w / aspect.max(0.01)).round());
    match display {
        Some((tex, _)) => {
            ui.add(egui::Image::new(egui::load::SizedTexture::new(tex, size)).corner_radius(3.0));
        }
        None => {
            let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
            ui.painter().rect_filled(rect, 3.0, egui::Color32::BLACK);
        }
    }
}

fn panel_frame(fill: egui::Color32) -> Frame {
    Frame {
        fill,
        inner_margin: Margin::same(8),
        ..Default::default()
    }
}

/// How wide the side columns are for a window `w` points wide.
///
/// Fixed floors (600 left, 420 right) left a ~1200 px window with 150 px for
/// the inspector between them. The sides now take a share of the window and
/// give way first, so the middle keeps at least [`MIDDLE_MIN`].
#[derive(Debug, Clone, Copy, PartialEq)]
struct Columns {
    left_default: f32,
    left_range: (f32, f32),
    right: f32,
}

/// The least the middle column is left with, whatever the sides want.
const MIDDLE_MIN: f32 = 380.0;

fn columns(w: f32) -> Columns {
    let right = (w * 0.24).clamp(260.0, 420.0);
    let left_max = (w - right - MIDDLE_MIN).clamp(280.0, 800.0);
    let left_min = 280.0_f32.min(left_max);
    Columns {
        left_default: (w * 0.36).clamp(left_min, left_max),
        left_range: (left_min, left_max),
        right,
    }
}

/// Build: the structure on the left, the selection's controls in the middle,
/// the output and its audio on the right, and the catalog along the bottom.
fn build_workspace(ctx: &Context, s: &mut ShellState<'_>, fill: egui::Color32) -> f32 {
    let cols = columns(ctx.content_rect().width());
    // First, so it runs the full width under all three columns.
    bottom_drawer(ctx, s, fill, Workspace::Build);

    // Resizable inside a range rather than fixed, since the right size
    // depends on the window.
    egui::SidePanel::left("v2_structure")
        .default_width(cols.left_default)
        .width_range(cols.left_range.0..=cols.left_range.1)
        .resizable(true)
        .frame(panel_frame(fill))
        .show(ctx, |ui| {
            ScrollArea::vertical().show(ui, |ui| {
                let layer_badge = format!("{}/{}", s.layers.len(), 8);
                widgets::section(ui, "v2_layers", "Stack", Some(&layer_badge), true, |ui| {
                    let pics = stack_panel::StackPictures {
                        thumbs: s.layer_thumbs,
                        aspect: s.display.map_or(16.0 / 9.0, |(_, a)| a),
                    };
                    let r = ui.scope(|ui| {
                        stack_panel::draw_stack(
                            ui,
                            s.layers,
                            s.active_layer,
                            s.master_chain,
                            &postfx_on(s.postprocess),
                            &pics,
                        );
                    });
                    tour::anchor(ui, tour::Anchor::Stack, r.response.rect);
                });
                // Closed to start: in Build the stack is what this column is
                // for.
                preset_panel::draw_preset_section_open(ui, s.preset_store, false);
            });
        });

    let output = egui::SidePanel::right("v2_output")
        .exact_width(cols.right)
        .resizable(false)
        .frame(panel_frame(fill))
        .show(ctx, |ui| {
            // The output stays put; only Audio scrolls under it — the preview
            // is what you watch while scrolling the meters.
            output_section(ui, "v2_preview", s.display);
            ScrollArea::vertical().show(ui, |ui| {
                let bpm = s.uniforms.bpm * 300.0;
                let bpm_badge = (bpm > 1.0).then(|| format!("{bpm:.0}"));
                widgets::section(ui, "v2_audio", "Audio", bpm_badge.as_deref(), true, |ui| {
                    audio_panel::draw_audio_panel(ui, s.audio, s.uniforms);
                });
            });
        });

    egui::CentralPanel::default()
        .frame(panel_frame(fill))
        .show(ctx, |ui| {
            tour::anchor(ui, tour::Anchor::Inspector, ui.max_rect());
            ScrollArea::vertical().show(ui, |ui| {
                inspector_width(ui);
                if stack_panel::master_selected(ui.ctx()) {
                    master_inspector(ui, s);
                } else {
                    layer_inspector(ui, s);
                }
            });
        });
    output.response.rect.left()
}

/// What the bottom drawer can show.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum DrawerTab {
    Catalog,
    Scenes,
}

impl DrawerTab {
    fn label(self) -> &'static str {
        match self {
            DrawerTab::Catalog => "CATALOG",
            DrawerTab::Scenes => "SCENES",
        }
    }

    /// The tabs a workspace's drawer has. Perform plays scenes but loads
    /// no effects, so it has no catalog.
    fn for_workspace(ws: Workspace) -> &'static [DrawerTab] {
        match ws {
            Workspace::Perform => &[DrawerTab::Scenes],
            _ => &[DrawerTab::Catalog, DrawerTab::Scenes],
        }
    }
}

/// The drawer along the bottom: the catalog and the scenes as tabs in Build
/// (#3124, #3173), the scenes alone in Perform. Its height is the user's to
/// drag; closed, it is one line that says how to open it. Each workspace
/// keeps its own open state, height and tab.
fn bottom_drawer(ctx: &Context, s: &mut ShellState<'_>, fill: egui::Color32, ws: Workspace) {
    let tc = theme_colors(ctx);
    let key = match ws {
        Workspace::Perform => "perform",
        _ => "build",
    };
    let open_id = egui::Id::new("v2_drawer_open").with(key);
    let tab_id = egui::Id::new("v2_drawer_tab").with(key);
    let tabs = DrawerTab::for_workspace(ws);
    let open = ctx.data(|d| d.get_temp::<bool>(open_id)).unwrap_or(true);
    let tab = ctx
        .data(|d| d.get_temp::<DrawerTab>(tab_id))
        .filter(|t| tabs.contains(t))
        .unwrap_or(tabs[0]);
    let visible = s.effect_loader.effects.iter().filter(|e| !e.hidden).count();
    let touring = tour::is_running(ctx);
    let scene = s.scene;
    let playing = scene.timeline.as_ref().filter(|t| t.active);

    // The whole bar toggles, and when closed that is the whole panel. A tab
    // takes its own click: it shows that tab, opening the drawer if needed.
    let header = |ui: &mut egui::Ui| {
        let h = if open {
            MIN_INTERACT_HEIGHT
        } else {
            ui.available_height()
        };
        let mut picked = None;
        let r = widgets::header_row(ui, h, |ui| {
            widgets::draw_section_arrow(ui, open, tc.text_secondary);
            if tabs.len() == 1 {
                ui.label(
                    egui::RichText::new(tab.label())
                        .size(12.0)
                        .strong()
                        .color(tc.text_secondary),
                );
            } else {
                // A tour step points at one tab; the others stay shut.
                for &t in tabs {
                    let text = egui::RichText::new(t.label()).size(12.0).strong();
                    let on = open && t == tab;
                    if ui
                        .add_enabled(on || !touring, egui::Button::selectable(on, text))
                        .on_disabled_hover_text(tour::NOT_DURING)
                        .clicked()
                    {
                        picked = Some(t);
                    }
                }
            }
            ui.add_space(4.0);
            // What the drawer holds, and whether a scene is playing: said
            // whichever tab is showing, so a running cue list is never out
            // of sight.
            let count = match tab {
                DrawerTab::Catalog => format!("{visible} effects"),
                DrawerTab::Scenes => match scene.scene_store_names.len() {
                    1 => "1 scene".to_string(),
                    n => format!("{n} scenes"),
                },
            };
            ui.label(
                egui::RichText::new(count)
                    .size(12.0)
                    .color(tc.text_secondary),
            );
            if let Some(t) = playing {
                ui.add_space(8.0);
                widgets::paint_mark(ui, widgets::Mark::Active);
                let cue = scene
                    .cue_list
                    .get(t.current_cue)
                    .map_or("", |c| c.preset_name.as_str());
                ui.label(
                    egui::RichText::new(format!(
                        "Playing cue {} of {}: {cue}",
                        t.current_cue + 1,
                        t.cue_count
                    ))
                    .size(12.0),
                );
            }
            ui.add_space(12.0);
            let hint = match (open, tab) {
                (false, _) => "Click to open",
                (true, DrawerTab::Catalog) => {
                    "Click a picture to load it into the selected layer, or drag it onto the stack."
                }
                (true, DrawerTab::Scenes) => "Space goes to the next cue. T starts and stops.",
            };
            ui.label(
                egui::RichText::new(hint)
                    .size(12.0)
                    .color(tc.text_secondary),
            );
        });
        if let Some(t) = picked {
            ui.ctx().data_mut(|d| {
                d.insert_temp(tab_id, t);
                d.insert_temp(open_id, true);
            });
        } else if r.clicked() && !touring {
            ui.ctx().data_mut(|d| d.insert_temp(open_id, !open));
        }
    };

    let frame = Frame {
        fill,
        inner_margin: Margin::symmetric(10, 6),
        ..Default::default()
    };
    if !open {
        egui::TopBottomPanel::bottom(egui::Id::new("v2_drawer_closed").with(key))
            .exact_height(30.0)
            .frame(frame)
            .show(ctx, header);
        return;
    }
    let screen_h = ctx.content_rect().height();
    drawer_panel(key, screen_h).frame(frame).show(ctx, |ui| {
        drawer_body(ui, |ui| {
            header(ui);
            ui.add_space(4.0);
            match tab {
                DrawerTab::Catalog => {
                    tour::anchor(ui, tour::Anchor::Catalog, ui.max_rect());
                    let active = s.layers.get(s.active_layer);
                    let target = catalog_panel::Target {
                        current: active.and_then(|l| l.effect_index),
                        locked: active.is_some_and(|l| l.locked),
                        can_add: s.layers.len() < crate::bindings::catalog::MAX_LAYERS,
                        active: s.active_layer,
                    };
                    catalog_panel::draw_catalog(
                        ui,
                        s.effect_loader,
                        &s.settings.favorite_effects,
                        s.catalog_thumbs,
                        &target,
                    );
                }
                DrawerTab::Scenes => {
                    let (loader, thumbs) = (s.effect_loader, &mut *s.catalog_thumbs);
                    let mut pics = |ctx: &egui::Context, name: &str| {
                        let e = loader.effects.iter().find(|e| e.name == name)?;
                        thumbs.get(ctx, e)
                    };
                    cue_strip::draw(ui, scene, &mut pics);
                }
            }
        });
    });
}

/// Open Build's drawer on the catalog.
pub(crate) fn show_catalog(ctx: &Context) {
    ctx.data_mut(|d| {
        d.insert_temp(
            egui::Id::new("v2_drawer_tab").with("build"),
            DrawerTab::Catalog,
        );
        d.insert_temp(egui::Id::new("v2_drawer_open").with("build"), true);
    });
}

/// The drawer's panel: `key` names the workspace it belongs to, so each
/// keeps its own height.
fn drawer_panel(key: &str, screen_h: f32) -> egui::TopBottomPanel {
    egui::TopBottomPanel::bottom(egui::Id::new("v2_drawer").with(key))
        .default_height(drawer_default_height(screen_h))
        .height_range(drawer_min_height(screen_h)..=drawer_max_height(screen_h))
        .resizable(true)
}

/// Draw the drawer's contents in exactly the drawer's space. A resizable
/// egui panel keeps the height its contents USED last frame: the catalog
/// filled it and the Scenes tab didn't, so each tab switch resized the
/// drawer, and a scene loaded into a short one was cut off until the next
/// switch. The contents go in a child of the drawer's own size, so neither
/// a short tab nor a tall one changes it; only a drag does.
fn drawer_body(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    let rect = ui.available_rect_before_wrap();
    let mut body = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    body.set_clip_rect(rect.intersect(ui.clip_rect()));
    add(&mut body);
    ui.advance_cursor_after_rect(rect);
}

/// Shortest the drawer can be dragged: one whole row of the catalog with
/// its toolbar and footer. Any shorter and "+ New effect" scrolled away
/// under the grid.
fn drawer_min_height(screen_h: f32) -> f32 {
    (DRAWER_CHROME + catalog_panel::height_for_rows(1)).min(drawer_max_height(screen_h))
}

/// Tallest the drawer can be dragged in a window `screen_h` tall.
fn drawer_max_height(screen_h: f32) -> f32 {
    (screen_h * 0.6).max(200.0)
}

/// The drawer's frame margins and header, above what a tab draws.
const DRAWER_CHROME: f32 = 12.0 + MIN_INTERACT_HEIGHT + 4.0;

/// The drawer's height until someone drags it: a quarter of the window, and
/// never less than three quarters of what two whole catalog rows need, so
/// the second row shows its top edge. At one row and nothing more, nothing
/// said there were more below; at a third of the window (Kevin, #3173) it
/// was a quarter too tall.
fn drawer_default_height(screen_h: f32) -> f32 {
    let two_rows = DRAWER_CHROME + catalog_panel::height_for_rows(2);
    (0.75 * (screen_h / 3.0).max(two_rows))
        .clamp(drawer_min_height(screen_h), drawer_max_height(screen_h))
}

/// Widest the inspector's content gets, however wide its column is.
const INSPECTOR_MAX_WIDTH: f32 = 720.0;

/// Hold the inspector to a reading width: at full width a description ran to
/// ~130 characters a line and every slider stretched across 600 px. `min()`
/// because a bare `set_max_width` inside a ScrollArea acts as a MINIMUM too —
/// the content stayed 720 px however narrow the column got, and clipped every
/// row's right-hand buttons.
fn inspector_width(ui: &mut egui::Ui) {
    ui.set_max_width(ui.available_width().min(INSPECTOR_MAX_WIDTH));
}

/// The post-processing stages switched on, in words, for Master's row.
fn postfx_on(pp: &PostProcessDef) -> Vec<&'static str> {
    if !pp.enabled {
        return Vec::new();
    }
    [
        (pp.bloom_enabled, "bloom"),
        (pp.ca_enabled, "chromatic aberration"),
        (pp.vignette_enabled, "vignette"),
        (pp.grain_enabled, "grain"),
    ]
    .into_iter()
    .filter_map(|(on, name)| on.then_some(name))
    .collect()
}

/// The inspector's heading: what scope, what it is called, what it does.
fn inspector_heading(ui: &mut egui::Ui, kicker: &str, name: &str, desc: &str) {
    let tc = theme_colors(ui.ctx());
    ui.label(
        egui::RichText::new(kicker)
            .size(12.0)
            .color(tc.text_secondary),
    );
    ui.label(egui::RichText::new(name).size(20.0).strong());
    if !desc.is_empty() {
        ui.label(
            egui::RichText::new(desc)
                .size(13.0)
                .color(tc.text_secondary),
        );
    }
    ui.add_space(8.0);
}

/// A chain's state and the way into the canvas, for either scope.
fn chain_line(
    ui: &mut egui::Ui,
    badge: Option<crate::gpu::layer::ChainBadge>,
    empty: &str,
    open: impl FnOnce(&egui::Context),
) {
    let tc = theme_colors(ui.ctx());
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Chain").size(13.0).strong());
        let words = match badge {
            None => empty.to_string(),
            Some(b) => {
                let nodes = match b.nodes {
                    1 => "1 node".to_string(),
                    n => format!("{n} nodes"),
                };
                if b.active {
                    format!("{nodes}, reaches Output")
                } else {
                    format!("{nodes}, inactive: nothing reaches Output yet")
                }
            }
        };
        ui.label(
            egui::RichText::new(words)
                .size(13.0)
                .color(tc.text_secondary),
        );
        let touring = tour::is_running(ui.ctx());
        if ui
            .add_enabled(
                !touring,
                egui::Button::new(egui::RichText::new("Edit chain  (C)").size(12.0)),
            )
            .on_disabled_hover_text(tour::NOT_DURING)
            .clicked()
        {
            open(ui.ctx());
        }
    });
}

/// Layer scope: everything that belongs to the selected layer, and nothing
/// that belongs to the preset as a whole.
fn layer_inspector(ui: &mut egui::Ui, s: &mut ShellState<'_>) {
    let Some(layer) = s.layers.get(s.active_layer) else {
        inspector_heading(ui, "Layer", "No layer", "");
        return;
    };
    let kind = stack_panel::layer_kind(layer);
    let kicker = format!(
        "Layer {} of {} · {kind}",
        s.active_layer + 1,
        s.layers.len()
    );
    let desc = if layer.is_media {
        layer.media_file_name.clone().unwrap_or_default()
    } else if layer.effect_index.is_none() {
        "Nothing yet: click a picture in the catalog to load an effect here.".to_string()
    } else {
        layer
            .effect_index
            .and_then(|i| s.effect_loader.effects.get(i))
            .map(|e| e.description.clone())
            .unwrap_or_default()
    };
    inspector_heading(ui, &kicker, stack_panel::layer_name(layer), &desc);

    let bottom = s.layers.iter().rposition(|l| l.enabled) == Some(s.active_layer);
    let r = ui.scope(|ui| {
        widgets::section(ui, "v2_blend", "Blend", None, true, |ui| {
            blend_controls(ui, layer, s.active_layer, bottom);
        });
    });
    tour::anchor(ui, tour::Anchor::Blend, r.response.rect);

    if let Some(ref info) = s.webcam_info {
        widgets::section(ui, "v2_webcam", "Camera", None, true, |ui| {
            super::panels::webcam_panel::draw_webcam_panel(ui, info);
        });
    } else if let Some(ref info) = s.media_info {
        widgets::section(ui, "v2_media", "Media", None, true, |ui| {
            media_panel::draw_media_panel(ui, info);
        });
    } else {
        let binds = param_panel::ParamBinds {
            bus: s.bindings,
            layer: s.active_layer,
            effect: layer.effect_name.as_deref().unwrap_or(""),
        };
        let r = ui.scope(|ui| {
            widgets::section(ui, "v2_params", "Parameters", None, true, |ui| {
                param_panel::draw_param_panel(ui, s.params, s.midi, s.osc, Some(&binds));
            });
        });
        tour::anchor(ui, tour::Anchor::Parameters, r.response.rect);
        bindings_line(ui, s.bindings, s.active_layer);
        if let Some(ref pinfo) = s.particle_info {
            let badge = if pinfo.alive_count >= 1000 {
                format!("{:.1}K", pinfo.alive_count as f32 / 1000.0)
            } else {
                format!("{}", pinfo.alive_count)
            };
            widgets::section(ui, "v2_particles", "Particles", Some(&badge), false, |ui| {
                super::panels::particle_panel::draw_particle_panel(ui, pinfo);
            });
        }
        effect_sections(ui, s, layer);
    }

    ui.add_space(6.0);
    let i = s.active_layer;
    let r = ui.scope(|ui| {
        chain_line(ui, layer.chain, "none yet", |ctx| {
            ctx.data_mut(|d| d.insert_temp(egui::Id::new("open_trama_on_layer"), i));
        });
    });
    tour::anchor(ui, tour::Anchor::ChainLine, r.response.rect);
}

/// The sections only some effects have, in the order the Classic panel
/// draws them: Obstacle (particle effects), Lattice, Helix, and the
/// effect's own audio mappings (#3238).
fn effect_sections(ui: &mut egui::Ui, s: &ShellState<'_>, layer: &crate::gpu::layer::LayerInfo) {
    use super::panels::{audio_mappings_panel, helix_panel, lattice_panel, obstacle_panel};
    if let Some(o) = s.obstacle_info.as_ref().filter(|o| o.has_particles) {
        let badge = o.enabled.then_some("ON");
        widgets::section(ui, "v2_obstacle", "Obstacle", badge, false, |ui| {
            obstacle_panel::draw_obstacle_panel(ui, o);
        });
    }
    if let Some(l) = &s.lattice_info {
        widgets::section(ui, "v2_lattice", "Lattice (3D CA)", None, true, |ui| {
            lattice_panel::draw_lattice_panel(ui, l);
        });
    }
    if let Some(h) = &s.helix_info {
        widgets::section(ui, "v2_helix", "Helix (audio ribbon)", None, true, |ui| {
            helix_panel::draw_helix_panel(ui, h);
        });
    }
    let mappings = layer
        .effect_index
        .and_then(|i| s.effect_loader.effects.get(i))
        .map_or(&[][..], |fx| fx.audio_mappings.as_slice());
    if !mappings.is_empty() {
        let badge = mappings.len().to_string();
        widgets::section(
            ui,
            "v2_audio_react",
            "Audio Reactivity",
            Some(&badge),
            false,
            |ui| {
                audio_mappings_panel::draw_audio_mappings(ui, mappings);
            },
        );
    }
}

/// How many bindings drive this layer, and the way into the matrix: under
/// the Parameters, where someone looking for "make this move" is looking
/// (#3127, Kevin's call in #3209).
fn bindings_line(ui: &mut egui::Ui, bus: &crate::bindings::bus::BindingBus, layer: usize) {
    let tc = theme_colors(ui.ctx());
    let here = bus
        .bindings
        .iter()
        .filter(|b| b.enabled && !b.source.is_empty() && b.target.layer() == Some(layer))
        .count();
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Bindings").size(13.0).strong());
        let words = match here {
            0 => "none on this layer".to_string(),
            1 => "1 on this layer".to_string(),
            n => format!("{n} on this layer"),
        };
        ui.label(
            egui::RichText::new(words)
                .size(13.0)
                .color(tc.text_secondary),
        );
        let touring = tour::is_running(ui.ctx());
        if ui
            .add_enabled(
                !touring,
                egui::Button::new(egui::RichText::new("Open bindings  (B)").size(12.0)),
            )
            .on_disabled_hover_text(tour::NOT_DURING)
            .clicked()
        {
            ui.ctx()
                .data_mut(|d| d.insert_temp(egui::Id::new("open_binding_matrix"), true));
        }
    });
}

/// Blend mode, visibility and opacity: how the layer lands on the stack.
fn blend_controls(ui: &mut egui::Ui, layer: &crate::gpu::layer::LayerInfo, i: usize, bottom: bool) {
    use crate::gpu::layer::BlendMode;
    let tc = theme_colors(ui.ctx());
    ui.add_enabled_ui(!layer.locked, |ui| {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Mode").size(13.0));
            egui::ComboBox::from_id_salt("v2_blend_mode")
                .selected_text(layer.blend_mode.display_name())
                .width(180.0)
                .show_ui(ui, |ui| {
                    for &mode in BlendMode::ALL {
                        // The displacement family warps instead of coloring.
                        if mode == BlendMode::DISPLACEMENT[0] {
                            ui.separator();
                        }
                        let r = ui
                            .selectable_label(mode == layer.blend_mode, mode.display_name())
                            .on_hover_text(mode.description());
                        if r.clicked() && mode != layer.blend_mode {
                            ui.ctx().data_mut(|d| {
                                d.insert_temp(egui::Id::new("layer_blend"), mode.as_u32());
                            });
                        }
                    }
                });
            ui.add_space(12.0);
            let mut visible = layer.enabled;
            if ui.checkbox(&mut visible, "Visible").changed() {
                ui.ctx().data_mut(|d| {
                    d.insert_temp(egui::Id::new("layer_toggle_enable"), (i, visible));
                });
            }
        });
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Opacity").size(13.0));
            let mut opacity = layer.opacity;
            let r = ui.add(
                egui::Slider::new(&mut opacity, 0.0..=1.0)
                    .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
            );
            if r.changed() {
                ui.ctx()
                    .data_mut(|d| d.insert_temp(egui::Id::new("layer_opacity"), opacity));
            }
        });
        if layer.blend_mode.is_displacement() {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Displace").size(13.0));
                let mut amount = layer.displace_amount;
                let r = ui.add(
                    egui::Slider::new(&mut amount, 0.0..=1.0)
                        .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
                );
                if r.changed() {
                    ui.ctx()
                        .data_mut(|d| d.insert_temp(egui::Id::new("layer_displace"), amount));
                }
            });
        }
    });
    let note = if layer.locked {
        Some("This layer is locked. Unlock it from its row's right-click menu.")
    } else if bottom {
        Some(
            "The bottom layer has nothing beneath it, so its blend mode is not applied; \
             only its opacity is.",
        )
    } else {
        None
    };
    if let Some(note) = note {
        ui.label(
            egui::RichText::new(note)
                .size(12.0)
                .color(tc.text_secondary),
        );
    }
}

/// Where Master's post-processing came from, and the way back to an effect's
/// recommended look. Post-processing belongs to Master alone; what an effect
/// contributes is the block its author wrote into its `.pfx` — adopted by
/// itself when the effect is the only layer, and offered here otherwise.
fn postfx_source(ui: &mut egui::Ui, s: &ShellState<'_>) {
    let tc = theme_colors(ui.ctx());
    // One entry per effect on the stack, once each, with whether it
    // recommends a look of its own and whether Master already matches it.
    let mut effects: Vec<(usize, &str, bool, bool)> = Vec::new();
    for (i, l) in s.layers.iter().enumerate() {
        let Some(fx) = l.effect_index.and_then(|e| s.effect_loader.effects.get(e)) else {
            continue;
        };
        if l.is_media || effects.iter().any(|e| e.1 == fx.name) {
            continue;
        }
        let recommends = fx.postprocess.is_some();
        let matches = recommends && s.postfx_matches.get(i).copied().unwrap_or(false);
        effects.push((i, fx.name.as_str(), recommends, matches));
    }
    let source = match effects.iter().find(|e| e.3) {
        Some((_, name, _, _)) => format!("Matches {name}'s recommended look."),
        None => "This preset's own settings.".to_string(),
    };
    ui.label(
        egui::RichText::new(format!("{source} Selecting a layer never changes them."))
            .size(12.0)
            .color(tc.text_secondary),
    );

    // Every way back is always shown, so none seems to vanish; one that
    // would change nothing is greyed out and says why.
    let previous = s.postprocess_previous.filter(|p| *p != &*s.postprocess);
    let at_defaults = *s.postprocess == PostProcessDef::default();
    let send = |ui: &egui::Ui, key: &str| {
        ui.ctx()
            .data_mut(|d| d.insert_temp(egui::Id::new(key), true));
    };
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new("Reset to:").size(12.0));
        let b = ui.add_enabled(
            previous.is_some(),
            egui::Button::new("Previous settings").small(),
        );
        let b = if previous.is_some() {
            b.on_hover_text(
                "The settings Master had before its last reset. Press again to come \
                 back to these.",
            )
        } else {
            b.on_disabled_hover_text("Nothing to go back to yet: this is here after a reset.")
        };
        if b.clicked() {
            send(ui, "postprocess_to_previous");
        }

        let b = ui
            .add_enabled(!at_defaults, egui::Button::new("Defaults").small())
            .on_hover_text("Post-processing's built-in settings")
            .on_disabled_hover_text("These are already the built-in settings.");
        if b.clicked() {
            send(ui, "postprocess_to_defaults");
        }

        for &(i, name, recommends, matches) in &effects {
            let b = ui
                .add_enabled(recommends && !matches, egui::Button::new(name).small())
                .on_hover_text(format!(
                    "{name}'s effect file recommends its own post-processing. Copy it \
                     to Master, replacing the settings below."
                ))
                .on_disabled_hover_text(if matches {
                    format!("Master already has {name}'s recommended look.")
                } else {
                    format!("{name} recommends no post-processing of its own.")
                });
            if b.clicked() {
                ui.ctx().data_mut(|d| {
                    d.insert_temp(egui::Id::new("adopt_layer_postprocess"), i);
                });
            }
        }
    });
}

/// Master scope: what runs on the finished blend, and is saved with the
/// preset rather than with any one layer.
fn master_inspector(ui: &mut egui::Ui, s: &mut ShellState<'_>) {
    inspector_heading(
        ui,
        "Master · runs on the blended stack",
        "Master",
        "Post-processing and the master chain apply after every layer is blended. \
         Master's row at the top of the stack shows the result.",
    );
    ui.add_space(6.0);
    let r = ui.scope(|ui| {
        widgets::section(ui, "v2_postfx", "Post-Processing", None, true, |ui| {
            postfx_source(ui, s);
            ui.add_space(4.0);
            postfx_panel::draw_postfx_panel(ui, s.postprocess);
        });
    });
    tour::anchor(ui, tour::Anchor::PostProcessing, r.response.rect);
    widgets::section(ui, "v2_volumetric", "Volumetric (R3)", None, true, |ui| {
        volumetric_panel::draw_volumetric_panel(ui, s.volumetric_enabled, s.volumetric_params);
    });
    ui.add_space(6.0);
    chain_line(
        ui,
        s.master_chain,
        "empty: passes the blend through",
        |ctx| {
            ctx.data_mut(|d| d.insert_temp(egui::Id::new("open_trama_on_master"), true));
        },
    );
}

/// Perform: presets on the left, the output large in the middle, the scenes
/// along the bottom.
fn perform_workspace(ctx: &Context, s: &mut ShellState<'_>, fill: egui::Color32) {
    let cols = columns(ctx.content_rect().width());
    // First, so it runs the full width under both columns.
    bottom_drawer(ctx, s, fill, Workspace::Perform);
    egui::SidePanel::left("v2_presets")
        .default_width(cols.left_default)
        .width_range(cols.left_range.0..=cols.left_range.1)
        .resizable(true)
        .frame(panel_frame(fill))
        .show(ctx, |ui| {
            ScrollArea::vertical().show(ui, |ui| {
                preset_panel::draw_preset_section(ui, s.preset_store);
                let layer_badge = format!("{}/{}", s.layers.len(), 8);
                widgets::section(
                    ui,
                    "v2_perform_layers",
                    "Layers",
                    Some(&layer_badge),
                    true,
                    |ui| {
                        layer_panel::draw_layer_panel(ui, s.layers, s.active_layer, s.master_chain);
                    },
                );
            });
        });

    egui::CentralPanel::default()
        .frame(panel_frame(fill))
        .show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                let w = (ui.available_width() - 20.0).max(160.0);
                if let Some((tex, aspect)) = s.display {
                    let h = (w / aspect.max(0.01)).round();
                    let h = h.min(ui.available_height() - 60.0).max(90.0);
                    let size = egui::vec2((h * aspect).round(), h);
                    ui.add(
                        egui::Image::new(egui::load::SizedTexture::new(tex, size))
                            .corner_radius(3.0),
                    );
                }
                ui.add_space(8.0);
                let bpm = s.uniforms.bpm * 300.0;
                if bpm > 1.0 {
                    ui.label(egui::RichText::new(format!("{bpm:.0} BPM")).size(14.0));
                }
                // Sending the output to a second display belongs with the
                // output wherever it is shown — including here, where there is
                // no section to put it in.
                ui.add_space(10.0);
                ui.scope(|ui| {
                    ui.set_max_width(360.0);
                    let ow = output_window_panel::read(ui.ctx()).unwrap_or_default();
                    output_window_panel::draw(ui, &ow);
                });
            });
        });
}

/// Setup: a list of pages and one page at a time ([`super::setup`]), with
/// the output on the right.
fn setup_workspace(ctx: &Context, s: &mut ShellState<'_>, fill: egui::Color32) -> f32 {
    // On the right at Build's width, so the output is in the same place in
    // every workspace.
    let cols = columns(ctx.content_rect().width());
    let output = egui::SidePanel::right("v2_setup_preview")
        .exact_width(cols.right)
        .resizable(false)
        .frame(panel_frame(fill))
        .show(ctx, |ui| {
            output_section(ui, "v2_setup_out", s.display);
        });
    super::setup::draw(ctx, s, fill);
    output.response.rect.left()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lay a parameter-style row out in the inspector's frame, in a window
    /// `w` wide. Returns (how far the row's right-hand button reaches past the
    /// column, how wide the row is).
    fn row_extent(w: f32) -> (f32, f32) {
        let ctx = Context::default();
        crate::ui::theme::colors::set_theme_colors(
            &ctx,
            crate::ui::theme::palette::Palette::GRAY.colors(),
        );
        let mut out = (0.0, 0.0);
        // A few passes: egui settles sizes over frames.
        for _ in 0..3 {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(w, 900.0),
                )),
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default()
                    .frame(panel_frame(egui::Color32::BLACK))
                    .show(ctx, |ui| {
                        let edge = ui.max_rect().right();
                        ScrollArea::vertical().show(ui, |ui| {
                            inspector_width(ui);
                            let mut v = 0.5f32;
                            let row = ui.horizontal(|ui| {
                                ui.label("splat_force");
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        let o = ui.button("O");
                                        ui.spacing_mut().slider_width = ui.available_width();
                                        ui.add(egui::Slider::new(&mut v, 0.0..=1.0));
                                        o.rect.right()
                                    },
                                )
                                .inner
                            });
                            out = (row.inner - edge, row.response.rect.width());
                        });
                    });
            });
        }
        out
    }

    /// Run `draw` in a panel `size` large for a few frames; returns the
    /// height of what it drew.
    fn drawn_height(size: egui::Vec2, mut draw: impl FnMut(&mut egui::Ui)) -> f32 {
        let ctx = Context::default();
        crate::ui::theme::colors::set_theme_colors(
            &ctx,
            crate::ui::theme::palette::Palette::GRAY.colors(),
        );
        let mut h = 0.0;
        for _ in 0..3 {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default()
                    .frame(panel_frame(egui::Color32::BLACK))
                    .show(ctx, |ui| {
                        h = ui.scope(|ui| draw(ui)).response.rect.height();
                    });
            });
        }
        h
    }

    // The Scenes tab fills its drawer and no more, whatever the drawer's
    // height: the scene list and the strip take the height they are given,
    // and scroll past it. A row laid out with a bare `with_layout` takes all
    // the height below it; that bit the section headers and the preset rows
    // in #3125, and a click test passed both times.
    #[test]
    fn the_scenes_tab_stays_inside_its_drawer() {
        for h in [160.0, 300.0, 600.0] {
            for playing in [false, true] {
                let info = cue_strip::sample(playing);
                let drawn = drawn_height(egui::vec2(1400.0, h), |ui| {
                    cue_strip::draw(ui, &info, &mut |_, _| None);
                });
                assert!(
                    drawn <= h,
                    "a {h} px drawer drew {drawn} px (playing {playing})"
                );
            }
        }
    }

    /// The drawer's height after each frame, drawing `body_h` points of
    /// content per frame in turn.
    fn drawer_heights(body_h: &[f32], fill: bool) -> Vec<f32> {
        let ctx = Context::default();
        body_h
            .iter()
            .map(|&h| {
                let input = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1400.0, 1000.0),
                    )),
                    ..Default::default()
                };
                let mut got = 0.0;
                let _ = ctx.run(input, |ctx| {
                    got = drawer_panel("t", 1000.0)
                        .show(ctx, |ui| {
                            let body = |ui: &mut egui::Ui| {
                                ui.allocate_space(egui::vec2(10.0, h));
                            };
                            if fill {
                                drawer_body(ui, body);
                            } else {
                                body(ui);
                            }
                        })
                        .response
                        .rect
                        .height();
                });
                got
            })
            .collect()
    }

    // Switching between a tab that fills the drawer and one that doesn't
    // resized it every time, and a scene loaded into the shrunken drawer
    // was cut off. The height is the user's alone.
    #[test]
    fn the_drawer_keeps_its_height_whatever_a_tab_draws() {
        let tabs = [60.0, 60.0, 900.0, 60.0, 300.0, 60.0];
        let heights = drawer_heights(&tabs, true);
        let want = drawer_default_height(1000.0);
        for (i, h) in heights.iter().enumerate() {
            assert!(
                (h - want).abs() < 1.0,
                "frame {i}: the drawer is {h}, not {want}"
            );
        }
        // Drawn straight into the panel, a short tab shrinks the drawer and
        // a tall one grows it.
        let loose = drawer_heights(&tabs, false);
        assert!(loose[1] < want - 20.0, "{loose:?}");
        assert!(loose[2] > want + 50.0, "{loose:?}");
    }

    #[test]
    fn the_default_drawer_is_a_quarter_of_the_window_and_peeks_at_a_second_row() {
        let two_rows = DRAWER_CHROME + catalog_panel::height_for_rows(2);
        let one_row = DRAWER_CHROME + catalog_panel::height_for_rows(1);
        assert_eq!(drawer_default_height(1800.0), 450.0);
        for h in [700.0, 900.0, 1080.0, 1440.0] {
            let d = drawer_default_height(h);
            assert!(d >= (h / 4.0).min(drawer_max_height(h)) - 0.5, "{h}: {d}");
            // One whole row and the top of the next.
            assert!(d > one_row + 20.0 && d >= 0.75 * two_rows - 0.5, "{h}: {d}");
        }
    }

    #[test]
    fn the_drawer_drags_no_shorter_than_one_catalog_row() {
        let one_row = DRAWER_CHROME + catalog_panel::height_for_rows(1);
        for h in [700.0, 1000.0, 1440.0] {
            assert_eq!(drawer_min_height(h), one_row, "{h}");
            assert!(drawer_default_height(h) >= one_row, "{h}");
        }
        // A window too small for even that still leaves room above.
        assert!(drawer_min_height(300.0) <= drawer_max_height(300.0));
    }

    #[test]
    fn perform_has_scenes_and_no_catalog() {
        assert_eq!(
            DrawerTab::for_workspace(Workspace::Perform),
            &[DrawerTab::Scenes]
        );
        assert_eq!(
            DrawerTab::for_workspace(Workspace::Build),
            &[DrawerTab::Catalog, DrawerTab::Scenes]
        );
    }

    // Kevin's ~1200 px window left the inspector ~150 px between a 600 px
    // left floor and a fixed 420 px right. Put those floors back in
    // `columns` and the first assertion fails.
    #[test]
    fn the_middle_column_keeps_its_width_on_a_small_window() {
        for w in [1100.0, 1206.0, 1440.0, 1920.0, 3840.0] {
            let c = columns(w);
            let middle = w - c.left_range.1 - c.right;
            assert!(
                middle >= MIDDLE_MIN - 0.5,
                "at {w} px the middle column can shrink to {middle:.0} px"
            );
            assert!(c.left_range.0 <= c.left_default && c.left_default <= c.left_range.1);
        }
        // A big window still gets the full-size columns.
        let c = columns(1920.0);
        assert!(c.right >= 420.0 - 0.5 && c.left_default >= 650.0, "{c:?}");
    }

    // Kevin narrowed the middle column and each parameter row's "O" button
    // went under its edge. A bare `set_max_width(720)` inside a ScrollArea
    // held the content at 720 px however narrow the column got. Put the bare
    // form back in `inspector_width` and the first assertion fails by exactly
    // what the column lacks.
    #[test]
    fn inspector_rows_fit_the_column_and_stop_at_a_reading_width() {
        for w in [900.0, 700.0, 500.0, 300.0] {
            let (past, _) = row_extent(w);
            assert!(
                past <= 0.5,
                "at {w} px the row reaches {past:.0} px past its column"
            );
        }
        let (_, width) = row_extent(1600.0);
        assert!(
            width <= INSPECTOR_MAX_WIDTH + 0.5,
            "in a wide column the row is {width:.0} px, past the {INSPECTOR_MAX_WIDTH} cap"
        );
    }

    // Classic showed these for the effects that have them and the
    // workspace layout did not (#3238): Obstacle for particle effects,
    // Lattice, Helix, and the effect's own audio mappings. Each must show
    // for its effect, inside the middle column, on a laptop's window.
    #[test]
    fn the_inspector_has_every_effects_own_sections() {
        use crate::gpu::particle::types::{ObstacleFit, ObstacleMode};
        use crate::ui::panels::{helix_panel, lattice_panel, obstacle_panel};
        use crate::ui::shell_harness::ShellHarness;
        let mut h = ShellHarness::new(egui::vec2(1206.0, 3000.0));
        h.obstacle_info = Some(obstacle_panel::ObstacleInfo {
            enabled: true,
            mode: ObstacleMode::Bounce,
            fit: ObstacleFit::Stretch,
            threshold: 0.5,
            elasticity: 0.5,
            source: String::new(),
            image_path: None,
            has_particles: true,
            webcam_available: false,
            video_available: false,
            depth_available: false,
            depth_model_downloaded: false,
            depth_downloading: None,
            depth_download_error: None,
            webcam_devices: vec![],
            webcam_device_index: 0,
            water_enabled: false,
            water_level: 0.0,
            water_source: 0.0,
            water_drain: 0.0,
            water_flux: 0.0,
            model_spin: 1.0,
            model_display: 0.0,
            fluid_enabled: false,
            fluid_speed: 0.0,
            fluid_coupling: 0.0,
            fluid_vorticity: 0.0,
            fluid_viscosity: 0.0,
            fluid_grid: 64,
        });
        h.lattice_info = Some(lattice_panel::LatticeInfo {
            params: Default::default(),
            defaults: Default::default(),
        });
        h.helix_info = Some(helix_panel::HelixInfo {
            params: Default::default(),
            defaults: Default::default(),
        });
        h.loader.effects[0].audio_mappings = vec![crate::effect::format::AudioMapping {
            feature: "bass".into(),
            target: "trail_decay".into(),
        }];
        stack_panel::select_layer(&h.ctx, ShellHarness::EFFECT_LAYER);
        h.settle(3);
        let texts = h.texts();
        let column = texts
            .iter()
            .find(|t| t.0 == "PARAMETERS")
            .expect("the effect layer's inspector")
            .2;
        for title in [
            "OBSTACLE",
            "LATTICE (3D CA)",
            "HELIX (AUDIO RIBBON)",
            "AUDIO REACTIVITY",
        ] {
            let t = texts
                .iter()
                .find(|t| t.0 == title)
                .unwrap_or_else(|| panic!("no {title} section"));
            assert_eq!(t.2, column, "{title} is not in the inspector");
        }
        for (t, r, clip) in texts.iter().filter(|t| t.2 == column) {
            assert!(
                r.right() <= clip.right() + 1.0,
                "{t:?} runs out of the inspector"
            );
        }

        // An effect without particles has no Obstacle; one without them has
        // no Lattice or Helix either.
        h.obstacle_info.as_mut().unwrap().has_particles = false;
        h.lattice_info = None;
        h.helix_info = None;
        h.settle(2);
        let texts = h.texts();
        for title in ["OBSTACLE", "LATTICE (3D CA)", "HELIX (AUDIO RIBBON)"] {
            assert!(
                !texts.iter().any(|t| t.0 == title),
                "{title} shows without its effect"
            );
        }
    }
}
