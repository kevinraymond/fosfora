//! The voice path, V3: the agent (board #3751, `docs/xr/VOICE_DESIGN.md`,
//! "V3 as built"). A sentence the grammar (`intent.rs`) cannot match goes
//! to a language model with the room's state and comes back as the same
//! [`Intent`](crate::intent::Intent) list the grammar produces, through
//! [`Intent::from_json`](crate::intent::Intent::from_json), or as one
//! sentence for the label. The action list is the boundary: every action is
//! one a hand already performs, applied through V2's path.
//!
//! **The wearer picks the provider** in `<config dir>/voice.json`
//! ([`parse_config`]): the Anthropic Messages API ([`Anthropic`]), or any
//! OpenAI-compatible chat completions endpoint ([`OpenAi`]): a hosted one,
//! or a server on the LAN (llama.cpp's server, Ollama, LM Studio) that
//! needs no key and keeps the sentence in the room. Both network providers
//! ([`Http`]) wrap the same task: one fixed instruction ([`INSTRUCTION`]),
//! one output schema ([`schema`]), and one user message with the room as
//! JSON ([`RoomState`]) and the sentence ([`user_content`]).
//!
//! **V5, the on-device provider** (`local.rs`): our own decision model on
//! the headset, no network and no key, behind the same [`Provider`] trait
//! and the same [`Reply`]. It is the agent whenever its files are
//! installed and `voice.json` names no other provider ([`choose`]).
//!
//! **Pure and desktop-tested:** the config, the private-address rule
//! ([`check_url`]), both request builders and both reply parsers, the
//! room's JSON, the label ([`label`]) and the log lines. **The call**
//! ([`ask`], on its own thread through [`spawn`]) is a thin `ureq` wrapper
//! with a time limit; the frame loop only polls its channel.
//!
//! **The key** lives in `voice.json` on the headset, never in the repo:
//! [`Config`]'s and [`HttpRequest`]'s `Debug` print no key, and no log line
//! here carries a header.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::intent::{AgentAction, Intent, Miss, Reason, Vocabulary};
use crate::lanes::{LaneBox, RoomLanes};
use crate::surfaces::{COLOR_NAMES, SurfaceBehavior, kind_name};

/// The config file's name, under the app's config dir.
pub const CONFIG_FILE: &str = "voice.json";
/// The `provider` value that names the on-device provider (`local.rs`).
pub const LOCAL: &str = "local";
/// The longest a call may take, the retry included (s).
pub const TIMEOUT: Duration = Duration::from_secs(12);
/// What the label reads while a call runs, and how long it may stay (s):
/// past the call's timeout, so the call's own answer replaces it.
pub const THINKING_LABEL: &str = "Thinking\u{2026}";
pub const THINKING_S: f32 = 13.0;
/// How long the label shows the agent's answer or an error (s).
pub const LABEL_S: f32 = 2.5;
/// The reply's token ceiling. Anthropic's is shared with the model's
/// thinking, which runs at every effort, so it is four times the
/// OpenAI-compatible one; the reply itself is short on both.
pub const ANTHROPIC_MAX_TOKENS: u32 = 4096;
pub const OPENAI_MAX_TOKENS: u32 = 1024;
/// The longest `say` the label shows (characters); the instruction asks
/// for 60, a longer one is cut with an ellipsis.
pub const MAX_SAY_CHARS: usize = 80;
/// The Anthropic defaults.
pub const ANTHROPIC_URL: &str = "https://api.anthropic.com";
pub const ANTHROPIC_MODEL: &str = "claude-opus-5-5";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const ANTHROPIC_BETA: &str = "server-side-fallback-2026-07-01";
/// The OpenAI default (the model has none: it must be named).
pub const OPENAI_URL: &str = "https://api.openai.com/v1";

/// The action kinds the schema allows, the [`AgentAction::kind`] values
/// [`Intent::from_json`] reads. `cloud` and `particles` are both the hand
/// menu's "Particles" toggle (the grammar takes both words); `pitcher` is
/// the menu's "Pitcher" toggle.
pub const ACTION_KINDS: [&str; 16] = [
    "next_effect",
    "prev_effect",
    "effect",
    "edit_room",
    "cloud",
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
    "describe",
];

/// The system instruction, shared by both providers and fixed, so the
/// Anthropic API can cache it: one paragraph.
pub const INSTRUCTION: &str = "You are the assistant of a mixed-reality room in which music drives light on the room's real surfaces. You get the room as JSON (the world effect showing and every effect there is; every surface with its name, its kind, its size in meters and what it runs now: behavior, color, band and strength; the name of the surface the wearer points at, or null; the behaviors each kind can run; the color names; the band names), then one sentence the wearer said. Answer only with one JSON object and nothing before or after it: {\"actions\": [...], \"say\": \"...\"}. Each action is {\"kind\": ..., \"target\": ..., \"value\": ...}, three strings. The kinds: next_effect and prev_effect step the world effect; effect switches to the effect named in value; edit_room, cloud (the particle cloud, also called particles), pitcher (the particle pitcher) and music turn on or off by value \"on\" or \"off\"; rescan scans the room again; recenter brings the cloud back around the wearer; all_none sets every surface to none; behavior sets what a surface runs to value, a behavior from the list of the target's own kind, or none; color sets its color to value, a color name (own is the kind's own color, key follows the music's key); band sets which part of the music drives it to value, a band name (rms is the overall level, bass, mid and high the three bands); strength sets its brightness by value up, down, half, full or off; describe shows what the target runs. The behaviors look like this: embers shed glowing embers off a top face on the beat; sparks fly up off a top face with the bass; spectrum draws the music's spectrum as bars across a wall; rings are rings of light spreading on the beat; streamlines are lines of light flowing across the face; curls are tight swirling curls with a faint fill; pulse is a whole-face glow that jumps on the downbeat and breathes with the bar; aurora is flowing curtains of light; prism is a kaleidoscope turning about the center; shards are cells with glowing edges; astrolabe is a dial assembling ring by ring; bezel is chrome hugging the face's edges; fenestra is panels snapping into place across the bar; reticle is crosshairs taking a new target every bar; tessera is a grid of tiles revealing in waves; none is nothing. The target is a surface's name exactly as the room gives it, a kind word (table, floor, wall, ceiling, frame, other) for every surface of that kind, \"pointed\" for the surface the wearer points at, or \"\" for the kinds that take no target; value is \"\" for the kinds that take none. Use the pointed surface when the sentence says this, that or here, or names no surface. List the actions in the order to apply them, as few as do what was asked. say is one short sentence for the wearer's label, at most 60 characters, saying what you did. When the sentence asks for something these actions cannot express, return no actions and say so in say. Never invent a surface, effect, behavior, color or band: use only the names the room gives.";

/// Which API a config talks to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    /// The Anthropic Messages API.
    Anthropic,
    /// An OpenAI-compatible chat completions endpoint.
    OpenAi,
}

impl ProviderKind {
    /// The name `voice.json` and the log use.
    pub fn name(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenAi => "openai",
        }
    }
}

/// `voice.json`, read once at launch: which provider, where, which model,
/// the key, and (OpenAI-compatible only) extra request fields.
#[derive(Clone, PartialEq)]
pub struct Config {
    pub provider: ProviderKind,
    /// Without a trailing slash.
    pub base_url: String,
    pub model: String,
    /// Empty: no key (a local server).
    pub api_key: String,
    /// Merged into the top level of an OpenAI-compatible request body
    /// (Ollama's `"think": false`); empty changes nothing. The Anthropic
    /// request ignores it.
    pub extra: Map<String, Value>,
}

impl std::fmt::Debug for Config {
    /// Everything but the key, which reads as set or not.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("provider", &self.provider)
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field(
                "api_key",
                &if self.api_key.is_empty() {
                    "none"
                } else {
                    "set"
                },
            )
            .field("extra", &self.extra)
            .finish()
    }
}

impl Config {
    /// The launch log line's tail: `anthropic · claude-opus-5-5 ·
    /// api.anthropic.com` (never the key).
    pub fn summary(&self) -> String {
        format!(
            "{} \u{b7} {} \u{b7} {}",
            self.provider.name(),
            self.model,
            host(&self.base_url)
        )
    }

    /// The provider this config names (an [`Http`] one; it upcasts to
    /// `Arc<dyn Provider + Send + Sync>`).
    pub fn provider(&self) -> Arc<dyn Http + Send + Sync> {
        match self.provider {
            ProviderKind::Anthropic => Arc::new(Anthropic {
                base_url: self.base_url.clone(),
                model: self.model.clone(),
                api_key: self.api_key.clone(),
            }),
            ProviderKind::OpenAi => Arc::new(OpenAi {
                base_url: self.base_url.clone(),
                model: self.model.clone(),
                api_key: self.api_key.clone(),
                extra: self.extra.clone(),
            }),
        }
    }
}

/// `voice.json` as written: every field optional.
#[derive(Deserialize, Default)]
#[serde(default)]
struct RawConfig {
    provider: Option<String>,
    base_url: Option<String>,
    model: Option<String>,
    api_key: Option<String>,
    extra: Option<Value>,
}

/// A field's value, `None` when absent or blank.
fn given(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
}

