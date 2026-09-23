//! The v2 effect catalog (#3124): every effect as a picture, in families,
//! searchable, with favorites.
//!
//! Click a picture to load it into the selected layer. Drag it onto the stack
//! to put it somewhere else: onto a row replaces that layer's effect, onto the
//! edge between rows adds a new layer there (`stack_panel` draws where it
//! would land). Both send intents `main.rs` handles, carrying the effect's
//! NAME rather than its index — the list is rescanned when an effect file
//! changes, and an index held across that would load the wrong effect.

use egui::{Color32, RichText, Sense, Stroke, TextureId, Ui, Vec2};

use crate::effect::format::{EffectType, PfxEffect};
use crate::effect::loader::EffectLoader;
use crate::ui::catalog_thumbs::CatalogThumbs;
use crate::ui::theme::colors::theme_colors;

/// The families an effect's `category` names, in tab order. The tabs are for
/// finding a look, so they say what an effect looks like or works on, not how
/// it is built — how it is built is in each picture's tooltip.
pub const FAMILIES: [(&str, &str, &str); 7] = [
    (
        "particles",
        "Particles",
        "Particle systems driven by forces: gravity, fields, attractors",
    ),
    ("fluid", "Fluids", "Smoke, ink, water and weather"),
    (
        "life",
        "Life and growth",
        "Simulated organisms, flocks and cellular automata",
    ),
    (
        "pattern",
        "Light and pattern",
        "Light, geometry, spectra and scopes",
    ),
    ("3d", "3D", "Scenes and flythroughs with depth"),
    (
        "media",
        "Your media",
        "Effects that re-make your own image, video, model or camera",
    ),
    (
        "overlay",
        "Overlays",
        "Instrument chrome meant to sit over other layers",
    ),
];

/// Where a dragged effect lands on the stack.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CatalogDrop {
    /// Onto this layer, replacing its effect.
    Replace(usize),
    /// As a new layer at this index; the layer there and below move down.
    Insert(usize),
}

/// What a catalog picture carries while it is dragged.
#[derive(Clone, Debug)]
pub struct EffectDrag {
    pub name: String,
}

/// The intent key for [`CatalogDrop`]; its value is `(effect name, where)`.
pub const DROP_INTENT: &str = "catalog_drop";

/// Send the intent that puts `name` on the stack at `at`.
pub fn send_drop(ctx: &egui::Context, name: String, at: CatalogDrop) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::new(DROP_INTENT), (name, at)));
}

/// Picture size in the grid, in points. 16:9, like the output.
const PIC: Vec2 = Vec2::new(176.0, 99.0);
const NAME_H: f32 = 20.0;

/// Which tab is open.
#[derive(Clone, PartialEq, Eq)]
enum Tab {
    All,
    Favorites,
    Family(&'static str),
    /// Effects whose category names no family — anything a user wrote before
    /// families existed, which all say "effect".
    Other,
    /// Effects the user wrote.
    Yours,
}

impl Tab {
    fn key(&self) -> String {
        match self {
            Tab::All => "all".into(),
            Tab::Favorites => "fav".into(),
            Tab::Family(k) => (*k).into(),
            Tab::Other => "other".into(),
            Tab::Yours => "yours".into(),
        }
    }

    fn from_key(k: &str) -> Tab {
        match k {
            "fav" => Tab::Favorites,
            "other" => Tab::Other,
            "yours" => Tab::Yours,
            _ => FAMILIES
                .iter()
                .find(|f| f.0 == k)
                .map_or(Tab::All, |f| Tab::Family(f.0)),
        }
    }

    fn holds(&self, e: &PfxEffect, favorites: &[String]) -> bool {
        match self {
            Tab::All => true,
            Tab::Favorites => favorites.contains(&e.name),
            Tab::Family(k) => e.category == *k,
            Tab::Other => family_of(e).is_none(),
            Tab::Yours => !EffectLoader::is_builtin(e),
        }
    }
}

/// The family an effect belongs to, if its category names one.
pub fn family_of(e: &PfxEffect) -> Option<&'static str> {
    FAMILIES.iter().find(|f| f.0 == e.category).map(|f| f.1)
}

