//! The voice path, V5: the on-device provider (board #3751,
//! `docs/xr/VOICE_DESIGN.md`, "V5 as built"). A sentence the grammar
//! (`intent.rs`) misses goes to our own 17M decision model on the headset
//! instead of a hosted one: no network, no key, a fifth of a second. The
//! model is an encoder that scores candidates against an instruction and
//! the room's state; four typed decisions in a row (is it a request, which
//! action kind, which surface, which value) make one [`AgentAction`],
//! which becomes an [`Intent`] through the grammar's own resolution
//! ([`Intent::from_json`]), exactly as a network provider's answer does.
//!
//! **The spec is data** ([`parse_spec`]): `assets/xr/models/s1-17m-spec.json`
//! is the training pipeline's `provider-spec.json`, copied verbatim, read
//! at launch next to the model. The four steps' instructions, the request
//! step's yes and no criteria and its threshold, every candidate list and
//! text, the target texts per kind and the prompt layout come from it, so
//! a retrained model with new wording ships with a new spec and no code
//! change. What the code fixes is what the training toolkit fixes: the
//! prompt's assembly ([`prefix_ids`], [`document_ids`]), the state's JSON
//! ([`state_json`], [`noul_state`]) and the model's inputs and output.
//!
//! **Rendering**, pure and desktop-tested against the toolkit's own token
//! ids (`tests/data/s1-17m-parity.json`): the prefix is `[CLS]`, the system
//! text (empty here), `enc("Instruction: ")`, the instruction, `enc("\n")`,
//! `enc("State: ")`, the state and `[SEP]`, each part tokenized on its own
//! with no special tokens, the instruction and the state truncated to share
//! the 512-token budget ([`allocate`]); a candidate is
//! `enc("Candidate: <id>: <text>")` cut to 63 tokens and `[SEP]`. The
//! request step's state wraps the room in the noul envelope with its yes
//! and no texts.
//!
//! **The cascade** ([`cascade`]): the request step (the noul column, below
//! the spec's threshold a miss); the kind over the fifteen (the choice
//! column, softmaxed over the decision's candidates); the target when the
//! kind takes one (every surface by its name, `all <kind>s` for each kind
//! with two or more surfaces, `pointed` when something is pointed at); the
//! value from the list the kind and its target allow. A confidence floor on
//! the kind and the value ([`Floors`], `debug.fosfora.localmin`) turns a
//! weak answer into a miss, so it never changes the room. Every decision is
//! one run of the model with one prefix and all its candidates.
//!
//! **The runtime** (Android, and the desktop tests): ONNX Runtime through
//! `ort` with `load-dynamic`, `libonnxruntime.so` from the
//! onnxruntime-android AAR, dlopened by name (the app's native library
//! directory is on the namespace's search path, as for the OpenXR loader),
//! the Activity's `nativeLibraryDir` as the fallback; the tokenizer through
//! `tokenizers`. One session, on the `fosfora-local` worker thread, loaded
//! after the speech model, kept for the process ([`Local`]).
//!
//! **Teardown.** `ort` 2.0.0-rc.13, which the device bench ran, releases
//! its environment from the executable's `.fini_array`, which on Android
//! runs after the dlopened runtime's static destructors and aborts at
//! libc `exit`. The app reaches neither: it runs `ort` 2.0.0-rc.11 (the
//! core pins it, and one lock holds one `ort`), which keeps no exit-time
//! release at all (its environment lives while a session does), and the
//! app never returns through libc `exit`: `android_main` returning only
//! finishes the activity (`android-activity`'s glue calls
//! `ANativeActivity_finish` and the thread ends), the process stays cached
//! until the system kills it with SIGKILL (force-stop, the low-memory
//! killer), and nothing dlcloses `libfosfora_xr.so`. No vendored patch. The
//! one hazard rc.11 does carry is a second environment in one process (ONNX
//! Runtime cannot create one after releasing the first), which a relaunch
//! in the cached process would hit; the session is therefore never
//! dropped: the worker parks it in a process-wide slot when the app goes
//! away, and the next launch takes it back.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::Value;

use crate::agent::{Reply, RoomState, SurfaceState};
use crate::intent::{AgentAction, Intent, Miss, Reason, Surface, Vocabulary};
use crate::surfaces::{KIND_NAMES, KIND_NONE, SurfaceBehavior, kind_from_name};

/// The provider's name, as `voice.json` and the log use it.
pub const PROVIDER: &str = crate::agent::LOCAL;
/// The model's name in the log.
pub const MODEL_NAME: &str = "s1-17m-int8";
/// Where the files are under the assets dir, and their names
/// (`assets/xr/models/` in the repo; `MODELS.txt` and `LICENSE.md` there).
pub const MODELS_DIR: &str = "xr/models";
pub const MODEL_FILE: &str = "s1-17m-int8.onnx";
pub const TOKENIZER_FILE: &str = "s1-17m-tokenizer.json";
pub const TOKENIZER_CONFIG_FILE: &str = "s1-17m-tokenizer_config.json";
pub const SPEC_FILE: &str = "s1-17m-spec.json";
/// The runtime library, by the name the AAR packages it under.
pub const LIBRARY: &str = "libonnxruntime.so";
/// The session's intra-op threads (`debug.fosfora.localthreads`), as
/// whisper's (`debug.fosfora.voicethreads`).
pub const THREADS: usize = 3;
/// The confidence floor on the kind and the value
/// (`debug.fosfora.localmin`).
pub const FLOOR: f32 = 0.35;
/// The model's output columns: `logits [n, 3]`, choice, noul, score.
const CHOICE: usize = 0;
const NOUL: usize = 1;
#[cfg(any(target_os = "android", test))]
const COLUMNS: usize = 3;
/// What the spec's `model.output` must say for the columns above.
const OUTPUT_COLUMNS: &str = "columns choice, noul, score";

// ------------------------------------------------------------------ the spec

/// One candidate of a decision: the id the answer names, and the text the
/// model reads.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Candidate {
    pub id: String,
    pub text: String,
}

impl Candidate {
    fn new(id: &str, text: &str) -> Self {
        Self {
            id: id.to_owned(),
            text: text.to_owned(),
        }
    }
}

/// The tokenizer's special tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct SpecialIds {
    pub cls: u32,
    pub sep: u32,
    pub pad: u32,
}

/// The token budgets: the prefix (`query_length`), a candidate
/// (`document_length`), and the special tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub query: usize,
    pub document: usize,
    pub ids: SpecialIds,
}

/// Which of the prefix's two parts comes first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    InstructionState,
    StateInstruction,
}

impl Layout {
    /// The layout a string names by its first word (`instruction_state
    /// (the model was trained on both orders 1:1; use this one)`).
    pub fn parse(s: &str) -> Option<Self> {
        match s.split_whitespace().next()? {
            "instruction_state" => Some(Self::InstructionState),
            "state_instruction" => Some(Self::StateInstruction),
            _ => None,
        }
    }
}

/// The request step: its instruction, the yes and no texts, the
/// candidates' order, and the threshold on p(true).
#[derive(Debug, Clone, PartialEq)]
pub struct RequestStep {
    pub instruction: String,
    pub yes: String,
    pub no: String,
    pub order: Vec<String>,
    pub threshold: f32,
}

/// The kind step: its instruction and the kinds.
#[derive(Debug, Clone, PartialEq)]
pub struct KindStep {
    pub instruction: String,
    pub candidates: Vec<Candidate>,
}

/// The target step: its instruction, the kinds that take a target, the
/// text a surface of each kind reads as, a kind's group text, and the
/// pointed surface's text.
#[derive(Debug, Clone, PartialEq)]
pub struct TargetStep {
    pub instruction: String,
    pub only_for: Vec<String>,
    pub kind_text: BTreeMap<String, String>,
    pub group_text: BTreeMap<String, String>,
    pub pointed_text: String,
}

/// The value step: a question per kind, each kind's behaviors and their
/// texts, the fixed lists (color, band, strength, the toggles), the
/// effects' text, and the kinds that take no value.
#[derive(Debug, Clone, PartialEq)]
pub struct ValueStep {
    pub questions: BTreeMap<String, String>,
    pub behaviors: BTreeMap<String, Vec<String>>,
    pub behavior_text: BTreeMap<String, String>,
    pub lists: BTreeMap<String, Vec<Candidate>>,
    pub effect_text: String,
    pub none_for: Vec<String>,
}

/// `provider-spec.json`, read ([`parse_spec`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Spec {
    pub limits: Limits,
    pub layout: Layout,
    pub request: RequestStep,
    pub kind: KindStep,
    pub target: TargetStep,
    pub value: ValueStep,
    /// The spec's notes on how its ids reach the app (`band level` is
    /// `rms`, `pointed` is `Target::Pointed`, …), for the log; the mapping
    /// itself is the grammar's ([`Intent::from_json`]).
    pub id_mapping: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct RawSpec {
    model: RawModel,
    rendering: RawRendering,
    cascade: Vec<RawStep>,
    #[serde(default)]
    id_mapping_to_app: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct RawModel {
    output: String,
    special_ids: SpecialIds,
    query_length: usize,
    document_length: usize,
    query_truncation: String,
}

#[derive(Deserialize)]
struct RawRendering {
    layout: String,
}

#[derive(Deserialize)]
#[serde(tag = "step", rename_all = "snake_case")]
enum RawStep {
    Request {
        instruction: String,
        yes: String,
        no: String,
        criteria_order: Vec<String>,
        rule: String,
    },
    Kind {
        instruction: String,
        candidates: Vec<Candidate>,
    },
    Target {
        instruction: String,
        only_for: Vec<String>,
        candidates: String,
    },
    Value {
        questions: BTreeMap<String, String>,
        candidates: BTreeMap<String, Value>,
        none_for: Vec<String>,
    },
}

#[derive(Deserialize)]
struct RawBehaviors {
    by_kind: BTreeMap<String, Vec<String>>,
    texts: BTreeMap<String, String>,
}

/// The `n`th JSON object embedded in `text` (the target step's prose
/// carries its two maps inline), as a string map.
fn embedded_map(text: &str, n: usize) -> Result<BTreeMap<String, String>, String> {
    let mut rest = text;
    for _ in 0..n {
        let object = crate::agent::first_object(rest).ok_or("missing")?;
        let end = rest.find(object).unwrap_or_default() + object.len();
        rest = &rest[end..];
    }
    let object = crate::agent::first_object(rest).ok_or("missing")?;
    serde_json::from_str(object).map_err(|e| e.to_string())
}

/// The text between `'` quotes right after `marker` in `text`.
fn quoted_after<'a>(text: &'a str, marker: &str) -> Option<&'a str> {
    let rest = &text[text.find(marker)? + marker.len()..];
    let rest = rest.strip_prefix('\'')?;
    Some(&rest[..rest.find('\'')?])
}

