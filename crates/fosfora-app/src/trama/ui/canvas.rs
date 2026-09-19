//! The trama graph canvas: an egui-snarl view over the node graph.
//!
//! [`NodeGraph`] is the single source of truth; the snarl holds only view
//! state (canvas positions, which node id sits where). Every mutation flows
//! through the [`SnarlViewer`] callbacks, which update both structures
//! atomically — a graph-refused edit (cycle, arity) leaves the snarl
//! untouched and explains itself in the status line. Live rewire needs no
//! apply button: an accepted edit bumps the graph version and the executor
//! replans next frame.

use egui_snarl::ui::{PinInfo, PinPlacement, SnarlPin, SnarlStyle, SnarlViewer, SnarlWidget};
use egui_snarl::{InPin, OutPin, Snarl};

use super::super::effect::{EffectKind, TramaRegistry};
use super::super::exec::executor::TramaExecutor;
use super::super::graph::NodeGraph;
use super::super::node::{NodeId, NodeKind};
use super::super::{CanvasTarget, TramaSystem};

/// Thumbnail display size — half the 192×108 preview texture, so it stays
/// crisp on hidpi and nodes stay compact. Tune by eye in play-tests.
const PREVIEW_DISPLAY: egui::Vec2 = egui::vec2(96.0, 54.0);

/// Compact node chrome. egui-snarl's defaults are sized for a graph that is
/// the whole application; here the canvas shares a window with an inspector
/// and a patch is a handful of nodes, so every default worked against it.
///
/// The one that mattered most is `max_scale`. snarl's first view FITS THE
/// CONTENT to the viewport, clamped to `max_scale` (default 2.0) — and a chain
/// is born holding a single Output node, so every canvas opened at 2x: titles,
/// pins and thumbnails all drawn double size (owner play-test: "the nodes are
/// very chunky ... it really fills up the available space"). Capping at 1.0
/// makes 1:1 the largest view; zooming OUT for a big patch still works.
const PIN_SIZE: f32 = 9.0;
/// snarl's default ratio, set explicitly because the wire remove button
/// rebuilds the wire's curve from it (`wire_geom`) — the two must agree.
const WIRE_FRAME_SIZE: f32 = PIN_SIZE * 3.0;

fn canvas_style(style: &egui::Style) -> SnarlStyle {
    let node_frame = egui::Frame::window(style)
        .inner_margin(egui::Margin::same(4))
        .corner_radius(4)
        .shadow(egui::Shadow::NONE);
    SnarlStyle {
        max_scale: Some(1.0),
        node_frame: Some(node_frame),
        header_frame: Some(node_frame.inner_margin(egui::Margin::symmetric(4, 2))),
        // The whole node frame already drags, so the header needs no blank
        // grab area beside the title.
        header_drag_space: Some(egui::Vec2::ZERO),
        // Pins straddle the frame edge instead of each claiming an inside
        // column either side of the thumbnail.
        pin_placement: Some(PinPlacement::Edge),
        pin_size: Some(PIN_SIZE),
        wire_frame_size: Some(WIRE_FRAME_SIZE),
        ..SnarlStyle::new()
    }
}

/// One chain's view state. Kept per chain rather than once for the canvas:
/// every graph numbers its nodes from zero, so a `NodeId` from another chain
/// either names the wrong node or names nothing at all. Sharing one snarl left
/// the previous layer's nodes sitting on the canvas as unresolvable "?" boxes
/// the moment you selected a different layer.
pub struct ChainView {
    /// Snarl payload is the trama [`NodeId`] itself — the index map for free.
    /// Positions live here and nowhere else, which is why switching chains
    /// swaps this rather than rebuilding it: a rebuild would lose every
    /// node's position. They are saved through [`CanvasState::position`], not
    /// through snarl's serde feature — that would write the wire set a second
    /// time, against `NodeGraph` being the one source of truth for wires.
    pub snarl: Snarl<NodeId>,
    /// The inspected node. Ours, not egui-snarl's: snarl 0.9 only selects on
    /// shift/cmd-click or a rect-drag (owner play-test: "the inspector never
    /// shows any content"), so a plain press on a node selects here instead.
    pub selected: Option<NodeId>,
}

