//! The trama graph canvas: an egui-snarl view over the node graph.
//!
//! [`NodeGraph`] is the single source of truth; the snarl holds only view
//! state (canvas positions, which node id sits where). Every mutation flows
//! through the [`SnarlViewer`] callbacks, which update both structures
//! atomically — a graph-refused edit (cycle, arity) leaves the snarl
//! untouched and explains itself in the status line. Live rewire needs no
//! apply button: an accepted edit bumps the graph version and the executor
//! replans next frame.

use egui_snarl::ui::{PinInfo, SnarlPin, SnarlViewer, SnarlWidget};
use egui_snarl::{InPin, OutPin, Snarl};

use super::super::TramaSystem;
use super::super::effect::{EffectKind, TramaRegistry};
use super::super::graph::NodeGraph;
use super::super::node::{NodeId, NodeKind};

pub struct CanvasState {
    /// Snarl payload is the trama [`NodeId`] itself — the index map for free.
    /// Positions live here (M3 serializes them via snarl's serde feature).
    pub snarl: Snarl<NodeId>,
    /// The inspected node. Ours, not egui-snarl's: snarl 0.9 only selects on
    /// shift/cmd-click or a rect-drag (owner play-test: "the inspector never
    /// shows any content"), so a plain press on a node selects here instead.
    pub selected: Option<NodeId>,
    /// Last refused edit, shown under the header until the next accepted one.
    pub status: Option<String>,
    /// Where the canvas widget sat last frame. egui-snarl persists its
    /// viewport as a *screen-space* transform, so nodes would stay put while
    /// the host window moves; the frame-to-frame origin delta re-anchors them
    /// via `current_transform` below.
    last_origin: Option<egui::Pos2>,
}

impl CanvasState {
    pub fn new(graph: &NodeGraph) -> Self {
        let mut snarl = Snarl::new();
        snarl.insert_node(egui::pos2(480.0, 200.0), graph.output_node());
        Self {
            snarl,
            selected: None,
            status: None,
            last_origin: None,
        }
    }
}

struct CanvasViewer<'a> {
    graph: &'a mut NodeGraph,
    registry: &'a TramaRegistry,
    status: &'a mut Option<String>,
    /// The inspected node (see [`CanvasState::selected`]).
    selected: &'a mut Option<NodeId>,
    /// Whether the pointer sat over any node this frame — lets the caller
    /// distinguish a background click (deselect) from a node click.
    pointer_on_node: bool,
    /// Screen-space shift of the canvas since last frame (window drag,
    /// header lines appearing); applied to the snarl viewport so nodes ride
    /// along with their container.
    translate: egui::Vec2,
}

impl CanvasViewer<'_> {
    fn pin_of(pin_input: usize) -> u8 {
        u8::try_from(pin_input).unwrap_or(u8::MAX)
    }
}

