//! Repository chores, run as `cargo xtask <command>`.
//!
//! Currently one command: `new-effect`, which writes a trama effect file that
//! already compiles, already has a modulatable parameter, and already does
//! something visible. The app watches `assets/trama/effects/` and treats a
//! path it has never seen as a new effect, so the file appears in a running
//! app's node palette without a restart.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("new-effect") => new_effect(&args[1..]),
        Some("--help" | "-h") | None => {
            print!("{USAGE}");
            return std::process::ExitCode::SUCCESS;
        }
        Some(other) => Err(format!("unknown command `{other}`\n\n{USAGE}")),
    };
    match result {
        Ok(message) => {
            print!("{message}");
            std::process::ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

const USAGE: &str = "\
cargo xtask <command>

Commands:
  new-effect <id> [options]   Scaffold a trama effect in assets/trama/effects/

new-effect options:
  --name <text>    Display name in the node palette (default: the id, title-cased)
  --source         A source node: generates a picture, takes no input
  --inputs <n>     1 or 2 inputs (default 1; ignored with --source)
  --force          Overwrite the file if it already exists
";

struct Options {
    id: String,
    name: String,
    inputs: u8,
    source: bool,
    force: bool,
}

fn parse(args: &[String]) -> Result<Options, String> {
    let mut id = None;
    let mut name = None;
    let mut inputs = None;
    let mut source = false;
    let mut force = false;

    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut value = |flag: &str| {
            it.next()
                .cloned()
                .ok_or_else(|| format!("`{flag}` needs a value"))
        };
        match arg.as_str() {
            "--source" => source = true,
            "--force" => force = true,
            "--name" => name = Some(value("--name")?),
            "--inputs" => {
                let raw = value("--inputs")?;
                inputs = Some(
                    raw.parse::<u8>()
                        .map_err(|_| format!("`--inputs` takes 1 or 2, not `{raw}`"))?,
                );
            }
            other if other.starts_with('-') => return Err(format!("unknown option `{other}`")),
            other if id.is_none() => id = Some(other.to_string()),
            other => return Err(format!("unexpected argument `{other}`")),
        }
    }

    let id = id.ok_or_else(|| format!("new-effect needs an id\n\n{USAGE}"))?;
    // The id is the file stem AND the registry key AND what a saved chain
    // names, so a manifest whose id does not match its stem is a load error.
    // Catch the spelling here rather than at the app's next launch.
    if id.is_empty()
        || !id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        return Err(format!(
            "id `{id}` must be lowercase ascii, digits and underscores: it is the file name, \
             the registry key, and what a saved chain refers to"
        ));
    }

    let inputs = if source {
        if inputs.is_some_and(|n| n != 0) {
            return Err("a source takes no input: drop `--inputs` or `--source`".into());
        }
        0
    } else {
        let n = inputs.unwrap_or(1);
        if n != 1 && n != 2 {
            return Err(format!("an effect takes 1 or 2 inputs, not {n}"));
        }
        n
    };

    let name = name.unwrap_or_else(|| title_case(&id));
    Ok(Options {
        id,
        name,
        inputs,
        source,
        force,
    })
}

fn title_case(id: &str) -> String {
    id.split('_')
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + c.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The repository root: the directory holding this crate's parent workspace.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf()
}

fn new_effect(args: &[String]) -> Result<String, String> {
    let o = parse(args)?;
    let dir = repo_root().join("assets/trama/effects");
    if !dir.is_dir() {
        return Err(format!("{} is not a directory", dir.display()));
    }
    let path = dir.join(format!("{}.wgsl", o.id));
    if path.exists() && !o.force {
        return Err(format!(
            "{} already exists (pass --force to overwrite it)",
            path.display()
        ));
    }

    std::fs::write(&path, template(&o)).map_err(|e| format!("{}: {e}", path.display()))?;

    let mut out = String::new();
    let _ = writeln!(out, "wrote {}", path.display());
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "The app watches that directory, so if it is running the node is in the palette \
         already — no restart. Select a layer, press G, and add \"{}\".",
        o.name
    );
    let _ = writeln!(
        out,
        "Every Float parameter modulates: open the mod row under its slider."
    );
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "If the file does not compile the app keeps the last good version and puts the \
         compiler's text in the node inspector under a `· ERROR` title."
    );
    Ok(out)
}