/// The number after `>=` in a rule (`stop unless p(true) >= 0.5`).
fn threshold(rule: &str) -> Option<f32> {
    rule.split(">=")
        .nth(1)?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

/// Read `provider-spec.json`: the model's limits and special ids, the
/// layout, and the four steps (each exactly once). Refused (the provider
/// stays off, with the reason) when the output's columns, the query's
/// truncation or the layout are not the ones this code renders for, a
/// step is missing, or a candidate list does not parse; every behavior
/// must be one the app knows.
pub fn parse_spec(json: &str) -> Result<Spec, String> {
    let raw: RawSpec = serde_json::from_str(json).map_err(|e| format!("the spec: {e}"))?;
    if !raw.model.output.contains(OUTPUT_COLUMNS) {
        return Err(format!(
            "the spec's output is not \"{OUTPUT_COLUMNS}\": {}",
            raw.model.output
        ));
    }
    if raw.model.query_truncation != "balanced" {
        return Err(format!(
            "the spec's query truncation is {}, not balanced",
            raw.model.query_truncation
        ));
    }
    let layout = Layout::parse(&raw.rendering.layout)
        .ok_or_else(|| format!("the spec's layout {} is unknown", raw.rendering.layout))?;
    let limits = Limits {
        query: raw.model.query_length,
        document: raw.model.document_length,
        ids: raw.model.special_ids,
    };
    if limits.document < 2 || limits.query < 8 {
        return Err("the spec's lengths are too short".to_owned());
    }
    let (mut request, mut kind, mut target, mut value) = (None, None, None, None);
    for step in raw.cascade {
        match step {
            RawStep::Request {
                instruction,
                yes,
                no,
                criteria_order,
                rule,
            } => {
                let mut ids = criteria_order.clone();
                ids.sort();
                if ids != ["false", "true"] {
                    return Err("the request step's criteria are not false and true".to_owned());
                }
                let threshold = threshold(&rule)
                    .filter(|t| (0.0..=1.0).contains(t))
                    .ok_or_else(|| format!("the request step's rule has no threshold: {rule}"))?;
                request = Some(RequestStep {
                    instruction,
                    yes,
                    no,
                    order: criteria_order,
                    threshold,
                });
            }
            RawStep::Kind {
                instruction,
                candidates,
            } => {
                kind = Some(KindStep {
                    instruction,
                    candidates,
                });
            }
            RawStep::Target {
                instruction,
                only_for,
                candidates,
            } => {
                let map = |n| {
                    embedded_map(&candidates, n)
                        .map_err(|e| format!("the target step's candidates, map {n}: {e}"))
                };
                let (kind_text, group_text) = (map(0)?, map(1)?);
                for k in kind_text.keys().chain(group_text.keys()) {
                    if kind_from_name(k).is_none() {
                        return Err(format!("the target step names an unknown kind {k}"));
                    }
                }
                let pointed_text = quoted_after(&candidates, "'pointed' (")
                    .ok_or("the target step's candidates name no pointed text")?
                    .to_owned();
                target = Some(TargetStep {
                    instruction,
                    only_for,
                    kind_text,
                    group_text,
                    pointed_text,
                });
            }
            RawStep::Value {
                questions,
                candidates,
                none_for,
            } => value = Some(value_step(questions, candidates, none_for)?),
        }
    }
    let missing = |name: &str| format!("the spec has no {name} step");
    Ok(Spec {
        limits,
        layout,
        request: request.ok_or_else(|| missing("request"))?,
        kind: kind.ok_or_else(|| missing("kind"))?,
        target: target.ok_or_else(|| missing("target"))?,
        value: value.ok_or_else(|| missing("value"))?,
        id_mapping: raw.id_mapping_to_app,
    })
}

/// The value step's candidates: `behavior` (by kind, with texts),
/// `effect` (prose naming the effects' text), the toggles (one list under
/// a key naming its kinds in parentheses), and plain lists.
fn value_step(
    questions: BTreeMap<String, String>,
    candidates: BTreeMap<String, Value>,
    none_for: Vec<String>,
) -> Result<ValueStep, String> {
    let mut behaviors = BTreeMap::new();
    let mut behavior_text = BTreeMap::new();
    let mut lists = BTreeMap::new();
    let mut effect_text = None;
    for (key, v) in candidates {
        let list = || -> Result<Vec<Candidate>, String> {
            serde_json::from_value(v.clone()).map_err(|e| format!("the value list {key}: {e}"))
        };
        if key == "behavior" {
            let b: RawBehaviors = serde_json::from_value(v.clone())
                .map_err(|e| format!("the value step's behaviors: {e}"))?;
            for (kind, ids) in &b.by_kind {
                if kind_from_name(kind).is_none() {
                    return Err(format!("the value step names an unknown kind {kind}"));
                }
                for id in ids {
                    if SurfaceBehavior::from_name(id).is_none() {
                        return Err(format!("the value step names an unknown behavior {id}"));
                    }
                    if !b.texts.contains_key(id) {
                        return Err(format!("the value step has no text for {id}"));
                    }
                }
            }
            behaviors = b.by_kind;
            behavior_text = b.texts;
        } else if key == "effect" {
            let prose = v.as_str().ok_or("the value step's effect is not text")?;
            effect_text = Some(
                quoted_after(prose, "text ")
                    .ok_or("the value step's effect names no text")?
                    .to_owned(),
            );
        } else if let Some(kinds) = key
            .strip_prefix("toggles (")
            .and_then(|k| k.strip_suffix(')'))
        {
            let list = list()?;
            for kind in kinds.split(',').map(str::trim).filter(|k| !k.is_empty()) {
                lists.insert(kind.to_owned(), list.clone());
            }
        } else {
            lists.insert(key.clone(), list()?);
        }
    }
    Ok(ValueStep {
        questions,
        behaviors,
        behavior_text,
        lists,
        effect_text: effect_text.ok_or("the value step has no effect text")?,
        none_for,
    })
}

// ------------------------------------------------------------- rendering

/// Text to token ids, with no special tokens added (the tokenizer, or a
/// stand-in in the tests).
pub trait Encode {
    fn encode(&self, text: &str) -> Result<Vec<u32>, String>;
}

/// The toolkit's balanced budget: half of `budget` reserved for each of
/// the instruction and the context (the odd token to the instruction),
/// then what one leaves unused given to the other.
pub fn allocate(instruction: usize, context: usize, budget: usize) -> (usize, usize) {
    let mut i = instruction.min(budget.div_ceil(2));
    let mut c = context.min(budget / 2);
    i += (instruction - i).min(budget - i - c);
    c += (context - c).min(budget - i - c);
    (i, c)
}

/// The prefix's token ids, as the training toolkit builds them:
/// `[CLS]`, the system text with `"\n\n"` (when it has any), then the
/// instruction and the state in `layout`'s order, each behind its marker
/// (`"Instruction: "`, `"State: "`) and joined by `"\n"`, then `[SEP]`.
/// Each part is tokenized on its own; the instruction and the state share
/// what the markers and the system text leave of `limits.query`
/// ([`allocate`]), their tails cut.
pub fn prefix_ids(
    enc: &dyn Encode,
    limits: &Limits,
    layout: Layout,
    system: &str,
    instruction: &str,
    state: &str,
) -> Result<Vec<i64>, String> {
    let cut = |mut ids: Vec<u32>| {
        ids.truncate(limits.query);
        ids
    };
    let system = if system.trim().is_empty() {
        Vec::new()
    } else {
        cut(enc.encode(&format!("{system}\n\n"))?)
    };
    let instruction = cut(enc.encode(instruction)?);
    let state = cut(enc.encode(state)?);
    let im = enc.encode("Instruction: ")?;
    let sm = enc.encode("State: ")?;
    let sep = enc.encode("\n")?;
    let overhead = 2 + im.len() + sm.len() + sep.len() + system.len();
    let budget = limits
        .query
        .checked_sub(overhead)
        .filter(|&b| b >= 2)
        .ok_or("the system text and the markers leave fewer than two tokens")?;
    let (ni, nc) = allocate(instruction.len(), state.len(), budget);
    let ins = im.iter().chain(&instruction[..ni]);
    let ctx = sm.iter().chain(&state[..nc]);
    let mut ids = vec![i64::from(limits.ids.cls)];
    ids.extend(system.iter().map(|&t| i64::from(t)));
    let body: Vec<u32> = match layout {
        Layout::InstructionState => ins.chain(&sep).chain(ctx).copied().collect(),
        Layout::StateInstruction => ctx.chain(&sep).chain(ins).copied().collect(),
    };
    ids.extend(body.into_iter().map(i64::from));
    ids.push(i64::from(limits.ids.sep));
    Ok(ids)
}

/// A candidate's token ids: its text cut to `limits.document - 1` tokens,
/// then `[SEP]`.
pub fn document_ids(enc: &dyn Encode, limits: &Limits, text: &str) -> Result<Vec<i64>, String> {
    let mut ids: Vec<i64> = enc
        .encode(text)?
        .into_iter()
        .take(limits.document - 1)
        .map(i64::from)
        .collect();
    ids.push(i64::from(limits.ids.sep));
    Ok(ids)
}

/// A candidate's text as the model reads it: `Candidate: <id>: <text>`.
pub fn candidate_text(c: &Candidate) -> String {
    format!("Candidate: {}: {}", c.id, c.text)
}

/// A string as JSON, escaped as Python's `json.dumps(…, ensure_ascii=False)`
/// escapes it (`"`, `\` and control characters only).
fn json_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_owned())
}

/// A surface in the state: its behavior, and its color when not its own.
/// Band and strength stay out: no decision needs them, and every token is
/// paid on the headset.
fn surface_text(s: &SurfaceState) -> String {
    if s.color == "own" {
        s.behavior.clone()
    } else {
        format!("{}, {}", s.behavior, s.color)
    }
}

/// The state object, its items joined by `comma` and its keys by `colon`.
fn state_object(sentence: &str, room: &RoomState, comma: &str, colon: &str) -> String {
    let join = |items: Vec<String>| items.join(comma);
    let effects = join(room.effects.iter().map(|e| json_str(e)).collect());
    let surfaces = join(
        room.surfaces
            .iter()
            .map(|s| format!("{}{colon}{}", json_str(&s.name), json_str(&surface_text(s))))
            .collect(),
    );
    let fields = [
        ("sentence", json_str(sentence.trim())),
        (
            "pointed",
            json_str(room.pointed.as_deref().unwrap_or("nothing")),
        ),
        (
            "effect",
            json_str(room.effect.as_deref().unwrap_or("nothing")),
        ),
        ("effects", format!("[{effects}]")),
        ("surfaces", format!("{{{surfaces}}}")),
    ];
    format!(
        "{{{}}}",
        join(
            fields
                .iter()
                .map(|(k, v)| format!("{}{colon}{v}", json_str(k)))
                .collect()
        )
    )
}