impl SnarlViewer<NodeId> for CanvasViewer<'_> {
    fn title(&mut self, node: &NodeId) -> String {
        match self.graph.node(*node).map(|n| &n.kind) {
            Some(NodeKind::Output) => "Output".to_string(),
            Some(NodeKind::Source { effect } | NodeKind::Effect { effect }) => self
                .registry
                .get(effect)
                .map_or_else(|| effect.0.clone(), |def| def.name.clone()),
            None => "?".to_string(),
        }
    }

    fn inputs(&mut self, node: &NodeId) -> usize {
        self.graph.node(*node).map_or(0, |n| usize::from(n.inputs))
    }

    fn outputs(&mut self, node: &NodeId) -> usize {
        match self.graph.node(*node).map(|n| &n.kind) {
            Some(NodeKind::Output) | None => 0,
            Some(_) => 1,
        }
    }

    fn show_input(
        &mut self,
        _pin: &InPin,
        _ui: &mut egui::Ui,
        _snarl: &mut Snarl<NodeId>,
    ) -> impl SnarlPin + 'static {
        // Texture-typed only in M0 — a plain circle is the whole story.
        PinInfo::circle()
    }

    fn show_output(
        &mut self,
        _pin: &OutPin,
        _ui: &mut egui::Ui,
        _snarl: &mut Snarl<NodeId>,
    ) -> impl SnarlPin + 'static {
        PinInfo::circle()
    }

    fn connect(&mut self, from: &OutPin, to: &InPin, snarl: &mut Snarl<NodeId>) {
        let f = snarl[from.id.node];
        let t = snarl[to.id.node];
        match self.graph.connect(f, t, Self::pin_of(to.id.input)) {
            Ok(()) => {
                // The graph replaced any wire on this pin; mirror that.
                snarl.drop_inputs(to.id);
                snarl.connect(from.id, to.id);
                *self.status = None;
            }
            Err(e) => *self.status = Some(e.to_string()),
        }
    }

    fn disconnect(&mut self, from: &OutPin, to: &InPin, snarl: &mut Snarl<NodeId>) {
        let t = snarl[to.id.node];
        self.graph.disconnect(t, Self::pin_of(to.id.input));
        snarl.disconnect(from.id, to.id);
    }

    fn drop_inputs(&mut self, pin: &InPin, snarl: &mut Snarl<NodeId>) {
        let t = snarl[pin.id.node];
        self.graph.disconnect(t, Self::pin_of(pin.id.input));
        snarl.drop_inputs(pin.id);
    }

    fn drop_outputs(&mut self, pin: &OutPin, snarl: &mut Snarl<NodeId>) {
        for remote in &pin.remotes {
            let t = snarl[remote.node];
            self.graph.disconnect(t, Self::pin_of(remote.input));
        }
        snarl.drop_outputs(pin.id);
    }

    fn current_transform(
        &mut self,
        to_global: &mut egui::emath::TSTransform,
        _snarl: &mut Snarl<NodeId>,
    ) {
        to_global.translation += self.translate;
    }

    fn final_node_rect(
        &mut self,
        node: egui_snarl::NodeId,
        rect: egui::Rect,
        ui: &mut egui::Ui,
        snarl: &mut Snarl<NodeId>,
    ) {
        // Plain-press selection. egui-snarl 0.9 selects only on shift/cmd
        // click or a background rect-drag, which no one discovers — pressing
        // a node (click or drag-grab) is what inspects it. Nodes draw in
        // insertion order, so on overlap the last (topmost) hook call wins.
        //
        // The pointer test is manual: `Ui::rect_contains_pointer` demands
        // the pointer's topmost layer BE this ui's layer, but snarl paints
        // nodes in a non-Area sublayer that `layer_id_at` never returns, so
        // that test is unconditionally false here. Transform the rect to
        // global ourselves and skip the layer check.
        let to_global = ui
            .ctx()
            .layer_transform_to_global(ui.layer_id())
            .unwrap_or(egui::emath::TSTransform::IDENTITY);
        let global_rect = to_global * rect.intersect(ui.clip_rect());
        let pointer = ui.input(|i| i.pointer.interact_pos());
        if pointer.is_some_and(|pos| global_rect.contains(pos)) {
            self.pointer_on_node = true;
            if ui.input(|i| i.pointer.primary_pressed()) {
                *self.selected = Some(snarl[node]);
            }
        }
        // Selection ring: a bright outline (luminance, not hue — snarl's own
        // highlight only tracks its internal shift-click set, not this one).
        if *self.selected == Some(snarl[node]) {
            let tc = crate::ui::theme::colors::theme_colors(ui.ctx());
            ui.painter().rect_stroke(
                rect.expand(3.0),
                4.0,
                egui::Stroke::new(1.5, tc.text_primary),
                egui::StrokeKind::Outside,
            );
        }
    }

    fn has_graph_menu(&mut self, _pos: egui::Pos2, _snarl: &mut Snarl<NodeId>) -> bool {
        true
    }

    fn show_graph_menu(&mut self, pos: egui::Pos2, ui: &mut egui::Ui, snarl: &mut Snarl<NodeId>) {
        ui.label("Add node");
        ui.separator();
        for (label, kind) in [
            ("Sources", EffectKind::Source),
            ("Effects", EffectKind::Effect),
        ] {
            ui.menu_button(label, |ui| {
                for def in self.registry.effects.iter().filter(|d| d.kind == kind) {
                    if ui.button(&def.name).clicked() {
                        let node_kind = match def.kind {
                            EffectKind::Source => NodeKind::Source {
                                effect: def.id.clone(),
                            },
                            EffectKind::Effect => NodeKind::Effect {
                                effect: def.id.clone(),
                            },
                        };
                        let id = self.graph.add_node(node_kind, def.inputs, &def.params);
                        snarl.insert_node(pos, id);
                        ui.close();
                    }
                }
            });
        }
    }

    fn has_node_menu(&mut self, _node: &NodeId) -> bool {
        true
    }

    fn show_node_menu(
        &mut self,
        node: egui_snarl::NodeId,
        _inputs: &[InPin],
        _outputs: &[OutPin],
        ui: &mut egui::Ui,
        snarl: &mut Snarl<NodeId>,
    ) {
        let id = snarl[node];
        let is_output = matches!(self.graph.node(id).map(|n| &n.kind), Some(NodeKind::Output));
        if is_output {
            ui.label("Output");
            return;
        }
        let mut bypass = self.graph.node(id).is_some_and(|n| n.bypass);
        if ui.checkbox(&mut bypass, "Bypass").changed() {
            let _ = self.graph.set_bypass(id, bypass);
        }
        if ui.button("Delete").clicked() {
            if self.graph.remove_node(id).is_ok() {
                snarl.remove_node(node);
                if *self.selected == Some(id) {
                    *self.selected = None;
                }
            }
            ui.close();
        }
    }
}