impl ChainView {
    /// Seed from the graph as it stands. `layout` holds the positions a
    /// loaded chain was saved with; any node it does not name (and every node
    /// of a chain that was never saved) gets the automatic column layout.
    fn new(graph: &NodeGraph, layout: &[(NodeId, [f32; 2])]) -> Self {
        let mut snarl = Snarl::new();
        let saved = |id: NodeId| {
            layout
                .iter()
                .find(|(n, _)| *n == id)
                .map(|(_, p)| egui::pos2(p[0], p[1]))
        };
        let out = graph.output_node();
        snarl.insert_node(saved(out).unwrap_or(egui::pos2(480.0, 200.0)), out);
        let mut y = 200.0;
        for id in graph.topo_order() {
            if id == out {
                continue;
            }
            snarl.insert_node(saved(id).unwrap_or(egui::pos2(120.0, y)), id);
            y += 160.0;
        }
        // Wires too, or a rebuilt view shows a patch that looks disconnected
        // and still renders connected.
        let at = |id: NodeId, snarl: &Snarl<NodeId>| {
            snarl.node_ids().find(|&(_, &n)| n == id).map(|(s, _)| s)
        };
        for w in graph.wires() {
            if let (Some(from), Some(to)) = (at(w.from, &snarl), at(w.to, &snarl)) {
                snarl.connect(
                    egui_snarl::OutPinId {
                        node: from,
                        output: 0,
                    },
                    egui_snarl::InPinId {
                        node: to,
                        input: usize::from(w.to_input),
                    },
                );
            }
        }
        Self {
            snarl,
            selected: None,
        }
    }
}

#[derive(Default)]
pub struct CanvasState {
    /// One view per chain, created on first sight of that chain.
    views: std::collections::HashMap<super::super::node::ChainId, ChainView>,
    /// Saved node positions for a chain that was LOADED and has not been shown
    /// yet. A view is only built when its chain is first drawn, so until then
    /// this is the only copy — it seeds the view, and answers [`Self::position`]
    /// so that saving a preset again before ever opening the canvas does not
    /// flatten the layout it was loaded with.
    layouts: std::collections::HashMap<super::super::node::ChainId, Vec<(NodeId, [f32; 2])>>,
    /// Last refused edit, shown under the header until the next accepted one.
    pub status: Option<String>,
    /// Where the canvas widget sat last frame. egui-snarl persists its
    /// viewport as a *screen-space* transform, so nodes would stay put while
    /// the host window moves; the frame-to-frame origin delta re-anchors them
    /// via `current_transform` below.
    last_origin: Option<egui::Pos2>,
}

impl CanvasState {
    /// Forget a chain's view. Slots are reused once a layer is removed, so
    /// without this the next chain to land on that slot would inherit the
    /// removed one's nodes — the same cross-wiring, one level up.
    ///
    /// The master chain is the exception. It is "dropped" only because its
    /// output target was released — nothing reaches Output and nobody is
    /// looking — while its graph, a field of `TramaSystem`, lives on. Its view
    /// holds the only copy of the node positions, so it stays.
    pub fn drop_chain(&mut self, chain: super::super::node::ChainId) {
        if chain != super::super::node::ChainId::Master {
            self.views.remove(&chain);
            self.layouts.remove(&chain);
        }
    }

    /// A chain's graph was REPLACED — a preset or a `.fio.json` was loaded
    /// into it. Whatever view it had describes nodes that no longer exist, the
    /// master's included, so it goes; the next draw rebuilds it from `layout`.
    pub fn replace_chain(
        &mut self,
        chain: super::super::node::ChainId,
        layout: Vec<(NodeId, [f32; 2])>,
    ) {
        self.views.remove(&chain);
        self.layouts.insert(chain, layout);
    }

    /// Where a node sits on the canvas, for saving: from the live view if the
    /// chain has been shown, otherwise from the layout it was loaded with.
    pub fn position(&self, chain: super::super::node::ChainId, node: NodeId) -> Option<[f32; 2]> {
        if let Some(view) = self.views.get(&chain) {
            return view
                .snarl
                .nodes_pos_ids()
                .find(|(_, _, n)| **n == node)
                .map(|(_, pos, _)| [pos.x, pos.y]);
        }
        self.layouts
            .get(&chain)?
            .iter()
            .find(|(n, _)| *n == node)
            .map(|(_, p)| *p)
    }

