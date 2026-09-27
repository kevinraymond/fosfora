//! Guided tours (#3126, GH #32): a spotlight on one part of the interface
//! at a time, with a short note beside it.
//!
//! Three pieces:
//!
//! - **Anchors.** A widget that a tour may point at registers where it was
//!   drawn this frame with [`anchor`]. [`Anchor`] is an enum rather than a
//!   string, so a step cannot name something nothing draws: the compiler
//!   holds the names and the shell tests hold the rest (every step's anchor
//!   must land on screen, from a state chosen to hide it).
//! - **Steps.** Each is an anchor, the words, and what to do on arriving at
//!   it: which workspace to open, which collapsed sections to open
//!   ([`reveals`]). A step arranges the interface once, when it opens, and
//!   then leaves it to the user.
//! - **The spotlight.** Everything but the anchor is dimmed and takes no
//!   clicks, so a stray click cannot change what the note describes; the
//!   anchor itself stays live, because trying it is the point. The note goes
//!   on whichever side of the anchor has room.
//!
//! While a tour shows, nothing may take the interface out from under it:
//! the ways out that sit inside a lit area (Edit chain, Bind, loading a
//! preset, the drawer's tabs, making an effect) are switched off with
//! [`is_running`], every keyboard shortcut but Esc is ignored, and
//! [`gate`] drops any request to open another view that still gets
//! through. The one view a tour opens itself is the binding matrix: a step
//! says whether it wants it ([`matrix_wanted`]), and the matrix follows.
//!
//! A tour runs only in the workspace layout: the Classic panels register no
//! anchors. Finishing or skipping one sends `tour_done` with its key, which
//! `main.rs` records in settings, so the First run tour starts by itself
//! once and afterwards only from the Tours menu or Setup › Tutorials.

use egui::{Align2, Area, Context, Id, Order, Pos2, Rect, RichText, Sense, Ui, Vec2};

use super::theme::colors::theme_colors;
use super::theme::tokens::{BODY_SIZE, SMALL_SIZE};

/// A part of the interface a tour can point at.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Anchor {
    /// The audio input picker and the meters under it.
    AudioInput,
    /// The catalog tab of the bottom drawer.
    Catalog,
    /// The layer stack, Master included.
    Stack,
    /// The middle column: whatever is selected in the stack.
    Inspector,
    /// The Presets section, where the stack is saved.
    Presets,
    /// The selected layer's Parameters section in the inspector.
    Parameters,
    /// The first row of the Parameters section: a control and its Bind,
    /// M and O.
    ParamRow,
    /// The binding matrix's left column: what can drive a control.
    MatrixSources,
    /// The binding matrix's middle column: one card per binding.
    MatrixCards,
    /// The binding matrix's right column: what a binding can drive.
    MatrixTargets,
    /// The binding matrix's Preset and Global tabs, and Templates.
    MatrixScope,
}

/// The tours there are.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tour {
    FirstRun,
    Bindings,
}

impl Tour {
    pub const ALL: &[Tour] = &[Tour::FirstRun, Tour::Bindings];

    /// The name settings records it under once it is finished or skipped.
    /// Never change one: a renamed key replays the tour for everyone.
    pub fn key(self) -> &'static str {
        match self {
            Tour::FirstRun => "first_run",
            Tour::Bindings => "bindings",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Tour::FirstRun => "First run",
            Tour::Bindings => "Bindings",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Tour::FirstRun => {
                "Choose what it listens to, find an effect, read the stack, shape it, save it."
            }
            Tour::Bindings => "Make a control follow the bass, a MIDI knob or an OSC message.",
        }
    }

    pub fn steps(self) -> &'static [Step] {
        match self {
            Tour::FirstRun => FIRST_RUN,
            Tour::Bindings => BINDINGS,
        }
    }
}

/// What a step does with the binding matrix.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Matrix {
    Shut,
    Open,
    /// Open, and the step makes one binding: the first source clicked makes
    /// it, and each click after that changes its source, so the card the
    /// next steps describe is the last source picked.
    PickSource,
}

/// One stop on a tour.
pub struct Step {
    pub anchor: Anchor,
    pub title: &'static str,
    pub body: &'static str,
    /// Arrange the interface so the anchor is drawn: run once, as the step
    /// opens. Anything inside a collapsible section also needs the section
    /// in `reveal`.
    pub prepare: fn(&Context),
    /// Collapsible sections to open while this step shows, by the id their
    /// draw call gives them.
    pub reveal: &'static [&'static str],
    /// Whether the binding matrix shows during this step. The tour holds it
    /// open or shut: the matrix's own ways to close are off meanwhile.
    pub matrix: Matrix,
    /// Drawn in the note under the words, in an inset: a thing the step
    /// describes, drawn by the code that draws it for real, so the picture
    /// cannot drift from the interface.
    pub picture: Option<fn(&mut Ui)>,
}

/// Build with the drawer showing the catalog and a layer in the inspector.
fn to_build(ctx: &Context) {
    super::shell::Workspace::Build.write(ctx);
    super::panels::stack_panel::show_layer_in_inspector(ctx);
}

/// The catalog, with Layer 1 selected for the click to load into.
fn to_catalog(ctx: &Context) {
    to_build(ctx);
    super::shell::show_catalog(ctx);
    super::panels::stack_panel::select_layer(ctx, 0);
}

/// The First run tour: the five steps of the M0 mockup (board E), in the
/// order someone setting up for the first time needs them.
const FIRST_RUN: &[Step] = &[
    Step {
        anchor: Anchor::AudioInput,
        title: "Choose what it listens to",
        body: "Pick the audio input here. The meters under it move when sound arrives; \
               if they stay flat, try another input.",
        prepare: to_build,
        reveal: &["v2_audio"],
        matrix: Matrix::Shut,
        picture: None,
    },
    Step {
        anchor: Anchor::Catalog,
        title: "Find an effect",
        body: "The catalog holds every effect. Browse by family or search, then click a \
               picture to load it into Layer 1. During the tour only a click works; \
               afterwards you can also drag a picture onto the stack to add a layer.",
        prepare: to_catalog,
        reveal: &[],
        matrix: Matrix::Shut,
        picture: None,
    },
    Step {
        anchor: Anchor::Stack,
        title: "Read the stack bottom to top",
        body: "Each picture is the output so far. Layer 1's is its effect blended onto \
               the layers beneath it, and the small inset is Layer 1 on its own. Master, \
               at the top, is what goes out. Click a row to edit it.",
        prepare: to_build,
        reveal: &[],
        matrix: Matrix::Shut,
        picture: None,
    },
    Step {
        anchor: Anchor::Inspector,
        title: "Shape it",
        body: "The inspector shows whatever you selected in the stack: how the layer \
               blends first, then the effect's own controls. After the tour, Edit chain \
               (C) adds node effects to the layer.",
        prepare: to_build,
        reveal: &[],
        matrix: Matrix::Shut,
        picture: None,
    },
    Step {
        anchor: Anchor::Presets,
        title: "Keep it",
        body: "Name the preset and press Save. A preset keeps the whole stack: layers, \
               chains and Master. Replay this tour from Setup › Tutorials.",
        prepare: to_build,
        reveal: &["sec_presets"],
        matrix: Matrix::Shut,
        picture: None,
    },
];