fn type_words(e: &PfxEffect) -> String {
    let base = match e.effect_type() {
        EffectType::Shader => "Shader",
        EffectType::Particle => "Particles",
        EffectType::Feedback => "Shader with feedback",
    };
    match &e.particles {
        Some(p) if p.max_count >= 1000 => format!("{base} · up to {}K", p.max_count / 1000),
        Some(p) => format!("{base} · up to {}", p.max_count),
        None => base.to_string(),
    }
}

/// What the catalog needs to know about the selected layer.
pub struct Target {
    /// The effect on the selected layer, highlighted in the grid.
    pub current: Option<usize>,
    /// The selected layer ignores edits, so a click cannot load into it.
    pub locked: bool,
    /// The stack has room for another layer.
    pub can_add: bool,
    /// The selected layer's index, where "Add as a new layer" puts one.
    pub active: usize,
}

/// The catalog body: tabs and search, then the grid, then the authoring
/// buttons.
pub fn draw_catalog(
    ui: &mut Ui,
    loader: &EffectLoader,
    favorites: &[String],
    thumbs: &mut CatalogThumbs,
    target: &Target,
) {
    let tc = theme_colors(ui.ctx());
    let visible: Vec<(usize, &PfxEffect)> = loader
        .effects
        .iter()
        .enumerate()
        .filter(|(_, e)| !e.hidden)
        .collect();

    let tab_id = egui::Id::new("v2_catalog_tab");
    let search_id = egui::Id::new("v2_catalog_search");
    let mut tab = ui
        .ctx()
        .data(|d| d.get_temp::<String>(tab_id))
        .map_or(Tab::All, |k| Tab::from_key(&k));
    let mut query: String = ui.ctx().data(|d| d.get_temp(search_id)).unwrap_or_default();

    // Tabs, each with its count; a tab with nothing in it is left out, except
    // Favorites, which says how to fill it.
    let mut tabs: Vec<(Tab, String, &str)> = vec![(Tab::All, "All".into(), "Every effect")];
    tabs.push((
        Tab::Favorites,
        "\u{2605} Favorites".into(),
        "Effects you starred. Hover a picture and click its star.",
    ));
    for &(key, label, tip) in &FAMILIES {
        tabs.push((Tab::Family(key), label.into(), tip));
    }
    tabs.push((
        Tab::Other,
        "Other".into(),
        "Effects whose category names no family",
    ));
    tabs.push((Tab::Yours, "Yours".into(), "Effects you made"));

    ui.horizontal_wrapped(|ui| {
        for (t, label, tip) in &tabs {
            let n = visible
                .iter()
                .filter(|(_, e)| t.holds(e, favorites))
                .count();
            if n == 0 && !matches!(t, Tab::All | Tab::Favorites) {
                continue;
            }
            let r = ui
                .selectable_label(*t == tab, RichText::new(format!("{label} {n}")).size(13.0))
                .on_hover_text(*tip);
            if r.clicked() {
                tab = t.clone();
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if !query.is_empty() && ui.small_button("\u{00d7}").on_hover_text("Clear").clicked() {
                query.clear();
            }
            // Never focused by itself: typing belongs to the keyboard
            // shortcuts until someone clicks here.
            ui.add(
                egui::TextEdit::singleline(&mut query)
                    .hint_text("Search names and descriptions")
                    .desired_width(240.0),
            );
        });
    });
    ui.ctx().data_mut(|d| {
        d.insert_temp(tab_id, tab.key());
        d.insert_temp(search_id, query.clone());
    });

    let q = query.trim().to_lowercase();
    let mut shown: Vec<(usize, &PfxEffect)> = visible
        .iter()
        .copied()
        .filter(|(_, e)| tab.holds(e, favorites))
        .filter(|(_, e)| {
            q.is_empty()
                || e.name.to_lowercase().contains(&q)
                || e.description.to_lowercase().contains(&q)
        })
        .collect();
    // A name match is what someone typing a name wants: put those first.
    if !q.is_empty() {
        shown.sort_by_key(|(_, e)| !e.name.to_lowercase().contains(&q));
    }

    ui.add_space(6.0);
    let footer_h = 30.0;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .max_height((ui.available_height() - footer_h).max(PIC.y + NAME_H))
        .show(ui, |ui| {
            if shown.is_empty() {
                let msg = if tab == Tab::Favorites && q.is_empty() {
                    "No favorites yet. Hover a picture and click its star."
                } else {
                    "Nothing matches."
                };
                ui.label(RichText::new(msg).size(13.0).color(tc.text_secondary));
                return;
            }
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(10.0, 10.0);
                for &(i, e) in &shown {
                    let pic = thumbs.get(ui.ctx(), e);
                    tile(ui, i, e, pic, favorites, target);
                }
            });
        });

    footer(ui, loader, target);
}

