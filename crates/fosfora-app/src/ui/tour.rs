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
//!   anchor itself stays live, because trying it is the point. A step may
//!   leave a second part lit to watch ([`Step::watch`]): the output, while
//!   a blend mode is tried. The note goes on whichever side of the anchor
//!   has room, clear of both.
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
    /// The inspector's Chain line: the layer's chain and Edit chain.
    ChainLine,
    /// The chain editor's canvas: nodes and wires.
    ChainCanvas,
    /// The chain editor's right-hand panel: the selected node's controls.
    ChainInspector,
    /// The chain editor's Layer and Master tabs.
    ChainTabs,
    /// The chain editor's Export and Import.
    ChainFiles,
    /// The selected layer's Blend section: mode, visibility, opacity.
    Blend,
    /// The selected layer's row in the stack.
    LayerRow,
    /// The buttons under the stack that add a layer.
    StackAdd,
    /// Master's Post-Processing section in the inspector.
    PostProcessing,
    /// The output preview.
    Output,
}

/// The tours there are.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tour {
    FirstRun,
    Bindings,
    Chains,
    Layers,
}

impl Tour {
    pub const ALL: &[Tour] = &[Tour::FirstRun, Tour::Bindings, Tour::Chains, Tour::Layers];

    /// The name settings records it under once it is finished or skipped.
    /// Never change one: a renamed key replays the tour for everyone.
    pub fn key(self) -> &'static str {
        match self {
            Tour::FirstRun => "first_run",
            Tour::Bindings => "bindings",
            Tour::Chains => "chains",
            Tour::Layers => "layers",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Tour::FirstRun => "First run",
            Tour::Bindings => "Bindings",
            Tour::Chains => "Chains",
            Tour::Layers => "Layers and blending",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Tour::FirstRun => {
                "Choose what it listens to, find an effect, read the stack, shape it, save it."
            }
            Tour::Bindings => "Make a control follow the bass, a MIDI knob or an OSC message.",
            Tour::Chains => "Build a trama node chain on a layer, and on the master.",
            Tour::Layers => {
                "Stack layers, blend each onto the ones beneath, and finish the mix on Master."
            }
        }
    }

    /// Does any step open the chain editor? Those need a GPU to draw.
    #[cfg(test)]
    pub fn uses_chains(self) -> bool {
        self.steps().iter().any(|s| {
            matches!(
                s.modal,
                Modal::LayerChain | Modal::LayerChainStarter | Modal::MasterChain
            )
        })
    }

    pub fn steps(self) -> &'static [Step] {
        match self {
            Tour::FirstRun => FIRST_RUN,
            Tour::Bindings => BINDINGS,
            Tour::Chains => CHAINS,
            Tour::Layers => LAYERS,
        }
    }
}

/// Which view a step holds open over the workspace, if any. The tour holds
/// it open or shut while the step shows: the view's own ways to close are
/// off meanwhile, and a view the tour opened closes when the tour ends.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Modal {
    None,
    /// The binding matrix.
    Matrix,
    /// The matrix, and the step makes one binding: the first source clicked
    /// makes it, and each click after that changes its source, so the card
    /// the next steps describe is the last source picked.
    MatrixPickSource,
    /// The chain editor on the selected layer's chain.
    LayerChain,
    /// The chain editor on the selected layer's chain, and an empty chain
    /// gets the starter chain ([`crate::trama::starter`]) as the step opens.
    LayerChainStarter,
    /// The chain editor on the master chain.
    MasterChain,
}

impl Modal {
    fn matrix(self) -> bool {
        matches!(self, Modal::Matrix | Modal::MatrixPickSource)
    }
}

/// Which chain the chain editor shows during a step that holds it open.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ChainTab {
    Layer,
    Master,
}

/// What a step wants of the chain editor.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ChainEditor {
    Shut,
    Open(ChainTab),
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
    /// The view held open over the workspace during this step.
    pub modal: Modal,
    /// A second part left lit, for watching what the anchor changes: the
    /// output while a blend mode is tried. It takes clicks like the anchor.
    /// A collapsible section it sits in goes in `reveal` too.
    pub watch: Option<Anchor>,
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
        modal: Modal::None,
        watch: None,
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
        modal: Modal::None,
        watch: None,
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
        modal: Modal::None,
        watch: None,
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
        modal: Modal::None,
        watch: None,
        picture: None,
    },
    Step {
        anchor: Anchor::Presets,
        title: "Keep it",
        body: "Name the preset and press Save. A preset keeps the whole stack: layers, \
               chains and Master. Replay this tour from Setup › Tutorials.",
        prepare: to_build,
        reveal: &["sec_presets"],
        modal: Modal::None,
        watch: None,
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
        modal: Modal::None,
        watch: None,
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
        modal: Modal::MatrixPickSource,
        watch: None,
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
        modal: Modal::Matrix,
        watch: None,
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
        modal: Modal::Matrix,
        watch: None,
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
        modal: Modal::Matrix,
        watch: None,
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
        modal: Modal::None,
        watch: None,
        picture: None,
    },
];