/// The state every decision of a sentence shares, as the generator wrote
/// it (Python's default separators): the sentence, the pointed surface's
/// name or `nothing`, the world effect showing and every effect, and each
/// surface's behavior (`"behavior, color"` when its color is not its own).
pub fn state_json(sentence: &str, room: &RoomState) -> String {
    state_object(sentence, room, ", ", ": ")
}

/// The request step's state: the yes and no texts before the evidence,
/// then the state, compact (`{"noul":{"yes":…,"no":…},"state":{…}}`).
pub fn noul_state(yes: &str, no: &str, sentence: &str, room: &RoomState) -> String {
    format!(
        "{{\"noul\":{{\"yes\":{},\"no\":{}}},\"state\":{}}}",
        json_str(yes),
        json_str(no),
        state_object(sentence, room, ",", ":")
    )
}

// ------------------------------------------------------------ the decisions

/// The cascade's steps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Request,
    Kind,
    Target,
    Value,
}

impl Step {
    pub fn name(self) -> &'static str {
        match self {
            Self::Request => "request",
            Self::Kind => "kind",
            Self::Target => "target",
            Self::Value => "value",
        }
    }
}

/// One decision: its step, its instruction, the model's output column it
/// reads, and its candidates.
#[derive(Debug, Clone, PartialEq)]
pub struct Decision {
    pub step: Step,
    pub instruction: String,
    pub column: usize,
    pub candidates: Vec<Candidate>,
}

/// A kind's group target: `all <kind>s` ("all tables", "all others").
pub fn group_id(kind: &str) -> String {
    format!("all {kind}s")
}

impl Spec {
    /// Is the sentence a request: the noul column over `false` and `true`
    /// in the spec's order, read with the yes and no texts.
    pub fn request_decision(&self) -> Decision {
        let r = &self.request;
        Decision {
            step: Step::Request,
            instruction: r.instruction.clone(),
            column: NOUL,
            candidates: r
                .order
                .iter()
                .map(|id| Candidate::new(id, if id == "true" { &r.yes } else { &r.no }))
                .collect(),
        }
    }

    /// Which action kind.
    pub fn kind_decision(&self) -> Decision {
        Decision {
            step: Step::Kind,
            instruction: self.kind.instruction.clone(),
            column: CHOICE,
            candidates: self.kind.candidates.clone(),
        }
    }

    /// Which surface, for a kind that takes one (`None` otherwise): every
    /// surface whose kind the spec names, by its name, reading as its kind;
    /// then `all <kind>s` for each kind with two or more surfaces, in the
    /// app's kind order; then `pointed` when something is pointed at.
    pub fn target_decision(&self, kind: &str, room: &RoomState) -> Option<Decision> {
        let t = &self.target;
        if !t.only_for.iter().any(|k| k == kind) {
            return None;
        }
        let mut candidates: Vec<Candidate> = room
            .surfaces
            .iter()
            .filter_map(|s| {
                t.kind_text
                    .get(&s.kind)
                    .map(|text| Candidate::new(&s.name, text))
            })
            .collect();
        for (_, k) in KIND_NAMES {
            let several = room.surfaces.iter().filter(|s| s.kind == k).count() >= 2;
            if let (true, Some(text)) = (several, t.group_text.get(k)) {
                candidates.push(Candidate::new(&group_id(k), text));
            }
        }
        if room.pointed.is_some() {
            candidates.push(Candidate::new("pointed", &t.pointed_text));
        }
        Some(Decision {
            step: Step::Target,
            instruction: t.instruction.clone(),
            column: CHOICE,
            candidates,
        })
    }

    /// The kind word of what `target` names in `room`: the pointed
    /// surface's, a group's, or a surface's by name.
    pub fn target_kind<'a>(&self, target: &str, room: &'a RoomState) -> Option<&'a str> {
        let by_name = |name: &str| {
            room.surfaces
                .iter()
                .find(|s| s.name == name)
                .map(|s| s.kind.as_str())
        };
        if target == "pointed" {
            return by_name(room.pointed.as_deref()?);
        }
        KIND_NAMES
            .iter()
            .find(|(_, k)| group_id(k) == target)
            .and_then(|(_, k)| room.surfaces.iter().find(|s| s.kind == *k))
            .map(|s| s.kind.as_str())
            .or_else(|| by_name(target))
    }

    /// Which value, for a kind that takes one (`None` otherwise, or when
    /// the spec has no question for it): a behavior from the target's
    /// kind's list, an effect of the room's, or the kind's fixed list.
    pub fn value_decision(
        &self,
        kind: &str,
        target: Option<&str>,
        room: &RoomState,
    ) -> Option<Decision> {
        let v = &self.value;
        if v.none_for.iter().any(|k| k == kind) {
            return None;
        }
        let instruction = v.questions.get(kind)?.clone();
        let candidates = match kind {
            "behavior" => {
                let of = self.target_kind(target?, room)?;
                v.behaviors
                    .get(of)?
                    .iter()
                    .map(|b| Candidate::new(b, v.behavior_text.get(b).map_or("", String::as_str)))
                    .collect()
            }
            "effect" => room
                .effects
                .iter()
                .map(|e| Candidate::new(e, &v.effect_text))
                .collect(),
            _ => v.lists.get(kind)?.clone(),
        };
        Some(Decision {
            step: Step::Value,
            instruction,
            column: CHOICE,
            candidates,
        })
    }

    /// A decision's token ids for `sentence` in `room`: the prefix (the
    /// noul state for the request step, the plain state otherwise) and one
    /// row per candidate.
    pub fn render(
        &self,
        enc: &dyn Encode,
        d: &Decision,
        sentence: &str,
        room: &RoomState,
    ) -> Result<(Vec<i64>, Vec<Vec<i64>>), String> {
        let state = if d.step == Step::Request {
            noul_state(&self.request.yes, &self.request.no, sentence, room)
        } else {
            state_json(sentence, room)
        };
        let prefix = prefix_ids(enc, &self.limits, self.layout, "", &d.instruction, &state)?;
        let docs = d
            .candidates
            .iter()
            .map(|c| document_ids(enc, &self.limits, &candidate_text(c)))
            .collect::<Result<_, _>>()?;
        Ok((prefix, docs))
    }
}

// --------------------------------------------------------------- the cascade

/// A softmax, stable for large logits.
pub fn softmax(logits: &[f32]) -> Vec<f32> {
    let max = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let exp: Vec<f32> = logits.iter().map(|x| (x - max).exp()).collect();
    let sum: f32 = exp.iter().sum();
    exp.iter().map(|e| e / sum).collect()
}

/// The first of the largest (numpy's `argmax`).
fn argmax(p: &[f32]) -> Option<usize> {
    p.iter()
        .enumerate()
        .fold(None, |best: Option<(usize, f32)>, (i, &x)| match best {
            Some((_, b)) if b >= x => best,
            _ => Some((i, x)),
        })
        .map(|(i, _)| i)
}

/// The confidence floors: a kind or a value whose probability is under
/// its floor is a miss. The target has none: twenty surfaces share its
/// probability, and its answer is checked against the room anyway.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Floors {
    pub kind: f32,
    pub value: f32,
}

impl Floors {
    /// The same floor on the kind and the value (`debug.fosfora.localmin`).
    pub fn both(p: f32) -> Self {
        Self { kind: p, value: p }
    }
}

impl Default for Floors {
    fn default() -> Self {
        Self::both(FLOOR)
    }
}

/// One step's answer: the id it picked and its probability (the request
/// step: p(true)).
#[derive(Debug, Clone, PartialEq)]
pub struct Pick {
    pub step: Step,
    pub id: String,
    pub p: f32,
}

/// What the cascade came to.
#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    /// One action for the room.
    Act(AgentAction),
    /// The request step said no (p(true) under the spec's threshold).
    NotARequest,
    /// A step's pick was under its floor.
    Unsure(Step),
    /// A step had nothing to choose from (no surfaces, no effects).
    Nothing(Step),
}

/// The cascade's picks in order, and its verdict.
#[derive(Debug, Clone, PartialEq)]
pub struct Answer {
    pub picks: Vec<Pick>,
    pub verdict: Verdict,
}

/// The four decisions for one sentence in `room`, each through `decide`
/// (the decision's candidates' probabilities, in order): the request step
/// (stop under the spec's threshold), the kind (stop under its floor),
/// the target when the kind takes one, the value when it takes one (stop
/// under its floor). The verdict's action names the spec's ids
/// (`behavior`, `table 14` or `all tables` or `pointed`, `amber`).
pub fn cascade(
    spec: &Spec,
    room: &RoomState,
    floors: Floors,
    mut decide: impl FnMut(&Decision) -> Result<Vec<f32>, String>,
) -> Result<Answer, String> {
    let mut picks = Vec::new();
    let mut ask = |d: &Decision, picks: &mut Vec<Pick>| -> Result<Option<(String, f32)>, String> {
        if d.candidates.is_empty() {
            return Ok(None);
        }
        let p = decide(d)?;
        if p.len() != d.candidates.len() || p.iter().any(|x| !x.is_finite()) {
            return Err(format!(
                "the {} step gave {} probabilities for {} candidates",
                d.step.name(),
                p.len(),
                d.candidates.len()
            ));
        }
        let (id, p) = if d.step == Step::Request {
            let yes = d
                .candidates
                .iter()
                .position(|c| c.id == "true")
                .unwrap_or(0);
            ("true".to_owned(), p[yes])
        } else {
            let i = argmax(&p).unwrap_or(0);
            (d.candidates[i].id.clone(), p[i])
        };
        picks.push(Pick {
            step: d.step,
            id: id.clone(),
            p,
        });
        Ok(Some((id, p)))
    };
    let done = |picks: Vec<Pick>, verdict| Ok(Answer { picks, verdict });

    let request = spec.request_decision();
    match ask(&request, &mut picks)? {
        Some((_, p)) if p >= spec.request.threshold => {}
        Some(_) => return done(picks, Verdict::NotARequest),
        None => return done(picks, Verdict::Nothing(Step::Request)),
    }
    let kind = match ask(&spec.kind_decision(), &mut picks)? {
        Some((id, p)) if p >= floors.kind => id,
        Some(_) => return done(picks, Verdict::Unsure(Step::Kind)),
        None => return done(picks, Verdict::Nothing(Step::Kind)),
    };
    let target = match spec.target_decision(&kind, room) {
        Some(d) => match ask(&d, &mut picks)? {
            Some((id, _)) => Some(id),
            None => return done(picks, Verdict::Nothing(Step::Target)),
        },
        None => None,
    };
    let value = match spec.value_decision(&kind, target.as_deref(), room) {
        Some(d) => match ask(&d, &mut picks)? {
            Some((id, p)) if p >= floors.value => Some(id),
            Some(_) => return done(picks, Verdict::Unsure(Step::Value)),
            None => return done(picks, Verdict::Nothing(Step::Value)),
        },
        None => None,
    };
    done(
        picks,
        Verdict::Act(AgentAction {
            kind,
            target: target.unwrap_or_default(),
            value: value.unwrap_or_default(),
        }),
    )
}

