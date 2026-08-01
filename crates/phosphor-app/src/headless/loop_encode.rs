//! Loop encode (#2063, P2.5): pipe the driver's raw RGBA frames into an ffmpeg
//! subprocess, VJ-standard codecs first. Same philosophy as the recording
//! encoder: ffmpeg is a hard PATH dependency, spawned per render, never
//! bundled or linked.
//!
//! Alpha note (INV-A / docs/alpha.md): frames arrive PREMULTIPLIED. Until the
//! P2.6 Resolume verdict lands, both HAP Alpha and ProRes 4444 encode the
//! premultiplied bytes as-is; the verdict fixes any per-codec conversion here,
//! as a non-optional step — no user toggle.

use std::io::Write;
use std::path::Path;
use std::process::{Child, Command, Stdio};

use crate::headless::loop_spec::{LoopCodec, LoopSpec};

/// Does the local ffmpeg have the encoder this codec needs? Returns the
/// encoder name on success; an actionable error otherwise.
pub fn probe_encoder(codec: LoopCodec) -> Result<&'static str, String> {
    let (encoder, hint) = match codec {
        LoopCodec::Hap | LoopCodec::HapAlpha => ("hap", "a full ffmpeg build with --enable-snappy"),
        LoopCodec::Prores4444 => ("prores_ks", "any mainline ffmpeg"),
        LoopCodec::H264 => ("libx264", "an ffmpeg build with libx264"),
        LoopCodec::Hevc => ("libx265", "an ffmpeg build with libx265"),
    };
    let out = Command::new("ffmpeg")
        .args(["-hide_banner", "-encoders"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .map_err(|e| format!("ffmpeg not found on PATH: {e}"))?;
    let list = String::from_utf8_lossy(&out.stdout);
    if list.lines().any(|l| {
        let mut cols = l.split_whitespace();
        cols.next().is_some() && cols.next() == Some(encoder)
    }) {
        Ok(encoder)
    } else {
        Err(format!(
            "ffmpeg has no '{encoder}' encoder (needed for {codec:?}); install {hint}"
        ))
    }
}

/// Spawn ffmpeg reading raw RGBA frames on stdin for `spec`, writing `out`.
pub fn spawn(spec: &LoopSpec, fps_exact: u32, out: &Path) -> Result<Child, String> {
    let encoder = probe_encoder(spec.codec)?;
    let [w, h] = spec.resolution;

    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-y", "-hide_banner", "-loglevel", "error"]);
    cmd.args([
        "-f",
        "rawvideo",
        "-pix_fmt",
        "rgba",
        "-s",
        &format!("{w}x{h}"),
        "-r",
        &fps_exact.to_string(),
        "-i",
        "pipe:0",
    ]);
    cmd.args(["-c:v", encoder]);
    match spec.codec {
        LoopCodec::Hap => {
            cmd.args(["-format", "hap", "-chunks", "8"]);
        }
        LoopCodec::HapAlpha => {
            cmd.args(["-format", "hap_alpha", "-chunks", "8"]);
        }
        LoopCodec::Prores4444 => {
            cmd.args(["-profile:v", "4444", "-pix_fmt", "yuva444p10le"]);
        }
        LoopCodec::H264 | LoopCodec::Hevc => {
            cmd.args(["-crf", "18", "-preset", "medium", "-pix_fmt", "yuv420p"]);
            cmd.args(["-movflags", "+faststart"]);
        }
    }
    cmd.arg(out.as_os_str());
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    cmd.spawn().map_err(|e| format!("spawning ffmpeg: {e}"))
}

/// Render `spec` and encode it to `out` in one pass. Returns the effective
/// timing for the CLI to report.
pub fn render_and_encode(
    spec: &LoopSpec,
    out: &Path,
) -> Result<crate::headless::loop_spec::LoopTiming, String> {
    spec.snap()?; // fail fast before spawning ffmpeg
    let mut child = spawn(spec, spec.fps, out)?;
    let mut stdin = child.stdin.take().expect("piped stdin");

    let result = crate::headless::loop_driver::render_loop(spec, |frame, rgba| {
        stdin
            .write_all(rgba)
            .map_err(|e| format!("ffmpeg stdin closed at frame {frame}: {e}"))
    });
    drop(stdin);
    let status = child
        .wait()
        .map_err(|e| format!("waiting for ffmpeg: {e}"))?;
    // Surface the render error first (it likely caused the encoder exit too).
    let timing = result?;
    if !status.success() {
        let mut err = String::new();
        if let Some(mut stderr) = child.stderr.take() {
            use std::io::Read;
            let _ = stderr.read_to_string(&mut err);
        }
        return Err(format!(
            "ffmpeg exited with {status}: {}",
            err.lines().last().unwrap_or("(no stderr)")
        ));
    }
    Ok(timing)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::headless::loop_spec::LoopSpec;

    fn ffmpeg_present() -> bool {
        Command::new("ffmpeg")
            .arg("-version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok()
    }

    fn ffprobe(path: &Path, entries: &str) -> String {
        let out = Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-show_entries",
                entries,
                "-of",
                "default=nw=1",
            ])
            .arg(path)
            .output()
            .expect("ffprobe runs");
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// The encode pipeline without a GPU: synthetic raw frames piped through
    /// the real spawn() for each codec, ffprobe-verified container shape.
    /// Runs in CI (ffmpeg installed per decision #2066); skips gracefully on
    /// dev machines without ffmpeg — the flags are what CI pins.
    #[test]
    fn encode_pipeline_produces_probeable_files() {
        if !ffmpeg_present() {
            eprintln!("skipping: ffmpeg not on PATH (CI runs this)");
            return;
        }
        let dir = std::env::temp_dir().join("fosfora-loop-encode-test");
        let _ = std::fs::create_dir_all(&dir);

        for (codec, want_pix) in [
            (LoopCodec::HapAlpha, "rgba"),
            (LoopCodec::Hap, "rgb0"),
            (LoopCodec::Prores4444, "yuva444p"), // 10/12-bit varies by build; alpha 4:4:4:4 is the contract
            (LoopCodec::H264, "yuv420p"),
        ] {
            let spec = LoopSpec {
                version: 1,
                effect: "test".into(),
                params: Default::default(),
                bpm: 120.0,
                bars: 1,
                fps: 30,
                resolution: [128, 72],
                codec,
                audio: Default::default(),
                audio_file: None,
                background: Default::default(),
            };
            let out = dir.join(format!("t.{}", codec.extension()));
            let mut child = match spawn(&spec, 30, &out) {
                Ok(c) => c,
                Err(e) if e.contains("no '") => {
                    // Encoder missing from this ffmpeg build (e.g. no snappy):
                    // the probe's actionable error IS the tested behavior.
                    eprintln!("skipping {codec:?}: {e}");
                    continue;
                }
                Err(e) => panic!("{codec:?}: {e}"),
            };
            let mut stdin = child.stdin.take().unwrap();
            let frame: Vec<u8> = (0..128 * 72 * 4).map(|i| ((i * 7) % 251) as u8).collect();
            for _ in 0..12 {
                stdin.write_all(&frame).unwrap();
            }
            drop(stdin);
            assert!(child.wait().unwrap().success(), "{codec:?} encode failed");

            let probe = ffprobe(&out, "stream=codec_name,pix_fmt,nb_frames,width");
            assert!(probe.contains("width=128"), "{codec:?}: {probe}");
            assert!(probe.contains("nb_frames=12"), "{codec:?}: {probe}");
            assert!(
                probe.contains(&format!("pix_fmt={want_pix}")),
                "{codec:?}: expected {want_pix} in {probe}"
            );
        }
    }
}