/// Build, with the topmost layer that runs an effect in the inspector: the
/// launch stack's Layer 1 is empty, and an empty layer has nothing to bind.
fn to_effect_layer(ctx: &Context) {
    to_build(ctx);
    ctx.data_mut(|d| d.insert_temp(Id::new(SELECT_EFFECT_LAYER), true));
}

/// The matrix, with the selected layer's first control picked as the
/// target, so one click on a source makes a binding.
fn to_matrix_armed(ctx: &Context) {
    to_build(ctx);
    ctx.data_mut(|d| d.insert_temp(Id::new(ARM_FIRST_PARAM), true));
}

/// Asks the shell to select the topmost layer that runs an effect.
pub const SELECT_EFFECT_LAYER: &str = "tour_select_effect_layer";
/// Asks the binding matrix to pick the selected layer's first control as
/// the target.
pub const ARM_FIRST_PARAM: &str = "tour_arm_first_param";

/// The Bindings tour: the M0 mockup's "make a parameter follow the bass, a
/// MIDI knob or an OSC message", from the control to the matrix and back.
const BINDINGS: &[Step] = &[
    Step {
        anchor: Anchor::Parameters,
        title: "Any control can move by itself",
        body: "A layer's controls can follow the music, a MIDI knob or an OSC message. \
               After the tour, Bind on a row starts a binding for that control, and Open \
               bindings under the section shows them all (B). A control that follows \
               something names it on its row, like this one following the bass:",
        prepare: to_effect_layer,
        reveal: &["v2_params"],
        matrix: Matrix::Shut,
        picture: Some(super::panels::param_panel::draw_example_row),
    },
    Step {
        anchor: Anchor::MatrixSources,
        title: "Pick what drives it",
        body: "This is the binding matrix, opened with the layer's first control picked \
               as the target (the line at the bottom says which). The left column is \
               everything that can drive it: the audio analysis, MIDI, OSC and camera \
               tracking. Click a source, Bass for one, to make the binding. Changed your \
               mind? Click another.",
        prepare: to_matrix_armed,
        reveal: &[],
        matrix: Matrix::PickSource,
        picture: None,
    },
    Step {
        anchor: Anchor::MatrixCards,
        title: "Shape it",
        body: "Each binding is a card: source, then target. Click a card to open it and \
               shape the response: the range it moves the control through, smoothing, \
               invert, a gate. Untick Enabled on an open card to turn it off without \
               losing it.",
        prepare: to_build,
        reveal: &[],
        matrix: Matrix::Open,
        picture: None,
    },
    Step {
        anchor: Anchor::MatrixTargets,
        title: "Or start from the other side",
        body: "The right column is everything a binding can drive: each layer's \
               controls, how a layer blends, post-processing, particles and the scene \
               transport. Click a target, then a source: the order doesn't matter.",
        prepare: to_build,
        reveal: &[],
        matrix: Matrix::Open,
        picture: None,
    },
    Step {
        anchor: Anchor::MatrixScope,
        title: "This preset, or every preset",
        body: "Preset bindings are saved with the preset when you press Save. Global \
               ones stay whichever preset you load, which suits a controller's knobs. \
               Templates add a ready-made set to the selected layer.",
        prepare: to_build,
        reveal: &[],
        matrix: Matrix::Open,
        picture: None,
    },
    Step {
        anchor: Anchor::ParamRow,
        title: "The quick path for one knob",
        body: "M learns a MIDI control: click it, then move a knob. O does the same for \
               an OSC address. Those follow the selection: the knob drives the control \
               of that name on whichever layer is selected. A binding stays on its \
               layer and can be shaped. Replay this tour from Setup \u{203a} Tutorials.",
        prepare: to_build,
        reveal: &["v2_params"],
        matrix: Matrix::Shut,
        picture: None,
    },
];

// ── State ─────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Debug)]
struct Running {
    tour: Tour,
    step: usize,
    /// The step's anchor has been scrolled into view.
    scrolled: bool,
    /// The step has not been drawn yet.
    fresh: bool,
}

fn state_id() -> Id {
    Id::new("tour_running")
}

fn running(ctx: &Context) -> Option<Running> {
    ctx.data(|d| d.get_temp::<Running>(state_id()))
}

/// The tour showing and the index of its step, if one is.
pub fn current(ctx: &Context) -> Option<(Tour, usize)> {
    running(ctx).map(|r| (r.tour, r.step))
}

/// Is a tour showing? Controls that would leave it are off while it is.
pub fn is_running(ctx: &Context) -> bool {
    running(ctx).is_some()
}

/// The hover text of a control switched off during a tour.
pub const NOT_DURING: &str = "Not during the tour";

/// Drop the requests that would open another view over the one a step
/// describes (the chain editor, the bindings, the shader editor, another
/// preset, full output) or reshape the stack it describes (adding,
/// removing or clearing layers). The controls that send them are off during a tour;
/// this catches whatever path still gets one through. Each is removed as
/// the type its reader reads it: a different type would remove nothing.
///
/// Call it after the interface is drawn and before anything reads those
/// requests: in `main.rs`, before the modals.
pub fn gate(ctx: &Context) {
    if !is_running(ctx) {
        return;
    }
    ctx.data_mut(|d| {
        for key in LEAVING_USIZE {
            d.remove::<usize>(Id::new(key));
        }
        for key in LEAVING_BOOL {
            d.remove::<bool>(Id::new(key));
        }
        d.remove::<u32>(Id::new(LEAVING_U32));
        d.remove::<(usize, String)>(Id::new(LEAVING_PARAM));
    });
}

