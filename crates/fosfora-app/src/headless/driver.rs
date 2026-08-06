//! `--render-scene`: analyse the song, then render the scene against its own
//! feature stream, writing stills and audio-muxed clips (#2027).
//!
//! Two passes over one decode. Pass 1 is `analyze::drive` → structure, which
//! decides the capture schedule; pass 2 is `analyze::drive_with` feeding
//! [`SceneRenderer::step`] hop by hop. The canonical tick is the hop rate
//! (512 / sample_rate ≈ 11.6 ms): binding `smooth` is a per-frame EMA and
//! `burst_on_beat` is level-triggered, so hop-rate ticking keeps beat pulses
//! one frame wide and smoothing in the live regime. Every hop renders (feedback
//! and particle state must evolve continuously); only scheduled hops capture.
//!
//! Clips stream to an ffmpeg child muxed with the exact song slice — the
//! artifact a human judges — rather than exploding into PNG-per-frame.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use anyhow::{Context, Result, bail};

use crate::analyze::structure_offline;
use crate::audio::ANALYSIS_HOP;
use crate::headless::scene_renderer::SceneRenderer;
use crate::headless::schedule::{Capture, SamplingSchedule, ScheduleCfg};
use crate::settings::ParticleQuality;

pub struct RenderSceneArgs {
    pub scene_dir: PathBuf,
    pub song: PathBuf,
    pub out: PathBuf,
    pub width: u32,
    pub height: u32,
    pub quality: ParticleQuality,
    pub window_secs: f64,
}

struct ClipEncoder {
    child: Child,
    path: PathBuf,
}