/// The Chains tour: trama node chains, from the inspector's Chain line into
/// the chain editor. The canvas step puts a starter chain on an empty layer
/// chain (Kevin's call, #3224), so the steps after it point at real nodes.
const CHAINS: &[Step] = &[
    Step {
        anchor: Anchor::ChainLine,
        title: "A chain runs after the effect",
        body: "Each layer has a chain of trama nodes that work on its picture after its \
               effect: blur, kaleidoscope, color key, feedback and more. This line says \
               what the chain holds. After the tour, Edit chain (C) opens it; the next \
               steps open it for you.",
        prepare: to_effect_layer,
        reveal: &[],
        modal: Modal::None,
        watch: None,
        picture: None,
    },
    Step {
        anchor: Anchor::ChainCanvas,
        title: "Nodes and wires",
        body: "If this layer's chain was empty, the tour started one: Layer input, the \
               layer's own picture, runs into Kaleidoscope, which runs into Output. What \
               reaches Output is what the layer shows, so the output has changed. \
               Right-click the canvas to add a node, drag from one pin to another to wire \
               them, and right-click a wire to remove it.",
        prepare: to_build,
        reveal: &[],
        modal: Modal::LayerChainStarter,
        watch: None,
        picture: None,
    },
    Step {
        anchor: Anchor::ChainInspector,
        title: "Shape a node",
        body: "Click a node to see its controls here. Under each one, mod makes it \
               follow an oscillator or the music. Right-click a node to bypass or delete \
               it: delete the tour's two nodes and the layer is as it was.",
        prepare: to_build,
        reveal: &[],
        modal: Modal::LayerChain,
        watch: None,
        picture: None,
    },
    Step {
        anchor: Anchor::ChainTabs,
        title: "One layer, or the whole mix",
        body: "The Layer tab shows the selected layer's chain. Master's chain runs on the \
               mix of every layer, before tonemapping: a look for the whole output goes \
               there. Its tab counts its nodes, so a forgotten master chain shows.",
        prepare: to_build,
        reveal: &[],
        modal: Modal::MasterChain,
        watch: None,
        picture: None,
    },
    Step {
        anchor: Anchor::ChainFiles,
        title: "Keep it, or share it",
        body: "A preset saves every chain with it. After the tour, Export writes this \
               chain to a .fio.json file and Import replaces it with one, so a chain can \
               move between presets or people. Replay this tour from Setup \u{203a} \
               Tutorials.",
        prepare: to_build,
        reveal: &[],
        modal: Modal::LayerChain,
        watch: None,
        picture: None,
    },
];

/// Build, with the layer the Layers tour explains selected: see
/// [`blend_layer`].
fn to_blend_layer(ctx: &Context) {
    to_build(ctx);
    ctx.data_mut(|d| d.insert_temp(Id::new(SELECT_BLEND_LAYER), true));
}

/// Build, with Master in the inspector.
fn to_master(ctx: &Context) {
    super::shell::Workspace::Build.write(ctx);
    super::panels::stack_panel::show_master_in_inspector(ctx);
}

/// Asks the shell to select the layer the Layers tour explains, starting
/// one if it has to ([`blend_layer`]).
pub const SELECT_BLEND_LAYER: &str = "tour_select_blend_layer";

/// What the Layers tour loads into an empty top layer (Kevin's call,
/// #3231): bright bands on black, so in Screen both it and the picture
/// beneath show, and each other mode visibly changes that.
pub const STARTER_EFFECT: &str = "Aurora";
/// The mode the starter layer lands in.
pub const STARTER_BLEND: crate::gpu::layer::BlendMode = crate::gpu::layer::BlendMode::Screen;

/// Which layer the Layers tour explains.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BlendLayer {
    /// This one, as it is.
    Explain(usize),
    /// This one, empty: load [`STARTER_EFFECT`] into it first.
    Start(usize),
}

/// The layer the Layers tour explains: one whose blend shows, so it is
/// visible, runs something and has a visible layer beneath it. The selected
/// layer if it is one, else the topmost. With none, the topmost empty
/// visible layer with a visible layer beneath it gets the starter (the
/// launch stack's Layer 1). With neither, `None`: the tour explains
/// whatever is selected.
pub fn blend_layer(layers: &[crate::gpu::layer::LayerInfo], selected: usize) -> Option<BlendLayer> {
    let bottom = layers.iter().rposition(|l| l.enabled)?;
    let above = |i: usize| i < bottom && layers[i].enabled;
    let empty = |i: usize| super::panels::stack_panel::layer_kind(&layers[i]) == "Empty";
    let shows = |i: usize| above(i) && !empty(i);
    if selected < layers.len() && shows(selected) {
        return Some(BlendLayer::Explain(selected));
    }
    if let Some(i) = (0..layers.len()).find(|&i| shows(i)) {
        return Some(BlendLayer::Explain(i));
    }
    (0..layers.len())
        .find(|&i| above(i) && empty(i) && !layers[i].locked)
        .map(BlendLayer::Start)
}

/// Load [`STARTER_EFFECT`] into layer `i` in [`STARTER_BLEND`], through the
/// requests the catalog and the Blend section send. `main.rs` reads the
/// catalog's first, and it selects the layer the mode then goes to.
pub fn start_layer(ctx: &Context, i: usize) {
    use super::panels::catalog_panel::{CatalogDrop, send_drop};
    send_drop(ctx, STARTER_EFFECT.to_string(), CatalogDrop::Replace(i));
    super::panels::stack_panel::select_layer(ctx, i);
    ctx.data_mut(|d| d.insert_temp(Id::new("layer_blend"), STARTER_BLEND.as_u32()));
}