fn template(o: &Options) -> String {
    let Options {
        id, name, inputs, ..
    } = o;
    let kind = if o.source { "source" } else { "effect" };

    // Each shape gets a body that is already worth looking at: a scaffold that
    // renders an unchanged picture cannot tell you whether you wired it up
    // right or wrote a no-op.
    let (params, rates, doc, body) = if o.source {
        (
            r#"    { "type": "Float", "name": "scale", "default": 4.0, "min": 0.5, "max": 24.0 },
    { "type": "Float", "name": "speed", "default": 0.4, "min": -4.0, "max": 4.0 },
    { "type": "Color", "name": "tint",  "default": [1.0, 0.6, 0.2, 1.0] }"#,
            ",\n  \"rates\": [\"speed\"]",
            "\
// `speed` is a RATE (see \"rates\" above): param(1u) arrives as its running
// integral, the distance travelled so far, NOT as the slider value. Never
// write `u.time * speed` — every change in speed would be multiplied by the
// app's uptime, and a modulated speed strobes.
//
// A Color takes four scalar slots, so `tint` is param(2u)..param(5u).",
            "\
    let res = u.resolution;
    let uv = frag_coord.xy / res;
    let p = (uv - 0.5) * vec2f(res.x / res.y, 1.0);

    // Concentric rings travelling outward. Replace all of this.
    let rings = 0.5 + 0.5 * cos((length(p) * param(0u) - param(1u)) * 6.2831853);
    let a = param(5u) * rings;
    return vec4f(vec3f(param(2u), param(3u), param(4u)) * a, a);",
        )
    } else if *inputs == 2 {
        (
            r#"    { "type": "Float", "name": "amount", "default": 0.5, "min": 0.0, "max": 1.0 }"#,
            "",
            "\
// An unwired input reads as transparent black (the executor binds a 1x1
// placeholder), so a half-patched node fades out rather than failing.",
            "\
    let uv = frag_coord.xy / u.resolution;
    // Replace this: input0 and input1 are both premultiplied, so a weighted
    // sum of them is exactly the right operation.
    return mix(input0(uv), input1(uv), param(0u));",
        )
    } else {
        (
            r#"    { "type": "Float", "name": "amount", "default": 0.5, "min": 0.0, "max": 1.0 }"#,
            "",
            "\
// `amount` defaults to half rather than to 1.0 on purpose: a scaffold that
// renders its input unchanged cannot tell you whether you wired the node in
// correctly or wrote a no-op. Adding this node should visibly do something.
//
// Color here is PREMULTIPLIED (INV-A, docs/alpha.md): RGB is already scaled by
// coverage, and RGB > A is legal and means additive light. Scaling the whole
// vec4f keeps that true. A non-linear tone curve does not — for those, divide
// coverage back out first and multiply it in again, the way levels.wgsl does.",
            "\
    let uv = frag_coord.xy / u.resolution;
    let c = input0(uv);

    // Replace this: a plain fade, so you can see the node is in the chain and
    // that its slider reaches the GPU.
    return c * param(0u);",
        )
    };

    format!(
        r#"/*! trama
{{
  "name": "{name}",
  "id": "{id}",
  "kind": "{kind}",
  "inputs": {inputs},
  "params": [
{params}
  ]{rates}
}}
*/
// {name} — one sentence on what a user sees, then one on what the parameters
// are for. This comment is the only documentation the next person gets.
//
{doc}

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {{
{body}
}}
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(args: &[&str]) -> Result<Options, String> {
        parse(&args.iter().map(|s| (*s).to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn id_becomes_a_display_name_unless_one_is_given() {
        assert_eq!(title_case("chromatic_aberration"), "Chromatic Aberration");
        assert_eq!(title_case("key"), "Key");
        assert_eq!(opts(&["my_effect"]).unwrap().name, "My Effect");
        assert_eq!(opts(&["x", "--name", "Fancy"]).unwrap().name, "Fancy");
    }

    // The id is the file stem, the registry key and what a saved chain names.
    // A mismatch is a load error at the app's next launch, which is a long way
    // from here.
    #[test]
    fn a_bad_id_is_refused_here_rather_than_at_load() {
        for bad in ["My Effect", "my-effect", "MyEffect", "my.effect", ""] {
            assert!(opts(&[bad]).is_err(), "`{bad}` should be refused");
        }
        assert!(opts(&["my_effect2"]).is_ok());
    }

    #[test]
    fn input_arity_matches_what_the_manifest_allows() {
        assert_eq!(opts(&["x"]).unwrap().inputs, 1);
        assert_eq!(opts(&["x", "--inputs", "2"]).unwrap().inputs, 2);
        assert_eq!(opts(&["x", "--source"]).unwrap().inputs, 0);
        // The manifest rejects these, so the scaffold has to as well.
        assert!(opts(&["x", "--inputs", "0"]).is_err());
        assert!(opts(&["x", "--inputs", "3"]).is_err());
        assert!(opts(&["x", "--source", "--inputs", "1"]).is_err());
    }

    // Every template has to satisfy the same manifest rules the app enforces:
    // a param count that fits the slots it indexes, `rates` naming a real
    // Float, and an id matching the file stem. The app-side parser is the real
    // check; this one catches the shapes before the file is ever written.
    #[test]
    fn every_template_shape_has_a_well_formed_manifest() {
        for args in [
            vec!["demo"],
            vec!["demo", "--inputs", "2"],
            vec!["demo", "--source"],
        ] {
            let o = opts(&args).unwrap();
            let text = template(&o);
            assert!(text.starts_with("/*! trama\n"), "{args:?}: manifest header");
            let json = text
                .split_once("/*! trama\n")
                .and_then(|(_, rest)| rest.split_once("\n*/"))
                .expect("manifest closes")
                .0;
            assert!(json.contains(r#""id": "demo""#), "{args:?}: id");
            assert!(
                json.contains(&format!(r#""inputs": {}"#, o.inputs)),
                "{args:?}: inputs"
            );
            // A source declares a rate, so its template must name a Float it
            // actually has — `rates` pointing at nothing is a load error.
            if json.contains("\"rates\"") {
                assert!(json.contains(r#""name": "speed""#), "{args:?}: rate exists");
            }
            assert!(
                text.contains("@fragment\nfn fs_main("),
                "{args:?}: entry point"
            );
            // The rule the whole rates mechanism exists to prevent.
            for line in text.lines() {
                let code = line.split("//").next().unwrap_or("");
                assert!(
                    !(code.contains("u.time") && code.contains("param(")),
                    "{args:?}: template multiplies a param by absolute time:\n{line}"
                );
            }
        }
    }
}