/// The requests [`gate`] drops, by the type each is sent as.
const LEAVING_USIZE: [&str; 3] = ["open_trama_on_layer", "pending_preset", "remove_layer"];
const LEAVING_BOOL: [&str; 9] = [
    "open_trama_on_master",
    "open_binding_matrix",
    "new_effect_prompt",
    "copy_builtin_prompt",
    "open_shader_editor",
    "request_hide_overlay",
    "add_layer",
    "add_media_layer",
    "clear_all_layers",
];
const LEAVING_U32: &str = "add_webcam_layer";
/// A control's Bind: (layer, control name).
const LEAVING_PARAM: &str = "bind_param";

/// Has anything drawn this frame asked to leave the view? For tests, which
/// look before [`gate`] drops the request.
#[cfg(test)]
pub(crate) fn leaving_requested(ctx: &Context) -> Option<&'static str> {
    ctx.data(|d| {
        LEAVING_USIZE
            .into_iter()
            .find(|k| d.get_temp::<usize>(Id::new(*k)).is_some())
            .or_else(|| {
                LEAVING_BOOL
                    .into_iter()
                    .find(|k| d.get_temp::<bool>(Id::new(*k)).is_some())
            })
            .or_else(|| d.get_temp::<u32>(Id::new(LEAVING_U32)).map(|_| LEAVING_U32))
            .or_else(|| {
                d.get_temp::<(usize, String)>(Id::new(LEAVING_PARAM))
                    .map(|_| LEAVING_PARAM)
            })
    })
}

/// Should the binding matrix show? `None` when no tour is showing (the
/// matrix is the user's), else whether the step showing wants it.
pub fn matrix_wanted(ctx: &Context) -> Option<bool> {
    running(ctx)
        .and_then(step_of)
        .map(|s| s.matrix != Matrix::Shut)
}

/// Is the step showing one where a source click picks the source of the
/// step's own binding ([`Matrix::PickSource`])?
pub fn picks_source(ctx: &Context) -> bool {
    running(ctx)
        .and_then(step_of)
        .is_some_and(|s| s.matrix == Matrix::PickSource)
}

/// Start `tour` from its first step.
pub fn start(ctx: &Context, tour: Tour) {
    go_to(ctx, tour, 0);
}

fn go_to(ctx: &Context, tour: Tour, step: usize) {
    let Some(s) = tour.steps().get(step) else {
        return;
    };
    (s.prepare)(ctx);
    ctx.data_mut(|d| {
        d.insert_temp(
            state_id(),
            Running {
                tour,
                step,
                scrolled: false,
                fresh: true,
            },
        );
    });
}

/// Stop the tour showing, if any, and record it as seen: finishing and
/// skipping both mean "don't start this by itself again".
pub fn end(ctx: &Context) {
    if let Some(r) = running(ctx) {
        ctx.data_mut(|d| {
            d.remove::<Running>(state_id());
            d.insert_temp(Id::new("tour_done"), r.tour.key());
        });
    }
}

fn step_of(r: Running) -> Option<&'static Step> {
    r.tour.steps().get(r.step)
}

/// Must the collapsible section `id` be open for the step showing? Sections
/// call this and open themselves (and stay open: the user can close them
/// again, but not while the step needs them).
pub fn reveals(ctx: &Context, id: &str) -> bool {
    running(ctx)
        .and_then(step_of)
        .is_some_and(|s| s.reveal.contains(&id))
}

// ── Anchors ───────────────────────────────────────────────────────────

fn anchors_id() -> Id {
    Id::new("tour_anchors")
}

type Anchors = (u64, Vec<(Anchor, Rect)>);

/// Record that `a` was drawn at `rect` this frame. Only the part inside
/// `ui`'s clip shows, so only that part is recorded; an anchor drawn in
/// pieces (the input picker, then the meters) records their union. When the
/// tour is pointing at it, the first registration scrolls it into view.
pub fn anchor(ui: &Ui, a: Anchor, rect: Rect) {
    let ctx = ui.ctx();
    let Some(r) = running(ctx) else {
        return;
    };
    let shown = rect.intersect(ui.clip_rect());
    if !shown.is_positive() {
        // Scrolled out of sight: still worth scrolling to.
        if step_of(r).is_some_and(|s| s.anchor == a) && !r.scrolled {
            ui.scroll_to_rect(rect, None);
            mark_scrolled(ctx, r);
        }
        return;
    }
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|d| {
        let (p, list) = d.get_temp_mut_or_default::<Anchors>(anchors_id());
        if *p != pass {
            *p = pass;
            list.clear();
        }
        match list.iter_mut().find(|(k, _)| *k == a) {
            Some((_, r)) => *r = r.union(shown),
            None => list.push((a, shown)),
        }
    });
    if step_of(r).is_some_and(|s| s.anchor == a) && !r.scrolled {
        ui.scroll_to_rect(rect, None);
        mark_scrolled(ctx, r);
    }
}

fn mark_scrolled(ctx: &Context, r: Running) {
    ctx.data_mut(|d| {
        d.insert_temp(
            state_id(),
            Running {
                scrolled: true,
                ..r
            },
        );
    });
}

/// Where `a` was drawn this frame, if it was.
pub fn anchor_rect(ctx: &Context, a: Anchor) -> Option<Rect> {
    let pass = ctx.cumulative_pass_nr();
    ctx.data(|d| d.get_temp::<Anchors>(anchors_id()))
        .filter(|(p, _)| *p == pass)
        .and_then(|(_, list)| list.into_iter().find(|(k, _)| *k == a))
        .map(|(_, r)| r)
}

// ── The spotlight ─────────────────────────────────────────────────────

/// How far the lit hole reaches past its anchor.
const HOLE_PAD: f32 = 6.0;
/// The note's width.
pub const CALLOUT_WIDTH: f32 = 340.0;
/// Between the hole and the note, and between the note and the window edge.
const GAP: f32 = 14.0;
/// Over everything but the anchor. Darker than the modals' backdrop: egui
/// blends in linear light, where that one takes a Gray panel only from 72
/// to 37 and a Light one from 255 to about 190, and here the rest of the
/// window should drop back.
const DIM: egui::Color32 = egui::Color32::from_black_alpha(215);

