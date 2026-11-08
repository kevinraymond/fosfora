//! Embedded clojurust nREPL (`repl` feature).
//!
//! Set `FOSFORA_NREPL=<port>` (`0` lets the OS pick) and fosfora starts a
//! clojurust interpreter on its own thread with an nREPL server on
//! `127.0.0.1:<port>`, writing `.nrepl-port` in the working directory so
//! CIDER, Calva and Conjure can auto-connect.
//!
//! The interpreter never touches render state. Every command it issues is a
//! [`WsInMessage`], the same vocabulary the web remote speaks, pushed into a
//! queue that [`crate::web::WebSystem::update`] drains on the render thread.
//! Reads go the other way through snapshots the web system already builds:
//! the full state JSON (10 Hz) and the audio JSON (10 Hz).
//!
//! clojurust values are not `Send`, so the runtime is built, used and dropped
//! on the `cljrs-repl` thread alone. Its GC heap is per-thread, so a
//! collection pauses that thread only, never rendering.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use cljrs_gc::GcPtr;
use cljrs_reader::{Form, FormKind, Parser};
use cljrs_runtime::tiered::{Env, GlobalEnv};
use cljrs_value::{Arity, NativeFn, Value, ValueError, ValueResult};

use crate::inbound::DropOldestSender;
use crate::web::types::WsInMessage;

mod media;

/// Namespace of the raw natives. User code goes through `fosfora`.
const NATIVE_NS: &str = "fosfora.native";

/// The `fosfora` namespace: the friendly API over the natives.
const FOSFORA_SRC: &str = include_str!("fosfora.cljrs");

/// What the interpreter thread shares with the render thread.
pub(crate) struct Bridge {
    /// Commands into the render thread (drained by `WebSystem::update`).
    pub(crate) commands: DropOldestSender<WsInMessage>,
    /// Latest full-state JSON, as the web remote receives it.
    pub(crate) state: Arc<Mutex<String>>,
    /// Latest audio-feature JSON, as the web remote receives it.
    pub(crate) audio: Arc<Mutex<String>>,
}

/// A running embedded nREPL. Dropping it stops the server and joins the
/// interpreter thread.
pub struct Repl {
    port: u16,
    shutdown: cljrs_nrepl::ShutdownHandle,
    thread: Option<JoinHandle<()>>,
}

impl Repl {
    pub fn port(&self) -> u16 {
        self.port
    }
}