/// Read `voice.json`: `provider` `anthropic` (the default) or `openai`;
/// `base_url` the provider's public one unless given; `model`
/// [`ANTHROPIC_MODEL`] for Anthropic and required for OpenAI; `api_key`
/// required for Anthropic, optional for OpenAI (a local server has none);
/// `extra` an object, default empty. The base URL must pass
/// [`check_url`]. The error is the reason the agent is off, for the log;
/// it never quotes the key (a JSON error gives only its line and column).
pub fn parse_config(json: &str) -> Result<Config, String> {
    let raw = parse_config_raw(json)?;
    let provider = match given(raw.provider).as_deref() {
        None | Some("anthropic") => ProviderKind::Anthropic,
        Some("openai") => ProviderKind::OpenAi,
        Some(other) => return Err(format!("unknown provider \"{other}\" in voice.json")),
    };
    let base_url = given(raw.base_url)
        .unwrap_or_else(|| {
            match provider {
                ProviderKind::Anthropic => ANTHROPIC_URL,
                ProviderKind::OpenAi => OPENAI_URL,
            }
            .to_owned()
        })
        .trim_end_matches('/')
        .to_owned();
    let model = match (given(raw.model), provider) {
        (Some(m), _) => m,
        (None, ProviderKind::Anthropic) => ANTHROPIC_MODEL.to_owned(),
        (None, ProviderKind::OpenAi) => return Err("no model in voice.json".to_owned()),
    };
    let api_key = given(raw.api_key).unwrap_or_default();
    if api_key.is_empty() && provider == ProviderKind::Anthropic {
        return Err("no api_key in voice.json".to_owned());
    }
    let extra = match raw.extra {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(m)) => m,
        Some(_) => return Err("extra in voice.json is not an object".to_owned()),
    };
    check_url(&base_url)?;
    Ok(Config {
        provider,
        base_url,
        model,
        api_key,
        extra,
    })
}

/// [`parse_config`] on the file at `path`; a missing file is its own
/// reason.
pub fn load(path: &std::path::Path) -> Result<Config, String> {
    match std::fs::read_to_string(path) {
        Ok(json) => parse_config(&json),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err(format!("no {CONFIG_FILE} at {}", path.display()))
        }
        Err(e) => Err(format!("{} unreadable: {e}", path.display())),
    }
}

/// `voice.json`'s text at `path`, `None` when there is no file.
pub fn read(path: &std::path::Path) -> Result<Option<String>, String> {
    match std::fs::read_to_string(path) {
        Ok(json) => Ok(Some(json)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{} unreadable: {e}", path.display())),
    }
}

/// Which provider answers (V5): the on-device one, or a network one with
/// its config.
#[derive(Debug, Clone, PartialEq)]
pub enum Choice {
    Local,
    Network(Config),
}

/// The provider rule (V5). `file` is `voice.json`'s text (`None`: no
/// file); `local` is `Ok` when the on-device model's files are installed,
/// else why not. No file, or a file naming no `provider`: the on-device
/// provider when it is installed; without it, no file leaves the agent off
/// and a file is read as V3 read it (Anthropic by default). `provider`
/// `local` asks for it by name (off, with the reason, when it is not
/// installed). `anthropic` and `openai` keep V3's behavior exactly
/// ([`parse_config`]). The error is the reason the agent is off.
pub fn choose(file: Option<&str>, local: Result<(), String>) -> Result<Choice, String> {
    let Some(json) = file else {
        return match local {
            Ok(()) => Ok(Choice::Local),
            Err(why) => Err(format!("no {CONFIG_FILE}, and {why}")),
        };
    };
    // A broken file is V3's error, whatever is installed.
    let named = parse_config_raw(json)?.provider;
    match (given(named).as_deref(), local) {
        (Some(LOCAL) | None, Ok(())) => Ok(Choice::Local),
        (Some(LOCAL), Err(why)) => Err(format!("provider {LOCAL} in {CONFIG_FILE}, but {why}")),
        _ => parse_config(json).map(Choice::Network),
    }
}

/// `voice.json` as written, or V3's error for a file that is no JSON
/// object of the expected fields.
fn parse_config_raw(json: &str) -> Result<RawConfig, String> {
    serde_json::from_str(json).map_err(|e| {
        format!(
            "voice.json is not valid ({:?} at line {} column {})",
            e.classify(),
            e.line(),
            e.column()
        )
        .to_lowercase()
    })
}

/// The host of `url`: after the scheme, before the path, the port and any
/// user info; an IPv6 literal keeps its brackets.
pub fn host(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let hostport = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    if hostport.starts_with('[') {
        return hostport.find(']').map_or(hostport, |end| &hostport[..=end]);
    }
    hostport.split(':').next().unwrap_or_default()
}

/// Whether `host` is a private address: `localhost`, loopback (127/8,
/// `[::1]`), 10/8, 172.16/12 or 192.168/16.
pub fn is_private(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    if let Some(v6) = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
        return v6
            .parse::<std::net::Ipv6Addr>()
            .is_ok_and(|ip| ip.is_loopback());
    }
    host.parse::<std::net::Ipv4Addr>()
        .is_ok_and(|ip| ip.is_private() || ip.is_loopback())
}

/// The private-address rule: `https://` anywhere; plain `http://` only to
/// a private address ([`is_private`]), since a key over plain HTTP on the
/// internet is a leak; any other scheme is refused.
pub fn check_url(url: &str) -> Result<(), String> {
    let lower = url.to_ascii_lowercase();
    let h = host(url);
    if h.is_empty() {
        return Err(format!("no host in base_url \"{url}\""));
    }
    if lower.starts_with("https://") {
        Ok(())
    } else if lower.starts_with("http://") {
        if is_private(h) {
            Ok(())
        } else {
            Err(format!(
                "plain http:// to {h}, which is not a private address (use https://)"
            ))
        }
    } else {
        Err(format!(
            "base_url \"{url}\" is neither https:// nor http://"
        ))
    }
}

/// One surface as the model sees it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SurfaceState {
    /// Its friendly name, the one a target names ("desk", "table 14").
    pub name: String,
    /// Its kind word (`surfaces::kind_name`).
    pub kind: String,
    /// The acting face's two sides (m), larger first.
    pub size_m: [f32; 2],
    pub behavior: String,
    pub color: String,
    pub band: String,
    pub strength: f32,
}

/// The room as the model sees it: the world effect showing and every
/// effect, every surface, the pointed one's name, each kind's behaviors
/// (`SurfaceBehavior::catalogue`), the color names and the band names.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RoomState {
    pub effect: Option<String>,
    pub effects: Vec<String>,
    pub surfaces: Vec<SurfaceState>,
    pub pointed: Option<String>,
    pub behaviors: BTreeMap<String, Vec<String>>,
    pub colors: Vec<String>,
    pub bands: Vec<String>,
}

/// A color index as the model names it: the hand menu's names, the kind's
/// own as "own".
fn color_word(index: u32) -> &'static str {
    match index {
        0 => "own",
        i => crate::surfaces::color_name(i),
    }
}

/// Two decimals, for the JSON.
fn rounded(x: f32) -> f32 {
    (x * 100.0).round() / 100.0
}

/// The room's state from the vocabulary the grammar uses, the effect
/// showing (`effect`, an index into `vocab.effects`), each box's acting
/// face size (`sizes`, by lane box index; missing reads 0) and the lanes'
/// current assignments.
pub fn room_state(
    vocab: &Vocabulary<'_>,
    effect: Option<usize>,
    sizes: &[[f32; 2]],
    lanes: &RoomLanes,
    boxes: &[LaneBox<'_>],
) -> RoomState {
    let surfaces: Vec<SurfaceState> = vocab
        .surfaces
        .iter()
        .map(|s| {
            let behavior = lanes
                .effective(s.index, boxes)
                .map_or(SurfaceBehavior::None, |(b, _)| b);
            let (color, band, strength) = lanes.params_of(s.index, boxes);
            let [a, b] = sizes.get(s.index).copied().unwrap_or_default();
            SurfaceState {
                name: s.name.clone(),
                kind: kind_name(s.kind).to_owned(),
                size_m: [rounded(a.max(b)), rounded(a.min(b))],
                behavior: behavior.name().to_owned(),
                color: color_word(color).to_owned(),
                band: crate::surface_fx::band_name(band).to_owned(),
                strength: rounded(strength),
            }
        })
        .collect();
    let mut behaviors = BTreeMap::new();
    for s in &vocab.surfaces {
        behaviors
            .entry(kind_name(s.kind).to_owned())
            .or_insert_with(|| {
                SurfaceBehavior::catalogue(s.kind)
                    .iter()
                    .filter(|b| vocab.behaviors.contains(b))
                    .map(|b| b.name().to_owned())
                    .collect()
            });
    }
    RoomState {
        effect: effect.and_then(|i| vocab.effects.get(i)).cloned(),
        effects: vocab.effects.to_vec(),
        surfaces,
        pointed: vocab
            .pointed
            .and_then(|p| vocab.surfaces.iter().find(|s| s.index == p))
            .map(|s| s.name.clone()),
        behaviors,
        colors: (0..COLOR_NAMES.len() as u32)
            .map(|i| color_word(i).to_owned())
            .collect(),
        bands: crate::surface_fx::BAND_NAMES
            .iter()
            .map(|b| (*b).to_owned())
            .collect(),
    }
}

/// The output schema both providers send: `actions` (each `kind` from
/// [`ACTION_KINDS`], `target`, `value`) and `say`; every object closed
/// (`additionalProperties: false`) and every field required.
pub fn schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["actions", "say"],
        "properties": {
            "say": { "type": "string" },
            "actions": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["kind", "target", "value"],
                    "properties": {
                        "kind": { "type": "string", "enum": ACTION_KINDS },
                        "target": { "type": "string" },
                        "value": { "type": "string" }
                    }
                }
            }
        }
    })
}

/// The user message both providers send: the room as JSON, then the
/// sentence.
pub fn user_content(sentence: &str, room: &RoomState) -> String {
    let room = serde_json::to_string(room).unwrap_or_else(|_| "{}".to_owned());
    format!("The room:\n{room}\n\nThe sentence: \"{}\"", sentence.trim())
}

/// One HTTP request: a `POST` to `url` with these headers and this JSON
/// body. Its `Debug` prints the header names only.
#[derive(Clone, PartialEq)]
pub struct HttpRequest {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Value,
}