/// The provider's reply for an answer: the one action, or a miss
/// (NoMatch, the sentence as heard), which the label shows as the
/// grammar's "Didn't catch that" with its hint. No `say`: the model writes
/// none, so the label is the intent's own reply text.
pub fn reply(sentence: &str, answer: &Answer) -> Reply {
    match &answer.verdict {
        Verdict::Act(action) => Reply {
            actions: vec![action.clone()],
            ..Reply::default()
        },
        _ => Reply {
            miss: Some(Miss {
                reason: Reason::NoMatch,
                heard: sentence.trim().to_owned(),
            }),
            ..Reply::default()
        },
    }
}

/// The room's vocabulary rebuilt from its state (the agent's room has the
/// surfaces in lane box order, so a surface's index is its position): for
/// the log's intent, resolved as the frame will resolve it.
#[derive(Debug, Clone, PartialEq)]
pub struct RoomVocabulary {
    effects: Vec<String>,
    surfaces: Vec<Surface>,
    pointed: Option<usize>,
}

impl RoomVocabulary {
    pub fn of(room: &RoomState) -> Self {
        let surfaces: Vec<Surface> = room
            .surfaces
            .iter()
            .enumerate()
            .map(|(index, s)| Surface {
                name: s.name.clone(),
                kind: kind_from_name(&s.kind).unwrap_or(KIND_NONE),
                index,
            })
            .collect();
        let pointed = room
            .pointed
            .as_ref()
            .and_then(|p| surfaces.iter().find(|s| &s.name == p))
            .map(|s| s.index);
        Self {
            effects: room.effects.clone(),
            surfaces,
            pointed,
        }
    }

    pub fn vocabulary(&self) -> Vocabulary<'_> {
        Vocabulary {
            effects: &self.effects,
            behaviors: &SurfaceBehavior::ALL,
            surfaces: self.surfaces.clone(),
            pointed: self.pointed,
        }
    }
}

/// What an answer comes to, for the log: the intent (`Color { … }`), the
/// grammar's miss for an action it refuses, or why the cascade stopped.
pub fn outcome_text(spec: &Spec, floors: Floors, answer: &Answer, room: &RoomState) -> String {
    match &answer.verdict {
        Verdict::Act(action) => {
            let vocab = RoomVocabulary::of(room);
            match Intent::from_json(action, &vocab.vocabulary()) {
                Ok(intent) => format!("{intent:?}"),
                Err(miss) => format!("miss {:?}", miss.reason),
            }
        }
        Verdict::NotARequest => format!(
            "miss NoMatch (not a request, under {})",
            spec.request.threshold
        ),
        Verdict::Unsure(step) => {
            let floor = if *step == Step::Kind {
                floors.kind
            } else {
                floors.value
            };
            format!("miss NoMatch ({} under {floor:.2})", step.name())
        }
        Verdict::Nothing(step) => format!("miss NoMatch (no {} to choose from)", step.name()),
    }
}

/// The per-sentence log line: `voice local: 212 ms (request 0.99 · kind
/// color 0.97 · target table 14 0.88 · value amber 0.93) → Color { … }`.
pub fn log_line(ms: u128, picks: &[Pick], outcome: &str) -> String {
    let steps: Vec<String> = picks
        .iter()
        .map(|p| match p.step {
            Step::Request => format!("request {:.2}", p.p),
            s => format!("{} {} {:.2}", s.name(), p.id, p.p),
        })
        .collect();
    format!(
        "voice local: {ms} ms ({}) \u{2192} {outcome}",
        steps.join(" \u{b7} ")
    )
}

/// The per-step timing line that follows it: `voice local: steps request
/// 31 · kind 48 · target 92 · value 41 ms · prefix 204 tokens`.
pub fn steps_line(steps: &[(Step, f32)], prefix_tokens: usize) -> String {
    let each: Vec<String> = steps
        .iter()
        .map(|(s, ms)| format!("{} {ms:.0}", s.name()))
        .collect();
    format!(
        "voice local: steps {} ms \u{b7} prefix {prefix_tokens} tokens",
        each.join(" \u{b7} ")
    )
}

// ------------------------------------------------------------- the files

/// The provider's files in the models dir.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Files {
    pub dir: std::path::PathBuf,
}

impl Files {
    pub fn in_dir(dir: &std::path::Path) -> Self {
        Self {
            dir: dir.to_path_buf(),
        }
    }

    pub fn model(&self) -> std::path::PathBuf {
        self.dir.join(MODEL_FILE)
    }

    pub fn tokenizer(&self) -> std::path::PathBuf {
        self.dir.join(TOKENIZER_FILE)
    }

    pub fn tokenizer_config(&self) -> std::path::PathBuf {
        self.dir.join(TOKENIZER_CONFIG_FILE)
    }

    pub fn spec(&self) -> std::path::PathBuf {
        self.dir.join(SPEC_FILE)
    }

    /// `Ok` when every file is there, else which are missing (the reason
    /// the provider is off).
    pub fn check(&self) -> Result<(), String> {
        let missing: Vec<&str> = [MODEL_FILE, SPEC_FILE, TOKENIZER_FILE, TOKENIZER_CONFIG_FILE]
            .into_iter()
            .filter(|f| !self.dir.join(f).is_file())
            .collect();
        if missing.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "no {} at {}",
                missing.join(", "),
                self.dir.display()
            ))
        }
    }
}

/// How the provider runs: the session's intra-op threads, the floors, and
/// where to look for the runtime library, in order (`libonnxruntime.so` by
/// name first).
#[derive(Debug, Clone, PartialEq)]
pub struct Options {
    pub threads: usize,
    pub floors: Floors,
    pub library: Vec<std::path::PathBuf>,
}

// ------------------------------------------------- the runtime and the worker

#[cfg(any(target_os = "android", test))]
pub use self::device::{Loaded, Local, Model, Tokens, init_runtime};

#[cfg(any(target_os = "android", test))]
mod device {
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use std::sync::mpsc::{Receiver, Sender, channel};
    use std::time::Instant;

    use log::{error, info};
    use ort::session::Session;
    use ort::session::builder::GraphOptimizationLevel;
    use ort::value::Tensor;

    use super::{
        COLUMNS, Encode, Files, MODEL_NAME, Options, Reply, RoomState, Spec, SpecialIds, Step,
        cascade, log_line, outcome_text, parse_spec, reply, softmax, steps_line,
    };
    use crate::agent::{AgentError, Provider};

    /// The tokenizer, with its own truncation and padding off (the
    /// renderer cuts each part itself).
    pub struct Tokens(tokenizers::Tokenizer);

    impl Tokens {
        /// Load `tokenizer.json`, and check that `tokenizer_config.json`'s
        /// special tokens are the spec's ids.
        pub fn load(path: &Path, config: &Path, ids: &SpecialIds) -> Result<Self, String> {
            let mut t = tokenizers::Tokenizer::from_file(path)
                .map_err(|e| format!("the tokenizer {}: {e}", path.display()))?;
            t.with_truncation(None).map_err(|e| e.to_string())?;
            t.with_padding(None);
            let config: serde_json::Value = std::fs::read_to_string(config)
                .map_err(|e| e.to_string())
                .and_then(|s| serde_json::from_str(&s).map_err(|e| e.to_string()))
                .map_err(|e| format!("the tokenizer config {}: {e}", config.display()))?;
            for (key, id) in [
                ("cls_token", ids.cls),
                ("sep_token", ids.sep),
                ("pad_token", ids.pad),
            ] {
                let token = config.get(key).and_then(|v| v.as_str()).unwrap_or_default();
                if t.token_to_id(token) != Some(id) {
                    return Err(format!(
                        "the tokenizer's {key} {token:?} is not the spec's id {id}"
                    ));
                }
            }
            Ok(Self(t))
        }
    }

    impl Encode for Tokens {
        fn encode(&self, text: &str) -> Result<Vec<u32>, String> {
            self.0
                .encode(text, false)
                .map(|e| e.get_ids().to_vec())
                .map_err(|e| e.to_string())
        }
    }

    /// Load ONNX Runtime from the first of `candidates` that loads (a bare
    /// name goes through the dynamic linker's search path); which one it
    /// was. Loading it again in the same process is a no-op.
    pub fn init_runtime(candidates: &[PathBuf]) -> Result<PathBuf, String> {
        let mut errors = Vec::new();
        for path in candidates {
            match ort::init_from(path) {
                Ok(builder) => {
                    builder.with_name("fosfora").commit();
                    return Ok(path.clone());
                }
                Err(e) => errors.push(e.to_string()),
            }
        }
        Err(format!("ONNX Runtime did not load: {}", errors.join("; ")))
    }

    /// The decision model's session.
    pub struct Model {
        session: Session,
        path: PathBuf,
    }

    impl Model {
        /// A session on `path` with `threads` intra-op threads and one
        /// inter-op thread, the graph optimized fully, intra-op spinning
        /// off (the threads sleep between sentences instead of spinning
        /// next to the render loop).
        pub fn load(path: &Path, threads: usize) -> Result<Self, String> {
            let session = Session::builder()
                .and_then(|b| b.with_optimization_level(GraphOptimizationLevel::Level3))
                .and_then(|b| b.with_intra_threads(threads))
                .and_then(|b| b.with_inter_threads(1))
                .and_then(|b| b.with_intra_op_spinning(false))
                .and_then(|b| b.commit_from_file(path))
                .map_err(|e| format!("the model {}: {e}", path.display()))?;
            Ok(Self {
                session,
                path: path.to_path_buf(),
            })
        }