fn tile_id(name: &str) -> egui::Id {
    egui::Id::new("v2_catalog_tile").with(name)
}

/// One picture with its name under it.
fn tile(
    ui: &mut Ui,
    index: usize,
    e: &PfxEffect,
    pic: Option<TextureId>,
    favorites: &[String],
    target: &Target,
) {
    let tc = theme_colors(ui.ctx());
    let size = Vec2::new(PIC.x, PIC.y + NAME_H);
    // Keyed by name rather than by position, so a drag survives the grid
    // reflowing under it (a search result arriving, a favorite added).
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let resp = ui.interact(rect, tile_id(&e.name), Sense::click_and_drag());
    resp.dnd_set_drag_payload(EffectDrag {
        name: e.name.clone(),
    });
    let is_fav = favorites.contains(&e.name);
    let pic_rect = egui::Rect::from_min_size(rect.min, PIC);

    // The star takes clicks before the tile does.
    let star_rect = egui::Rect::from_center_size(
        pic_rect.right_top() + Vec2::new(-14.0, 14.0),
        Vec2::splat(24.0),
    );
    let star = ui.interact(star_rect, resp.id.with("star"), Sense::click());

    if ui.is_rect_visible(rect) {
        let current = target.current == Some(index);
        let painter = ui.painter();
        draw_picture(painter, pic_rect, pic, e, &tc, Color32::WHITE);
        // The selected layer's effect: a strong outline and a bold name, not a
        // hue.
        let stroke = if current {
            Stroke::new(2.5_f32, tc.text_primary)
        } else if resp.hovered() {
            Stroke::new(1.5_f32, tc.hover_border)
        } else {
            Stroke::new(1.0_f32, tc.card_border)
        };
        painter.rect_stroke(pic_rect, 3.0, stroke, egui::StrokeKind::Outside);

        let mut name = RichText::new(crate::ui::widgets::truncate_chars(&e.name, 24)).size(13.0);
        if current {
            name = name.strong();
        }
        let galley = egui::WidgetText::from(name).into_galley(
            ui,
            Some(egui::TextWrapMode::Truncate),
            PIC.x,
            egui::TextStyle::Body,
        );
        painter.galley(
            egui::pos2(rect.left(), pic_rect.bottom() + 4.0),
            galley,
            tc.text_primary,
        );

        if is_fav || resp.hovered() || star.hovered() {
            painter.circle_filled(star_rect.center(), 10.0, Color32::from_black_alpha(160));
            let glyph = if is_fav { "\u{2605}" } else { "\u{2606}" };
            painter.text(
                star_rect.center(),
                egui::Align2::CENTER_CENTER,
                glyph,
                egui::FontId::proportional(15.0),
                Color32::WHITE,
            );
        }
    }

    if star.clicked() {
        toggle_favorite(ui.ctx(), &e.name);
    }
    star.on_hover_text(if is_fav {
        "Remove from Favorites"
    } else {
        "Add to Favorites"
    });

    if resp.dragged() {
        drag_ghost(ui.ctx(), pic, e);
    }
    if resp.clicked() {
        load_into_selected(ui.ctx(), index, target);
    }
    let resp = resp.on_hover_ui(|ui| {
        ui.set_max_width(340.0);
        ui.label(RichText::new(&e.name).size(14.0).strong());
        let family = family_of(e).unwrap_or("Other");
        ui.label(
            RichText::new(format!("{family} · {}", type_words(e)))
                .size(12.0)
                .color(tc.text_secondary),
        );
        if !e.description.is_empty() {
            ui.label(RichText::new(&e.description).size(13.0));
        }
        ui.add_space(4.0);
        let click = if target.locked {
            "The selected layer is locked, so a click does nothing."
        } else {
            "Click: load into the selected layer."
        };
        ui.label(
            RichText::new(format!(
                "{click} Drag onto the stack to replace a layer's effect or add a layer."
            ))
            .size(12.0)
            .color(tc.text_secondary),
        );
    });
    resp.context_menu(|ui| tile_menu(ui, index, e, is_fav, target));
}