impl std::fmt::Debug for HttpRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: Vec<&str> = self.headers.iter().map(|(k, _)| k.as_str()).collect();
        f.debug_struct("HttpRequest")
            .field("url", &self.url)
            .field("headers", &names)
            .field("body", &self.body)
            .finish()
    }
}

/// A call's token counts, from the API's `usage`; `None` where a server
/// leaves them out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Usage {
    pub input: Option<u64>,
    pub output: Option<u64>,
    /// Anthropic's `cache_read_input_tokens`.
    pub cache_read: Option<u64>,
}

/// The model's answer: the actions in order, the label's sentence, and
/// what the call cost; or (V5, the on-device provider) a miss: the
/// sentence is not a request, or no answer cleared its confidence floor,
/// which the label shows as the grammar's own "Didn't catch that" with
/// its hint, and nothing in the room changes.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Reply {
    pub actions: Vec<AgentAction>,
    pub say: String,
    pub usage: Usage,
    pub miss: Option<Miss>,
}

/// Why a call gave no reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentError {
    /// The endpoint could not be reached (no route, no DNS, refused).
    NoNetwork(String),
    /// A status other than 200 (0: no status, a TLS or client error), with
    /// the response body.
    Http(u16, String),
    /// The model declined (Anthropic's refusal category, or empty).
    Refused(String),
    /// The time limit passed.
    Timeout,
    /// The answer was not the schema's JSON, or was cut off.
    Parse(String),
}

impl AgentError {
    /// What the label says.
    pub fn label(&self) -> &'static str {
        match self {
            Self::NoNetwork(_) => "No network for that",
            Self::Refused(_) => "I can't help with that one",
            Self::Timeout => "Took too long",
            Self::Http(..) | Self::Parse(_) => "The agent didn't answer",
        }
    }
}

impl std::fmt::Display for AgentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoNetwork(e) => write!(f, "no network ({e})"),
            Self::Http(status, body) => write!(f, "HTTP {status}: {body}"),
            Self::Refused(category) if category.is_empty() => write!(f, "refused"),
            Self::Refused(category) => write!(f, "refused ({category})"),
            Self::Timeout => write!(f, "timed out"),
            Self::Parse(e) => write!(f, "unparsed answer ({e})"),
        }
    }
}

/// The reply as the schema has it, read leniently: a missing field is
/// empty.
#[derive(Deserialize, Default)]
#[serde(default)]
struct ReplyJson {
    actions: Vec<AgentAction>,
    say: String,
}

/// The first balanced `{…}` in `text` (strings and their escapes
/// skipped), so prose around the JSON, or a code fence, does no harm.
pub fn first_object(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (i, c) in text[start..].char_indices() {
        if in_string {
            match (escaped, c) {
                (true, _) => escaped = false,
                (false, '\\') => escaped = true,
                (false, '"') => in_string = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[start..=start + i]);
                }
            }
            _ => {}
        }
    }
    None
}

/// The model's text as the schema's object: the first `{…}` in it.
fn parse_text(text: &str, usage: Usage) -> Result<Reply, AgentError> {
    let object = first_object(text).ok_or_else(|| AgentError::Parse("no JSON object".into()))?;
    let r: ReplyJson =
        serde_json::from_str(object).map_err(|e| AgentError::Parse(e.to_string()))?;
    Ok(Reply {
        actions: r.actions,
        say: r.say.trim().to_owned(),
        usage,
        miss: None,
    })
}

/// A JSON number at `v[key]`, if any.
fn count(v: &Value, key: &str) -> Option<u64> {
    v.get(key).and_then(Value::as_u64)
}

/// One provider: who answers a sentence the grammar missed, off the frame
/// thread.
pub trait Provider {
    /// The provider's name for the log (`anthropic`, `openai`, `local`).
    fn name(&self) -> &'static str;
    /// The model it asks.
    fn model(&self) -> &str;
    /// Ask about `sentence` in `room` without blocking: the receiver gets
    /// the one outcome (the frame loop polls it). A network provider posts
    /// its request on a thread of its own ([`spawn`]); the on-device one
    /// queues the sentence for its worker (`local.rs`).
    fn answer(&self, sentence: &str, room: &RoomState) -> Receiver<Result<Reply, AgentError>>;
}

/// A network provider: the request for a sentence, and its answer read.
pub trait Http: Provider {
    /// The request for `sentence` in `room`: url, headers (the key among
    /// them), body.
    fn request(&self, sentence: &str, room: &RoomState) -> HttpRequest;
    /// The answer to a request: its status and body.
    fn parse(&self, status: u16, body: &str) -> Result<Reply, AgentError>;
    /// `req` without the output format, when `status` and `body` say the
    /// endpoint refused it; `None` otherwise (or when it has none left).
    /// The instruction already asks for the JSON alone, and the parse
    /// reads the first `{…}`, so an endpoint without the format still
    /// works when the model obeys.
    fn without_format(&self, req: &HttpRequest, status: u16, body: &str) -> Option<HttpRequest>;
}

/// The Anthropic Messages API (`POST {base_url}/v1/messages`).
#[derive(Clone, PartialEq, Eq)]
pub struct Anthropic {
    pub base_url: String,
    pub model: String,
    pub api_key: String,
}

impl Provider for Anthropic {
    fn name(&self) -> &'static str {
        ProviderKind::Anthropic.name()
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn answer(&self, sentence: &str, room: &RoomState) -> Receiver<Result<Reply, AgentError>> {
        spawn(
            Arc::new(self.clone()),
            self.request(sentence, room),
            TIMEOUT,
        )
    }
}

impl Http for Anthropic {
    /// Effort low, the schema as the output format, the server-side
    /// fallback on a refusal, the instruction as a cached system block.
    /// No `thinking` (the model thinks by default; an explicit setting is
    /// refused), no `temperature`, no `tool_choice`, no prefill.
    fn request(&self, sentence: &str, room: &RoomState) -> HttpRequest {
        HttpRequest {
            url: format!("{}/v1/messages", self.base_url),
            headers: vec![
                ("content-type".into(), "application/json".into()),
                ("x-api-key".into(), self.api_key.clone()),
                ("anthropic-version".into(), ANTHROPIC_VERSION.into()),
                ("anthropic-beta".into(), ANTHROPIC_BETA.into()),
            ],
            body: json!({
                "model": self.model,
                "max_tokens": ANTHROPIC_MAX_TOKENS,
                "output_config": {
                    "effort": "low",
                    "format": { "type": "json_schema", "schema": schema() }
                },
                "fallbacks": "default",
                "system": [{
                    "type": "text",
                    "text": INSTRUCTION,
                    "cache_control": { "type": "ephemeral" }
                }],
                "messages": [{ "role": "user", "content": user_content(sentence, room) }]
            }),
        }
    }

    /// Status not 200: `Http`. A refusal: `Refused` with its category. Cut
    /// off at the ceiling: `Parse("cut off")`. Else the first text block
    /// (after any thinking blocks) read by the schema.
    fn parse(&self, status: u16, body: &str) -> Result<Reply, AgentError> {
        if status != 200 {
            return Err(AgentError::Http(status, body.to_owned()));
        }
        let v: Value = serde_json::from_str(body).map_err(|e| AgentError::Parse(e.to_string()))?;
        match v.get("stop_reason").and_then(Value::as_str) {
            Some("refusal") => {
                let category = v
                    .pointer("/stop_details/category")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                return Err(AgentError::Refused(category.to_owned()));
            }
            Some("max_tokens") => return Err(AgentError::Parse("cut off".into())),
            _ => {}
        }
        let usage = v.get("usage").map_or_else(Usage::default, |u| Usage {
            input: count(u, "input_tokens"),
            output: count(u, "output_tokens"),
            cache_read: count(u, "cache_read_input_tokens"),
        });
        let text = v
            .get("content")
            .and_then(Value::as_array)
            .and_then(|blocks| {
                blocks
                    .iter()
                    .find(|b| b.get("type").and_then(Value::as_str) == Some("text"))
            })
            .and_then(|b| b.get("text"))
            .and_then(Value::as_str)
            .ok_or_else(|| AgentError::Parse("no text block".into()))?;
        parse_text(text, usage)
    }

    /// A 400 that names the output format (`output_config` or `format`):
    /// the request without `output_config.format` (the effort stays).
    fn without_format(&self, req: &HttpRequest, status: u16, body: &str) -> Option<HttpRequest> {
        if status != 400 || !(body.contains("output_config") || body.contains("format")) {
            return None;
        }
        let mut req = req.clone();
        req.body
            .get_mut("output_config")
            .and_then(Value::as_object_mut)?
            .remove("format")?;
        Some(req)
    }
}

/// An OpenAI-compatible chat completions endpoint (`POST
/// {base_url}/chat/completions`): OpenAI, or a server on the LAN.
#[derive(Clone, PartialEq)]
pub struct OpenAi {
    pub base_url: String,
    pub model: String,
    /// Empty: no `Authorization` header.
    pub api_key: String,
    /// Merged into the body's top level.
    pub extra: Map<String, Value>,
}

impl Provider for OpenAi {
    fn name(&self) -> &'static str {
        ProviderKind::OpenAi.name()
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn answer(&self, sentence: &str, room: &RoomState) -> Receiver<Result<Reply, AgentError>> {
        spawn(
            Arc::new(self.clone()),
            self.request(sentence, room),
            TIMEOUT,
        )
    }
}