/// Drawn from `main.rs` between the overlay's `begin_frame`/`end_frame`, the
/// same hosting pattern as the shader editor — `draw_panels` stays untouched.
pub fn draw_trama_window(ctx: &egui::Context, trama: &mut TramaSystem) {
    if !trama.canvas_open {
        return;
    }
    let (pool_in_use, pool_total) = trama.pool_stats();
    let mut open = trama.canvas_open;
    let TramaSystem {
        mode,
        graph,
        registry,
        canvas,
        last_error,
        audio_view,
        ..
    } = trama;
    // Last frame's selection — the inspector draws before the canvas, the
    // standard one-frame egui lag.
    let selected = canvas.selected;
    egui::Window::new("trama")
        .default_size([1020.0, 520.0])
        .open(&mut open)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(mode, super::super::RenderMode::Layers, "Layers");
                ui.selectable_value(mode, super::super::RenderMode::Trama, "Trama");
                ui.separator();
                ui.weak(format!("pool {pool_in_use}/{pool_total}"));
                if !registry.errors.is_empty() {
                    ui.separator();
                    let n = registry.errors.len();
                    ui.colored_label(
                        ui.visuals().warn_fg_color,
                        format!("{n} effect file(s) failed"),
                    )
                    .on_hover_text(
                        registry
                            .errors
                            .iter()
                            .map(|(f, e)| format!("{f}: {e}"))
                            .collect::<Vec<_>>()
                            .join("\n"),
                    );
                }
            });
            if let Some(err) = last_error.as_deref().or(canvas.status.as_deref()) {
                ui.colored_label(ui.visuals().error_fg_color, err);
            }
            if *mode == super::super::RenderMode::Trama
                && graph.input_source(graph.output_node(), 0).is_none()
            {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    "Nothing reaches Output — the screen stays black. Right-click the \
                     canvas to add nodes; drag from a pin to wire them.",
                );
            }
            ui.separator();
            egui::SidePanel::right("trama-inspector")
                .resizable(true)
                .default_width(300.0)
                .show_inside(ui, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        super::inspector::draw_inspector(ui, graph, registry, audio_view, selected);
                    });
                });
            egui::CentralPanel::default().show_inside(ui, |ui| {
                let origin = ui.next_widget_position();
                let translate = canvas.last_origin.map_or(egui::Vec2::ZERO, |o| origin - o);
                canvas.last_origin = Some(origin);
                let mut viewer = CanvasViewer {
                    graph,
                    registry,
                    status: &mut canvas.status,
                    selected: &mut canvas.selected,
                    pointer_on_node: false,
                    translate,
                };
                let background = SnarlWidget::new().id_salt("trama-canvas").show(
                    &mut canvas.snarl,
                    &mut viewer,
                    ui,
                );
                // A completed click on empty canvas clears the selection; the
                // pointer_on_node guard keeps node clicks (which select in
                // `final_node_rect` on press) from immediately deselecting.
                if background.clicked() && !viewer.pointer_on_node {
                    *viewer.selected = None;
                }
            });
        });
    trama.canvas_open = open;
}