impl Drop for Repl {
    fn drop(&mut self) {
        self.shutdown.shutdown();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// The port requested by `FOSFORA_NREPL`, if the embedded nREPL is wanted.
pub fn requested_port() -> Option<u16> {
    let raw = std::env::var("FOSFORA_NREPL").ok()?;
    let raw = raw.trim();
    if raw.is_empty() {
        return Some(0);
    }
    match raw.parse() {
        Ok(port) => Some(port),
        Err(_) => {
            log::warn!("FOSFORA_NREPL={raw:?} is not a port number; nREPL not started");
            None
        }
    }
}

/// Start the interpreter thread and its nREPL server on `127.0.0.1:port`.
/// Blocks until the server is listening (or failed to boot).
pub(crate) fn start(port: u16, bridge: Bridge) -> anyhow::Result<Repl> {
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let thread = std::thread::Builder::new()
        .name("cljrs-repl".into())
        // The tree-walking interpreter recurses deeply; the cljrs CLI uses 64 MB too.
        .stack_size(64 << 20)
        .spawn(move || match boot(port, bridge) {
            Ok(mut poller) => {
                let _ = ready_tx.send(Ok((poller.port(), poller.shutdown_handle())));
                let mut eval = cljrs_runtime::tiered::eval;
                while poller.poll_timeout(&mut eval, Duration::from_millis(50))
                    == cljrs_nrepl::PollStatus::Open
                {}
            }
            Err(e) => {
                let _ = ready_tx.send(Err(e));
            }
        })?;
    let (port, shutdown) = ready_rx
        .recv()
        .map_err(|_| anyhow::anyhow!("cljrs-repl thread exited during boot"))??;
    log::info!("nREPL server started on port {port} on host 127.0.0.1 - nrepl://127.0.0.1:{port}");
    Ok(Repl {
        port,
        shutdown,
        thread: Some(thread),
    })
}

/// Build the runtime, register the natives, preload `fosfora` into `user`,
/// and bind the nREPL listener. Runs on the interpreter thread.
fn boot(port: u16, bridge: Bridge) -> anyhow::Result<cljrs_nrepl::Poller> {
    let runtime = cljrs_runtime::Runtime::builder()
        .builtin_source("fosfora", FOSFORA_SRC)
        .build()
        .map_err(|e| anyhow::anyhow!("cljrs runtime: {e:?}"))?;
    cljrs_stdlib::install(&runtime);
    let globals = runtime.into_globals();
    register_natives(&globals, bridge);

    let mut env = Env::new(globals.clone(), "user");
    eval_str(&mut env, "(require '[fosfora :as f])")?;

    let config = cljrs_nrepl::Config {
        addr: ([127, 0, 0, 1], port).into(),
        port_file: Some(PathBuf::from(".nrepl-port")),
    };
    let server =
        cljrs_nrepl::start(config, globals).map_err(|e| anyhow::anyhow!("nREPL: {e:?}"))?;
    Ok(server.into_poller())
}

fn eval_str(env: &mut Env, src: &str) -> anyhow::Result<()> {
    let forms = Parser::new(src.to_string(), "<fosfora-boot>".to_string())
        .parse_all()
        .map_err(|e| anyhow::anyhow!("parse {src}: {e:?}"))?;
    for form in &forms {
        cljrs_runtime::tiered::eval(form, env).map_err(|e| anyhow::anyhow!("eval {src}: {e:?}"))?;
    }
    Ok(())
}

fn def_native(
    globals: &GlobalEnv,
    name: &'static str,
    arity: Arity,
    f: impl Fn(&[Value]) -> ValueResult<Value> + 'static,
) {
    let native = NativeFn::with_closure(format!("{NATIVE_NS}/{name}"), arity, f);
    globals.intern(
        NATIVE_NS,
        Arc::from(name),
        Value::NativeFunction(GcPtr::new(native)),
    );
}

fn str_arg(args: &[Value], i: usize) -> ValueResult<String> {
    match args.get(i) {
        Some(Value::Str(s)) => Ok(s.get().clone()),
        Some(other) => Err(ValueError::WrongType {
            expected: "string",
            got: other.type_name().to_string(),
        }),
        None => Err(ValueError::Other(format!("missing argument {i}"))),
    }
}

fn str_value(s: String) -> Value {
    Value::Str(GcPtr::new(s))
}

fn register_natives(globals: &GlobalEnv, bridge: Bridge) {
    let Bridge {
        commands,
        state,
        audio,
    } = bridge;
    media::register(globals, commands.clone());

    // (send-edn "{:type \"set_param\" ...}") queues one web-remote message.
    def_native(globals, "send-edn", Arity::Fixed(1), move |args| {
        let edn = str_arg(args, 0)?;
        let json = edn_to_json(&edn).map_err(ValueError::Other)?;
        let text = json.to_string();
        match crate::web::client::parse_client_message(&text) {
            Some(msg) => {
                commands.send(msg);
                Ok(Value::Bool(true))
            }
            None => Err(ValueError::Other(format!(
                "fosfora: not a message the remote understands: {text}"
            ))),
        }
    });

    // (state-edn) / (audio-edn): the latest snapshots, as EDN text.
    def_native(globals, "state-edn", Arity::Fixed(0), move |_| {
        Ok(str_value(snapshot_edn(&state)))
    });
    def_native(globals, "audio-edn", Arity::Fixed(0), move |_| {
        Ok(str_value(snapshot_edn(&audio)))
    });

    // (trama-dir): where trama effect files live (hot-reloaded on write).
    def_native(globals, "trama-dir", Arity::Fixed(0), |_| {
        Ok(str_value(
            crate::trama::effect::trama_effects_dir()
                .display()
                .to_string(),
        ))
    });

    // (write-trama! "id" "wgsl source") writes <trama-dir>/<id>.wgsl. The
    // watcher compiles it on the render thread; a broken file keeps the last
    // good version.
    def_native(globals, "write-trama!", Arity::Fixed(2), |args| {
        let id = str_arg(args, 0)?;
        let src = str_arg(args, 1)?;
        if id.is_empty()
            || !id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        {
            return Err(ValueError::Other(format!(
                "trama id must be [a-z0-9_]+, got {id:?}"
            )));
        }
        let path = crate::trama::effect::trama_effects_dir().join(format!("{id}.wgsl"));
        std::fs::write(&path, src)
            .map_err(|e| ValueError::Other(format!("write {}: {e}", path.display())))?;
        Ok(str_value(path.display().to_string()))
    });

    globals.mark_loaded(NATIVE_NS);
}

fn snapshot_edn(snapshot: &Mutex<String>) -> String {
    let json = snapshot.lock().map(|s| s.clone()).unwrap_or_default();
    if json.is_empty() {
        return "nil".to_string();
    }
    match serde_json::from_str::<serde_json::Value>(&json) {
        Ok(v) => {
            let mut out = String::with_capacity(json.len());
            json_to_edn(&v, &mut out);
            out
        }
        Err(_) => "nil".to_string(),
    }
}

// ── EDN <-> JSON ────────────────────────────────────────────────────────────

/// Read one EDN form (as `pr-str` prints a map) into JSON. Keywords become
/// strings without the colon, so `{:type "trigger"}` is `{"type":"trigger"}`.
fn edn_to_json(edn: &str) -> Result<serde_json::Value, String> {
    let forms = Parser::new(edn.to_string(), "<fosfora-send>".to_string())
        .parse_all()
        .map_err(|e| format!("bad EDN: {e:?}"))?;
    match forms.as_slice() {
        [form] => form_to_json(form),
        _ => Err(format!("expected one form, got {}", forms.len())),
    }
}

fn form_to_json(form: &Form) -> Result<serde_json::Value, String> {
    use serde_json::Value as J;
    Ok(match &form.kind {
        FormKind::Nil => J::Null,
        FormKind::Bool(b) => J::Bool(*b),
        FormKind::Int(i) => J::from(*i),
        FormKind::Float(f) => serde_json::Number::from_f64(*f)
            .map(J::Number)
            .ok_or_else(|| format!("non-finite number {f}"))?,
        FormKind::Str(s) => J::String(s.clone()),
        FormKind::Keyword(k) | FormKind::Symbol(k) => {
            J::String(k.trim_start_matches(':').to_string())
        }
        FormKind::Vector(items) | FormKind::List(items) | FormKind::Set(items) => J::Array(
            items
                .iter()
                .map(form_to_json)
                .collect::<Result<Vec<_>, _>>()?,
        ),
        FormKind::Map(kv) => {
            let mut obj = serde_json::Map::new();
            for pair in kv.chunks(2) {
                let key = match form_to_json(&pair[0])? {
                    J::String(s) => s,
                    other => other.to_string(),
                };
                obj.insert(key, form_to_json(&pair[1])?);
            }
            J::Object(obj)
        }
        other => return Err(format!("unsupported EDN form {other:?}")),
    })
}

/// Write JSON as EDN. Object keys that are valid keyword names become
/// keywords (`"bpm"` is `:bpm`); anything else stays a string key.
fn json_to_edn(v: &serde_json::Value, out: &mut String) {
    use serde_json::Value as J;
    match v {
        J::Null => out.push_str("nil"),
        J::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        J::Number(n) => out.push_str(&n.to_string()),
        J::String(s) => push_edn_str(s, out),
        J::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(' ');
                }
                json_to_edn(item, out);
            }
            out.push(']');
        }
        J::Object(map) => {
            out.push('{');
            for (i, (k, item)) in map.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                if is_keyword_name(k) {
                    out.push(':');
                    out.push_str(k);
                } else {
                    push_edn_str(k, out);
                }
                out.push(' ');
                json_to_edn(item, out);
            }
            out.push('}');
        }
    }
}

fn is_keyword_name(s: &str) -> bool {
    let mut chars = s.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || "*+!-_?".contains(c))
        && chars.all(|c| c.is_ascii_alphanumeric() || "*+!-_?.".contains(c))
}

fn push_edn_str(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edn_map_becomes_the_json_the_remote_parses() {
        let json = edn_to_json(r#"{:type "set_param", :name "speed", :value 0.5}"#).unwrap();
        assert_eq!(json["type"], "set_param");
        assert_eq!(json["name"], "speed");
        assert_eq!(json["value"], 0.5);
        let msg = crate::web::client::parse_client_message(&json.to_string());
        assert!(matches!(msg, Some(WsInMessage::SetParam { ref name, .. }) if name == "speed"));
    }

    #[test]
    fn json_snapshot_reads_back_as_edn_keywords() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"type":"audio","bpm":120.5,"bands":[0.1,0.2],"odd key":"a\"b"}"#,
        )
        .unwrap();
        let mut out = String::new();
        json_to_edn(&v, &mut out);
        assert!(out.contains(":bpm 120.5"), "{out}");
        assert!(out.contains(":bands [0.1 0.2]"), "{out}");
        assert!(out.contains(r#""odd key" "a\"b""#), "{out}");
    }
}