impl Http for OpenAi {
    /// The schema as a strict `response_format`, the instruction as the
    /// system message; no `temperature`, no tools; `extra`'s members
    /// merged into the top level last.
    fn request(&self, sentence: &str, room: &RoomState) -> HttpRequest {
        let mut headers = vec![("content-type".to_owned(), "application/json".to_owned())];
        if !self.api_key.is_empty() {
            headers.push(("authorization".into(), format!("Bearer {}", self.api_key)));
        }
        let mut body = json!({
            "model": self.model,
            "max_tokens": OPENAI_MAX_TOKENS,
            "response_format": {
                "type": "json_schema",
                "json_schema": { "name": "room_actions", "strict": true, "schema": schema() }
            },
            "messages": [
                { "role": "system", "content": INSTRUCTION },
                { "role": "user", "content": user_content(sentence, room) }
            ]
        });
        if let Some(top) = body.as_object_mut() {
            for (k, v) in &self.extra {
                top.insert(k.clone(), v.clone());
            }
        }
        HttpRequest {
            url: format!("{}/chat/completions", self.base_url),
            headers,
            body,
        }
    }

    /// Status not 200: `Http`. `finish_reason` "length": `Parse("cut
    /// off")`; "`content_filter`": `Refused("")`. Else the first choice's
    /// content, its first `{…}` read by the schema.
    fn parse(&self, status: u16, body: &str) -> Result<Reply, AgentError> {
        if status != 200 {
            return Err(AgentError::Http(status, body.to_owned()));
        }
        let v: Value = serde_json::from_str(body).map_err(|e| AgentError::Parse(e.to_string()))?;
        let choice = v
            .pointer("/choices/0")
            .ok_or_else(|| AgentError::Parse("no choices".into()))?;
        match choice.get("finish_reason").and_then(Value::as_str) {
            Some("length") => return Err(AgentError::Parse("cut off".into())),
            Some("content_filter") => return Err(AgentError::Refused(String::new())),
            _ => {}
        }
        let usage = v.get("usage").map_or_else(Usage::default, |u| Usage {
            input: count(u, "prompt_tokens"),
            output: count(u, "completion_tokens"),
            cache_read: None,
        });
        let text = choice
            .pointer("/message/content")
            .and_then(Value::as_str)
            .ok_or_else(|| AgentError::Parse("no message content".into()))?;
        parse_text(text, usage)
    }

    /// A 400 (or 422) whose body names `response_format`: the request
    /// without it.
    fn without_format(&self, req: &HttpRequest, status: u16, body: &str) -> Option<HttpRequest> {
        if !matches!(status, 400 | 422) || !body.contains("response_format") {
            return None;
        }
        let mut req = req.clone();
        req.body.as_object_mut()?.remove("response_format")?;
        Some(req)
    }
}

/// Whether a grammar miss goes to the agent: a sentence no template fits
/// (NoMatch), and one that fits a template's shape but names a behavior
/// the catalogue lacks ("a campfire on the desk", UnknownBehavior), which
/// a model can read for its meaning. The other misses stay the grammar's
/// own answers: a surface the room lacks (the agent cannot know it
/// either), a behavior the kind does not run, which surface, and nothing
/// pointed at.
pub fn forwards(reason: &Reason) -> bool {
    matches!(reason, Reason::NoMatch | Reason::UnknownBehavior(_))
}

/// The per-call log line: `voice agent: anthropic claude-opus-5-5 · 1840
/// ms · in 1532 cached 1210 out 88 · 2 actions · schema`, a count a server
/// left out as `-`; `path` says whether the output format went or the call
/// was retried without it.
pub fn call_line(
    provider: &dyn Provider,
    ms: u128,
    outcome: &Result<Reply, AgentError>,
    path: &str,
) -> String {
    let n = |c: Option<u64>| c.map_or_else(|| "-".to_owned(), |c| c.to_string());
    let what = match outcome {
        Ok(r) => {
            let cached = r
                .usage
                .cache_read
                .map(|c| format!(" cached {c}"))
                .unwrap_or_default();
            format!(
                "in {}{cached} out {} \u{b7} {} actions",
                n(r.usage.input),
                n(r.usage.output),
                r.actions.len()
            )
        }
        Err(e) => e.to_string(),
    };
    format!(
        "voice agent: {} {} \u{b7} {ms} ms \u{b7} {what} \u{b7} {path}",
        provider.name(),
        provider.model()
    )
}

/// One `POST` within a time limit: the status and the body, any status (the
/// body of a 400 names the field to fix).
fn post(req: &HttpRequest, timeout: Duration) -> Result<(u16, String), AgentError> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .build()
        .into();
    let mut call = agent.post(&req.url);
    for (k, v) in &req.headers {
        call = call.header(k.as_str(), v.as_str());
    }
    let map = |e: ureq::Error| match e {
        ureq::Error::Timeout(_) => AgentError::Timeout,
        ureq::Error::Io(io) if io.kind() == std::io::ErrorKind::TimedOut => AgentError::Timeout,
        ureq::Error::HostNotFound | ureq::Error::ConnectionFailed | ureq::Error::Io(_) => {
            AgentError::NoNetwork(e.to_string())
        }
        other => AgentError::Http(0, other.to_string()),
    };
    let mut response = call.send(req.body.to_string()).map_err(map)?;
    let status = response.status().as_u16();
    let body = response.body_mut().read_to_string().map_err(map)?;
    Ok((status, body))
}

/// The call: `req` posted within `timeout`; on a refused output format
/// ([`Http::without_format`]) once more without it, in the time left;
/// the answer read by the provider. Blocking: run it off the frame thread
/// ([`spawn`]). Logs one line per call ([`call_line`]), and a non-200
/// body in full (never a header).
pub fn ask(provider: &dyn Http, req: HttpRequest, timeout: Duration) -> Result<Reply, AgentError> {
    let started = Instant::now();
    let mut req = req;
    let mut path = "schema";
    let outcome = loop {
        let left = timeout.saturating_sub(started.elapsed());
        if left.is_zero() {
            break Err(AgentError::Timeout);
        }
        let (status, body) = match post(&req, left) {
            Ok(answer) => answer,
            Err(e) => break Err(e),
        };
        if status != 200 {
            log::warn!("voice agent: {} answered {status}: {body}", provider.name());
            if path == "schema"
                && let Some(again) = provider.without_format(&req, status, &body)
            {
                log::warn!(
                    "voice agent: {} refused the output format; once more without it",
                    provider.name()
                );
                req = again;
                path = "no schema (retried)";
                continue;
            }
        }
        break provider.parse(status, &body);
    };
    log::info!(
        "{}",
        call_line(provider, started.elapsed().as_millis(), &outcome, path)
    );
    outcome
}

/// [`ask`] on its own thread: the receiver gets the one outcome.
pub fn spawn(
    provider: Arc<dyn Http + Send + Sync>,
    req: HttpRequest,
    timeout: Duration,
) -> Receiver<Result<Reply, AgentError>> {
    let (tx, rx) = channel();
    let fail = tx.clone();
    let spawned = std::thread::Builder::new()
        .name("fosfora-agent".into())
        .spawn(move || {
            // The frame loop may have moved on (a newer sentence): then
            // nobody listens, and the answer is dropped.
            let _ = tx.send(ask(provider.as_ref(), req, timeout));
        });
    if let Err(e) = spawned {
        let _ = fail.send(Err(AgentError::Http(0, format!("no thread: {e}"))));
    }
    rx
}

/// The label for a reply, and how long it shows: `say` when every action
/// was applied (the actions' own labels are in the log); with some dropped,
/// the applied ones' labels and how many were skipped; with all dropped,
/// the first miss's own text (`intent::miss_text`); with nothing at all,
/// that the agent did not answer. A `say` past [`MAX_SAY_CHARS`] is cut.
pub fn label(say: &str, applied: &[String], missed: &[Miss]) -> (String, f32) {
    let say = say.trim();
    let text = match (applied, missed) {
        (_, []) if !say.is_empty() => {
            if say.chars().count() > MAX_SAY_CHARS {
                let cut: String = say.chars().take(MAX_SAY_CHARS - 1).collect();
                format!("{}\u{2026}", cut.trim_end())
            } else {
                say.to_owned()
            }
        }
        ([], []) => AgentError::Parse(String::new()).label().to_owned(),
        (_, []) => applied.join("; "),
        ([], [first, ..]) => crate::intent::miss_text(first),
        (_, _) => format!("{}; skipped {}", applied.join("; "), missed.len()),
    };
    (text, LABEL_S)
}

/// The reply's log line: `voice agent: heard "<sentence>" → [<intent or
/// miss>, …] · say "<say>"`.
pub fn heard_line(sentence: &str, outcomes: &[Result<Intent, Miss>], say: &str) -> String {
    let what: Vec<String> = outcomes
        .iter()
        .map(|o| match o {
            Ok(intent) => format!("{intent:?}"),
            Err(miss) => format!("miss {:?}", miss.reason),
        })
        .collect();
    format!(
        "voice agent: heard \"{}\" \u{2192} [{}] \u{b7} say \"{say}\"",
        sentence.trim(),
        what.join(", ")
    )
}