    #[cfg(test)]
    fn view_count(&self) -> usize {
        self.views.len()
    }

    /// Stand-in for what `draw_trama_window` does at the top of a frame.
    #[cfg(test)]
    pub(crate) fn open_view(
        &mut self,
        chain: super::super::node::ChainId,
        graph: &NodeGraph,
    ) -> &mut ChainView {
        let layout = self.layouts.remove(&chain).unwrap_or_default();
        self.views
            .entry(chain)
            .or_insert_with(|| ChainView::new(graph, &layout))
    }

    #[cfg(test)]
    pub(crate) fn has_view(&self, chain: super::super::node::ChainId) -> bool {
        self.views.contains_key(&chain)
    }
}

struct CanvasViewer<'a> {
    graph: &'a mut NodeGraph,
    /// Which chain is on the canvas — thumbnails are keyed by
    /// `(chain, node)`, since every chain numbers its nodes from zero.
    chain: super::super::node::ChainId,
    registry: &'a TramaRegistry,
    /// Read-only: thumbnail texture lookups for node bodies.
    executor: &'a TramaExecutor,
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
    /// How the selected node is marked: a bright frame stroke and a lifted
    /// header. Luminance, never hue — and drawn as part of the node's own
    /// frame, so the edge pins paint over it instead of under a ring.
    selected_stroke: egui::Stroke,
    selected_header_fill: egui::Color32,
    /// Where each pin was drawn this frame, in canvas space. snarl does not
    /// expose pin positions, and the wire remove button needs both ends.
    pins: std::rc::Rc<std::cell::RefCell<PinCenters>>,
    /// Canvas space to screen space, captured from the node layer.
    to_global: egui::emath::TSTransform,
}

#[derive(Default)]
struct PinCenters {
    inputs: std::collections::HashMap<egui_snarl::InPinId, egui::Pos2>,
    outputs: std::collections::HashMap<egui_snarl::OutPinId, egui::Pos2>,
}

enum PinEnd {
    In(egui_snarl::InPinId),
    Out(egui_snarl::OutPinId),
}

/// A plain pin that reports where it was drawn.
struct TrackedPin {
    info: PinInfo,
    end: PinEnd,
    sink: std::rc::Rc<std::cell::RefCell<PinCenters>>,
}

impl SnarlPin for TrackedPin {
    fn pin_rect(&self, x: f32, y0: f32, y1: f32, size: f32) -> egui::Rect {
        self.info.pin_rect(x, y0, y1, size)
    }