        /// One decision: one prefix, its candidates (padded with `pad`),
        /// and the `column` of `logits [n, 3]`.
        pub fn logits(
            &mut self,
            prefix: &[i64],
            docs: &[Vec<i64>],
            pad: i64,
            column: usize,
        ) -> Result<Vec<f32>, String> {
            let n = docs.len();
            let width = docs.iter().map(Vec::len).max().unwrap_or(0);
            let mut ids = vec![pad; n * width];
            let mut mask = vec![false; n * width];
            for (i, d) in docs.iter().enumerate() {
                ids[i * width..i * width + d.len()].copy_from_slice(d);
                mask[i * width..i * width + d.len()].fill(true);
            }
            let tensor = |e: ort::Error| e.to_string();
            let inputs = ort::inputs![
                "prefix_ids" => Tensor::from_array(([1usize, prefix.len()], prefix.to_vec())).map_err(tensor)?,
                "prefix_mask" => Tensor::from_array(([1usize, prefix.len()], vec![true; prefix.len()])).map_err(tensor)?,
                "doc_ids" => Tensor::from_array(([n, width], ids)).map_err(tensor)?,
                "doc_mask" => Tensor::from_array(([n, width], mask)).map_err(tensor)?,
                "owners" => Tensor::from_array(([n], vec![0i64; n])).map_err(tensor)?,
            ];
            let out = self.session.run(inputs).map_err(|e| e.to_string())?;
            let (_, logits) = out["logits"]
                .try_extract_tensor::<f32>()
                .map_err(|e| e.to_string())?;
            if logits.len() != n * COLUMNS {
                return Err(format!("logits of {} for {n} candidates", logits.len()));
            }
            Ok((0..n).map(|i| logits[i * COLUMNS + column]).collect())
        }
    }

    /// The session, parked for the process when the app goes away and
    /// taken back by the next launch in the same process: ONNX Runtime
    /// cannot create a second environment after releasing the first, so
    /// the session (which holds the environment) is never released.
    static PARKED: Mutex<Option<Model>> = Mutex::new(None);

    /// Everything a sentence needs, loaded once.
    struct Engine {
        spec: Spec,
        tokens: Tokens,
        model: Model,
        options: Options,
    }

    impl Engine {
        fn load(files: &Files, options: &Options) -> Result<(Self, Loaded), String> {
            let started = Instant::now();
            let json = std::fs::read_to_string(files.spec())
                .map_err(|e| format!("the spec {}: {e}", files.spec().display()))?;
            let spec = parse_spec(&json)?;
            let tokens = Tokens::load(
                &files.tokenizer(),
                &files.tokenizer_config(),
                &spec.limits.ids,
            )?;
            let library = init_runtime(&options.library)?;
            let parked = PARKED
                .lock()
                .map_or(None, |mut p| p.take())
                .filter(|m| m.path == files.model());
            let reused = parked.is_some();
            let model = match parked {
                Some(m) => m,
                None => Model::load(&files.model(), options.threads)?,
            };
            let loaded = Loaded {
                ms: started.elapsed().as_millis(),
                library: library.display().to_string(),
                reused,
            };
            Ok((
                Self {
                    spec,
                    tokens,
                    model,
                    options: options.clone(),
                },
                loaded,
            ))
        }

        /// One sentence through the cascade, logged; the reply.
        fn answer(&mut self, sentence: &str, room: &RoomState) -> Result<Reply, AgentError> {
            let started = Instant::now();
            let mut steps: Vec<(Step, f32)> = Vec::new();
            let mut prefix_tokens = 0;
            let (spec, tokens, model) = (&self.spec, &self.tokens, &mut self.model);
            let pad = i64::from(spec.limits.ids.pad);
            let answer = cascade(spec, room, self.options.floors, |d| {
                let t = Instant::now();
                let (prefix, docs) = spec.render(tokens, d, sentence, room)?;
                prefix_tokens = prefix_tokens.max(prefix.len());
                let p = softmax(&model.logits(&prefix, &docs, pad, d.column)?);
                steps.push((d.step, t.elapsed().as_secs_f32() * 1e3));
                Ok(p)
            });
            let ms = started.elapsed().as_millis();
            match answer {
                Ok(answer) => {
                    let outcome = outcome_text(spec, self.options.floors, &answer, room);
                    info!("{}", log_line(ms, &answer.picks, &outcome));
                    info!("{}", steps_line(&steps, prefix_tokens));
                    Ok(reply(sentence, &answer))
                }
                Err(e) => {
                    error!("voice local: {ms} ms \u{2192} failed: {e}");
                    Err(AgentError::Parse(format!("local: {e}")))
                }
            }
        }
    }