/// The picture, or a placeholder naming the effect when there is none.
fn draw_picture(
    painter: &egui::Painter,
    rect: egui::Rect,
    pic: Option<TextureId>,
    e: &PfxEffect,
    tc: &crate::ui::theme::colors::ThemeColors,
    tint: Color32,
) {
    let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    match pic {
        Some(tex) => {
            painter.rect_filled(rect, 3.0, Color32::BLACK);
            painter.image(tex, rect, uv, tint);
        }
        None => {
            painter.rect_filled(rect, 3.0, tc.widget_bg);
            painter.text(
                rect.center() - Vec2::new(0.0, 8.0),
                egui::Align2::CENTER_CENTER,
                crate::ui::widgets::truncate_chars(&e.name, 18),
                egui::FontId::proportional(14.0),
                tc.text_primary,
            );
            painter.text(
                rect.center() + Vec2::new(0.0, 12.0),
                egui::Align2::CENTER_CENTER,
                "no picture yet",
                egui::FontId::proportional(11.0),
                tc.text_secondary,
            );
        }
    }
}

/// The picture follows the pointer while it is dragged.
fn drag_ghost(ctx: &egui::Context, pic: Option<TextureId>, e: &PfxEffect) {
    let Some(pos) = ctx.pointer_latest_pos() else {
        return;
    };
    ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
    let tc = theme_colors(ctx);
    let layer = egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("v2_catalog_ghost"));
    let painter = ctx.layer_painter(layer);
    let size = PIC * 0.7;
    let rect = egui::Rect::from_min_size(pos + Vec2::new(12.0, 12.0), size);
    draw_picture(&painter, rect, pic, e, &tc, Color32::from_white_alpha(220));
    painter.rect_stroke(
        rect,
        3.0,
        Stroke::new(1.5_f32, tc.text_primary),
        egui::StrokeKind::Outside,
    );
}

fn toggle_favorite(ctx: &egui::Context, name: &str) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::new("toggle_favorite_effect"), name.to_string()));
}

fn load_into_selected(ctx: &egui::Context, index: usize, target: &Target) {
    if target.locked || target.current == Some(index) {
        return;
    }
    // The inspector follows: loading an effect is about a layer.
    super::stack_panel::show_layer_in_inspector(ctx);
    ctx.data_mut(|d| d.insert_temp(egui::Id::new("pending_effect"), index));
}

fn tile_menu(ui: &mut Ui, index: usize, e: &PfxEffect, is_fav: bool, target: &Target) {
    if ui
        .add_enabled(
            !target.locked && target.current != Some(index),
            egui::Button::new("Load into the selected layer"),
        )
        .on_disabled_hover_text(if target.locked {
            "The selected layer is locked"
        } else {
            "It is already there"
        })
        .clicked()
    {
        load_into_selected(ui.ctx(), index, target);
        ui.close();
    }
    if ui
        .add_enabled(
            target.can_add,
            egui::Button::new("Add as a new layer above the selected one"),
        )
        .on_disabled_hover_text("The stack is full")
        .clicked()
    {
        super::stack_panel::show_layer_in_inspector(ui.ctx());
        send_drop(ui.ctx(), e.name.clone(), CatalogDrop::Insert(target.active));
        ui.close();
    }
    ui.separator();
    let fav = if is_fav {
        "Remove from Favorites"
    } else {
        "Add to Favorites"
    };
    if ui.button(fav).clicked() {
        toggle_favorite(ui.ctx(), &e.name);
        ui.close();
    }
    if !EffectLoader::is_builtin(e) {
        ui.separator();
        // Two clicks: deleting removes the effect's files from disk.
        let armed_id = egui::Id::new("v2_catalog_delete_armed");
        let armed = ui.ctx().data(|d| d.get_temp::<usize>(armed_id)) == Some(index);
        let label = if armed {
            format!("Click again to delete {}'s files", e.name)
        } else {
            "Delete effect…".to_string()
        };
        if ui.button(label).clicked() {
            if armed {
                ui.ctx().data_mut(|d| {
                    d.insert_temp(egui::Id::new("delete_effect"), index);
                    d.remove_temp::<usize>(armed_id);
                });
                ui.close();
            } else {
                ui.ctx().data_mut(|d| d.insert_temp(armed_id, index));
            }
        }
    }
}