/// The Layers and blending tour: the stack, how one layer lands on it, its
/// row, adding layers, and Master. The first step gives an empty top layer
/// an effect (Kevin's call, #3231), and the steps that change the picture
/// keep the output lit beside them.
const LAYERS: &[Step] = &[
    Step {
        anchor: Anchor::Stack,
        title: "Layers stack from the bottom up",
        body: "The bottom layer is drawn first, each layer above lands on the picture so \
               far, and Master, on top, is what goes out. The lines between rows say how \
               each one lands. If the top layer was empty, the tour loaded Aurora into it \
               so there is a blend to read. Each layer alone shows the layers before \
               blending.",
        prepare: to_blend_layer,
        reveal: &[],
        modal: Modal::None,
        watch: None,
        picture: None,
    },
    Step {
        anchor: Anchor::Blend,
        title: "How a layer lands",
        body: "Mode is how this layer meets the picture beneath: Normal covers it, Add \
               and Screen brighten, Multiply darkens, Difference inverts. The last three \
               bend the picture beneath instead of coloring it. Opacity fades the layer, \
               and Visible takes it out. Try a few and watch the output.",
        prepare: to_blend_layer,
        reveal: &["v2_blend", "v2_preview"],
        modal: Modal::None,
        watch: Some(Anchor::Output),
        picture: None,
    },
    Step {
        anchor: Anchor::LayerRow,
        title: "Order matters",
        body: "Hide takes the layer out and lets the picture below pass up. Right-click \
               the row to rename it, move it up or down, lock it against edits or pin its \
               place. A layer blends onto whatever is beneath it, so moving one can \
               change the whole picture.",
        prepare: to_blend_layer,
        reveal: &[],
        modal: Modal::None,
        watch: None,
        picture: None,
    },
    Step {
        anchor: Anchor::StackAdd,
        title: "Add a layer",
        body: "After the tour, these add a layer: an effect, an image or video, or a \
               camera. Or drag a catalog picture onto the stack: onto a row's top or \
               bottom edge adds a layer there, onto its middle replaces that layer's \
               effect. A stack holds up to eight layers.",
        prepare: to_blend_layer,
        reveal: &[],
        modal: Modal::None,
        watch: None,
        picture: None,
    },
    Step {
        anchor: Anchor::PostProcessing,
        title: "Master finishes the mix",
        body: "Post-processing runs once, on the blend of every layer: bloom, chromatic \
               aberration, vignette and grain. It belongs to the preset, not to a layer, \
               so selecting a layer never changes it. Replay this tour from Setup \
               \u{203a} Tutorials.",
        prepare: to_master,
        reveal: &["v2_postfx", "v2_preview"],
        modal: Modal::None,
        watch: Some(Anchor::Output),
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
    running(ctx).and_then(step_of).map(|s| s.modal.matrix())
}

/// Is the step showing one where a source click picks the source of the
/// step's own binding ([`Modal::MatrixPickSource`])?
pub fn picks_source(ctx: &Context) -> bool {
    running(ctx)
        .and_then(step_of)
        .is_some_and(|s| s.modal == Modal::MatrixPickSource)
}

/// Should the chain editor show, and on which chain? `None` when no tour is
/// showing: the editor is the user's.
pub fn chain_wanted(ctx: &Context) -> Option<ChainEditor> {
    running(ctx).and_then(step_of).map(|s| match s.modal {
        Modal::LayerChain | Modal::LayerChainStarter => ChainEditor::Open(ChainTab::Layer),
        Modal::MasterChain => ChainEditor::Open(ChainTab::Master),
        _ => ChainEditor::Shut,
    })
}

/// Is the step showing one that puts the starter chain on an empty layer
/// chain ([`Modal::LayerChainStarter`])? The editor seeds while this holds
/// and the chain it shows is an empty layer chain: once, since a seeded
/// chain is no longer empty.
pub fn seeds_chain(ctx: &Context) -> bool {
    running(ctx)
        .and_then(step_of)
        .is_some_and(|s| s.modal == Modal::LayerChainStarter)
}

/// Should the chain editor put the starter chain on the empty layer chain
/// it shows? True once per visit to a [`Modal::LayerChainStarter`] step: the
/// first call answers and marks the visit, whether or not the chain turns
/// out to be empty.
pub fn take_seed(ctx: &Context) -> bool {
    if !seeds_chain(ctx) {
        return false;
    }
    let id = Id::new(SEEDED);
    let seeded = ctx.data(|d| d.get_temp::<bool>(id).unwrap_or(false));
    if !seeded {
        ctx.data_mut(|d| d.insert_temp(id, true));
    }
    !seeded
}

/// Set by [`take_seed`] for the step showing; cleared as each step opens.
const SEEDED: &str = "tour_chain_seeded";

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
        d.remove::<bool>(Id::new(SEEDED));
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
/// that the note fits, with room to spare, never over the hole, and not
/// over anything in `avoid` (a step's watched part) while some side allows.
/// When no side has room (the anchor fills the window) it sits inside the
/// hole's lower right corner.
pub fn place_callout(hole: Rect, size: Vec2, screen: Rect, avoid: &[Rect]) -> Rect {
    let room = [
        (hole.left() - screen.left(), size.x),
        (screen.right() - hole.right(), size.x),
        (hole.top() - screen.top(), size.y),
        (screen.bottom() - hole.bottom(), size.y),
    ];
    let fits = |i: usize| room[i].0 >= room[i].1 + 2.0 * GAP;
    let clear = |i: usize| {
        let at = callout_on_side(Some(i), hole, size, screen);
        !avoid.iter().any(|a| a.intersects(at))
    };
    // Compare the room left over, so a tall note prefers a side.
    let most_room =
        |a: &usize, b: &usize| (room[*a].0 - room[*a].1).total_cmp(&(room[*b].0 - room[*b].1));
    let best = (0..4)
        .filter(|&i| fits(i) && clear(i))
        .max_by(most_room)
        .or_else(|| (0..4).filter(|&i| fits(i)).max_by(most_room));
    callout_on_side(best, hole, size, screen)
}

/// The note on side `side` of `hole` (left, right, above, below), or in its
/// lower right corner for `None`.
fn callout_on_side(side: Option<usize>, hole: Rect, size: Vec2, screen: Rect) -> Rect {
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
    let min = match side {
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

/// Rectangles covering `screen` except `holes`, none overlapping: the
/// screen cut into bands at every hole edge, each band's run of cells
/// outside every hole merged into one rectangle. One hole gives the four
/// around it: above, left, right, below.
fn around(holes: &[Rect], screen: Rect) -> Vec<Rect> {
    let cuts = |lo: f32, hi: f32, edges: &mut dyn Iterator<Item = f32>| {
        let mut v: Vec<f32> = std::iter::once(lo)
            .chain(edges.map(|e| e.clamp(lo, hi)))
            .chain(std::iter::once(hi))
            .collect();
        v.sort_by(f32::total_cmp);
        v.dedup();
        v
    };
    let xs = cuts(
        screen.left(),
        screen.right(),
        &mut holes.iter().flat_map(|h| [h.left(), h.right()]),
    );
    let ys = cuts(
        screen.top(),
        screen.bottom(),
        &mut holes.iter().flat_map(|h| [h.top(), h.bottom()]),
    );
    let mut out = Vec::new();
    for y in ys.windows(2) {
        let mut run: Option<Rect> = None;
        for x in xs.windows(2) {
            let cell = Rect::from_min_max(Pos2::new(x[0], y[0]), Pos2::new(x[1], y[1]));
            if holes.iter().any(|h| h.contains(cell.center())) {
                out.extend(run.take());
            } else {
                run = Some(run.map_or(cell, |r| r.union(cell)));
            }
        }
        out.extend(run);
    }
    out
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
    // Lit only with the anchor: alone, a watched part would be a note
    // about something else.
    let watched = lit
        .and(step.watch)
        .and_then(|w| anchor_rect(ctx, w))
        .map(|a| hole(a, screen));

    // Each dimmed piece is its own area so that the hole has none: an area
    // over the hole would take the pointer there, clicks and scrolling both.
    // Middle, so menus and pop-ups opened from inside the hole (the input
    // picker's list) are drawn over the dimming and take their own clicks
    // by their order, not only because they opened last. The binding
    // matrix draws at Middle too while a tour shows, and opens after the
    // dimming exists: every frame the dimming asks to be on top again, and
    // egui's end-of-frame sort is stable, so it stays over the matrix even
    // on a frame the matrix is clicked and asks the same.
    let holes: Vec<Rect> = lit.into_iter().chain(watched).collect();
    let pieces = around(&holes, screen);
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
        Some(h) => place_callout(h, size, screen, watched.as_slice()),
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
    let p = ctx.layer_painter(egui::LayerId::new(Order::Foreground, Id::new("tour_ring")));
    for h in &holes {
        for (w, c) in [(4.0_f32, tc.on_selection), (2.0_f32, tc.selection)] {
            p.rect_stroke(*h, 6.0, egui::Stroke::new(w, c), egui::StrokeKind::Outside);
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
        for &tour in Tour::ALL.iter().filter(|t| !t.uses_chains()) {
            anchors_on_screen(tour, ShellHarness::new);
        }
    }

    // The chain editor needs a GPU: run with --ignored.
    #[test]
    #[ignore = "requires a wgpu adapter: draws the chain editor"]
    fn every_chains_step_lights_its_anchor_on_screen() {
        let _gpu = crate::gpu::test_gpu::gpu_guard();
        anchors_on_screen(Tour::Chains, ShellHarness::with_chains);
    }

    fn anchors_on_screen(tour: Tour, make: fn(Vec2) -> ShellHarness) {
        for size in [Vec2::new(1206.0, 760.0), Vec2::new(1920.0, 1080.0)] {
            {
                let mut h = make(size);
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
                    if let Some(w) = step.watch {
                        let wa = drawn(&h.ctx, w)
                            .unwrap_or_else(|| panic!("{at}: the watched {w:?} was not drawn"));
                        assert!(
                            wa.width() >= 120.0 && wa.height() >= 60.0,
                            "{at}: {w:?} is only {wa:?}"
                        );
                        assert!(screen.contains_rect(wa), "{at}: {wa:?} is off screen");
                        assert!(
                            !note.intersects(hole(wa, screen)),
                            "{at}: the note {note:?} covers the watched {wa:?}"
                        );
                    }
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
        for &tour in Tour::ALL.iter().filter(|t| !t.uses_chains()) {
            clicking_inside_stays(tour, ShellHarness::new);
        }
    }

    #[test]
    #[ignore = "requires a wgpu adapter: draws the chain editor"]
    fn nothing_in_a_lit_chains_area_leaves_the_step() {
        let _gpu = crate::gpu::test_gpu::gpu_guard();
        clicking_inside_stays(Tour::Chains, ShellHarness::with_chains);
    }

    fn clicking_inside_stays(tour: Tour, make: fn(Vec2) -> ShellHarness) {
        let mut h = make(Vec2::new(1400.0, 900.0));
        start(&h.ctx, tour);
        for (i, step) in tour.steps().iter().enumerate() {
            h.settle(30);
            let lit = drawn(&h.ctx, step.anchor).unwrap();
            let watched = step.watch.map(|w| drawn(&h.ctx, w).unwrap());
            let spots = std::iter::once(lit).chain(watched).flat_map(|r| {
                let cols = ((r.width() - 6.0) / 30.0).ceil().max(0.0) as usize;
                let rows = ((r.height() - 6.0) / 30.0).ceil().max(0.0) as usize;
                (0..rows).flat_map(move |j| {
                    (0..cols).map(move |i| {
                        Pos2::new(
                            r.left() + 6.0 + 30.0 * i as f32,
                            r.top() + 6.0 + 30.0 * j as f32,
                        )
                    })
                })
            });
            for at in spots.collect::<Vec<_>>() {
                h.click(at);
                // Straight after the click: the tour sets the tab again
                // next frame, so a switch shows only now.
                if let (Some(c), Some(ChainEditor::Open(tab))) = (&h.chains, chain_wanted(&h.ctx)) {
                    let want = match tab {
                        ChainTab::Layer => crate::trama::CanvasTarget::SelectedLayer,
                        ChainTab::Master => crate::trama::CanvasTarget::Master,
                    };
                    assert!(c.trama.canvas_target == want, "a click switched the tab");
                }
                h.settle(1);
            }
            let at = format!("{} step {}", tour.name(), i + 1);
            assert_eq!(h.leaving, None, "{at}: a click inside asked to leave");
            let dialogs =
                crate::trama::persist::DIALOGS_ASKED.load(std::sync::atomic::Ordering::Relaxed);
            assert_eq!(dialogs, 0, "{at}: a click asked for a file dialog");
            assert_eq!(current(&h.ctx), Some((tour, i)));
            h.settle(30);
            assert!(
                drawn(&h.ctx, step.anchor).is_some(),
                "{at}: clicking inside hid {:?}",
                step.anchor
            );
            assert_eq!(h.matrix.open, step.modal.matrix(), "{at}: the matrix");
            if let Some(c) = &h.chains {
                assert_eq!(
                    c.trama.canvas_open,
                    matches!(chain_wanted(&h.ctx), Some(ChainEditor::Open(_))),
                    "{at}: the chain editor"
                );
            }
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
        let n = place_callout(right, size, screen, &[]);
        assert!(n.right() <= right.left(), "{n:?}");
        // The drawer along the bottom: above it.
        let drawer = Rect::from_min_max(Pos2::new(0.0, 640.0), Pos2::new(1400.0, 870.0));
        let n = place_callout(drawer, size, screen, &[]);
        assert!(n.bottom() <= drawer.top(), "{n:?}");
        // The left column: right of it.
        let left = Rect::from_min_max(Pos2::new(8.0, 50.0), Pos2::new(500.0, 620.0));
        let n = place_callout(left, size, screen, &[]);
        assert!(n.left() >= left.right(), "{n:?}");
        for target in [right, drawer, left, screen.shrink(2.0)] {
            let n = place_callout(target, size, screen, &[]);
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

    // ── The Chains tour (#3128): GPU, run with --ignored ──────────────

    /// The graph of harness layer `i`'s chain, if it has one.
    fn chain_nodes(h: &ShellHarness, i: usize) -> Option<usize> {
        let c = h.chains.as_ref().unwrap();
        c.stack.layers[i]
            .chain
            .as_ref()
            .map(|c| c.graph.nodes().len())
    }

    fn contributes(h: &ShellHarness, i: usize) -> bool {
        let c = h.chains.as_ref().unwrap();
        c.stack.layers[i]
            .chain
            .as_ref()
            .is_some_and(|c| c.graph.contributes())
    }

    // Kevin's call (#3224): the canvas step puts Layer input -> Kaleidoscope
    // -> Output on the selected effect layer's empty chain, so the picture
    // changes and the next steps have nodes to point at. Once: deleting
    // them on the step does not bring them back, and neither does Back.
    #[test]
    #[ignore = "requires a wgpu adapter: draws the chain editor"]
    fn the_chains_tour_starts_a_chain_on_the_effect_layer_once() {
        let _gpu = crate::gpu::test_gpu::gpu_guard();
        let mut h = ShellHarness::with_chains(Vec2::new(1400.0, 900.0));
        // The editor was last open on Layer 1, so for a frame after it opens
        // again it still shows that chain: the seed must wait for the
        // selected layer's.
        h.chains.as_mut().unwrap().trama.canvas_open = true;
        h.settle(3);
        h.chains.as_mut().unwrap().trama.canvas_open = false;
        h.settle(2);
        assert_eq!(chain_nodes(&h, 0), Some(1), "Layer 1 has an empty chain");
        start(&h.ctx, Tour::Chains);
        h.settle(30);
        assert_eq!(h.active_layer, ShellHarness::EFFECT_LAYER);
        assert_eq!(
            chain_nodes(&h, 0),
            Some(1),
            "the empty Layer 1 is untouched"
        );

        h.key(Key::Enter);
        h.settle(30);
        let e = ShellHarness::EFFECT_LAYER;
        assert_eq!(chain_nodes(&h, e), Some(3), "input, effect, output");
        assert!(contributes(&h, e), "the picture reaches Output");
        assert_eq!(
            chain_nodes(&h, 0),
            Some(1),
            "only the selected layer's chain"
        );
        // Kevin's settings, by the shipped manifest's own names.
        {
            let c = h.chains.as_ref().unwrap();
            let g = &c.stack.layers[e].chain.as_ref().unwrap().graph;
            let fx = g
                .nodes()
                .iter()
                .find(|n| matches!(n.kind, crate::trama::node::NodeKind::Effect { .. }))
                .unwrap();
            for &(name, want) in crate::trama::starter::STARTER_SETTINGS {
                let got = match fx.params.get(name) {
                    Some(crate::params::ParamValue::Float(v)) => Some(*v),
                    _ => None,
                };
                assert_eq!(got, Some(want), "{name}");
            }
        }
        // Segments are whole numbers, and say so (#3128): the manifest's
        // "integers" reaches the inspector. 6.4 reads as the 6 it renders.
        {
            let c = h.chains.as_mut().unwrap();
            let g = &mut c.stack.layers[e].chain.as_mut().unwrap().graph;
            let fx = g
                .nodes()
                .iter()
                .find(|n| matches!(n.kind, crate::trama::node::NodeKind::Effect { .. }))
                .unwrap()
                .id;
            g.params_mut(fx)
                .unwrap()
                .params
                .set("segments", crate::params::ParamValue::Float(6.4));
        }
        h.settle(2);
        assert!(h.text_rect("6").is_some() && h.text_rect("6.40").is_none());
        {
            let c = h.chains.as_mut().unwrap();
            let g = &mut c.stack.layers[e].chain.as_mut().unwrap().graph;
            let fx = g
                .nodes()
                .iter()
                .find(|n| matches!(n.kind, crate::trama::node::NodeKind::Effect { .. }))
                .unwrap()
                .id;
            g.params_mut(fx)
                .unwrap()
                .params
                .set("segments", crate::params::ParamValue::Float(15.0));
        }
        // The effect is selected, so the inspector shows its controls.
        assert!(h.text_rect("Kaleidoscope").is_some() || h.text_rect("Pixelate").is_some());
        // And the stack's Chain line says so, as the app's would.
        assert!(h.layers[e].chain.is_some_and(|b| b.active && b.nodes == 2));

        // Deleted on the step: stays deleted.
        {
            let c = h.chains.as_mut().unwrap();
            let g = &mut c.stack.layers[e].chain.as_mut().unwrap().graph;
            let ids: Vec<_> = g
                .nodes()
                .iter()
                .map(|n| n.id)
                .filter(|&id| id != g.output_node())
                .collect();
            for id in ids {
                g.remove_node(id).unwrap();
            }
        }
        h.settle(5);
        assert_eq!(chain_nodes(&h, e), Some(1), "not seeded twice on one visit");
        // Back and forward again: an empty chain gets a fresh start.
        go_to(&h.ctx, Tour::Chains, 0);
        h.settle(30);
        go_to(&h.ctx, Tour::Chains, 1);
        h.settle(30);
        assert_eq!(chain_nodes(&h, e), Some(3));
    }

    // A chain with anything in it is the user's: the tour leaves it alone.
    #[test]
    #[ignore = "requires a wgpu adapter: draws the chain editor"]
    fn the_chains_tour_leaves_a_users_chain_alone() {
        let _gpu = crate::gpu::test_gpu::gpu_guard();
        let mut h = ShellHarness::with_chains(Vec2::new(1400.0, 900.0));
        let e = ShellHarness::EFFECT_LAYER;
        {
            let c = h.chains.as_mut().unwrap();
            c.stack.ensure_chain(e).unwrap();
            let g = &mut c.stack.layers[e].chain.as_mut().unwrap().graph;
            g.add_node(crate::trama::node::NodeKind::Feedback, 1, &[]);
        }
        start(&h.ctx, Tour::Chains);
        h.settle(30);
        h.key(Key::Enter);
        h.settle(30);
        assert_eq!(chain_nodes(&h, e), Some(2), "Output and the user's node");
    }

    // The editor draws after the shell; the dimming must cover all of it but
    // the lit part, and leaving the tour must put away the editor it opened.
    #[test]
    #[ignore = "requires a wgpu adapter: draws the chain editor"]
    fn the_dimming_covers_the_chain_editor_and_leaving_puts_it_away() {
        let _gpu = crate::gpu::test_gpu::gpu_guard();
        let mut h = ShellHarness::with_chains(Vec2::new(1400.0, 900.0));
        start(&h.ctx, Tour::Chains);
        h.settle(30);
        go_to(&h.ctx, Tour::Chains, 2); // the node inspector
        h.settle(30);
        let lit = drawn(&h.ctx, Anchor::ChainInspector).unwrap();
        let canvas = drawn(&h.ctx, Anchor::ChainCanvas).unwrap();
        let is_dim = |id: Option<Id>| (0..4).any(|i| id == Some(Id::new("tour_dim").with(i)));
        let layer_at = |h: &ShellHarness, p: Pos2| h.ctx.layer_id_at(p).map(|l| l.id);
        assert_eq!(layer_at(&h, lit.center()), Some(Id::new("trama_modal")));
        let note = callout_rect(&h.ctx).unwrap();
        let spot = [canvas.left_top(), canvas.left_bottom()]
            .into_iter()
            .map(|p| p + (canvas.center() - p) * 0.2)
            .find(|p| !note.contains(*p))
            .unwrap();
        assert!(is_dim(layer_at(&h, spot)), "the canvas is dimmed");
        // Beside the editor: closes nothing.
        h.click(Pos2::new(4.0, 300.0));
        assert!(h.chains.as_ref().unwrap().trama.canvas_open);
        // Its own backdrop never comes up over it, whatever raises it.
        h.ctx.move_to_top(egui::LayerId::new(
            Order::Middle,
            Id::new("trama_modal_backdrop"),
        ));
        h.settle(2);
        assert_eq!(
            layer_at(&h, lit.center()),
            Some(Id::new("trama_modal")),
            "the backdrop covered the lit panel"
        );

        h.key(Key::Escape);
        h.settle(2);
        assert_eq!(current(&h.ctx), None);
        assert!(!h.chains.as_ref().unwrap().trama.canvas_open);
    }

    // ── The Layers and blending tour (#3129) ──────────────────────────

    use crate::gpu::layer::{BlendMode, LayerInfo};

    fn info(name: &str, kind: &str) -> LayerInfo {
        LayerInfo {
            name: name.into(),
            custom_name: None,
            effect_index: (kind == "effect").then_some(0),
            effect_name: (kind == "effect").then(|| name.to_string()),
            blend_mode: BlendMode::default(),
            opacity: 1.0,
            displace_amount: 0.0,
            enabled: true,
            locked: false,
            pinned: false,
            has_particles: false,
            shader_error: None,
            is_media: kind == "media",
            media_file_name: None,
            media_is_animated: false,
            media_is_video: false,
            media_is_live: false,
            chain: None,
            needs_layer_below: false,
        }
    }

    // The layer the tour explains is one whose blend shows: visible, running
    // something, with a visible layer beneath. The bottom layer's mode is
    // never applied, so it is never the one; an empty top layer gets the
    // starter only when no layer qualifies.
    #[test]
    fn the_layers_tour_explains_a_layer_whose_blend_shows() {
        use BlendLayer::{Explain, Start};
        let launch = [info("Layer 1", "empty"), info("F", "effect")];
        assert_eq!(blend_layer(&launch, 0), Some(Start(0)));
        assert_eq!(blend_layer(&launch, 1), Some(Start(0)), "not the bottom F");

        let three = [info("A", "effect"), info("B", "media"), info("C", "effect")];
        assert_eq!(blend_layer(&three, 1), Some(Explain(1)), "the selected one");
        assert_eq!(blend_layer(&three, 2), Some(Explain(0)), "else the topmost");

        // Beneath means beneath and visible: with C hidden, B is the bottom.
        let mut hidden_bottom = three.clone();
        hidden_bottom[2].enabled = false;
        assert_eq!(blend_layer(&hidden_bottom, 1), Some(Explain(0)));
        let mut only_top_hidden = three.clone();
        only_top_hidden[0].enabled = false;
        assert_eq!(blend_layer(&only_top_hidden, 0), Some(Explain(1)));

        // A layer that runs something always wins over starting one.
        let mixed = [
            info("Layer 1", "empty"),
            info("B", "effect"),
            info("F", "effect"),
        ];
        assert_eq!(blend_layer(&mixed, 0), Some(Explain(1)));

        // Nothing to start on: locked, hidden, or nothing beneath.
        let mut locked = launch.clone();
        locked[0].locked = true;
        assert_eq!(blend_layer(&locked, 0), None);
        let mut hidden = launch.clone();
        hidden[0].enabled = false;
        assert_eq!(blend_layer(&hidden, 0), None);
        assert_eq!(blend_layer(&[info("F", "effect")], 0), None);
        assert_eq!(blend_layer(&[info("Layer 1", "empty")], 0), None);
        assert_eq!(blend_layer(&[], 0), None);
    }

    /// The harness layer named `name`'s index.
    fn layer_named(h: &ShellHarness, name: &str) -> Option<usize> {
        h.layers
            .iter()
            .position(|l| l.effect_name.as_deref() == Some(name))
    }

    // Kevin's call (#3231): on the launch stack the tour loads Aurora into
    // the empty Layer 1, in Screen, and explains it from then on; it stays
    // after the tour. The F beneath is untouched.
    #[test]
    fn the_layers_tour_starts_aurora_on_an_empty_top_layer() {
        let mut h = ShellHarness::new(Vec2::new(1400.0, 900.0));
        let f_before = h.layers[1].clone();
        start(&h.ctx, Tour::Layers);
        h.settle(30);
        assert_eq!(layer_named(&h, STARTER_EFFECT), Some(0));
        assert_eq!(h.layers[0].blend_mode, STARTER_BLEND);
        assert_eq!(h.active_layer, 0);
        assert_eq!(h.layers[1].effect_name, f_before.effect_name);
        assert_eq!(h.layers[1].blend_mode, f_before.blend_mode);
        // Its row says so, as the stack's lines are what step 1 reads.
        assert!(
            h.text_rect("\u{2191}  Aurora blends onto the picture below: Screen 100%")
                .is_some()
        );

        // The next steps explain it, and change nothing: a mode picked on
        // the Blend step stays.
        h.key(Key::Enter);
        h.settle(30);
        assert_eq!(h.active_layer, 0);
        h.layers[0].blend_mode = BlendMode::Multiply;
        for i in 2..Tour::Layers.steps().len() - 1 {
            go_to(&h.ctx, Tour::Layers, i);
            h.settle(30);
            assert_eq!(h.active_layer, 0, "step {}", i + 1);
        }
        assert_eq!(
            h.layers[0].blend_mode,
            BlendMode::Multiply,
            "no second start"
        );
        let last = Tour::Layers.steps().len() - 1;
        go_to(&h.ctx, Tour::Layers, last);
        h.settle(30);
        assert!(
            crate::ui::panels::stack_panel::master_selected(&h.ctx),
            "the last step is Master's"
        );
        go_to(&h.ctx, Tour::Layers, 0);
        h.settle(30);
        assert!(!crate::ui::panels::stack_panel::master_selected(&h.ctx));
        assert_eq!(h.layers[0].blend_mode, BlendMode::Multiply);
        h.key(Key::Escape);
        assert_eq!(layer_named(&h, STARTER_EFFECT), Some(0), "it stays");
    }

    // A stack with a layer to explain is the user's: the tour selects that
    // layer and loads nothing.
    #[test]
    fn the_layers_tour_leaves_a_running_layer_alone() {
        let mut h = ShellHarness::new(Vec2::new(1400.0, 900.0));
        h.layers[0].effect_index = Some(3);
        h.layers[0].effect_name = Some("Effect 3".into());
        h.layers[0].blend_mode = BlendMode::Difference;
        h.active_layer = 1;
        start(&h.ctx, Tour::Layers);
        h.settle(30);
        assert_eq!(h.active_layer, 0);
        assert_eq!(h.layers[0].effect_name.as_deref(), Some("Effect 3"));
        assert_eq!(h.layers[0].blend_mode, BlendMode::Difference);
        assert_eq!(layer_named(&h, STARTER_EFFECT), None);
    }

    fn is_dim(id: Option<Id>) -> bool {
        (0..16).any(|i| id == Some(Id::new("tour_dim").with(i)))
    }

    // The Blend step keeps the output lit: dimmed everywhere but the Blend
    // section and the output, and both take the pointer.
    #[test]
    fn the_blend_step_leaves_the_output_lit() {
        let mut h = ShellHarness::new(Vec2::new(1400.0, 900.0));
        go_to(&h.ctx, Tour::Layers, 1);
        h.settle(30);
        let screen = h.screen();
        let lit = hole(drawn(&h.ctx, Anchor::Blend).unwrap(), screen);
        let watched = hole(drawn(&h.ctx, Anchor::Output).unwrap(), screen);
        assert!(!lit.intersects(watched));
        let out = h.frame_output(vec![]);
        let dims: Vec<Rect> = out
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Rect(r) if r.fill == DIM => Some(r.rect),
                _ => None,
            })
            .collect();
        let covered: f32 = dims.iter().map(|r| r.area()).sum();
        let want = screen.area() - lit.area() - watched.area();
        assert!(
            (covered - want).abs() < 1.0,
            "dimmed {covered}, want {want}"
        );
        for d in &dims {
            assert!(!d.intersects(lit.shrink(0.5)) && !d.intersects(watched.shrink(0.5)));
        }
        let layer_at = |p: Pos2| h.ctx.layer_id_at(p).map(|l| l.id);
        assert!(!is_dim(layer_at(watched.center())), "the output is lit");
        assert!(!is_dim(layer_at(lit.center())), "the controls are lit");
        let between = Pos2::new(watched.left() - 20.0, watched.bottom() + 40.0);
        assert!(is_dim(layer_at(between)), "the rest is dimmed");
    }

    // The dimming is cut around every hole, whatever their places: it covers
    // the rest exactly once and none of any hole.
    #[test]
    fn the_dimming_is_cut_around_each_hole() {
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 700.0));
        let r = |x0: f32, y0: f32, x1: f32, y1: f32| {
            Rect::from_min_max(Pos2::new(x0, y0), Pos2::new(x1, y1))
        };
        let cases: [&[Rect]; 5] = [
            &[],
            &[r(100.0, 100.0, 300.0, 200.0)],
            // Side by side, one taller, like the Blend step.
            &[r(400.0, 150.0, 700.0, 300.0), r(740.0, 10.0, 990.0, 180.0)],
            // Overlapping, and one running off the screen.
            &[
                r(100.0, 100.0, 400.0, 400.0),
                r(300.0, 300.0, 1200.0, 500.0),
            ],
            &[screen],
        ];
        for holes in cases {
            let pieces = around(holes, screen);
            if holes.len() == 1 && holes[0] != screen {
                assert_eq!(pieces.len(), 4, "one hole, four pieces");
            }
            // A grid of sample points: each outside every hole is in exactly
            // one piece, each inside a hole in none.
            for j in 0..70 {
                for i in 0..100 {
                    let p = Pos2::new(5.0 + 10.0 * i as f32, 5.0 + 10.0 * j as f32);
                    let n = pieces.iter().filter(|q| q.contains(p)).count();
                    let inside = holes.iter().any(|h| h.contains(p));
                    assert_eq!(n, usize::from(!inside), "{holes:?} at {p:?}");
                }
            }
        }
    }

    // The note never covers what a step asks you to watch, if a side allows.
    #[test]
    fn the_note_stays_off_the_watched_part() {
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(1400.0, 900.0));
        let size = Vec2::new(CALLOUT_WIDTH, 200.0);
        // A tall section mid-window, the output down the right: the
        // right-hand side has the most room, and the output is there.
        let blend = Rect::from_min_max(Pos2::new(420.0, 90.0), Pos2::new(700.0, 700.0));
        let output = Rect::from_min_max(Pos2::new(1000.0, 60.0), Pos2::new(1390.0, 500.0));
        let free = place_callout(blend, size, screen, &[]);
        assert!(
            free.intersects(output),
            "the case needs the room to be there"
        );
        let n = place_callout(blend, size, screen, &[output]);
        assert!(!n.intersects(output) && !n.intersects(blend), "{n:?}");
        assert!(screen.contains_rect(n));
    }

    // And the tour passes the watched part along: with the stack column
    // dragged narrow the room is on the output's side, and the note must go
    // elsewhere rather than over the picture it asks you to watch.
    #[test]
    fn the_drawn_note_stays_off_the_watched_part() {
        let ctx = Context::default();
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(1400.0, 900.0));
        let blend = Rect::from_min_max(Pos2::new(420.0, 90.0), Pos2::new(700.0, 700.0));
        let output = Rect::from_min_max(Pos2::new(1000.0, 60.0), Pos2::new(1390.0, 500.0));
        go_to(&ctx, Tour::Layers, 1);
        for _ in 0..3 {
            let input = egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    anchor(ui, Anchor::Blend, blend);
                    anchor(ui, Anchor::Output, output);
                });
                draw(ctx);
            });
        }
        let note = callout_rect(&ctx).unwrap();
        assert!(!note.intersects(hole(output, screen)), "{note:?}");
        assert!(!note.intersects(hole(blend, screen)), "{note:?}");
    }
}
