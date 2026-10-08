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
//! **The tokenizer** (Android, and the desktop tests): the `tokenizers`
//! crate on the model's `tokenizer.json`. The cascade, the session and the
//! provider build on this.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::Value;

use crate::agent::{RoomState, SurfaceState};
use crate::surfaces::{KIND_NAMES, SurfaceBehavior, kind_from_name};

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

// ---------------------------------------------------------- the tokenizer

#[cfg(any(target_os = "android", test))]
pub use self::device::Tokens;

#[cfg(any(target_os = "android", test))]
mod device {
    use std::path::Path;

    use super::{Encode, SpecialIds};

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
}

#[cfg(test)]
mod tests {
    use super::*;
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
        // The behaviors per kind are the app's own catalogue, in its order.
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
}