/// Making effects: these act on the selected layer's effect, like the v1
/// panel's buttons they replace.
fn footer(ui: &mut Ui, loader: &EffectLoader, target: &Target) {
    let current = target.current.and_then(|i| loader.effects.get(i));
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if ui
            .button("+ New effect")
            .on_hover_text("Start a new effect from the template")
            .clicked()
        {
            ui.ctx()
                .data_mut(|d| d.insert_temp(egui::Id::new("new_effect_prompt"), true));
        }
        if let Some(e) = current.filter(|e| !e.hidden) {
            if ui
                .button(format!("Copy {} to a new effect", e.name))
                .on_hover_text("A copy you can edit, starting from the selected layer's effect")
                .clicked()
            {
                ui.ctx()
                    .data_mut(|d| d.insert_temp(egui::Id::new("copy_builtin_prompt"), true));
            }
            if !EffectLoader::is_builtin(e)
                && ui
                    .button(format!("Edit {}", e.name))
                    .on_hover_text("Open its shader in the editor")
                    .clicked()
            {
                ui.ctx()
                    .data_mut(|d| d.insert_temp(egui::Id::new("open_shader_editor"), true));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::layer::{BlendMode, LayerInfo};
    use crate::ui::panels::stack_panel;

    fn shipped() -> Vec<(String, PfxEffect)> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/effects");
        let mut out: Vec<(String, PfxEffect)> = std::fs::read_dir(&dir)
            .expect("assets/effects")
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "pfx"))
            .map(|p| {
                let e: PfxEffect =
                    serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
                (p.file_stem().unwrap().to_string_lossy().into_owned(), e)
            })
            .filter(|(_, e)| !e.hidden && EffectLoader::is_builtin(e))
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    // A shipped effect with no family lands under "Other", a tab that exists
    // for users' own effects. New effects must pick a family.
    #[test]
    fn every_shipped_effect_names_a_family() {
        let fx = shipped();
        assert!(fx.len() > 40, "only {} effects found", fx.len());
        let orphans: Vec<_> = fx
            .iter()
            .filter(|(_, e)| family_of(e).is_none())
            .map(|(s, e)| format!("{s} (category \"{}\")", e.category))
            .collect();
        assert!(
            orphans.is_empty(),
            "no family for: {orphans:?}; set \"category\" to one of {:?}",
            FAMILIES.map(|f| f.0)
        );
    }

    // A shipped effect with no picture shows a placeholder in the catalog.
    // Render one with `uv run scripts/build_thumbnails.py --effects <stem>`.
    #[test]
    fn every_shipped_effect_has_a_picture() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/thumbs");
        let missing: Vec<_> = shipped()
            .into_iter()
            .filter(|(stem, _)| crate::ui::catalog_thumbs::find(&dir, stem).is_none())
            .map(|(stem, _)| stem)
            .collect();
        assert!(
            missing.is_empty(),
            "no picture in assets/thumbs for: {missing:?}"
        );
    }

    fn effect(name: &str) -> PfxEffect {
        serde_json::from_str(&format!(r#"{{"name":"{name}","author":"Fosfora"}}"#)).unwrap()
    }

    fn layer(name: &str) -> LayerInfo {
        LayerInfo {
            name: name.into(),
            custom_name: None,
            effect_index: Some(0),
            effect_name: Some(name.into()),
            blend_mode: BlendMode::default(),
            opacity: 1.0,
            displace_amount: 0.0,
            enabled: true,
            locked: false,
            pinned: false,
            has_particles: false,
            shader_error: None,
            is_media: false,
            media_file_name: None,
            media_is_animated: false,
            media_is_video: false,
            media_is_live: false,
            chain: None,
            needs_layer_below: false,
        }
    }

    /// The stack and the catalog side by side, as in Build, driven by real
    /// pointer events. Each step is one frame's events.
    struct Harness {
        ctx: egui::Context,
        loader: EffectLoader,
        layers: Vec<LayerInfo>,
        thumbs: CatalogThumbs,
        time: f64,
    }

    impl Harness {
        fn new() -> Self {
            let ctx = egui::Context::default();
            crate::ui::theme::colors::set_theme_colors(
                &ctx,
                crate::ui::theme::colors::ThemeColors::dark(),
            );
            let mut loader = EffectLoader::for_test("");
            loader.effects = vec![effect("Sumi"), effect("Tide")];
            Self {
                ctx,
                loader,
                layers: vec![layer("Aurora"), layer("Drift")],
                thumbs: CatalogThumbs::new("/nonexistent".into()),
                time: 0.0,
            }
        }

        fn frame(&mut self, events: Vec<egui::Event>) {
            self.time += 1.0 / 60.0;
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1600.0, 1000.0),
                )),
                time: Some(self.time),
                events,
                ..Default::default()
            };
            let (loader, layers, thumbs) = (&self.loader, &self.layers, &mut self.thumbs);
            let _ = self.ctx.run(input, |ctx| {
                egui::SidePanel::left("stack")
                    .exact_width(700.0)
                    .show(ctx, |ui| {
                        let pics = stack_panel::StackPictures {
                            thumbs: None,
                            aspect: 16.0 / 9.0,
                        };
                        stack_panel::draw_stack(ui, layers, 0, None, &[], &pics);
                    });
                egui::CentralPanel::default().show(ctx, |ui| {
                    let target = Target {
                        current: None,
                        locked: false,
                        can_add: true,
                        active: 0,
                    };
                    draw_catalog(ui, loader, &[], thumbs, &target);
                });
            });
        }

        fn row_rect(&self, i: usize) -> egui::Rect {
            self.ctx
                .data(|d| d.get_temp(egui::Id::new(format!("v2_row_{i}")).with("rect")))
                .expect("row drawn")
        }

        fn tile_rect(&self, name: &str) -> egui::Rect {
            self.ctx
                .read_response(tile_id(name))
                .expect("tile drawn")
                .rect
        }

        /// Press on a tile, move across in steps, release at `to`. Returns the
        /// intent sent, if any.
        fn drag(&mut self, name: &str, to: egui::Pos2) -> Option<(String, CatalogDrop)> {
            use egui::{Event, PointerButton};
            self.frame(vec![]);
            let from = self.tile_rect(name).center();
            let press = |pos, pressed| Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            };
            self.frame(vec![Event::PointerMoved(from), press(from, true)]);
            for k in 1..=8 {
                let p = from + (to - from) * (k as f32 / 8.0);
                self.frame(vec![Event::PointerMoved(p)]);
            }
            self.frame(vec![Event::PointerMoved(to)]);
            self.frame(vec![press(to, false)]);
            self.ctx.data_mut(|d| {
                let id = egui::Id::new(DROP_INTENT);
                let v = d.get_temp(id);
                d.remove::<(String, CatalogDrop)>(id);
                v
            })
        }
    }

    // The whole gesture through egui's own drag-and-drop: a tile's payload
    // must survive the pointer leaving the catalog, and the row must read
    // which part of it the pointer is over. Stop the tile setting its
    // payload and every case here fails.
    #[test]
    fn dragging_a_picture_onto_the_stack_replaces_or_inserts() {
        let mut h = Harness::new();
        h.frame(vec![]);
        let row1 = h.row_rect(1);

        assert_eq!(
            h.drag("Tide", row1.center()),
            Some(("Tide".into(), CatalogDrop::Replace(1))),
            "the middle of a row replaces that layer's effect"
        );
        assert_eq!(
            h.drag("Sumi", egui::pos2(row1.center().x, row1.top() + 3.0)),
            Some(("Sumi".into(), CatalogDrop::Insert(1))),
            "the top edge of a row adds a layer above it"
        );
        assert_eq!(
            h.drag("Sumi", egui::pos2(row1.center().x, row1.bottom() - 3.0)),
            Some(("Sumi".into(), CatalogDrop::Insert(2))),
            "the bottom edge of a row adds a layer below it"
        );
        // Released back over the catalog: nothing happens.
        let home = h.tile_rect("Sumi").center() + egui::vec2(0.0, 2.0);
        assert_eq!(h.drag("Tide", home), None);
    }

    #[test]
    fn a_locked_layer_refuses_a_drop() {
        let mut h = Harness::new();
        h.layers[1].locked = true;
        h.frame(vec![]);
        let row1 = h.row_rect(1);
        assert_eq!(h.drag("Tide", row1.center()), None);
    }
}