/// The line naming what a reply's actions dropped, if any: `voice agent:
/// dropped 1 of 3: color desk orchid (NoMatch)`.
pub fn dropped_line(outcomes: &[Result<Intent, Miss>]) -> Option<String> {
    let dropped: Vec<String> = outcomes
        .iter()
        .filter_map(|o| o.as_ref().err())
        .map(|m| {
            let reason = match &m.reason {
                Reason::NoMatch => "NoMatch".to_owned(),
                r => format!("{r:?}"),
            };
            format!("{} ({reason})", m.heard)
        })
        .collect();
    (!dropped.is_empty()).then(|| {
        format!(
            "voice agent: dropped {} of {}: {}",
            dropped.len(),
            outcomes.len(),
            dropped.join(", ")
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intent::{Strength, Target, surfaces};
    use crate::room_file::room_id;
    use crate::surfaces::{KIND_CEILING, KIND_FLOOR, KIND_OTHER, KIND_TABLE, KIND_WALL};
    use SurfaceBehavior as B;

    /// A placeholder key; no real key is ever in the repo.
    const KEY: &str = "test-key-not-real";

    fn effects() -> Vec<String> {
        ["Flux Cloud", "Embers", "Flock"]
            .map(str::to_owned)
            .to_vec()
    }

    fn lane(n: u8, kind: u32, label: &'static str) -> LaneBox<'static> {
        LaneBox {
            uuid: std::array::from_fn(|i| if i == 0 { 0x40 + n } else { 0xb0 + i as u8 }),
            kind,
            label,
        }
    }

    /// The fixture room: "desk", "table 1", "table 2", "wall 3", "wall 4",
    /// "floor", "ceiling", "lamp".
    fn room() -> Vec<LaneBox<'static>> {
        vec![
            lane(0, KIND_TABLE, "DESK"),
            lane(1, KIND_TABLE, "TABLE"),
            lane(2, KIND_TABLE, "TABLE"),
            lane(3, KIND_WALL, "WALL_FACE"),
            lane(4, KIND_WALL, "WALL_FACE"),
            lane(5, KIND_FLOOR, "FLOOR"),
            lane(6, KIND_CEILING, "CEILING"),
            lane(7, KIND_OTHER, "LAMP"),
        ]
    }

    const SIZES: [[f32; 2]; 8] = [
        [1.2, 0.6],
        [0.8, 0.8],
        [1.6, 0.9],
        [4.123, 2.5],
        [2.5, 3.0],
        [4.0, 3.5],
        [4.0, 3.5],
        [0.3, 0.3],
    ];

    fn vocab<'a>(
        effects: &'a [String],
        boxes: &[LaneBox<'_>],
        pointed: Option<usize>,
    ) -> Vocabulary<'a> {
        Vocabulary {
            effects,
            behaviors: &SurfaceBehavior::ALL,
            surfaces: surfaces(boxes),
            pointed,
        }
    }

    /// Fresh lanes for the fixture under a temp dir named `tag`.
    fn lanes(tag: &str, boxes: &[LaneBox<'_>]) -> (RoomLanes, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("fosfora-agent-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut lanes = RoomLanes::new(dir.clone());
        lanes.update(room_id(boxes.iter().map(|b| b.uuid)), boxes);
        (lanes, dir)
    }

    fn fixture_room(tag: &str) -> RoomState {
        let (e, boxes) = (effects(), room());
        let (lanes, dir) = lanes(tag, &boxes);
        let room = room_state(&vocab(&e, &boxes, Some(0)), Some(1), &SIZES, &lanes, &boxes);
        let _ = std::fs::remove_dir_all(&dir);
        room
    }

    /// The fixture room's JSON, as the user message carries it.
    const ROOM_JSON: &str = r#"{"effect":"Embers","effects":["Flux Cloud","Embers","Flock"],"surfaces":[{"name":"desk","kind":"table","size_m":[1.2,0.6],"behavior":"streamlines","color":"own","band":"rms","strength":1.0},{"name":"table 1","kind":"table","size_m":[0.8,0.8],"behavior":"streamlines","color":"own","band":"rms","strength":1.0},{"name":"table 2","kind":"table","size_m":[1.6,0.9],"behavior":"streamlines","color":"own","band":"rms","strength":1.0},{"name":"wall 3","kind":"wall","size_m":[4.12,2.5],"behavior":"spectrum","color":"own","band":"rms","strength":1.0},{"name":"wall 4","kind":"wall","size_m":[3.0,2.5],"behavior":"spectrum","color":"own","band":"rms","strength":1.0},{"name":"floor","kind":"floor","size_m":[4.0,3.5],"behavior":"rings","color":"own","band":"bass","strength":1.0},{"name":"ceiling","kind":"ceiling","size_m":[4.0,3.5],"behavior":"none","color":"own","band":"rms","strength":1.0},{"name":"lamp","kind":"other","size_m":[0.3,0.3],"behavior":"curls","color":"own","band":"rms","strength":1.0}],"pointed":"desk","behaviors":{"ceiling":["none","rings","pulse","streamlines","aurora","astrolabe"],"floor":["none","rings","streamlines","curls","sparks","tessera","prism","astrolabe"],"other":["none","curls","streamlines","pulse","embers","shards","reticle","tessera"],"table":["none","streamlines","curls","pulse","embers","sparks","shards","prism"],"wall":["none","spectrum","streamlines","rings","pulse","aurora","fenestra","bezel"]},"colors":["own","blue","violet","warm white","amber","green","teal","rose","key"],"bands":["rms","bass","mid","high"]}"#;

    #[test]
    fn the_room_as_json() {
        let room = fixture_room("json");
        assert_eq!(serde_json::to_string(&room).unwrap(), ROOM_JSON);
        assert_eq!(
            user_content(" Something like a campfire on the desk. ", &room),
            format!(
                "The room:\n{ROOM_JSON}\n\nThe sentence: \"Something like a campfire on the desk.\""
            )
        );
    }

    #[test]
    fn the_config_defaults() {
        let c = parse_config(&format!(r#"{{"api_key": "{KEY}"}}"#)).unwrap();
        assert_eq!(c.provider, ProviderKind::Anthropic);
        assert_eq!(c.base_url, ANTHROPIC_URL);
        assert_eq!(c.model, "claude-opus-5-5");
        assert_eq!(c.api_key, KEY);
        assert!(c.extra.is_empty());
        let c = parse_config(&format!(
            r#"{{"provider": "anthropic", "model": " ", "base_url": "https://api.anthropic.com/", "api_key": "{KEY}"}}"#
        ))
        .unwrap();
        assert_eq!(
            (c.base_url.as_str(), c.model.as_str()),
            (ANTHROPIC_URL, ANTHROPIC_MODEL)
        );
        assert_eq!(
            c.summary(),
            "anthropic \u{b7} claude-opus-5-5 \u{b7} api.anthropic.com"
        );
        let c = parse_config(r#"{"provider": "openai", "model": "gpt-test"}"#).unwrap();
        assert_eq!(c.provider, ProviderKind::OpenAi);
        assert_eq!(c.base_url, OPENAI_URL);
        assert_eq!(c.api_key, "");
        let c = parse_config(
            r#"{"provider": "openai", "base_url": "http://192.168.1.20:11434/v1", "model": "qwen3.5", "extra": {"think": false}}"#,
        )
        .unwrap();
        assert_eq!(c.summary(), "openai \u{b7} qwen3.5 \u{b7} 192.168.1.20");
        assert_eq!(c.extra.get("think"), Some(&Value::Bool(false)));
    }

    #[test]
    fn the_config_refusals() {
        for (json, why) in [
            (r#"{"provider": "openai"}"#, "no model in voice.json"),
            (
                r#"{"provider": "openai", "model": ""}"#,
                "no model in voice.json",
            ),
            (r#"{}"#, "no api_key in voice.json"),
            (
                r#"{"provider": "anthropic", "api_key": " "}"#,
                "no api_key in voice.json",
            ),
            (
                r#"{"provider": "gemini", "model": "x"}"#,
                "unknown provider \"gemini\" in voice.json",
            ),
            (
                r#"{"provider": "openai", "model": "m", "extra": "think"}"#,
                "extra in voice.json is not an object",
            ),
            (
                r#"{"provider": "openai", "model": "m", "base_url": "http://example.com/v1"}"#,
                "plain http:// to example.com, which is not a private address (use https://)",
            ),
        ] {
            assert_eq!(parse_config(json).unwrap_err(), why, "{json}");
        }
        // A broken file names where, never what: the key stays out of the
        // log.
        let e = parse_config(&format!(r#"{{"api_key": "{KEY}", "model": 5}}"#)).unwrap_err();
        assert!(!e.contains(KEY), "{e}");
        assert!(
            e.starts_with("voice.json is not valid (data at line 1"),
            "{e}"
        );
        let e = parse_config(&format!(r#"{{"api_key": "{KEY}""#)).unwrap_err();
        assert!(!e.contains(KEY) && e.contains("eof"), "{e}");
        let missing = std::env::temp_dir().join("fosfora-agent-none/voice.json");
        assert!(load(&missing).unwrap_err().starts_with("no voice.json at "));
    }

    #[test]
    fn the_private_address_rule() {
        for url in [
            "https://api.anthropic.com",
            "https://api.openai.com/v1",
            "https://example.com:8443/v1",
            "http://192.168.1.20:11434/v1",
            "http://10.0.0.7:8080/v1",
            "http://172.16.0.1/v1",
            "http://172.31.255.9:1234/v1",
            "http://localhost:1234/v1",
            "http://127.0.0.1:8080",
            "http://[::1]:8080/v1",
            "HTTP://192.168.0.2/v1",
        ] {
            assert_eq!(check_url(url), Ok(()), "{url}");
        }
        for url in [
            "http://example.com/v1",
            "http://8.8.8.8/v1",
            "http://172.32.0.1/v1",
            "http://11.0.0.1/v1",
            "http://192.169.1.1/v1",
            "http://user@203.0.113.9/v1",
            "http://192.168.1.20.example.com/v1",
            "ftp://192.168.1.20/v1",
            "192.168.1.20:11434/v1",
            "https:///v1",
        ] {
            assert!(check_url(url).is_err(), "{url}");
        }
        assert_eq!(
            host("https://user:pw@api.example.com:443/v1?x"),
            "api.example.com"
        );
        assert_eq!(host("http://[::1]:8080/v1"), "[::1]");
        assert!(is_private("LOCALHOST") && !is_private("[2001:db8::1]"));
    }

    fn anthropic() -> Anthropic {
        Anthropic {
            base_url: ANTHROPIC_URL.into(),
            model: ANTHROPIC_MODEL.into(),
            api_key: KEY.into(),
        }
    }

    fn openai(key: &str, extra: Map<String, Value>) -> OpenAi {
        OpenAi {
            base_url: "http://192.168.1.20:11434/v1".into(),
            model: "qwen3.5".into(),
            api_key: key.into(),
            extra,
        }
    }

    const SENTENCE: &str = "Something like a campfire on the desk.";

    fn user() -> String {
        format!("The room:\n{ROOM_JSON}\n\nThe sentence: \"{SENTENCE}\"")
    }

    #[test]
    fn the_anthropic_request_matches_its_golden() {
        let req = anthropic().request(SENTENCE, &fixture_room("anthropic"));
        assert_eq!(req.url, "https://api.anthropic.com/v1/messages");
        assert_eq!(
            req.headers,
            [
                ("content-type", "application/json"),
                ("x-api-key", KEY),
                ("anthropic-version", "2023-06-01"),
                ("anthropic-beta", "server-side-fallback-2026-07-01"),
            ]
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
        );
        let golden = json!({
            "model": "claude-opus-5-5",
            "max_tokens": 4096,
            "output_config": {
                "effort": "low",
                "format": { "type": "json_schema", "schema": schema() }
            },
            "fallbacks": "default",
            "system": [{ "type": "text", "text": INSTRUCTION, "cache_control": { "type": "ephemeral" } }],
            "messages": [{ "role": "user", "content": user() }]
        });
        assert_eq!(req.body, golden);
        let top = req.body.as_object().unwrap();
        for absent in [
            "thinking",
            "temperature",
            "tool_choice",
            "tools",
            "top_p",
            "top_k",
        ] {
            assert!(!top.contains_key(absent), "{absent}");
        }
        // One user message, no assistant prefill.
        assert_eq!(req.body["messages"].as_array().unwrap().len(), 1);
        // The Debug form names the headers, never their values.
        assert!(!format!("{req:?}").contains(KEY));
    }

    #[test]
    fn the_openai_request_matches_its_golden() {
        let room = fixture_room("openai");
        let req = openai("", Map::new()).request(SENTENCE, &room);
        assert_eq!(req.url, "http://192.168.1.20:11434/v1/chat/completions");
        assert_eq!(
            req.headers,
            [("content-type".to_owned(), "application/json".to_owned())]
        );
        let golden = json!({
            "model": "qwen3.5",
            "max_tokens": 1024,
            "response_format": {
                "type": "json_schema",
                "json_schema": { "name": "room_actions", "strict": true, "schema": schema() }
            },
            "messages": [
                { "role": "system", "content": INSTRUCTION },
                { "role": "user", "content": user() }
            ]
        });
        assert_eq!(req.body, golden);
        let top = req.body.as_object().unwrap();
        for absent in ["temperature", "tools", "tool_choice", "think"] {
            assert!(!top.contains_key(absent), "{absent}");
        }
        // With a key, a bearer header; the body is the same.
        let keyed = openai(KEY, Map::new()).request(SENTENCE, &room);
        assert_eq!(
            keyed.headers[1],
            ("authorization".to_owned(), format!("Bearer {KEY}"))
        );
        assert_eq!(keyed.body, golden);
        assert!(!format!("{keyed:?}").contains(KEY));
    }

    #[test]
    fn extra_merges_into_the_openai_body_and_empty_changes_nothing() {
        let room = fixture_room("extra");
        let plain = openai("", Map::new()).request(SENTENCE, &room);
        let Value::Object(empty) = json!({}) else {
            unreachable!()
        };
        assert_eq!(openai("", empty).request(SENTENCE, &room), plain);
        let Value::Object(extra) = json!({ "think": false, "keep_alive": "5m" }) else {
            unreachable!()
        };
        let merged = openai("", extra).request(SENTENCE, &room);
        let mut want = plain.body.clone();
        want["think"] = json!(false);
        want["keep_alive"] = json!("5m");
        assert_eq!(merged.body, want);
        assert_eq!(merged.body["think"], json!(false));
        // Through the config too; the Anthropic request ignores it.
        let c = parse_config(&format!(
            r#"{{"api_key": "{KEY}", "extra": {{"think": false}}}}"#
        ))
        .unwrap();
        assert!(
            !c.provider()
                .request(SENTENCE, &room)
                .body
                .as_object()
                .unwrap()
                .contains_key("think")
        );
        let c = parse_config(r#"{"provider": "openai", "model": "qwen3.5", "base_url": "http://192.168.1.20:11434/v1", "extra": {"think": false}}"#).unwrap();
        assert_eq!(
            c.provider().request(SENTENCE, &room).body["think"],
            json!(false)
        );
    }

    /// Every object in `v` closed and requiring all its properties.
    fn walk(v: &Value, path: &str, objects: &mut usize) {
        match v {
            Value::Object(m) => {
                if m.get("type") == Some(&json!("object")) {
                    *objects += 1;
                    assert_eq!(m.get("additionalProperties"), Some(&json!(false)), "{path}");
                    let mut props: Vec<&String> =
                        m["properties"].as_object().unwrap().keys().collect();
                    let mut required: Vec<&str> = m["required"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|r| r.as_str().unwrap())
                        .collect();
                    props.sort();
                    required.sort_unstable();
                    assert_eq!(props, required, "{path}");
                }
                for (k, v) in m {
                    walk(v, &format!("{path}.{k}"), objects);
                }
            }
            Value::Array(a) => a.iter().for_each(|v| walk(v, path, objects)),
            _ => {}
        }
    }

    #[test]
    fn the_schema_closes_every_object() {
        let mut objects = 0;
        walk(&schema(), "$", &mut objects);
        assert_eq!(objects, 2);
        assert_eq!(
            schema()["properties"]["actions"]["items"]["properties"]["kind"]["enum"],
            json!(ACTION_KINDS)
        );
    }

    #[test]
    fn the_instruction_names_every_kind_and_behavior() {
        for kind in ACTION_KINDS {
            assert!(INSTRUCTION.contains(kind), "{kind}");
        }
        for b in SurfaceBehavior::ALL {
            assert!(INSTRUCTION.contains(b.name()), "{}", b.name());
        }
        assert!(!INSTRUCTION.contains('\n'));
    }

    const TWO: &str = r#"{"actions":[{"kind":"behavior","target":"desk","value":"embers"},{"kind":"color","target":"desk","value":"amber"}],"say":"Embers on the desk, in amber"}"#;

    fn two() -> Vec<AgentAction> {
        vec![
            AgentAction {
                kind: "behavior".into(),
                target: "desk".into(),
                value: "embers".into(),
            },
            AgentAction {
                kind: "color".into(),
                target: "desk".into(),
                value: "amber".into(),
            },
        ]
    }

    #[test]
    fn the_anthropic_parser() {
        let p = anthropic();
        let body = json!({
            "id": "msg_x", "type": "message", "role": "assistant", "model": "claude-opus-5-5",
            "content": [
                { "type": "thinking", "thinking": "", "signature": "s" },
                { "type": "text", "text": TWO }
            ],
            "stop_reason": "end_turn", "stop_details": null,
            "usage": { "input_tokens": 1532, "output_tokens": 88, "cache_read_input_tokens": 1210, "cache_creation_input_tokens": 0 }
        })
        .to_string();
        let reply = p.parse(200, &body).unwrap();
        assert_eq!(reply.actions, two());
        assert_eq!(reply.say, "Embers on the desk, in amber");
        assert_eq!(
            reply.usage,
            Usage {
                input: Some(1532),
                output: Some(88),
                cache_read: Some(1210)
            }
        );
        assert_eq!(
            call_line(&p, 1840, &Ok(reply), "schema"),
            "voice agent: anthropic claude-opus-5-5 \u{b7} 1840 ms \u{b7} in 1532 cached 1210 out 88 \u{b7} 2 actions \u{b7} schema"
        );
        let refusal = json!({
            "content": [], "stop_reason": "refusal",
            "stop_details": { "type": "refusal", "category": "cyber", "explanation": "…" }
        })
        .to_string();
        assert_eq!(
            p.parse(200, &refusal),
            Err(AgentError::Refused("cyber".into()))
        );
        let bare = json!({ "content": [], "stop_reason": "refusal", "stop_details": { "category": null } }).to_string();
        assert_eq!(p.parse(200, &bare), Err(AgentError::Refused(String::new())));
        let cut = json!({
            "content": [{ "type": "text", "text": "{\"actions\": [{\"kind\"" }],
            "stop_reason": "max_tokens"
        })
        .to_string();
        assert_eq!(p.parse(200, &cut), Err(AgentError::Parse("cut off".into())));
        let err = r#"{"type":"error","error":{"type":"invalid_request_error","message":"output_config.format: bad"}}"#;
        assert_eq!(p.parse(400, err), Err(AgentError::Http(400, err.into())));
        assert!(matches!(
            p.parse(200, "{\"content\": []}"),
            Err(AgentError::Parse(_))
        ));
    }

    #[test]
    fn the_openai_parser() {
        let p = openai("", Map::new());
        let body = |content: &str, finish: &str| {
            json!({
                "id": "chatcmpl-x", "object": "chat.completion",
                "choices": [{ "index": 0, "message": { "role": "assistant", "content": content }, "finish_reason": finish }],
                "usage": { "prompt_tokens": 1410, "completion_tokens": 125, "total_tokens": 1535 }
            })
            .to_string()
        };
        let reply = p.parse(200, &body(TWO, "stop")).unwrap();
        assert_eq!(reply.actions, two());
        assert_eq!(
            reply.usage,
            Usage {
                input: Some(1410),
                output: Some(125),
                cache_read: None
            }
        );
        // A server that ignored the schema and wrapped the JSON in prose
        // and a fence: the first object is the answer.
        let wrapped =
            format!("Sure! Here is what I would do:\n```json\n{TWO}\n```\nEnjoy {{the}} fire.");
        assert_eq!(
            p.parse(200, &body(&wrapped, "stop")).unwrap().actions,
            two()
        );
        assert_eq!(
            p.parse(200, &body(TWO, "length")),
            Err(AgentError::Parse("cut off".into()))
        );
        assert_eq!(
            p.parse(200, &body("", "content_filter")),
            Err(AgentError::Refused(String::new()))
        );
        assert_eq!(
            p.parse(500, "oops"),
            Err(AgentError::Http(500, "oops".into()))
        );
        assert!(matches!(
            p.parse(200, &body("no json here", "stop")),
            Err(AgentError::Parse(_))
        ));
        // No usage (some local servers): `-` in the log.
        let bare =
            json!({ "choices": [{ "message": { "content": TWO }, "finish_reason": "stop" }] })
                .to_string();
        let reply = p.parse(200, &bare).unwrap();
        assert_eq!(reply.usage, Usage::default());
        assert_eq!(
            call_line(&p, 2500, &Ok(reply), "no schema (retried)"),
            "voice agent: openai qwen3.5 \u{b7} 2500 ms \u{b7} in - out - \u{b7} 2 actions \u{b7} no schema (retried)"
        );
        assert_eq!(
            call_line(&p, 12000, &Err(AgentError::Timeout), "schema"),
            "voice agent: openai qwen3.5 \u{b7} 12000 ms \u{b7} timed out \u{b7} schema"
        );
    }

    #[test]
    fn the_first_object_skips_strings_and_prose() {
        assert_eq!(
            first_object("x {\"a\": \"}{\"} y {}"),
            Some("{\"a\": \"}{\"}")
        );
        assert_eq!(
            first_object("{\"a\": \"\\\"}\", \"b\": {}} tail"),
            Some("{\"a\": \"\\\"}\", \"b\": {}}")
        );
        assert_eq!(first_object("{\"a\": 1"), None);
        assert_eq!(first_object("none"), None);
    }

    #[test]
    fn a_refused_format_is_retried_without_it() {
        let room = fixture_room("retry");
        let a = anthropic();
        let req = a.request(SENTENCE, &room);
        let err = r#"{"type":"error","error":{"type":"invalid_request_error","message":"output_config.format: Extra inputs are not permitted"}}"#;
        let again = a.without_format(&req, 400, err).unwrap();
        assert_eq!(again.body["output_config"], json!({ "effort": "low" }));
        assert_eq!(again.headers, req.headers);
        assert_eq!(a.without_format(&again, 400, err), None);
        assert_eq!(a.without_format(&req, 401, err), None);
        assert_eq!(
            a.without_format(
                &req,
                400,
                r#"{"error":{"message":"max_tokens: too large"}}"#
            ),
            None
        );
        let o = openai("", Map::new());
        let req = o.request(SENTENCE, &room);
        let err = r#"{"error":{"message":"response_format type json_schema is unavailable"}}"#;
        let again = o.without_format(&req, 400, err).unwrap();
        assert!(
            !again
                .body
                .as_object()
                .unwrap()
                .contains_key("response_format")
        );
        assert_eq!(again.body["messages"], req.body["messages"]);
        assert_eq!(o.without_format(&again, 400, err), None);
        assert_eq!(
            o.without_format(&req, 400, r#"{"error":"model not found"}"#),
            None
        );
    }

    #[test]
    fn the_error_labels() {
        for (e, want) in [
            (AgentError::NoNetwork("dns".into()), "No network for that"),
            (
                AgentError::Refused(String::new()),
                "I can't help with that one",
            ),
            (AgentError::Timeout, "Took too long"),
            (
                AgentError::Http(401, String::new()),
                "The agent didn't answer",
            ),
            (
                AgentError::Parse("cut off".into()),
                "The agent didn't answer",
            ),
        ] {
            assert_eq!(e.label(), want, "{e:?}");
        }
    }

    #[test]
    fn the_config_debug_hides_the_key() {
        let c = parse_config(&format!(r#"{{"api_key": "{KEY}"}}"#)).unwrap();
        let shown = format!("{c:?} {}", c.summary());
        assert!(
            !shown.contains(KEY) && shown.contains("api_key: \"set\""),
            "{shown}"
        );
    }

    fn act(kind: &str, target: &str, value: &str) -> AgentAction {
        AgentAction {
            kind: kind.into(),
            target: target.into(),
            value: value.into(),
        }
    }

    #[test]
    fn from_json_maps_each_kind_through_the_grammar() {
        let (e, boxes) = (effects(), room());
        let v = vocab(&e, &boxes, Some(0));
        for (a, want) in [
            (act("next_effect", "", ""), Intent::NextEffect),
            (act("prev_effect", "", ""), Intent::PrevEffect),
            (act("effect", "", "Flux Cloud"), Intent::Effect(0)),
            (act("effect", "", "flock"), Intent::Effect(2)),
            (act("edit_room", "", "on"), Intent::EditRoom(true)),
            (act("cloud", "", "off"), Intent::Cloud(false)),
            (act("particles", "", "on"), Intent::Cloud(true)),
            (act("pitcher", "", "on"), Intent::Pitcher(true)),
            (act("music", "", "off"), Intent::Music(false)),
            (act("rescan", "", ""), Intent::Rescan),
            (act("recenter", "", ""), Intent::Recenter),
            (act("all_none", "", ""), Intent::AllNone),
            (
                act("behavior", "desk", "embers"),
                Intent::Behavior {
                    target: Target::Surface(0),
                    behavior: B::Embers,
                },
            ),
            (
                act("behavior", "pointed", "pulse"),
                Intent::Behavior {
                    target: Target::Pointed,
                    behavior: B::Pulse,
                },
            ),
            (
                act("behavior", "", "curls"),
                Intent::Behavior {
                    target: Target::Pointed,
                    behavior: B::Curls,
                },
            ),
            // A kind word is the kind, even where the sentence would ask
            // which one.
            (
                act("behavior", "table", "pulse"),
                Intent::Behavior {
                    target: Target::Kind(KIND_TABLE),
                    behavior: B::Pulse,
                },
            ),
            (
                act("behavior", "wall", "aurora"),
                Intent::Behavior {
                    target: Target::Kind(KIND_WALL),
                    behavior: B::Aurora,
                },
            ),
            (
                act("behavior", "every wall", "none"),
                Intent::Behavior {
                    target: Target::Kind(KIND_WALL),
                    behavior: B::None,
                },
            ),
            (
                act("behavior", "Table 2", "Streamlines"),
                Intent::Behavior {
                    target: Target::Surface(2),
                    behavior: B::Streamlines,
                },
            ),
            (
                act("color", "lamp", "amber"),
                Intent::Color {
                    target: Target::Surface(7),
                    color: 4,
                },
            ),
            (
                act("color", "desk", "own"),
                Intent::Color {
                    target: Target::Surface(0),
                    color: 0,
                },
            ),
            (
                act("color", "wall", "warm white"),
                Intent::Color {
                    target: Target::Kind(KIND_WALL),
                    color: 3,
                },
            ),
            (
                act("color", "pointed", "key"),
                Intent::Color {
                    target: Target::Pointed,
                    color: 8,
                },
            ),
            (
                act("band", "wall 3", "bass"),
                Intent::Band {
                    target: Target::Surface(3),
                    band: 1,
                },
            ),
            (
                act("band", "floor", "rms"),
                Intent::Band {
                    target: Target::Surface(5),
                    band: 0,
                },
            ),
            (
                act("strength", "desk", "down"),
                Intent::Strength {
                    target: Target::Surface(0),
                    strength: Strength::Down,
                },
            ),
            (
                act("strength", "table", "half"),
                Intent::Strength {
                    target: Target::Kind(KIND_TABLE),
                    strength: Strength::Half,
                },
            ),
            (
                act("strength", "lamp", "off"),
                Intent::Strength {
                    target: Target::Surface(7),
                    strength: Strength::Off,
                },
            ),
            (
                act("strength", "", "full"),
                Intent::Strength {
                    target: Target::Pointed,
                    strength: Strength::Full,
                },
            ),
            (
                act("describe", "ceiling", ""),
                Intent::Describe(Target::Surface(6)),
            ),
            (
                act("describe", "pointed", ""),
                Intent::Describe(Target::Pointed),
            ),
        ] {
            assert_eq!(Intent::from_json(&a, &v), Ok(want), "{a}");
        }
    }

    #[test]
    fn from_json_misses_as_the_grammar_does() {
        let (e, boxes) = (effects(), room());
        let v = vocab(&e, &boxes, Some(0));
        let reason = |a: AgentAction| Intent::from_json(&a, &v).unwrap_err().reason;
        assert_eq!(
            reason(act("color", "shelf", "blue")),
            Reason::UnknownSurface("shelf".into())
        );
        assert_eq!(
            reason(act("behavior", "table 9", "pulse")),
            Reason::UnknownSurface("table 9".into())
        );
        assert_eq!(
            reason(act("behavior", "desk", "fire")),
            Reason::UnknownBehavior("fire".into())
        );
        assert_eq!(
            reason(act("behavior", "wall 3", "embers")),
            Reason::NotOnThisKind {
                behavior: B::Embers,
                kind: KIND_WALL
            }
        );
        assert_eq!(
            reason(act("behavior", "ceiling", "sparks")),
            Reason::NotOnThisKind {
                behavior: B::Sparks,
                kind: KIND_CEILING
            }
        );
        assert_eq!(reason(act("dance", "desk", "")), Reason::NoMatch);
        assert_eq!(reason(act("color", "desk", "orchid")), Reason::NoMatch);
        assert_eq!(reason(act("band", "desk", "treble clef")), Reason::NoMatch);
        assert_eq!(reason(act("effect", "", "Campfire")), Reason::NoMatch);
        assert_eq!(reason(act("music", "", "louder")), Reason::NoMatch);
        assert_eq!(
            Intent::from_json(&act("color", "desk", "orchid"), &v)
                .unwrap_err()
                .heard,
            "color desk orchid"
        );
        // Nothing pointed: the pointed target misses for the same reason
        // as "in amber" said with nothing pointed.
        let nothing = vocab(&e, &boxes, None);
        assert_eq!(
            Intent::from_json(&act("color", "pointed", "amber"), &nothing)
                .unwrap_err()
                .reason,
            Reason::NoSurface
        );
    }

    /// A reply with a color the room does not have between two good
    /// actions: the good ones apply, the label says one was skipped, and
    /// one log line names the dropped action.
    #[test]
    fn a_mixed_reply_applies_the_valid_actions_and_names_the_dropped_one() {
        let (e, boxes) = (effects(), room());
        let (mut lanes, dir) = lanes("mixed", &boxes);
        let v = vocab(&e, &boxes, Some(0));
        let body = json!({
            "content": [{ "type": "text", "text": json!({
                "actions": [
                    { "kind": "behavior", "target": "desk", "value": "embers" },
                    { "kind": "color", "target": "desk", "value": "orchid" },
                    { "kind": "strength", "target": "desk", "value": "half" }
                ],
                "say": "A campfire on the desk"
            }).to_string() }],
            "stop_reason": "end_turn",
            "usage": { "input_tokens": 1500, "output_tokens": 90 }
        })
        .to_string();
        let reply = anthropic().parse(200, &body).unwrap();
        let outcomes: Vec<Result<Intent, Miss>> = reply
            .actions
            .iter()
            .map(|a| Intent::from_json(a, &v))
            .collect();
        let mut applied = Vec::new();
        let mut missed = Vec::new();
        for o in &outcomes {
            match o {
                Ok(intent) => {
                    applied.push(crate::intent::apply(intent, &v, &mut lanes, &boxes).unwrap());
                }
                Err(m) => missed.push(m.clone()),
            }
        }
        assert_eq!(applied, ["desk: embers", "desk: half strength"]);
        assert_eq!(lanes.effective(0, &boxes).map(|(b, _)| b), Some(B::Embers));
        assert_close!(lanes.params_of(0, &boxes).2, 0.5);
        assert_eq!(lanes.params_of(0, &boxes).0, 0, "the color stays");
        assert_eq!(
            dropped_line(&outcomes).as_deref(),
            Some("voice agent: dropped 1 of 3: color desk orchid (NoMatch)")
        );
        assert_eq!(
            heard_line(SENTENCE, &outcomes, &reply.say),
            "voice agent: heard \"Something like a campfire on the desk.\" \u{2192} [Behavior { target: Surface(0), behavior: Embers }, miss NoMatch, Strength { target: Surface(0), strength: Half }] \u{b7} say \"A campfire on the desk\""
        );
        assert_eq!(
            label(&reply.say, &applied, &missed),
            (
                "desk: embers; desk: half strength; skipped 1".to_owned(),
                LABEL_S
            )
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Which grammar misses reach the agent: no template fits, or an
    /// unknown behavior in a template's shape; the rest are answers.
    #[test]
    fn nomatch_and_unknown_behavior_reach_the_agent_the_other_misses_stay() {
        for (reason, forwarded) in [
            (Reason::NoMatch, true),
            (Reason::UnknownBehavior("campfire".into()), true),
            (Reason::UnknownSurface("shelf".into()), false),
            (
                Reason::NotOnThisKind {
                    behavior: B::Embers,
                    kind: KIND_WALL,
                },
                false,
            ),
            (
                Reason::Ambiguous(vec!["table 1".into(), "table 2".into()]),
                false,
            ),
            (Reason::NoSurface, false),
        ] {
            assert_eq!(forwards(&reason), forwarded, "{reason:?}");
        }
        let (e, boxes) = (effects(), room());
        let v = vocab(&e, &boxes, None);
        for (sentence, forwarded) in [
            ("A campfire on the desk.", true),
            ("Fire on the desk.", true),
            ("Make the walls calmer.", true),
            ("The shelf in blue.", false),
            ("Embers on wall 3.", false),
            ("Table in amber.", false),
            ("In amber.", false),
        ] {
            let miss = crate::intent::parse(sentence, &v).unwrap_err();
            assert_eq!(
                forwards(&miss.reason),
                forwarded,
                "{sentence}: {:?}",
                miss.reason
            );
        }
    }

    /// The replayed room the reviewer runs the `say` knob on (as
    /// `intent.rs` builds it: 5 tables, 5 storage, 4 walls, a floor, a
    /// window frame, a ceiling, then the stage floor).
    fn replayed_room() -> Vec<LaneBox<'static>> {
        let at = |k: usize| -> (u32, &'static str) {
            match k {
                0 | 3 | 6 | 12 | 14 => (KIND_TABLE, "TABLE"),
                1 | 4 | 9 | 11 | 13 => (KIND_OTHER, "STORAGE"),
                2 | 5 | 15 | 16 => (KIND_WALL, "WALL_FACE"),
                7 => (KIND_FLOOR, "FLOOR"),
                8 => (crate::surfaces::KIND_FRAME, "WINDOW_FRAME"),
                _ => (KIND_CEILING, "CEILING"),
            }
        };
        let mut boxes: Vec<LaneBox<'static>> = (0..17)
            .map(|k| {
                let (kind, label) = at(k);
                lane(k as u8, kind, label)
            })
            .collect();
        boxes.push(LaneBox {
            uuid: crate::room_file::STAGE_FLOOR_UUID,
            kind: KIND_FLOOR,
            label: "",
        });
        boxes
    }

    /// The five sentences for the `say` knob: each misses the grammar as
    /// NoMatch (so it reaches the agent), and the reply a model is expected
    /// to give maps onto intents in the replayed room, nothing pointed.
    #[test]
    fn the_five_sentences_reach_the_agent_and_their_replies_map() {
        let e = effects();
        let boxes = replayed_room();
        let v = vocab(&e, &boxes, None);
        let names: Vec<&str> = v.surfaces.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "table 0",
                "storage 1",
                "wall 2",
                "table 3",
                "storage 4",
                "wall 5",
                "table 6",
                "floor 7",
                "window",
                "storage 9",
                "ceiling",
                "storage 11",
                "table 12",
                "storage 13",
                "table 14",
                "wall 15",
                "wall 16",
                "floor 17"
            ]
        );
        let floors = Target::Kind(KIND_FLOOR);
        let walls = Target::Kind(KIND_WALL);
        for (sentence, reply, want) in [
            (
                "Something like a campfire on table 14.",
                vec![
                    act("behavior", "table 14", "embers"),
                    act("color", "table 14", "amber"),
                ],
                vec![
                    Intent::Behavior {
                        target: Target::Surface(14),
                        behavior: B::Embers,
                    },
                    Intent::Color {
                        target: Target::Surface(14),
                        color: 4,
                    },
                ],
            ),
            (
                "Make the walls calmer.",
                vec![
                    act("behavior", "wall", "aurora"),
                    act("strength", "wall", "down"),
                ],
                vec![
                    Intent::Behavior {
                        target: walls,
                        behavior: B::Aurora,
                    },
                    Intent::Strength {
                        target: walls,
                        strength: Strength::Down,
                    },
                ],
            ),
            (
                "Less going on.",
                vec![act("cloud", "", "off"), act("behavior", "wall", "none")],
                vec![
                    Intent::Cloud(false),
                    Intent::Behavior {
                        target: walls,
                        behavior: B::None,
                    },
                ],
            ),
            (
                "Make the room feel like the ocean.",
                vec![
                    act("behavior", "floor", "rings"),
                    act("color", "floor", "teal"),
                    act("behavior", "wall", "aurora"),
                    act("color", "wall", "blue"),
                ],
                vec![
                    Intent::Behavior {
                        target: floors,
                        behavior: B::Rings,
                    },
                    Intent::Color {
                        target: floors,
                        color: 6,
                    },
                    Intent::Behavior {
                        target: walls,
                        behavior: B::Aurora,
                    },
                    Intent::Color {
                        target: walls,
                        color: 1,
                    },
                ],
            ),
            (
                "Give the ceiling a starry night.",
                vec![
                    act("behavior", "ceiling", "astrolabe"),
                    act("color", "ceiling", "violet"),
                ],
                vec![
                    Intent::Behavior {
                        target: Target::Surface(10),
                        behavior: B::Astrolabe,
                    },
                    Intent::Color {
                        target: Target::Surface(10),
                        color: 2,
                    },
                ],
            ),
        ] {
            assert_eq!(
                crate::intent::parse(sentence, &v).map_err(|m| m.reason),
                Err(Reason::NoMatch),
                "{sentence}"
            );
            let got: Vec<Intent> = reply
                .iter()
                .map(|a| Intent::from_json(a, &v).unwrap())
                .collect();
            assert_eq!(got, want, "{sentence}");
        }
    }

    #[test]
    fn the_label_for_a_reply() {
        let applied = ["desk: embers".to_owned(), "desk: amber".to_owned()];
        let miss = Miss {
            reason: Reason::UnknownSurface("shelf".into()),
            heard: "color shelf blue".into(),
        };
        assert_eq!(
            label("Campfire on the desk", &applied, &[]).0,
            "Campfire on the desk"
        );
        assert_eq!(label(" ", &applied, &[]).0, "desk: embers; desk: amber");
        assert_eq!(
            label("I can't play songs", &[], &[]).0,
            "I can't play songs"
        );
        assert_eq!(label("", &[], &[]).0, "The agent didn't answer");
        assert_eq!(
            label("Blue shelf", &[], std::slice::from_ref(&miss)).0,
            "No surface called \"shelf\""
        );
        assert_eq!(
            label("x", &applied[..1], &[miss]).0,
            "desk: embers; skipped 1"
        );
        let long = "a".repeat(100);
        let (cut, s) = label(&long, &[], &[]);
        assert_eq!((cut.chars().count(), s), (MAX_SAY_CHARS, LABEL_S));
        assert!(cut.ends_with('\u{2026}'));
        assert_eq!(dropped_line(&[Ok(Intent::Rescan)]), None);
    }
}