/// The hole cut for an anchor: the anchor and a margin, inside the window.
pub fn hole(anchor: Rect, screen: Rect) -> Rect {
    anchor.expand(HOLE_PAD).intersect(screen)
}

/// Where the note goes: beside the hole, on the side with the most room
/// that the note fits, with room to spare, never over the hole. When no
/// side has room (the anchor fills the window) it sits inside the hole's
/// lower right corner.
pub fn place_callout(hole: Rect, size: Vec2, screen: Rect) -> Rect {
    let room = [
        (hole.left() - screen.left(), size.x),
        (screen.right() - hole.right(), size.x),
        (hole.top() - screen.top(), size.y),
        (screen.bottom() - hole.bottom(), size.y),
    ];
    let best = (0..4)
        .filter(|&i| room[i].0 >= room[i].1 + 2.0 * GAP)
        .max_by(|&a, &b| {
            // Compare the room left over, so a tall note prefers a side.
            (room[a].0 - room[a].1).total_cmp(&(room[b].0 - room[b].1))
        });
    let clamp_x = |x: f32| {
        x.clamp(
            screen.left() + GAP,
            (screen.right() - GAP - size.x).max(screen.left() + GAP),
        )
    };
    let clamp_y = |y: f32| {
        y.clamp(
            screen.top() + GAP,
            (screen.bottom() - GAP - size.y).max(screen.top() + GAP),
        )
    };
    let min = match best {
        Some(0) => Pos2::new(
            hole.left() - GAP - size.x,
            clamp_y(hole.center().y - size.y / 2.0),
        ),
        Some(1) => Pos2::new(hole.right() + GAP, clamp_y(hole.center().y - size.y / 2.0)),
        Some(2) => Pos2::new(
            clamp_x(hole.center().x - size.x / 2.0),
            hole.top() - GAP - size.y,
        ),
        Some(_) => Pos2::new(clamp_x(hole.center().x - size.x / 2.0), hole.bottom() + GAP),
        None => Pos2::new(
            clamp_x(hole.right() - GAP - size.x),
            clamp_y(hole.bottom() - GAP - size.y),
        ),
    };
    Rect::from_min_size(min, size)
}

/// The four rectangles of `screen` around `hole`: above, below, left, right.
fn around(hole: Rect, screen: Rect) -> [Rect; 4] {
    [
        Rect::from_min_max(screen.min, Pos2::new(screen.right(), hole.top())),
        Rect::from_min_max(Pos2::new(screen.left(), hole.bottom()), screen.max),
        Rect::from_min_max(
            Pos2::new(screen.left(), hole.top()),
            Pos2::new(hole.left(), hole.bottom()),
        ),
        Rect::from_min_max(
            Pos2::new(hole.right(), hole.top()),
            Pos2::new(screen.right(), hole.bottom()),
        ),
    ]
}

/// Draw the tour showing, if any. Call after everything it points at has
/// been drawn this frame, the modals included, and after [`gate`].
pub fn draw(ctx: &Context) {
    let Some(r) = running(ctx) else {
        return;
    };
    let Some(step) = step_of(r) else {
        end(ctx);
        return;
    };
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        end(ctx);
        return;
    }
    let tc = theme_colors(ctx);
    let screen = ctx.content_rect();
    // Not drawn this frame (the step has just opened a workspace): dim it
    // all and put the note in the middle until it is.
    let lit = anchor_rect(ctx, step.anchor).map(|a| hole(a, screen));

    // Each dimmed piece is its own area so that the hole has none: an area
    // over the hole would take the pointer there, clicks and scrolling both.
    // Middle, so menus and pop-ups opened from inside the hole (the input
    // picker's list) are drawn over the dimming and take their own clicks
    // by their order, not only because they opened last. The binding
    // matrix draws at Middle too while a tour shows, and opens after the
    // dimming exists: every frame the dimming asks to be on top again, and
    // egui's end-of-frame sort is stable, so it stays over the matrix even
    // on a frame the matrix is clicked and asks the same.
    let pieces = match lit {
        Some(h) => around(h, screen).to_vec(),
        None => vec![screen],
    };
    for (i, piece) in pieces.iter().enumerate() {
        if !piece.is_positive() {
            continue;
        }
        let dim = Area::new(Id::new("tour_dim").with(i))
            .order(Order::Middle)
            .fixed_pos(piece.min)
            .constrain(false)
            .show(ctx, |ui| {
                let (rect, _) = ui.allocate_exact_size(piece.size(), Sense::click_and_drag());
                ui.painter().rect_filled(rect, 0.0, DIM);
            });
        ctx.move_to_top(dim.response.layer_id);
    }

    let size = Vec2::new(CALLOUT_WIDTH, callout_height(ctx));
    let at = match lit {
        Some(h) => place_callout(h, size, screen),
        None => Rect::from_center_size(screen.center(), size),
    };
    let steps = r.tour.steps().len();
    let mut go: Option<Option<usize>> = None;
    let callout = Area::new(Id::new("tour_callout"))
        .order(Order::Foreground)
        .fixed_pos(at.min)
        .constrain(true)
        .pivot(Align2::LEFT_TOP)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style())
                .inner_margin(egui::Margin::same(14))
                .show(ui, |ui| {
                    ui.set_width(CALLOUT_WIDTH - 28.0);
                    ui.label(
                        RichText::new(format!(
                            "{} · step {} of {steps}",
                            r.tour.name(),
                            r.step + 1
                        ))
                        .size(SMALL_SIZE)
                        .color(tc.text_secondary),
                    );
                    ui.label(RichText::new(step.title).size(18.0).strong());
                    ui.add_space(2.0);
                    ui.label(RichText::new(step.body).size(BODY_SIZE));
                    if let Some(picture) = step.picture {
                        ui.add_space(6.0);
                        egui::Frame::new()
                            .fill(tc.card_bg)
                            .stroke(egui::Stroke::new(1.0_f32, tc.card_border))
                            .corner_radius(4.0)
                            .inner_margin(egui::Margin::symmetric(8, 6))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                picture(ui);
                            });
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Skip tour").clicked() {
                            go = Some(None);
                        }
                        let right = egui::Layout::right_to_left(egui::Align::Center);
                        super::widgets::layout_row(ui, right, |ui| {
                            let last = r.step + 1 == steps;
                            let next = ui.button(
                                RichText::new(if last { "Finish" } else { "Next" }).strong(),
                            );
                            if r.fresh {
                                // Keyboard users land on the way forward.
                                next.request_focus();
                            }
                            if next.clicked() {
                                go = Some((!last).then_some(r.step + 1));
                            }
                            if r.step > 0 && ui.button("Back").clicked() {
                                go = Some(Some(r.step - 1));
                            }
                        });
                    });
                });
        });
    ctx.data_mut(|d| {
        d.insert_temp(Id::new("tour_callout_h"), callout.response.rect.height());
        if let Some(now) = d.get_temp::<Running>(state_id()) {
            d.insert_temp(
                state_id(),
                Running {
                    fresh: false,
                    ..now
                },
            );
        }
    });

    // The lit edge, drawn over the dimming: the selection color inside its
    // own text color, so it reads on a dark surround and a light one alike.
    if let Some(h) = lit {
        let p = ctx.layer_painter(egui::LayerId::new(Order::Foreground, Id::new("tour_ring")));
        for (w, c) in [(4.0_f32, tc.on_selection), (2.0_f32, tc.selection)] {
            p.rect_stroke(h, 6.0, egui::Stroke::new(w, c), egui::StrokeKind::Outside);
        }
    }

    match go {
        Some(Some(i)) => go_to(ctx, r.tour, i),
        Some(None) => end(ctx),
        None => {}
    }
}