    /// The provider loaded: in how long, from which runtime library, and
    /// whether the session was a parked one.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Loaded {
        pub ms: u128,
        pub library: String,
        pub reused: bool,
    }

    /// A sentence for the worker, and where its reply goes.
    struct Job {
        sentence: String,
        room: RoomState,
        reply: Sender<Result<Reply, AgentError>>,
    }

    /// The on-device provider: a handle on the `fosfora-local` worker,
    /// which loads the spec, the tokenizer and the session when told to
    /// ([`Local::load`]) and then answers one sentence at a time, in order.
    /// A sentence asked before the load has finished waits for it.
    pub struct Local {
        jobs: Sender<Job>,
        go: Mutex<Option<Sender<()>>>,
    }

    impl Local {
        /// Start the worker, waiting for [`Local::load`]; the receiver
        /// gets the load's outcome once.
        pub fn start(
            files: Files,
            options: Options,
        ) -> std::io::Result<(Self, Receiver<Result<Loaded, String>>)> {
            let (jobs, job_rx) = channel::<Job>();
            let (go, go_rx) = channel::<()>();
            let (loaded_tx, loaded) = channel();
            std::thread::Builder::new()
                .name("fosfora-local".to_owned())
                .spawn(move || {
                    // No go: the app went away before the load.
                    if go_rx.recv().is_err() {
                        return;
                    }
                    let mut engine = match Engine::load(&files, &options) {
                        Ok((engine, l)) => {
                            let _ = loaded_tx.send(Ok(l));
                            engine
                        }
                        Err(e) => {
                            let _ = loaded_tx.send(Err(e));
                            return;
                        }
                    };
                    while let Ok(job) = job_rx.recv() {
                        let outcome = engine.answer(&job.sentence, &job.room);
                        // The frame loop may have moved on (a newer
                        // sentence): then the answer is dropped.
                        let _ = job.reply.send(outcome);
                    }
                    if let Ok(mut parked) = PARKED.lock() {
                        *parked = Some(engine.model);
                    }
                })?;
            Ok((
                Self {
                    jobs,
                    go: Mutex::new(Some(go)),
                },
                loaded,
            ))
        }

        /// Load now (once; later calls do nothing).
        pub fn load(&self) {
            if let Some(go) = self.go.lock().ok().and_then(|mut g| g.take()) {
                let _ = go.send(());
            }
        }
    }

    impl Provider for Local {
        fn name(&self) -> &'static str {
            super::PROVIDER
        }

        fn model(&self) -> &str {
            MODEL_NAME
        }

        fn answer(&self, sentence: &str, room: &RoomState) -> Receiver<Result<Reply, AgentError>> {
            let (reply, rx) = channel();
            let job = Job {
                sentence: sentence.trim().to_owned(),
                room: room.clone(),
                reply,
            };
            if let Err(e) = self.jobs.send(job) {
                let _ = e.0.reply.send(Err(AgentError::Parse(
                    "local: the worker thread is gone".to_owned(),
                )));
            }
            rx
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{Choice, choose};
    use crate::intent::{Strength, Target};
    use crate::surfaces::{KIND_FLOOR, KIND_TABLE, KIND_WALL};
    use serde_json::json;

    /// The committed spec, the training pipeline's `provider-spec.json`
    /// for run 3 (sha256 in `assets/xr/models/LICENSE.md`).
    const SPEC: &str = include_str!("../../../assets/xr/models/s1-17m-spec.json");
    /// The parity fixtures: the training toolkit's own token ids for its
    /// tokenization cases (`tokenization-parity.json`, `parity.json` with
    /// the FP32 export's logits), and every decision the cascade
    /// evaluation ran for nine of the forty test sentences on the replayed
    /// room (`tokens-ours-17m-run3-set1.json`: token ids, and the int8
    /// model's probabilities through ONNX Runtime). Derived from the run 3
    /// export; long repeated strings are `{repeat, times}`.
    const PARITY: &str = include_str!("../tests/data/s1-17m-parity.json");

    fn spec() -> Spec {
        parse_spec(SPEC).expect("the committed spec parses")
    }

    fn parity() -> Value {
        serde_json::from_str(PARITY).unwrap()
    }

    /// A fixture string: text, or `{repeat, times}`.
    fn text(v: &Value) -> String {
        match v {
            Value::String(s) => s.clone(),
            v => v["repeat"]
                .as_str()
                .unwrap()
                .repeat(v["times"].as_u64().unwrap() as usize),
        }
    }

    fn ids(v: &Value) -> Vec<i64> {
        v.as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_i64().unwrap())
            .collect()
    }

    fn rows(v: &Value) -> Vec<Vec<i64>> {
        v.as_array().unwrap().iter().map(ids).collect()
    }

    fn floats(v: &Value) -> Vec<f32> {
        v.as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_f64().unwrap() as f32)
            .collect()
    }

    /// The replayed room as the generator has it (`gen/world.py`
    /// `replayed_room`): storage called shelves, table 3 pointed, every
    /// surface on its kind's default, Flux Cloud showing.
    fn room() -> RoomState {
        let p = parity();
        let r = &p["room"];
        RoomState {
            effect: r["effect"].as_str().map(str::to_owned),
            effects: r["effects"]
                .as_array()
                .unwrap()
                .iter()
                .map(|e| e.as_str().unwrap().to_owned())
                .collect(),
            surfaces: r["surfaces"]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| SurfaceState {
                    name: s["name"].as_str().unwrap().to_owned(),
                    kind: s["kind"].as_str().unwrap().to_owned(),
                    size_m: [1.0, 0.5],
                    behavior: s["behavior"].as_str().unwrap().to_owned(),
                    color: "own".to_owned(),
                    band: "rms".to_owned(),
                    strength: 1.0,
                })
                .collect(),
            pointed: r["pointed"].as_str().map(str::to_owned),
            behaviors: BTreeMap::new(),
            colors: Vec::new(),
            bands: Vec::new(),
        }
    }

    /// The real tokenizer, when installed (`assets/xr/models/`, fetched
    /// by `scripts/xr/fetch-model.sh` or copied from the export; the
    /// directory `FOSFORA_S1_DIR` names otherwise). The desktop CI has
    /// none: the parity tests then say so and pass on the stand-in tests.
    fn tokenizer(spec: &Spec) -> Option<Tokens> {
        let files = Files::in_dir(&models_dir());
        if !files.tokenizer().is_file() {
            eprintln!(
                "skipped: no {} at {} (the token-id parity needs the real tokenizer)",
                TOKENIZER_FILE,
                files.dir.display()
            );
            return None;
        }
        Some(
            Tokens::load(
                &files.tokenizer(),
                &files.tokenizer_config(),
                &spec.limits.ids,
            )
            .unwrap(),
        )
    }

    fn models_dir() -> std::path::PathBuf {
        std::env::var_os("FOSFORA_S1_DIR").map_or_else(
            || std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/xr/models"),
            std::path::PathBuf::from,
        )
    }

    /// A stand-in tokenizer: one id per character (its code point), so the
    /// assembly's arithmetic is checked without the real one.
    struct Chars;

    impl Encode for Chars {
        fn encode(&self, text: &str) -> Result<Vec<u32>, String> {
            Ok(text.chars().map(u32::from).collect())
        }
    }

    fn decode(ids: &[i64]) -> String {
        ids.iter()
            .map(|&i| char::from_u32(i as u32).unwrap_or('?'))
            .collect()
    }

    #[test]
    fn the_spec_reads_as_data() {
        let s = spec();
        assert_eq!(
            s.limits,
            Limits {
                query: 512,
                document: 64,
                ids: SpecialIds {
                    cls: 50281,
                    sep: 50282,
                    pad: 50283
                }
            }
        );
        assert_eq!(s.layout, Layout::InstructionState);
        assert_eq!(s.request.order, ["false", "true"]);
        assert!((s.request.threshold - 0.5).abs() < 1e-6);
        assert!(s.request.yes.starts_with("a request:"));
        assert!(s.request.no.starts_with("not a request:"));
        let kinds: Vec<&str> = s.kind.candidates.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(
            kinds,
            [
                "next_effect",
                "previous_effect",
                "effect",
                "edit_room",
                "particles",
                "pitcher",
                "music",
                "rescan",
                "recenter",
                "all_none",
                "behavior",
                "color",
                "band",
                "strength",
                "describe"
            ]
        );
        assert_eq!(
            s.target.only_for,
            ["band", "behavior", "color", "describe", "strength"]
        );
        assert_eq!(s.target.kind_text["frame"], "window or door");
        assert_eq!(s.target.kind_text["other"], "object");
        assert_eq!(s.target.group_text["other"], "every other object");
        assert_eq!(s.target.pointed_text, "the surface pointed at");
        assert_eq!(s.value.effect_text, "world effect");
        assert_eq!(s.value.lists["color"].len(), 8);
        assert_eq!(
            s.value.lists["band"][0],
            Candidate::new("level", "overall loudness")
        );
        assert_eq!(s.value.lists["strength"].len(), 5);
        for toggle in ["edit_room", "particles", "pitcher", "music"] {
            let ids: Vec<&str> = s.value.lists[toggle]
                .iter()
                .map(|c| c.id.as_str())
                .collect();
            assert_eq!(ids, ["on", "off"], "{toggle}");
        }
        assert_eq!(
            s.value.none_for,
            [
                "next_effect",
                "previous_effect",
                "rescan",
                "recenter",
                "all_none",
                "describe"
            ]
        );
        // The behaviors per kind are the app's own catalog, in its order.
        for (kind, name) in KIND_NAMES {
            let app: Vec<&str> = SurfaceBehavior::catalogue(kind)
                .iter()
                .map(|b| b.name())
                .collect();
            assert_eq!(s.value.behaviors[name], app, "{name}");
        }
        assert_eq!(s.id_mapping["pointed"], "Target::Pointed");
    }

    #[test]
    fn a_spec_the_code_cannot_render_for_is_refused() {
        let mut v: Value = serde_json::from_str(SPEC).unwrap();
        v["model"]["output"] = json!("logits float [n, 2]; columns choice, noul");
        assert!(parse_spec(&v.to_string()).unwrap_err().contains("columns"));
        let mut v: Value = serde_json::from_str(SPEC).unwrap();
        v["rendering"]["layout"] = json!("sideways");
        assert!(parse_spec(&v.to_string()).unwrap_err().contains("layout"));
        let mut v: Value = serde_json::from_str(SPEC).unwrap();
        v["cascade"].as_array_mut().unwrap().remove(2);
        assert_eq!(
            parse_spec(&v.to_string()).unwrap_err(),
            "the spec has no target step"
        );
        let mut v: Value = serde_json::from_str(SPEC).unwrap();
        v["cascade"][0]["rule"] = json!("stop when unsure");
        assert!(
            parse_spec(&v.to_string())
                .unwrap_err()
                .contains("threshold")
        );
        let mut v: Value = serde_json::from_str(SPEC).unwrap();
        v["cascade"][3]["candidates"]["behavior"]["by_kind"]["table"][1] = json!("fire");
        assert!(parse_spec(&v.to_string()).unwrap_err().contains("fire"));
        assert!(parse_spec("{").is_err());
    }

    #[test]
    fn a_reworded_spec_changes_the_prompts_without_code() {
        let mut v: Value = serde_json::from_str(SPEC).unwrap();
        v["cascade"][1]["instruction"] = json!("What does the wearer want?");
        v["cascade"][3]["candidates"]["color"][4]["text"] = json!("amber, a campfire glow");
        v["cascade"][0]["rule"] = json!("stop unless p(true) >= 0.6");
        let s = parse_spec(&v.to_string()).unwrap();
        assert_eq!(s.kind_decision().instruction, "What does the wearer want?");
        let d = s
            .value_decision("color", Some("table 14"), &room())
            .unwrap();
        assert_eq!(
            d.candidates[4],
            Candidate::new("amber", "amber, a campfire glow")
        );
        assert!((s.request.threshold - 0.6).abs() < 1e-6);
    }

    #[test]
    fn the_budget_is_the_toolkits() {
        // Half each, the odd token to the instruction, the rest shared.
        assert_eq!(allocate(1000, 1000, 497), (249, 248));
        assert_eq!(allocate(10, 1000, 497), (10, 487));
        assert_eq!(allocate(1000, 10, 497), (487, 10));
        assert_eq!(allocate(10, 20, 497), (10, 20));
        assert_eq!(allocate(0, 0, 2), (0, 0));
    }

    #[test]
    fn the_prompt_assembly_with_a_stand_in_tokenizer() {
        let limits = Limits {
            query: 40,
            document: 8,
            ids: SpecialIds {
                cls: 1,
                sep: 2,
                pad: 3,
            },
        };
        let p = prefix_ids(&Chars, &limits, Layout::InstructionState, "", "Q?", "{}").unwrap();
        assert_eq!(p[0], 1);
        assert_eq!(*p.last().unwrap(), 2);
        assert_eq!(decode(&p[1..p.len() - 1]), "Instruction: Q?\nState: {}");
        let p = prefix_ids(&Chars, &limits, Layout::StateInstruction, "Be.", "Q?", "{}").unwrap();
        assert_eq!(
            decode(&p[1..p.len() - 1]),
            "Be.\n\nState: {}\nInstruction: Q?"
        );
        // A whitespace-only system text adds nothing.
        let p = prefix_ids(&Chars, &limits, Layout::InstructionState, "  ", "Q?", "{}").unwrap();
        assert_eq!(decode(&p[1..p.len() - 1]), "Instruction: Q?\nState: {}");
        // Over the budget: 40 - 2 - 13 - 7 - 1 = 17 tokens shared, 9 and 8.
        let p = prefix_ids(
            &Chars,
            &limits,
            Layout::InstructionState,
            "",
            "abcdefghijklmnop",
            "ABCDEFGHIJKLMNOP",
        )
        .unwrap();
        assert_eq!(p.len(), 40);
        assert_eq!(
            decode(&p[1..p.len() - 1]),
            "Instruction: abcdefghi\nState: ABCDEFGH"
        );
        // Markers that leave no room are refused.
        let tight = Limits {
            query: 23,
            ..limits
        };
        assert!(prefix_ids(&Chars, &tight, Layout::InstructionState, "", "a", "b").is_err());
        // A candidate: cut to document - 1, then [SEP].
        let d = document_ids(&Chars, &limits, "abcdefghij").unwrap();
        assert_eq!(decode(&d[..7]), "abcdefg");
        assert_eq!(d[7], 2);
        assert_eq!(document_ids(&Chars, &limits, "").unwrap(), [2]);
        assert_eq!(
            candidate_text(&Candidate::new("amber", "amber, orange")),
            "Candidate: amber: amber, orange"
        );
    }

    #[test]
    fn the_state_json_is_the_generators() {
        let p = parity();
        let s = spec();
        let r = room();
        assert_eq!(
            state_json("Table 14 in amber.", &r),
            p["room"]["state_example"].as_str().unwrap()
        );
        assert_eq!(
            noul_state(&s.request.yes, &s.request.no, "Table 14 in amber.", &r),
            p["room"]["noul_example"].as_str().unwrap()
        );
        // The spec's own example parses to the same object.
        let example: Value = serde_json::from_str(SPEC).unwrap();
        let ours: Value = serde_json::from_str(&state_json("Table 14 in amber.", &r)).unwrap();
        assert_eq!(ours, example["rendering"]["state_example"]);
        // A color not the kind's own rides along; escapes are JSON's.
        let mut r = room();
        r.surfaces[0].color = "amber".to_owned();
        r.pointed = None;
        let s = state_json("Say \"hi\"\n", &r);
        assert!(
            s.starts_with(r#"{"sentence": "Say \"hi\"", "pointed": "nothing", "#),
            "{s}"
        );
        assert!(s.contains(r#""table 0": "streamlines, amber""#), "{s}");
    }

    #[test]
    fn the_rendering_matches_the_toolkits_token_ids() {
        let s = spec();
        let Some(tok) = tokenizer(&s) else { return };
        let p = parity();
        for case in p["toolkit"].as_array().unwrap() {
            let layout = Layout::parse(case["layout"].as_str().unwrap()).unwrap();
            let prefix = prefix_ids(
                &tok,
                &s.limits,
                layout,
                case["system"].as_str().unwrap(),
                &text(&case["instruction"]),
                &text(&case["state"]),
            )
            .unwrap();
            assert_eq!(prefix, ids(&case["prefix_ids"]), "prefix of {case:.80}");
            let docs: Vec<Vec<i64>> = case["candidates"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| document_ids(&tok, &s.limits, &text(&c["text"])).unwrap())
                .collect();
            assert_eq!(docs, rows(&case["doc_ids"]), "candidates of {case:.80}");
        }
    }

    /// The decision the fixture's row names, rebuilt from the spec.
    fn decision_for(s: &Spec, row: &Value, r: &RoomState) -> Decision {
        let kind = row["kind"].as_str();
        match row["step"].as_str().unwrap() {
            "request" => s.request_decision(),
            "kind" => s.kind_decision(),
            "target" => s.target_decision(kind.unwrap(), r).unwrap(),
            "value" => s
                .value_decision(kind.unwrap(), row["target"].as_str(), r)
                .unwrap(),
            other => panic!("{other}"),
        }
    }

    #[test]
    fn the_room_decisions_are_the_trainings() {
        let s = spec();
        let r = room();
        let p = parity();
        let tok = tokenizer(&s);
        for sentence in p["room"]["sentences"].as_array().unwrap() {
            let said = sentence["sentence"].as_str().unwrap();
            for row in sentence["decisions"].as_array().unwrap() {
                let d = decision_for(&s, row, &r);
                let want: Vec<&str> = row["candidate_ids"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|c| c.as_str().unwrap())
                    .collect();
                let got: Vec<&str> = d.candidates.iter().map(|c| c.id.as_str()).collect();
                assert_eq!(got, want, "{said} {:?}", d.step);
                assert_eq!(d.column as u64, row["column"].as_u64().unwrap());
                if let Some(tok) = &tok {
                    let (prefix, docs) = s.render(tok, &d, said, &r).unwrap();
                    assert_eq!(
                        prefix,
                        ids(&row["prefix_ids"]),
                        "{said} {:?} prefix",
                        d.step
                    );
                    assert_eq!(
                        docs,
                        rows(&row["doc_ids"]),
                        "{said} {:?} candidates",
                        d.step
                    );
                }
            }
        }
    }

    #[test]
    fn the_target_candidates_follow_the_room() {
        let s = spec();
        let mut r = room();
        let d = s.target_decision("color", &r).unwrap();
        let tail: Vec<&Candidate> = d.candidates.iter().rev().take(4).collect();
        assert_eq!(
            tail[0],
            &Candidate::new("pointed", "the surface pointed at")
        );
        assert_eq!(tail[1], &Candidate::new("all others", "every other object"));
        assert_eq!(tail[3], &Candidate::new("all tables", "every table"));
        assert_eq!(d.candidates[8], Candidate::new("window", "window or door"));
        // Nothing pointed: no "pointed"; a kind with one surface: no group.
        r.pointed = None;
        let d = s.target_decision("color", &r).unwrap();
        assert!(
            d.candidates
                .iter()
                .all(|c| c.id != "pointed" && c.id != "all floors")
        );
        // Kinds without a target, and a value list by the target's kind.
        assert!(s.target_decision("next_effect", &r).is_none());
        assert!(s.value_decision("describe", Some("floor"), &r).is_none());
        let v = s.value_decision("behavior", Some("all walls"), &r).unwrap();
        assert_eq!(v.candidates[1], Candidate::new("spectrum", "spectrum bars"));
        let v = s.value_decision("behavior", Some("window"), &r).unwrap();
        assert_eq!(v.candidates.len(), 7);
        let v = s.value_decision("effect", None, &r).unwrap();
        assert_eq!(v.candidates[2], Candidate::new("Flock", "world effect"));
    }

    #[test]
    fn softmax_and_the_noul_read() {
        let p = softmax(&[1.0, 2.0, 3.0]);
        assert!((p.iter().sum::<f32>() - 1.0).abs() < 1e-6);
        assert!((p[2] - 0.665_240_9).abs() < 1e-6);
        // Large logits stay finite.
        let p = softmax(&[1000.0, 1000.0]);
        assert_eq!(p, [0.5, 0.5]);
        assert_eq!(argmax(&[0.2, 0.4, 0.4]), Some(1));
        assert_eq!(argmax(&[]), None);
        // The request step reads p(true) whatever the order.
        let s = spec();
        let r = room();
        let a = cascade(&s, &r, Floors::default(), |d| {
            assert_eq!(d.step, Step::Request);
            assert_eq!(d.column, NOUL);
            Ok(softmax(&[2.0, -1.0]))
        })
        .unwrap();
        assert_eq!(a.verdict, Verdict::NotARequest);
        assert!((a.picks[0].p - softmax(&[2.0, -1.0])[1]).abs() < 1e-6);
    }

    /// A canned cascade: each step's probabilities by its step, the argmax
    /// on the candidate `pick` names.
    fn canned(
        s: &Spec,
        r: &RoomState,
        floors: Floors,
        steps: &[(Step, &str, f32)],
    ) -> (Answer, Vec<Step>) {
        let mut asked = Vec::new();
        let a = cascade(s, r, floors, |d| {
            asked.push(d.step);
            let (_, pick, p) = steps
                .iter()
                .find(|(step, ..)| *step == d.step)
                .unwrap_or_else(|| panic!("{:?} not canned", d.step));
            let n = d.candidates.len();
            let rest = (1.0 - p) / (n - 1) as f32;
            Ok(d.candidates
                .iter()
                .map(|c| if c.id == *pick { *p } else { rest })
                .collect())
        })
        .unwrap();
        (a, asked)
    }

    #[test]
    fn the_cascade_stops_at_the_gate_and_the_floors() {
        let s = spec();
        let r = room();
        let f = Floors::default();
        let (a, asked) = canned(&s, &r, f, &[(Step::Request, "true", 0.3)]);
        assert_eq!(
            (a.verdict, asked),
            (Verdict::NotARequest, vec![Step::Request])
        );
        let (a, asked) = canned(
            &s,
            &r,
            f,
            &[(Step::Request, "true", 0.9), (Step::Kind, "color", 0.3)],
        );
        assert_eq!(a.verdict, Verdict::Unsure(Step::Kind));
        assert_eq!(asked, [Step::Request, Step::Kind]);
        let (a, asked) = canned(
            &s,
            &r,
            f,
            &[
                (Step::Request, "true", 0.9),
                (Step::Kind, "color", 0.8),
                (Step::Target, "table 14", 0.2),
                (Step::Value, "amber", 0.34),
            ],
        );
        assert_eq!(a.verdict, Verdict::Unsure(Step::Value));
        assert_eq!(asked.len(), 4);
        // A miss is the grammar's NoMatch with the sentence: nothing applies.
        let reply = reply(" Make it amber-ish. ", &a);
        assert!(reply.actions.is_empty());
        assert_eq!(
            reply.miss,
            Some(Miss {
                reason: Reason::NoMatch,
                heard: "Make it amber-ish.".to_owned()
            })
        );
        assert_eq!(
            outcome_text(&s, f, &a, &r),
            "miss NoMatch (value under 0.35)"
        );
        // The target has no floor; a lower floor lets the value through.
        let (a, _) = canned(
            &s,
            &r,
            Floors::both(0.3),
            &[
                (Step::Request, "true", 0.9),
                (Step::Kind, "color", 0.8),
                (Step::Target, "table 14", 0.2),
                (Step::Value, "amber", 0.34),
            ],
        );
        assert!(matches!(a.verdict, Verdict::Act(_)));
    }

    #[test]
    fn a_full_chain_comes_to_an_intent() {
        let s = spec();
        let r = room();
        let (a, asked) = canned(
            &s,
            &r,
            Floors::default(),
            &[
                (Step::Request, "true", 0.99),
                (Step::Kind, "color", 0.97),
                (Step::Target, "table 14", 0.88),
                (Step::Value, "amber", 0.93),
            ],
        );
        assert_eq!(
            asked,
            [Step::Request, Step::Kind, Step::Target, Step::Value]
        );
        let action = AgentAction {
            kind: "color".into(),
            target: "table 14".into(),
            value: "amber".into(),
        };
        assert_eq!(a.verdict, Verdict::Act(action.clone()));
        let reply = reply("Table fourteen like a sunset.", &a);
        assert_eq!(
            (reply.actions, reply.miss, reply.say),
            (vec![action], None, String::new())
        );
        let outcome = outcome_text(&s, Floors::default(), &a, &r);
        assert_eq!(outcome, "Color { target: Surface(14), color: 4 }");
        assert_eq!(
            log_line(212, &a.picks, &outcome),
            "voice local: 212 ms (request 0.99 \u{b7} kind color 0.97 \u{b7} target table 14 0.88 \u{b7} value amber 0.93) \u{2192} Color { target: Surface(14), color: 4 }"
        );
        assert_eq!(
            steps_line(&[(Step::Request, 31.2), (Step::Kind, 48.0)], 204),
            "voice local: steps request 31 \u{b7} kind 48 ms \u{b7} prefix 204 tokens"
        );
        // A kind that takes neither a target nor a value stops there.
        let (a, asked) = canned(
            &s,
            &r,
            Floors::default(),
            &[
                (Step::Request, "true", 0.99),
                (Step::Kind, "next_effect", 0.9),
            ],
        );
        assert_eq!(asked, [Step::Request, Step::Kind]);
        assert_eq!(outcome_text(&s, Floors::default(), &a, &r), "NextEffect");
        // A room with no effects has no effect to choose.
        let mut bare = room();
        bare.effects.clear();
        let (a, _) = canned(
            &s,
            &bare,
            Floors::default(),
            &[(Step::Request, "true", 0.99), (Step::Kind, "effect", 0.9)],
        );
        assert_eq!(a.verdict, Verdict::Nothing(Step::Value));
    }

    /// The spec's ids through the grammar's resolution, as the frame
    /// applies them.
    fn intent(kind: &str, target: &str, value: &str) -> Result<Intent, Miss> {
        let r = room();
        let v = RoomVocabulary::of(&r);
        Intent::from_json(
            &AgentAction {
                kind: kind.into(),
                target: target.into(),
                value: value.into(),
            },
            &v.vocabulary(),
        )
    }

    #[test]
    fn the_spec_ids_map_onto_intents() {
        use crate::surface_fx::{BAND_BASS, BAND_HIGH, BAND_MID, BAND_RMS};
        let pointed = Target::Pointed;
        let cases = [
            (("next_effect", "", ""), Intent::NextEffect),
            (("previous_effect", "", ""), Intent::PrevEffect),
            (("effect", "", "Embers"), Intent::Effect(1)),
            (("edit_room", "", "on"), Intent::EditRoom(true)),
            (("particles", "", "off"), Intent::Cloud(false)),
            (("pitcher", "", "on"), Intent::Pitcher(true)),
            (("music", "", "off"), Intent::Music(false)),
            (("rescan", "", ""), Intent::Rescan),
            (("recenter", "", ""), Intent::Recenter),
            (("all_none", "", ""), Intent::AllNone),
            (
                ("behavior", "all tables", "streamlines"),
                Intent::Behavior {
                    target: Target::Kind(KIND_TABLE),
                    behavior: SurfaceBehavior::Streamlines,
                },
            ),
            (
                ("behavior", "pointed", "embers"),
                Intent::Behavior {
                    target: pointed,
                    behavior: SurfaceBehavior::Embers,
                },
            ),
            (
                ("behavior", "ceiling", "none"),
                Intent::Behavior {
                    target: Target::Surface(10),
                    behavior: SurfaceBehavior::None,
                },
            ),
            (
                ("color", "pointed", "own color"),
                Intent::Color {
                    target: pointed,
                    color: 0,
                },
            ),
            (
                ("color", "all walls", "warm white"),
                Intent::Color {
                    target: Target::Kind(KIND_WALL),
                    color: 3,
                },
            ),
            (
                ("band", "table 14", "level"),
                Intent::Band {
                    target: Target::Surface(14),
                    band: BAND_RMS,
                },
            ),
            (
                ("band", "floor", "bass"),
                Intent::Band {
                    target: Target::Surface(7),
                    band: BAND_BASS,
                },
            ),
            (
                ("strength", "wall 5", "brighter"),
                Intent::Strength {
                    target: Target::Surface(5),
                    strength: Strength::Up,
                },
            ),
            (
                ("strength", "wall 5", "dimmer"),
                Intent::Strength {
                    target: Target::Surface(5),
                    strength: Strength::Down,
                },
            ),
            (
                ("strength", "shelf 4", "off"),
                Intent::Strength {
                    target: Target::Surface(4),
                    strength: Strength::Off,
                },
            ),
            (
                ("describe", "window", ""),
                Intent::Describe(Target::Surface(8)),
            ),
            (("describe", "pointed", ""), Intent::Describe(pointed)),
        ];
        for ((k, t, v), want) in cases {
            assert_eq!(intent(k, t, v), Ok(want), "{k} {t} {v}");
        }
        // Every candidate id of every list maps: each kind, each group the
        // room has, every color, band, strength and toggle value, every
        // behavior on a surface of its kind.
        let s = spec();
        let r = room();
        for k in &s.kind.candidates {
            let target = if s.target.only_for.contains(&k.id) {
                "pointed"
            } else {
                ""
            };
            let value = s
                .value_decision(&k.id, Some("pointed"), &r)
                .map(|d| d.candidates[0].id.clone())
                .unwrap_or_default();
            assert!(intent(&k.id, target, &value).is_ok(), "{} {value}", k.id);
        }
        for t in s.target_decision("describe", &r).unwrap().candidates {
            assert!(intent("describe", &t.id, "").is_ok(), "{}", t.id);
        }
        assert_eq!(
            intent("describe", "all floors", ""),
            Ok(Intent::Describe(Target::Kind(KIND_FLOOR)))
        );
        for kind in ["color", "band", "strength", "music", "edit_room"] {
            for c in &s
                .value_decision(kind, Some("table 0"), &r)
                .unwrap()
                .candidates
            {
                assert!(intent(kind, "table 0", &c.id).is_ok(), "{kind} {}", c.id);
            }
        }
        let bands: Vec<u32> = ["level", "bass", "mid", "high"]
            .iter()
            .map(|b| match intent("band", "wall 2", b) {
                Ok(Intent::Band { band, .. }) => band,
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(bands, [BAND_RMS, BAND_BASS, BAND_MID, BAND_HIGH]);
        for surface in &r.surfaces {
            for b in &s
                .value_decision("behavior", Some(&surface.name), &r)
                .unwrap()
                .candidates
            {
                assert!(
                    intent("behavior", &surface.name, &b.id).is_ok(),
                    "{} {}",
                    surface.name,
                    b.id
                );
            }
        }
    }

    #[test]
    fn the_provider_rule() {
        let none = || Err("no s1-17m-int8.onnx at /m".to_owned());
        // No file: local when installed, else off with both reasons.
        assert_eq!(choose(None, Ok(())), Ok(Choice::Local));
        assert_eq!(
            choose(None, none()),
            Err("no voice.json, and no s1-17m-int8.onnx at /m".to_owned())
        );
        // A file naming no provider: local when installed, else V3's
        // Anthropic default.
        let keyed = r#"{"api_key": "sk-test"}"#;
        assert_eq!(choose(Some(keyed), Ok(())), Ok(Choice::Local));
        assert_eq!(choose(Some("{}"), Ok(())), Ok(Choice::Local));
        match choose(Some(keyed), none()) {
            Ok(Choice::Network(c)) => assert_eq!(c.provider, crate::agent::ProviderKind::Anthropic),
            other => panic!("{other:?}"),
        }
        assert_eq!(
            choose(Some("{}"), none()),
            Err("no api_key in voice.json".to_owned())
        );
        // Named: local by name, or a network provider as V3 has it.
        assert_eq!(
            choose(Some(r#"{"provider": "local"}"#), Ok(())),
            Ok(Choice::Local)
        );
        assert_eq!(
            choose(Some(r#"{"provider": "local"}"#), none()),
            Err("provider local in voice.json, but no s1-17m-int8.onnx at /m".to_owned())
        );
        let openai = r#"{"provider": "openai", "model": "qwen3.5", "base_url": "http://192.168.1.20:11434/v1"}"#;
        match choose(Some(openai), Ok(())) {
            Ok(Choice::Network(c)) => assert_eq!(c.model, "qwen3.5"),
            other => panic!("{other:?}"),
        }
        match choose(
            Some(r#"{"provider": "anthropic", "api_key": "sk-test"}"#),
            Ok(()),
        ) {
            Ok(Choice::Network(c)) => assert_eq!(c.provider, crate::agent::ProviderKind::Anthropic),
            other => panic!("{other:?}"),
        }
        // A broken file is V3's error, installed or not.
        assert!(
            choose(Some("{"), Ok(()))
                .unwrap_err()
                .starts_with("voice.json is not valid")
        );
        assert!(choose(Some(r#"{"provider": "elsewhere"}"#), Ok(())).is_err());
    }

    #[test]
    fn the_files_say_which_are_missing() {
        let dir = std::env::temp_dir().join(format!("fosfora-local-files-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = Files::in_dir(&dir);
        let e = f.check().unwrap_err();
        assert!(e.starts_with("no s1-17m-int8.onnx, s1-17m-spec.json, s1-17m-tokenizer.json, s1-17m-tokenizer_config.json at "), "{e}");
        for name in [MODEL_FILE, SPEC_FILE, TOKENIZER_FILE] {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        assert!(
            f.check()
                .unwrap_err()
                .starts_with("no s1-17m-tokenizer_config.json at ")
        );
        std::fs::write(dir.join(TOKENIZER_CONFIG_FILE), b"x").unwrap();
        assert_eq!(f.check(), Ok(()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The host's ONNX Runtime (`ORT_DYLIB_PATH`, 1.23 or newer, e.g. the
    /// `onnxruntime` Python wheel's `capi/libonnxruntime.so.1.*`) and the
    /// model in `assets/xr/models/` (or `FOSFORA_S1_DIR`).
    fn host_model() -> (Files, std::path::PathBuf) {
        let lib = std::env::var_os("ORT_DYLIB_PATH")
            .map(std::path::PathBuf::from)
            .expect("set ORT_DYLIB_PATH to a host libonnxruntime.so (1.23 or newer)");
        let files = Files::in_dir(&models_dir());
        files.check().expect("the model's files");
        (files, lib)
    }

    /// The real model on the host, against the parity fixtures: the
    /// toolkit's cases (the FP32 export's logits, so the int8 model's
    /// probabilities within 0.05 and the same argmax) and every room
    /// decision (the int8 model's probabilities through ONNX Runtime,
    /// within 1e-3), first on the fixture's token ids, then rendered from
    /// the sentence; then the provider itself, end to end. Run with
    /// `ORT_DYLIB_PATH=… cargo test -p fosfora-xr local -- --ignored`.
    #[test]
    #[ignore = "needs a host ONNX Runtime (ORT_DYLIB_PATH) and the model in assets/xr/models/"]
    fn the_model_on_the_host_matches_its_parity() {
        let (files, lib) = host_model();
        init_runtime(&[lib]).unwrap();
        let s = spec();
        let tok = tokenizer(&s).unwrap();
        let mut model = Model::load(&files.model(), 4).unwrap();
        let pad = i64::from(s.limits.ids.pad);
        let p = parity();
        let column = |task: &str| {
            ["choice", "noul", "score"]
                .iter()
                .position(|t| *t == task)
                .unwrap()
        };
        for case in p["toolkit"].as_array().unwrap() {
            let Some(logits) = case["logits"].as_array() else {
                continue;
            };
            let c = column(case["task"].as_str().unwrap());
            let want = softmax(
                &logits
                    .iter()
                    .map(|row| row[c].as_f64().unwrap() as f32)
                    .collect::<Vec<_>>(),
            );
            let got = softmax(
                &model
                    .logits(&ids(&case["prefix_ids"]), &rows(&case["doc_ids"]), pad, c)
                    .unwrap(),
            );
            assert_eq!(argmax(&got), argmax(&want), "{case:.80}");
            for (g, w) in got.iter().zip(&want) {
                assert!((g - w).abs() < 0.05, "{got:?} vs {want:?}");
            }
        }
        let r = room();
        for sentence in p["room"]["sentences"].as_array().unwrap() {
            let said = sentence["sentence"].as_str().unwrap();
            for row in sentence["decisions"].as_array().unwrap() {
                let want = floats(&row["onnx_probabilities"]);
                let c = row["column"].as_u64().unwrap() as usize;
                let fixture = softmax(
                    &model
                        .logits(&ids(&row["prefix_ids"]), &rows(&row["doc_ids"]), pad, c)
                        .unwrap(),
                );
                let d = decision_for(&s, row, &r);
                let (prefix, docs) = s.render(&tok, &d, said, &r).unwrap();
                let rendered = softmax(&model.logits(&prefix, &docs, pad, c).unwrap());
                for got in [&fixture, &rendered] {
                    for (g, w) in got.iter().zip(&want) {
                        assert!(
                            (g - w).abs() < 1e-3,
                            "{said} {:?}: {got:?} vs {want:?}",
                            d.step
                        );
                    }
                }
            }
        }
        // The provider, end to end, on the worker thread.
        let options = Options {
            threads: 3,
            floors: Floors::default(),
            library: vec![std::env::var_os("ORT_DYLIB_PATH").unwrap().into()],
        };
        let (local, loaded) = Local::start(files, options).unwrap();
        local.load();
        let loaded = loaded.recv().unwrap().unwrap();
        eprintln!("loaded: {loaded:?}");
        for (said, want) in [
            ("Table 14 in amber.", Some(("color", "table 14", "amber"))),
            (
                "Make the floor pump with the kick drum.",
                Some(("band", "floor", "bass")),
            ),
            ("Hello.", None),
        ] {
            let reply = crate::agent::Provider::answer(&local, said, &r)
                .recv()
                .unwrap()
                .unwrap();
            match want {
                Some((k, t, v)) => assert_eq!(
                    reply.actions,
                    [AgentAction {
                        kind: k.into(),
                        target: t.into(),
                        value: v.into()
                    }],
                    "{said}"
                ),
                None => assert_eq!(
                    reply.miss.map(|m| m.reason),
                    Some(Reason::NoMatch),
                    "{said}"
                ),
            }
        }
    }
}