    fn draw(
        self,
        snarl_style: &SnarlStyle,
        style: &egui::Style,
        rect: egui::Rect,
        painter: &egui::Painter,
    ) -> egui_snarl::ui::PinWireInfo {
        // The hovered pin's rect is scaled about its center, so the center is
        // the wire's endpoint either way.
        let mut sink = self.sink.borrow_mut();
        match self.end {
            PinEnd::In(id) => sink.inputs.insert(id, rect.center()),
            PinEnd::Out(id) => sink.outputs.insert(id, rect.center()),
        };
        drop(sink);
        self.info.draw(snarl_style, style, rect, painter)
    }
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
            Some(NodeKind::Feedback) => "Feedback".to_string(),
            Some(NodeKind::ChainInput) => "Layer input".to_string(),
            Some(NodeKind::Source { effect } | NodeKind::Effect { effect }) => self
                .registry
                .get(effect)
                // Words, not just the magenta the executor paints: the node
                // says what it is waiting for.
                .map_or_else(|| format!("missing: {}", effect.0), |def| def.name.clone()),
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
        pin: &InPin,
        _ui: &mut egui::Ui,
        _snarl: &mut Snarl<NodeId>,
    ) -> impl SnarlPin + 'static {
        // Texture-typed only in M0 — a plain circle is the whole story.
        TrackedPin {
            info: PinInfo::circle(),
            end: PinEnd::In(pin.id),
            sink: self.pins.clone(),
        }
    }

    fn show_output(
        &mut self,
        pin: &OutPin,
        _ui: &mut egui::Ui,
        _snarl: &mut Snarl<NodeId>,
    ) -> impl SnarlPin + 'static {
        TrackedPin {
            info: PinInfo::circle(),
            end: PinEnd::Out(pin.id),
            sink: self.pins.clone(),
        }
    }

    fn node_frame(
        &mut self,
        default: egui::Frame,
        node: egui_snarl::NodeId,
        _inputs: &[InPin],
        _outputs: &[OutPin],
        snarl: &Snarl<NodeId>,
    ) -> egui::Frame {
        if *self.selected == Some(snarl[node]) {
            default.stroke(self.selected_stroke)
        } else {
            default
        }
    }

    fn header_frame(
        &mut self,
        default: egui::Frame,
        node: egui_snarl::NodeId,
        _inputs: &[InPin],
        _outputs: &[OutPin],
        snarl: &Snarl<NodeId>,
    ) -> egui::Frame {
        if *self.selected == Some(snarl[node]) {
            default
                .fill(self.selected_header_fill)
                .stroke(self.selected_stroke)
        } else {
            default
        }
    }

    fn show_header(
        &mut self,
        node: egui_snarl::NodeId,
        _inputs: &[InPin],
        _outputs: &[OutPin],
        ui: &mut egui::Ui,
        snarl: &mut Snarl<NodeId>,
    ) {
        // Not selectable: a default label swallows the press to start a text
        // selection, so grabbing a node by its title selected the word
        // instead of dragging the node (owner play-test). Inert text lets the
        // press fall through to the node frame, which drags and selects.
        let title = self.title(&snarl[node]);
        ui.add(egui::Label::new(title).selectable(false));
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
        self.to_global = to_global;
        let global_rect = to_global * rect.intersect(ui.clip_rect());
        let pointer = ui.input(|i| i.pointer.interact_pos());
        if pointer.is_some_and(|pos| global_rect.contains(pos)) {
            self.pointer_on_node = true;
            if ui.input(|i| i.pointer.primary_pressed()) {
                *self.selected = Some(snarl[node]);
            }
        }
        // The selection mark is the node's own frame (`node_frame` /
        // `header_frame`), not a ring painted here: a ring drawn after the
        // node covered the edge pins.
    }

    fn has_body(&mut self, node: &NodeId) -> bool {
        // Every rendering node gets a thumbnail slot; Output's content IS the
        // screen (and the window title bar already says which mode is live).
        !matches!(
            self.graph.node(*node).map(|n| &n.kind),
            Some(NodeKind::Output) | None
        )
    }

    fn show_body(
        &mut self,
        node: egui_snarl::NodeId,
        _inputs: &[InPin],
        _outputs: &[OutPin],
        ui: &mut egui::Ui,
        snarl: &mut Snarl<NodeId>,
    ) {
        let id = snarl[node];
        match self
            .executor
            .preview_tex(super::super::node::ChainNode::new(self.chain, id))
        {
            Some(tex) => {
                ui.image((tex, PREVIEW_DISPLAY));
            }
            None => {
                // Fixed-size stand-in (first frame after a node appears, or
                // previews not yet running) so the node never changes size.
                let (rect, _) = ui.allocate_exact_size(PREVIEW_DISPLAY, egui::Sense::hover());
                ui.painter()
                    .rect_filled(rect, 2.0, ui.visuals().faint_bg_color);
            }
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
        // Graph primitives (not effect files, so not in the registry).
        ui.menu_button("Utility", |ui| {
            if ui
                .button("Feedback")
                .on_hover_text("One-frame delay — the building block for echo loops")
                .clicked()
            {
                let id = self.graph.add_node(NodeKind::Feedback, 1, &[]);
                snarl.insert_node(pos, id);
                ui.close();
            }
            // At most one per chain (graph.validate enforces it), so offer it
            // only while this chain has none — a refused add would be a worse
            // way to learn the rule.
            let has_input = self.graph.chain_input().is_some();
            if ui
                .add_enabled(!has_input, egui::Button::new("Layer input"))
                .on_hover_text(
                    "The picture this chain was handed — the layer's own output, \
                     or the composited frame on the master chain",
                )
                .on_disabled_hover_text("This chain already has its layer input")
                .clicked()
            {
                let id = self.graph.add_node(NodeKind::ChainInput, 0, &[]);
                snarl.insert_node(pos, id);
                ui.close();
            }
        });
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

/// How close (screen px) the pointer must come to a wire to be offered its
/// remove button.
const WIRE_HOVER_PX: f32 = 8.0;
const WIRE_BUTTON_SIZE: f32 = 16.0;

/// One wire's relation to the pointer this frame.
#[derive(Clone, Copy)]
struct WireHit {
    /// Pointer-to-wire distance in screen pixels.
    distance_px: f32,
    /// Is the pointer on the spot where this wire's button goes?
    on_button: bool,
}

/// Which wire, if any, shows its remove button.
///
/// Being ON a button beats being near a wire: the button sits on its wire's
/// midpoint, and travelling to it can take the pointer closer to a crossing
/// wire — without this the button would jump away as you reached for it.
/// `near_allowed` is false while the pointer is over a node (nodes cover
/// wires) or a press is in progress (dragging a new wire across old ones must
/// not flash buttons), which leaves only a button already under the pointer.
fn wire_to_offer(hits: &[WireHit], near_allowed: bool) -> Option<usize> {
    if let Some(i) = hits.iter().position(|h| h.on_button) {
        return Some(i);
    }
    if !near_allowed {
        return None;
    }
    hits.iter()
        .enumerate()
        .filter(|(_, h)| h.distance_px <= WIRE_HOVER_PX)
        .min_by(|a, b| a.1.distance_px.total_cmp(&b.1.distance_px))
        .map(|(i, _)| i)
}

/// The little "x" on a hovered wire. Painted, not a font glyph (house
/// pattern), and it inverts luminance on hover rather than changing hue. In
/// a foreground layer because snarl's own layer sits above the window's and
/// would both paint over and out-click anything drawn in the window.
fn wire_remove_button(ctx: &egui::Context, center: egui::Pos2) -> bool {
    let size = egui::Vec2::splat(WIRE_BUTTON_SIZE);
    egui::Area::new(egui::Id::new("trama-wire-remove"))
        .order(egui::Order::Foreground)
        .fixed_pos(center - size * 0.5)
        .show(ctx, |ui| {
            let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
            let tc = crate::ui::theme::colors::theme_colors(ctx);
            let (fill, fg) = if resp.hovered() {
                (tc.text_primary, tc.panel)
            } else {
                (tc.panel, tc.text_primary)
            };
            let c = rect.center();
            let painter = ui.painter();
            painter.circle(c, 7.0, fill, egui::Stroke::new(1.0_f32, tc.text_primary));
            let d = 2.75;
            let stroke = egui::Stroke::new(1.5_f32, fg);
            painter.line_segment([c + egui::vec2(-d, -d), c + egui::vec2(d, d)], stroke);
            painter.line_segment([c + egui::vec2(d, -d), c + egui::vec2(-d, d)], stroke);
            resp.on_hover_text("Remove this wire").clicked()
        })
        .inner
}

/// Drawn from `main.rs` between the overlay's `begin_frame`/`end_frame`, the
/// same hosting pattern as the shader editor — `draw_panels` stays untouched.
pub fn draw_trama_window(
    ctx: &egui::Context,
    trama: &mut TramaSystem,
    layer_stack: &mut crate::gpu::layer::LayerStack,
) {
    if !trama.canvas_open {
        return;
    }
    let (pool_in_use, pool_total) = trama.pool_stats();
    let feedback_pairs = trama.feedback_stats();
    let preview_targets = trama.executor.preview_stats();
    let mut open = trama.canvas_open;
    let TramaSystem {
        master,
        registry,
        canvas,
        last_error,
        audio_view,
        executor,
        active_chain,
        canvas_target,
        io,
        ..
    } = trama;
    let active_chain = *active_chain;
    // The canvas edits the selected layer's chain. `App::update` materialized
    // it and set `active_chain` before this frame rendered, so the two agree;
    // the master chain is the fallback when there is no layer to select.
    let host = layer_stack
        .layers
        .iter()
        .position(|l| l.chain.as_ref().is_some_and(|c| c.id == active_chain));
    // The layer tab names the SELECTED layer even while the master tab is
    // showing, so it always says where a click on it will take you.
    let selected = layer_stack.active_layer;
    let layer_tab = match layer_stack.layers.get(selected) {
        Some(l) => {
            let name = l
                .custom_name
                .clone()
                .unwrap_or_else(|| format!("{} (layer {})", l.name, selected + 1));
            format!("Layer: {name}")
        }
        None => "Layer: (none)".to_string(),
    };
    // A master chain post-processes every frame whether or not anyone is
    // looking at it, so its tab carries the count — the one place a forgotten
    // master patch announces itself.
    let master_tab = match master.placed_nodes() {
        0 => "Master".to_string(),
        n => format!("Master ({n})"),
    };
    let graph = match host {
        Some(i) => {
            &mut layer_stack.layers[i]
                .chain
                .as_deref_mut()
                .expect("host was found by having a chain")
                .graph
        }
        None => master,
    };
    // Per-chain view: the snarl holds NodeIds, and every graph numbers its
    // nodes from zero, so one shared snarl shows the previous chain's ids as
    // unresolvable nodes.
    let CanvasState {
        views,
        layouts,
        status,
        last_origin,
    } = canvas;
    let view = views.entry(active_chain).or_insert_with(|| {
        // The view owns the positions from here on.
        let layout = layouts.remove(&active_chain).unwrap_or_default();
        ChainView::new(graph, &layout)
    });
    // Last frame's selection — the inspector draws before the canvas, the
    // standard one-frame egui lag.
    let selected = view.selected;
    egui::Window::new("trama")
        .default_size([1020.0, 520.0])
        .open(&mut open)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                // Tabs say what they are in words; the selected one is
                // marked by egui's fill and strong text, not by hue.
                for (target, text, tip) in [
                    (
                        CanvasTarget::SelectedLayer,
                        &layer_tab,
                        "The selected layer's chain — post-processes that layer \
                         only, and follows the layer panel's selection",
                    ),
                    (
                        CanvasTarget::Master,
                        &master_tab,
                        "The master chain — post-processes the composited frame, \
                         before tonemapping",
                    ),
                ] {
                    if ui
                        .selectable_label(*canvas_target == target, text)
                        .on_hover_text(tip)
                        .clicked()
                    {
                        *canvas_target = target;
                    }
                }
                ui.separator();
                if ui
                    .small_button("Export…")
                    .on_hover_text("Save this chain as a .fio.json file")
                    .clicked()
                {
                    io.export(crate::trama::ser::ChainDoc::capture(graph, |node| {
                        view.snarl
                            .nodes_pos_ids()
                            .find(|(_, _, n)| **n == node)
                            .map(|(_, pos, _)| [pos.x, pos.y])
                    }));
                }
                if ui
                    .small_button("Import…")
                    .on_hover_text("Replace this chain with one from a .fio.json file")
                    .clicked()
                {
                    io.import(active_chain);
                }
                ui.separator();
                ui.weak(format!(
                    "pool {pool_in_use}/{pool_total} · fb {feedback_pairs} · prev {preview_targets}"
                ));
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
            if let Some(err) = last_error.as_deref().or(status.as_deref()) {
                ui.colored_label(ui.visuals().error_fg_color, err);
            }
            if !graph.contributes() {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    "Nothing reaches Output — this chain is inactive and its host's \
                     picture passes through untouched. Right-click the canvas to add \
                     nodes; drag from a pin to wire them.",
                );
            }
            ui.separator();
            // Fixed width (house pattern, ui/panels/mod.rs): the inspector's
            // sliders greedily fill available width, so a resizable panel in
            // an auto-sizing window ratchets the window wider on every
            // selection. 315 px is the budget rows.rs columns are sized for.
            egui::SidePanel::right("trama-inspector")
                .resizable(false)
                .exact_width(315.0)
                .show_inside(ui, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        super::inspector::draw_inspector(ui, graph, registry, audio_view, selected);
                    });
                });
            egui::CentralPanel::default().show_inside(ui, |ui| {
                let origin = ui.next_widget_position();
                let translate = last_origin.map_or(egui::Vec2::ZERO, |o| origin - o);
                *last_origin = Some(origin);
                let tc = crate::ui::theme::colors::theme_colors(ui.ctx());
                let mut viewer = CanvasViewer {
                    graph,
                    chain: active_chain,
                    registry,
                    executor: &*executor,
                    status,
                    selected: &mut view.selected,
                    pointer_on_node: false,
                    translate,
                    selected_stroke: egui::Stroke::new(1.5_f32, tc.text_primary),
                    selected_header_fill: tc.hover_fill,
                    pins: Default::default(),
                    to_global: egui::emath::TSTransform::IDENTITY,
                };
                let background = SnarlWidget::new()
                    .id_salt("trama-canvas")
                    .style(canvas_style(ui.style()))
                    .show(&mut view.snarl, &mut viewer, ui);
                // A completed click on empty canvas clears the selection; the
                // pointer_on_node guard keeps node clicks (which select in
                // `final_node_rect` on press) from immediately deselecting.
                if background.clicked() && !viewer.pointer_on_node {
                    *viewer.selected = None;
                }

                // Hovering a wire offers a remove button at its midpoint.
                // snarl's own affordance is right-click-the-wire, which
                // nobody finds.
                if let Some(pointer) = ui.input(|i| i.pointer.hover_pos()) {
                    let to_global = viewer.to_global;
                    let local = to_global.inverse() * pointer;
                    let wires: Vec<_> = {
                        let pins = viewer.pins.borrow();
                        view.snarl
                            .wires()
                            .filter_map(|(out, inp)| {
                                let from = *pins.outputs.get(&out)?;
                                let to = *pins.inputs.get(&inp)?;
                                let curve =
                                    super::wire_geom::WireCurve::new(from, to, WIRE_FRAME_SIZE);
                                let button = to_global * curve.at(0.5);
                                let hit = WireHit {
                                    distance_px: curve.distance_to(local) * to_global.scaling,
                                    on_button: egui::Rect::from_center_size(
                                        button,
                                        egui::Vec2::splat(WIRE_BUTTON_SIZE),
                                    )
                                    .contains(pointer),
                                };
                                Some((out, inp, button, hit))
                            })
                            .collect()
                    };
                    let hits: Vec<WireHit> = wires.iter().map(|w| w.3).collect();
                    let near_allowed = background.rect.contains(pointer)
                        && !viewer.pointer_on_node
                        && !ui.input(|i| i.pointer.any_down());
                    if let Some(i) = wire_to_offer(&hits, near_allowed) {
                        let (out, inp, button, _) = wires[i];
                        if background.rect.contains(button) && wire_remove_button(ui.ctx(), button)
                        {
                            let to_node = view.snarl[inp.node];
                            viewer
                                .graph
                                .disconnect(to_node, CanvasViewer::pin_of(inp.input));
                            view.snarl.disconnect(out, inp);
                        }
                    }
                }
            });
        });
    trama.canvas_open = open;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trama::node::ChainId;

    fn open_on<'a>(
        canvas: &'a mut CanvasState,
        chain: ChainId,
        graph: &NodeGraph,
    ) -> &'a mut ChainView {
        canvas.open_view(chain, graph)
    }

    #[test]
    fn the_wire_remove_button_does_not_jump_away_as_you_reach_for_it() {
        let hit = |distance_px, on_button| WireHit {
            distance_px,
            on_button,
        };
        // Nothing within reach: no button.
        assert_eq!(wire_to_offer(&[hit(40.0, false)], true), None);
        // Nearest wire within reach wins.
        assert_eq!(
            wire_to_offer(&[hit(6.0, false), hit(2.0, false), hit(30.0, false)], true),
            Some(1)
        );
        // The pointer has reached wire 0's button, and a crossing wire is now
        // nearer than wire 0 itself. The button must stay put.
        assert_eq!(
            wire_to_offer(&[hit(5.0, true), hit(1.0, false)], true),
            Some(0)
        );
        // Over a node, or mid-press (dragging a new wire across old ones):
        // nearness offers nothing...
        assert_eq!(wire_to_offer(&[hit(1.0, false)], false), None);
        // ...but a button already under the pointer survives the press that
        // is about to click it.
        assert_eq!(wire_to_offer(&[hit(1.0, true)], false), Some(0));
    }

    #[test]
    fn a_view_seeded_from_a_wired_graph_draws_its_wires() {
        // A view is rebuilt from its graph whenever it was dropped while the
        // graph lived on. Laying out the nodes but not the wires shows a patch
        // that LOOKS disconnected and still renders connected.
        let mut g = NodeGraph::new_with_output();
        let input = g.add_node(NodeKind::ChainInput, 0, &[]);
        let delay = g.add_node(NodeKind::Feedback, 1, &[]);
        g.connect(input, delay, 0).unwrap();
        g.connect(delay, g.output_node(), 0).unwrap();

        let view = ChainView::new(&g, &[]);
        let drawn: Vec<(NodeId, NodeId, usize)> = view
            .snarl
            .wires()
            .map(|(o, i)| (view.snarl[o.node], view.snarl[i.node], i.input))
            .collect();
        assert_eq!(drawn.len(), g.wires().len(), "every graph wire is drawn");
        for w in g.wires() {
            assert!(
                drawn.contains(&(w.from, w.to, usize::from(w.to_input))),
                "missing {w:?}"
            );
        }
    }

    #[test]
    fn the_master_view_outlives_its_output_target() {
        // A layer chain is dropped because its layer — and so its graph — is
        // gone. The master chain is dropped only because its output target was
        // released (nothing reaches Output any more); its graph is a field of
        // `TramaSystem` and never goes away. Forgetting the view there throws
        // away every node position the moment you pull the last wire.
        let mut master = NodeGraph::new_with_output();
        let mut canvas = CanvasState::default();
        let view = canvas.open_view(ChainId::Master, &master);
        let ci = master.add_node(NodeKind::ChainInput, 0, &[]);
        view.snarl.insert_node(egui::pos2(10.0, 20.0), ci);

        canvas.drop_chain(ChainId::Master);
        assert!(canvas.has_view(ChainId::Master));
    }

    #[test]
    fn each_chain_keeps_its_own_nodes_and_positions() {
        // Every graph numbers its nodes from zero, so a NodeId means a
        // different node in each chain. One shared snarl left the previous
        // layer's nodes on the canvas as unresolvable "?" boxes the moment a
        // different layer was selected.
        let mut a = NodeGraph::new_with_output();
        let b = NodeGraph::new_with_output();

        let mut canvas = CanvasState::default();
        // Placing a node goes through the canvas, which adds it to the graph
        // and the snarl together — `show_graph_menu`'s shape.
        let view_a = open_on(&mut canvas, ChainId::Layer(0), &a);
        let ci = a.add_node(NodeKind::ChainInput, 0, &[]);
        view_a.snarl.insert_node(egui::pos2(10.0, 20.0), ci);
        view_a.selected = Some(ci);
        let a_nodes: Vec<NodeId> = view_a.snarl.node_ids().map(|(_, &id)| id).collect();
        assert_eq!(a_nodes.len(), 2, "Output plus the node just placed");

        // Switching to a chain that has only its Output must show only that.
        let view_b = open_on(&mut canvas, ChainId::Layer(1), &b);
        let b_nodes: Vec<NodeId> = view_b.snarl.node_ids().map(|(_, &id)| id).collect();
        assert_eq!(b_nodes, vec![b.output_node()], "no leftovers from chain A");
        assert_eq!(view_b.selected, None, "selection does not follow either");

        // …and switching back finds chain A exactly as it was left, positions
        // included. That is why the view is retained rather than rebuilt from
        // the graph: positions live only here.
        let view_a = open_on(&mut canvas, ChainId::Layer(0), &a);
        assert_eq!(view_a.selected, Some(ci));
        let placed = view_a
            .snarl
            .node_ids()
            .find(|&(_, &id)| id == ci)
            .map(|(n, _)| view_a.snarl.get_node_info(n).unwrap().pos);
        assert_eq!(placed, Some(egui::pos2(10.0, 20.0)), "position survived");
    }

    #[test]
    fn a_dropped_chain_leaves_no_view_for_the_next_one() {
        // Slots are reused when a layer is removed. A view left behind would
        // reappear under whatever chain lands on that slot next — the same
        // cross-wiring, one level up from the executor's.
        let mut a = NodeGraph::new_with_output();
        let mut canvas = CanvasState::default();
        let view = open_on(&mut canvas, ChainId::Layer(0), &a);
        let ci = a.add_node(NodeKind::ChainInput, 0, &[]);
        view.snarl.insert_node(egui::pos2(10.0, 20.0), ci);
        assert_eq!(canvas.view_count(), 1);

        canvas.drop_chain(ChainId::Layer(0));
        assert_eq!(canvas.view_count(), 0);

        let fresh = NodeGraph::new_with_output();
        let view = open_on(&mut canvas, ChainId::Layer(0), &fresh);
        let ids: Vec<NodeId> = view.snarl.node_ids().map(|(_, &id)| id).collect();
        assert_eq!(ids, vec![fresh.output_node()], "reused slot starts clean");
    }
}