/// The note's height as last drawn, or a guess before the first frame.
fn callout_height(ctx: &Context) -> f32 {
    ctx.data(|d| d.get_temp::<f32>(Id::new("tour_callout_h")))
        .unwrap_or(200.0)
}

/// Should the First run tour start by itself? Once, in the workspace
/// layout, for someone who has neither finished nor skipped it.
pub fn should_auto_start(settings: &crate::settings::SettingsConfig) -> bool {
    !settings.classic_layout
        && !settings
            .tours_done
            .iter()
            .any(|k| k == Tour::FirstRun.key())
}

/// The tours, each with a way to start it: Setup › Tutorials.
pub fn draw_tutorials(ui: &mut Ui) {
    let tc = theme_colors(ui.ctx());
    for &t in Tour::ALL {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new(t.name()).size(BODY_SIZE).strong());
                ui.label(
                    RichText::new(format!("{} {} steps.", t.description(), t.steps().len()))
                        .size(SMALL_SIZE)
                        .color(tc.text_secondary),
                );
            });
        });
        if ui.button(format!("Start {}", t.name())).clicked() {
            start(ui.ctx(), t);
        }
        ui.add_space(6.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::shell::Workspace;
    use crate::ui::shell_harness::ShellHarness;
    use egui::Key;

    /// Where `a` was drawn in the frame that just ended.
    fn drawn(ctx: &Context, a: Anchor) -> Option<Rect> {
        let pass = ctx.cumulative_pass_nr();
        ctx.data(|d| d.get_temp::<Anchors>(anchors_id()))
            .filter(|(p, _)| p + 1 == pass)
            .and_then(|(_, list)| list.into_iter().find(|(k, _)| *k == a))
            .map(|(_, r)| r)
    }

    fn callout_rect(ctx: &Context) -> Option<Rect> {
        ctx.memory(|m| m.area_rect(Id::new("tour_callout")))
    }

    /// Put the interface where a step's anchor is hidden: every section a
    /// step opens closed, Setup open, Build's drawer shut on Scenes, Master
    /// in the inspector.
    fn hide_everything(h: &mut ShellHarness) {
        h.settle(2);
        let revealed: Vec<&str> = Tour::ALL
            .iter()
            .flat_map(|t| t.steps())
            .flat_map(|s| s.reveal.iter().copied())
            .collect();
        for name in &revealed {
            crate::ui::widgets::close_section(&h.ctx, name);
        }
        h.settle(2);
        Workspace::Setup.write(&h.ctx);
        h.ctx.data_mut(|d| {
            d.insert_temp(Id::new("v2_drawer_open").with("build"), false);
        });
        h.ctx
            .data_mut(|d| d.insert_temp(Id::new("v2_master_selected"), true));
        h.settle(2);
    }

    // The tour is only as good as its anchors: a step pointing at something
    // that is not drawn, is collapsed, or is off screen shows a note about
    // nothing. Each step is walked from a state that hides its anchor, and
    // advanced the way a keyboard user would.
    #[test]
    fn every_step_lights_its_anchor_on_screen() {
        for size in [Vec2::new(1206.0, 760.0), Vec2::new(1920.0, 1080.0)] {
            for &tour in Tour::ALL {
                let mut h = ShellHarness::new(size);
                hide_everything(&mut h);
                start(&h.ctx, tour);
                for (i, step) in tour.steps().iter().enumerate() {
                    // Half a second: scrolling to an anchor is animated.
                    h.settle(30);
                    assert_eq!(current(&h.ctx), Some((tour, i)), "{size:?}");
                    let at = format!("{size:?} {} step {}", tour.name(), i + 1);
                    let a = drawn(&h.ctx, step.anchor)
                        .unwrap_or_else(|| panic!("{at}: {:?} was not drawn", step.anchor));
                    let screen = h.screen();
                    assert!(
                        a.width() >= 120.0 && a.height() >= 16.0,
                        "{at}: {:?} is only {a:?}",
                        step.anchor
                    );
                    assert!(screen.contains_rect(a), "{at}: {a:?} is off screen");
                    let note = callout_rect(&h.ctx).expect("the note is drawn");
                    assert!(
                        screen.contains_rect(note),
                        "{at}: the note {note:?} is off screen"
                    );
                    assert!(
                        (120.0..320.0).contains(&note.height()),
                        "{at}: the note is {} tall",
                        note.height()
                    );
                    assert!(
                        !note.intersects(hole(a, screen)),
                        "{at}: the note {note:?} covers {a:?}"
                    );
                    h.key(Key::Enter);
                }
                h.settle(1);
                assert_eq!(current(&h.ctx), None, "{} did not finish", tour.name());
                let done = h.ctx.data(|d| d.get_temp::<&str>(Id::new("tour_done")));
                assert_eq!(done, Some(tour.key()));
            }
        }
    }

    // Kevin's live check found ways out of a step inside the lit area
    // itself: Edit chain opened the chain editor over the tour, a preset
    // tile loaded another stack, the stack's own buttons added and cleared
    // layers under the step that explains them, and a drag from the catalog
    // did nothing the note explained. Click all over each lit area, as someone trying
    // things would: nothing may ask to leave, and the step must still show
    // its anchor.
    #[test]
    fn nothing_in_a_lit_area_leaves_the_step() {
        for &tour in Tour::ALL {
            clicking_inside_stays(tour);
        }
    }

    fn clicking_inside_stays(tour: Tour) {
        let mut h = ShellHarness::new(Vec2::new(1400.0, 900.0));
        start(&h.ctx, tour);
        for (i, step) in tour.steps().iter().enumerate() {
            h.settle(30);
            let lit = drawn(&h.ctx, step.anchor).unwrap();
            let mut y = lit.top() + 6.0;
            while y < lit.bottom() {
                let mut x = lit.left() + 6.0;
                while x < lit.right() {
                    h.click(Pos2::new(x, y));
                    h.settle(1);
                    x += 30.0;
                }
                y += 30.0;
            }
            let at = format!("{} step {}", tour.name(), i + 1);
            assert_eq!(h.leaving, None, "{at}: a click inside asked to leave");
            assert_eq!(current(&h.ctx), Some((tour, i)));
            h.settle(30);
            assert!(
                drawn(&h.ctx, step.anchor).is_some(),
                "{at}: clicking inside hid {:?}",
                step.anchor
            );
            assert_eq!(
                h.matrix.open,
                step.matrix != Matrix::Shut,
                "{at}: the matrix"
            );
            // As Next does: the clicks have moved the keyboard focus.
            go_to(&h.ctx, tour, i + 1);
        }
    }

    // The catalog step loads into Layer 1: the empty layer on top of the
    // launch stack, so the click shows as a blend in the next step.
    #[test]
    fn the_catalog_step_selects_layer_1() {
        let mut h = ShellHarness::new(Vec2::new(1400.0, 900.0));
        h.ctx
            .data_mut(|d| d.insert_temp(Id::new("v2_master_selected"), true));
        go_to(&h.ctx, Tour::FirstRun, 1);
        assert_eq!(
            h.ctx.data(|d| d.get_temp::<usize>(Id::new("select_layer"))),
            Some(0)
        );
        h.settle(3);
        assert!(!crate::ui::panels::stack_panel::master_selected(&h.ctx));
    }

    // A way out the lit areas don't switch off (a right-click menu, a path
    // added later) is still stopped: the tour drops the request before
    // main.rs reads it, with the type main.rs reads it as.
    #[test]
    fn requests_to_leave_are_dropped_during_a_tour() {
        let mut h = ShellHarness::new(Vec2::new(1400.0, 900.0));
        start(&h.ctx, Tour::FirstRun);
        h.settle(3);
        h.ctx.data_mut(|d| {
            d.insert_temp(Id::new("open_trama_on_layer"), 0usize);
            d.insert_temp(Id::new("pending_preset"), 1usize);
            d.insert_temp(Id::new("open_binding_matrix"), true);
            d.insert_temp(Id::new("open_shader_editor"), true);
            d.insert_temp(Id::new("bind_param"), (1usize, "trail_decay".to_string()));
        });
        h.settle(1);
        assert_eq!(leaving_requested(&h.ctx), None);
        assert!(!h.matrix.open, "a request opened the matrix mid-tour");
    }

    // The click test shows the dimming is there; this shows it is seen:
    // painted everywhere but the lit part.
    #[test]
    fn everything_but_the_anchor_is_dimmed() {
        let mut h = ShellHarness::new(Vec2::new(1400.0, 900.0));
        start(&h.ctx, Tour::FirstRun);
        h.settle(30);
        let out = h.frame_output(vec![]);
        let lit = hole(drawn(&h.ctx, Anchor::AudioInput).unwrap(), h.screen());
        let dims: Vec<Rect> = out
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Rect(r) if r.fill == DIM => Some(r.rect),
                _ => None,
            })
            .collect();
        let covered: f32 = dims.iter().map(|r| r.area()).sum();
        let want = h.screen().area() - lit.area();
        assert!(
            (covered - want).abs() < 1.0,
            "dimmed {covered}, want {want}"
        );
        assert!(dims.iter().all(|r| !r.intersects(lit.shrink(0.5))));
    }

    #[test]
    fn escape_and_skip_end_the_tour_and_record_it() {
        let mut h = ShellHarness::new(Vec2::new(1400.0, 900.0));
        start(&h.ctx, Tour::FirstRun);
        h.settle(3);
        h.key(Key::Escape);
        assert_eq!(current(&h.ctx), None);
        assert_eq!(
            h.ctx.data(|d| d.get_temp::<&str>(Id::new("tour_done"))),
            Some("first_run")
        );

        let mut h = ShellHarness::new(Vec2::new(1400.0, 900.0));
        start(&h.ctx, Tour::FirstRun);
        h.settle(3);
        h.key(Key::Enter);
        h.settle(3);
        assert_eq!(current(&h.ctx), Some((Tour::FirstRun, 1)));
        // Skip is the note's first button.
        let note = callout_rect(&h.ctx).unwrap();
        let skip = h.ctx.memory(|m| m.focused());
        assert!(skip.is_some(), "Next has the keyboard");
        h.click(Pos2::new(note.left() + 40.0, note.bottom() - 26.0));
        assert_eq!(current(&h.ctx), None, "Skip tour ended it");
    }

    /// A tour showing, with the anchor a combo box at the top left and a
    /// button outside it. Returns (button clicks, the combo's value).
    fn spotlight_clicks(at: Pos2, open_combo: bool) -> (usize, usize) {
        let ctx = Context::default();
        let (mut clicks, mut value) = (0, 0usize);
        let mut frame = |events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 700.0))),
                events,
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let combo = egui::ComboBox::from_id_salt("t")
                        .selected_text(format!("input {value}"))
                        .show_ui(ui, |ui| {
                            for i in 0..4 {
                                ui.selectable_value(&mut value, i, format!("input {i}"));
                            }
                        });
                    anchor(ui, Anchor::AudioInput, combo.response.rect);
                    ui.add_space(200.0);
                    if ui.button("outside").clicked() {
                        clicks += 1;
                    }
                });
                draw(ctx);
            });
        };
        start(&ctx, Tour::FirstRun);
        for _ in 0..3 {
            frame(vec![]);
        }
        let click = |pos| {
            let b = move |pressed| egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            };
            [
                vec![egui::Event::PointerMoved(pos), b(true)],
                vec![b(false)],
            ]
        };
        let combo = drawn(&ctx, Anchor::AudioInput).unwrap();
        if open_combo {
            for ev in click(combo.center()) {
                frame(ev);
            }
            frame(vec![]);
        }
        for ev in click(at) {
            frame(ev);
        }
        frame(vec![]);
        (clicks, value)
    }

    // The dimming must take every click outside the lit part and none
    // inside it, including the input picker's list, which opens over the
    // dimming: "pick the audio input here" has to work mid-tour.
    #[test]
    fn the_dimming_takes_clicks_outside_the_anchor_only() {
        // The button under the dimming.
        let (clicks, _) = spotlight_clicks(Pos2::new(40.0, 240.0), false);
        assert_eq!(clicks, 0, "a click on the dimming reached the button");
        // The picker's third entry, below the combo and over the dimming:
        // entries are about 18 px apart, starting under the combo.
        let (_, value) = spotlight_clicks(Pos2::new(60.0, 32.0 + 2.5 * 18.0), true);
        assert_eq!(value, 2, "the list opened over the dimming took no click");
    }

    #[test]
    fn the_note_goes_where_there_is_room_and_never_over_the_anchor() {
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(1400.0, 900.0));
        let size = Vec2::new(CALLOUT_WIDTH, 200.0);
        // The right-hand column: the note goes left of it.
        let right = Rect::from_min_max(Pos2::new(1080.0, 300.0), Pos2::new(1390.0, 420.0));
        let n = place_callout(right, size, screen);
        assert!(n.right() <= right.left(), "{n:?}");
        // The drawer along the bottom: above it.
        let drawer = Rect::from_min_max(Pos2::new(0.0, 640.0), Pos2::new(1400.0, 870.0));
        let n = place_callout(drawer, size, screen);
        assert!(n.bottom() <= drawer.top(), "{n:?}");
        // The left column: right of it.
        let left = Rect::from_min_max(Pos2::new(8.0, 50.0), Pos2::new(500.0, 620.0));
        let n = place_callout(left, size, screen);
        assert!(n.left() >= left.right(), "{n:?}");
        for target in [right, drawer, left, screen.shrink(2.0)] {
            let n = place_callout(target, size, screen);
            assert!(screen.contains_rect(n), "{target:?}: {n:?}");
        }
    }

    #[test]
    fn first_run_starts_by_itself_once_and_only_in_the_workspace() {
        let mut s = crate::settings::SettingsConfig {
            classic_layout: false,
            ..Default::default()
        };
        assert!(should_auto_start(&s));
        s.tours_done.push(Tour::FirstRun.key().into());
        assert!(!should_auto_start(&s));
        let classic = crate::settings::SettingsConfig::default();
        assert!(classic.classic_layout && !should_auto_start(&classic));
    }

    // ── The Bindings tour (#3127) ─────────────────────────────────────

    use crate::bindings::types::BindingTarget;
    use crate::ui::panels::binding_matrix::ArmedEnd;
    use crate::ui::shell_harness::PARAMS;

    /// The target the Bindings tour picks: the effect layer's first control.
    fn first_param() -> BindingTarget {
        BindingTarget::Param {
            layer: ShellHarness::EFFECT_LAYER,
            effect: "Effect 0".into(),
            param: PARAMS[0].into(),
        }
    }

    /// Where a source row of the matrix's left column takes a click: left
    /// of its dot, on the row. A group open from the start: Bands.
    fn a_source_row(h: &ShellHarness, n: usize) -> (String, Pos2) {
        let bands = crate::ui::panels::binding_helpers::audio_source_groups()
            .into_iter()
            .find(|(_, id, _)| id == "audio_bands")
            .expect("a Bands group");
        let key = bands.2[n].to_string();
        let dot = h.matrix.source_positions[&key];
        (key, Pos2::new(dot.x - 60.0, dot.y))
    }

    // The launch stack's Layer 1 is empty: a tour about controls that
    // showed it would light "No parameters". The first step selects a layer
    // with an effect, the matrix opens with its first control picked, and
    // one click on a source makes the binding the next steps describe.
    #[test]
    fn the_bindings_tour_binds_the_effect_layers_first_control() {
        let mut h = ShellHarness::new(Vec2::new(1400.0, 900.0));
        assert_eq!(h.active_layer, 0);
        start(&h.ctx, Tour::Bindings);
        h.settle(30);
        assert_eq!(h.active_layer, ShellHarness::EFFECT_LAYER);
        assert!(!h.matrix.open);
        // The note shows a bound row rather than describing one, whole.
        let note = callout_rect(&h.ctx).unwrap();
        let chip = h.text_rect("\u{25c0} Bass").expect("the picture's row");
        let learn = h.text_rects("M");
        assert!(note.contains_rect(chip), "{chip:?} is outside {note:?}");
        assert!(
            learn
                .iter()
                .any(|m| note.contains_rect(*m) && m.left() > chip.right()),
            "the picture's M sits right of the chip"
        );

        h.key(Key::Enter);
        h.settle(30);
        assert!(h.matrix.open, "the sources step opens the matrix");
        assert_eq!(h.matrix.armed, Some(ArmedEnd::Target(first_param())));

        let (first, at) = a_source_row(&h, 0);
        assert!(
            hole(drawn(&h.ctx, Anchor::MatrixSources).unwrap(), h.screen()).contains(at),
            "the row is lit"
        );
        h.click(at);
        h.settle(2);
        let made: Vec<_> = h.bindings.bindings.iter().collect();
        assert_eq!(made.len(), 1, "one click, one binding");
        assert_eq!(made[0].source, first);
        assert_eq!(made[0].target, first_param());
        assert_eq!(h.matrix.armed, None);
        let id = made[0].id.clone();

        // Kevin's live check: a second source left the first on the card
        // and armed itself. It is the step's binding's new source.
        let (second, at) = a_source_row(&h, 1);
        h.click(at);
        h.settle(2);
        assert_eq!(
            h.bindings.bindings.len(),
            1,
            "a change of mind, not a second binding"
        );
        assert_eq!(h.bindings.bindings[0].source, second);
        assert_eq!(h.matrix.armed, None);
        assert_eq!(h.matrix.expanded_binding_id.as_deref(), Some(id.as_str()));

        // Cards, then Back: the step shows its binding rather than arming
        // for another.
        go_to(&h.ctx, Tour::Bindings, 2);
        h.settle(30);
        go_to(&h.ctx, Tour::Bindings, 1);
        h.settle(30);
        assert_eq!(h.matrix.armed, None);
        assert_eq!(h.matrix.expanded_binding_id.as_deref(), Some(id.as_str()));
        let (source, at) = a_source_row(&h, 0);
        h.click(at);
        h.settle(2);
        assert_eq!(h.bindings.bindings.len(), 1);
        assert_eq!(h.bindings.bindings[0].source, source);

        // On to the end, as Next does (the click took the keyboard focus):
        // the last step is back in the inspector.
        let last = Tour::Bindings.steps().len() - 1;
        for i in 2..last {
            go_to(&h.ctx, Tour::Bindings, i);
            h.settle(30);
            assert!(h.matrix.open, "step {}", i + 1);
        }
        go_to(&h.ctx, Tour::Bindings, last);
        h.settle(30);
        assert!(!h.matrix.open, "the M and O step closes the matrix");
        let row = drawn(&h.ctx, Anchor::ParamRow).expect("the first row is lit");
        assert!(row.height() < 60.0, "one row, not the section: {row:?}");

        // After the tour a source click is the matrix's own again: it arms.
        h.key(Key::Escape);
        h.ctx
            .data_mut(|d| d.insert_temp(Id::new("open_binding_matrix"), true));
        h.settle(5);
        let (source, at) = a_source_row(&h, 1);
        h.click(at);
        h.settle(2);
        assert_eq!(h.matrix.armed, Some(ArmedEnd::Source(source)));
        assert_eq!(h.bindings.bindings.len(), 1);
    }

    // The control already has a binding when the tour reaches the step (a
    // second run of the tour, or one made by hand): the step takes that
    // binding as its own, and the first click changes its source. Before,
    // nothing was armed and the click armed the source instead.
    #[test]
    fn the_pick_step_takes_the_controls_existing_binding() {
        let mut h = ShellHarness::new(Vec2::new(1400.0, 900.0));
        let id = h.bindings.add_binding(
            "audio.rms".into(),
            first_param(),
            crate::bindings::types::BindingScope::Preset,
        );
        start(&h.ctx, Tour::Bindings);
        h.settle(30);
        go_to(&h.ctx, Tour::Bindings, 1);
        h.settle(30);
        assert_eq!(h.matrix.expanded_binding_id.as_deref(), Some(id.as_str()));
        let (source, at) = a_source_row(&h, 1);
        h.click(at);
        h.settle(2);
        assert_eq!(h.bindings.bindings.len(), 1);
        assert_eq!(h.bindings.bindings[0].source, source);
        assert_eq!(h.matrix.armed, None);
    }

    // The matrix draws after the shell and asks to be on top of its
    // backdrop; the dimming must still cover it, or a matrix step would be
    // a spotlight on the whole matrix with every part of it live.
    #[test]
    fn the_dimming_covers_the_matrix_but_the_lit_column() {
        let mut h = ShellHarness::new(Vec2::new(1400.0, 900.0));
        start(&h.ctx, Tour::Bindings);
        h.settle(30);
        h.key(Key::Enter);
        h.settle(30);
        let layer_at = |h: &ShellHarness, p: Pos2| h.ctx.layer_id_at(p).map(|l| l.id);
        let is_dim = |id: Option<Id>| (0..4).any(|i| id == Some(Id::new("tour_dim").with(i)));
        let sources = drawn(&h.ctx, Anchor::MatrixSources).unwrap();
        let cards = drawn(&h.ctx, Anchor::MatrixCards).unwrap();
        // Clear of the note, which sits beside the lit column.
        let note = callout_rect(&h.ctx).unwrap();
        let cards = Rect::from_min_max(Pos2::new(cards.left(), note.bottom() + 20.0), cards.max);
        assert!(cards.is_positive(), "room under the note: {cards:?}");
        assert_eq!(
            layer_at(&h, sources.center()),
            Some(Id::new("binding_matrix_area")),
            "the lit column is the matrix's"
        );
        assert!(is_dim(layer_at(&h, cards.center())), "the cards are dimmed");
        // A click on the dimmed cards makes nothing, and one beside the
        // matrix closes nothing, not even for the frame before the tour
        // would open it again.
        h.click(cards.center());
        assert!(h.bindings.bindings.is_empty());
        h.click(Pos2::new(4.0, 200.0));
        assert!(h.matrix.open, "a click beside it closed it");
        // Clicking inside the lit column raises the matrix; the dimming
        // must stay over the rest of it all the same.
        h.click(sources.center());
        h.settle(2);
        assert!(is_dim(layer_at(&h, cards.center())), "after a click inside");
        // Nor does its own backdrop come up over it, whatever raises it.
        h.ctx.move_to_top(egui::LayerId::new(
            Order::Middle,
            Id::new("matrix_backdrop"),
        ));
        h.settle(2);
        assert_eq!(
            layer_at(&h, sources.center()),
            Some(Id::new("binding_matrix_area")),
            "the backdrop covered the lit column"
        );
    }

    // Leaving the tour on a step inside the matrix puts the matrix away:
    // the tour opened it. A matrix the user opened is theirs.
    #[test]
    fn leaving_the_tour_in_the_matrix_puts_the_matrix_away() {
        for leave in [Key::Escape, Key::Space] {
            let mut h = ShellHarness::new(Vec2::new(1400.0, 900.0));
            start(&h.ctx, Tour::Bindings);
            h.settle(3);
            h.key(Key::Enter);
            h.settle(5);
            assert!(h.matrix.open);
            if leave == Key::Space {
                end(&h.ctx); // as Skip tour does
                h.settle(1);
            } else {
                h.key(Key::Escape);
            }
            h.settle(2);
            assert_eq!(current(&h.ctx), None);
            assert!(!h.matrix.open, "{leave:?} left the matrix open");
        }
        let mut h = ShellHarness::new(Vec2::new(1400.0, 900.0));
        h.ctx
            .data_mut(|d| d.insert_temp(Id::new("open_binding_matrix"), true));
        h.settle(3);
        assert!(h.matrix.open);
    }
}