impl ClipEncoder {
    /// Raw RGBA frames at the hop rate in on stdin; H.264 at 30 fps muxed with
    /// the matching slice of the song out. `-shortest` ends the clip at
    /// whichever stream runs out first.
    fn spawn(
        out_path: &Path,
        song: &Path,
        start_secs: f64,
        width: u32,
        height: u32,
        fps_in: f64,
    ) -> Result<Self> {
        let child = Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-y"])
            .args(["-f", "rawvideo", "-pix_fmt", "rgba"])
            .args(["-s", &format!("{width}x{height}")])
            .args(["-r", &format!("{fps_in:.6}")])
            .args(["-i", "pipe:0"])
            .args(["-ss", &format!("{start_secs:.3}")])
            .arg("-i")
            .arg(song)
            .args(["-map", "0:v", "-map", "1:a", "-shortest"])
            .args(["-r", "30", "-pix_fmt", "yuv420p"])
            .args(["-c:v", "libx264", "-preset", "veryfast", "-crf", "20"])
            .args(["-c:a", "aac", "-b:a", "160k"])
            .arg(out_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .context("spawning ffmpeg — is it installed?")?;
        Ok(Self {
            child,
            path: out_path.to_path_buf(),
        })
    }

    fn write_frame(&mut self, rgba: &[u8]) -> Result<()> {
        self.child
            .stdin
            .as_mut()
            .context("ffmpeg stdin closed")?
            .write_all(rgba)
            .context("writing frame to ffmpeg")
    }

    fn finish(mut self) -> Result<PathBuf> {
        drop(self.child.stdin.take());
        let status = self.child.wait().context("waiting for ffmpeg")?;
        if !status.success() {
            bail!("ffmpeg failed for {}", self.path.display());
        }
        Ok(self.path)
    }
}

pub fn run(args: &RenderSceneArgs) -> Result<()> {
    // ---- pass 1: analysis ----
    let audio = crate::analyze::decode::decode_file(&args.song)?;
    let stream = crate::analyze::drive(&audio);
    if stream.len() == 0 {
        bail!("song too short to analyse");
    }
    let seg = structure_offline::segment(&stream);
    let sched = SamplingSchedule::build(
        &stream,
        &seg,
        &ScheduleCfg {
            window_secs: args.window_secs,
            ..Default::default()
        },
    );
    log::info!(
        "[headless] {} sections, {} stills, {} clip window(s)",
        seg.sections.len(),
        sched.still_count(),
        sched.windows.len()
    );

    // ---- renderer ----
    let (device, queue, adapter) = crate::headless::gpu::create()?;
    log::info!("[headless] adapter: {adapter}");
    let mut sr = SceneRenderer::new(
        device,
        queue,
        args.width,
        args.height,
        args.quality,
        args.scene_dir.clone(),
    )?;
    let loaded = crate::headless::load::load_scene_dir(&args.scene_dir)?;
    let scene_name = loaded.scene.name.clone();
    sr.install_scene(loaded);

    let frames_dir = args.out.join("frames");
    let clips_dir = args.out.join("clips");
    std::fs::create_dir_all(&frames_dir)?;
    std::fs::create_dir_all(&clips_dir)?;

    // Mono fold of the whole song once, for the waveform texture peek.
    let mono: Vec<f32> = audio
        .interleaved
        .chunks_exact(2)
        .map(|s| (s[0] + s[1]) * 0.5)
        .collect();
    let peek_len = crate::gpu::audio_textures::WAVEFORM_PEEK;

    let dt = ANALYSIS_HOP as f32 / stream.sample_rate;
    let hop_hz = f64::from(stream.hop_hz());

    sr.start();

    // ---- pass 2: render every hop, capture per schedule ----
    let mut clip: Option<(usize, ClipEncoder)> = None;
    let mut clips_written: Vec<PathBuf> = Vec::new();
    let mut cue_spans: Vec<(usize, f64)> = Vec::new(); // (cue, start_secs)
    let mut last_cue = usize::MAX;
    let mut stills_written = 0usize;
    let mut result: Result<()> = Ok(());

    crate::analyze::drive_with(&audio, |hop, ts, out| {
        if result.is_err() {
            return;
        }
        let capture = sched.wants(hop);

        // Waveform peek: the trailing samples up to this hop, zero-padded at
        // the song head — the offline equivalent of the capture ring.
        let end = (hop + 1) * ANALYSIS_HOP;
        let start = end.saturating_sub(peek_len);
        let peek = &mono[start.min(mono.len())..end.min(mono.len())];

        sr.step(ts, dt, out, peek, !matches!(capture, Capture::No));

        // Track executed cue spans for run.json.
        let cue = sr.timeline.current_cue_index();
        if cue != last_cue {
            cue_spans.push((cue, ts));
            last_cue = cue;
        }

        match capture {
            Capture::No => {}
            Capture::Still { section } => {
                if let Some(rgba) = sr.read_captured_frame() {
                    let name = format!(
                        "s{section:02}_{}_at{:04.0}s_h{hop:07}.png",
                        seg.sections
                            .get(section)
                            .map(|s| s.label.as_str())
                            .unwrap_or("?"),
                        ts
                    );
                    if let Some(img) = image::RgbaImage::from_raw(args.width, args.height, rgba) {
                        if let Err(e) = img.save(frames_dir.join(&name)) {
                            result = Err(anyhow::anyhow!("writing {name}: {e}"));
                        } else {
                            stills_written += 1;
                        }
                    }
                }
            }
            Capture::Window { window } => {
                // Entering a new window? Close the previous clip, open the next.
                if clip.as_ref().map(|(w, _)| *w) != Some(window) {
                    if let Some((_, enc)) = clip.take() {
                        match enc.finish() {
                            Ok(p) => clips_written.push(p),
                            Err(e) => {
                                result = Err(e);
                                return;
                            }
                        }
                    }
                    let w = &sched.windows[window];
                    let path = clips_dir.join(format!("{}.mp4", w.label));
                    match ClipEncoder::spawn(
                        &path,
                        &args.song,
                        w.start_secs,
                        args.width,
                        args.height,
                        hop_hz,
                    ) {
                        Ok(enc) => clip = Some((window, enc)),
                        Err(e) => {
                            result = Err(e);
                            return;
                        }
                    }
                }
                if let (Some((_, enc)), Some(rgba)) = (clip.as_mut(), sr.read_captured_frame()) {
                    if let Err(e) = enc.write_frame(&rgba) {
                        result = Err(e);
                    }
                }
            }
        }
    });
    result?;
    if let Some((_, enc)) = clip.take() {
        clips_written.push(enc.finish()?);
    }

    // ---- run.json ----
    let run = serde_json::json!({
        "scene": scene_name,
        "scene_dir": args.scene_dir,
        "song": args.song,
        "adapter": adapter,
        "resolution": [args.width, args.height],
        "hop_hz": hop_hz,
        "sections": seg.sections.iter().map(|s| serde_json::json!({
            "label": s.label, "start_secs": s.start_secs, "end_secs": s.end_secs,
            "cluster": s.cluster, "energy_rank": s.energy_rank,
        })).collect::<Vec<_>>(),
        "cue_spans": cue_spans.iter().map(|(c, t)| serde_json::json!({
            "cue": c, "start_secs": t,
        })).collect::<Vec<_>>(),
        "stills": stills_written,
        "clips": clips_written,
        "warnings": sr.warnings,
    });
    std::fs::write(
        args.out.join("run.json"),
        serde_json::to_string_pretty(&run)?,
    )?;

    println!("{}", args.out.display());
    log::info!(
        "[headless] done: {} stills, {} clips, {} warning(s)",
        stills_written,
        clips_written.len(),
        sr.warnings.len()
    );
    Ok(())
}
