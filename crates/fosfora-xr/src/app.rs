//! The Android main loop: activity lifecycle events, OpenXR session events and
//! the frame loop, all on the `android_main` thread (see "Threads" in
//! `docs/xr/XR_DESIGN.md`).

use std::time::{Duration, Instant};

use android_activity::{AndroidApp, MainEvent, PollEvent};
use anyhow::{Context, Result};
use fosfora_app::settings::ParticleQuality;
use log::{error, info};

use crate::gfx::Gfx;
use crate::scene::XrScene;
use crate::xr::{Flow, XrContext, XrSession};

/// Logcat tag. `scripts/xr/run.sh log` filters on it.
pub const LOG_TAG: &str = "fosfora_xr";

/// S4: the effect renders offscreen at this size and lands on a quad
/// `QUAD_WIDTH_M` wide, centered at `QUAD_CENTER` in the stage space (1.5 m
/// ahead, 1.5 m up, facing the user).
const SCENE_WIDTH: u32 = 1280;
const SCENE_HEIGHT: u32 = 720;
const QUAD_WIDTH_M: f32 = 1.2;
pub const QUAD_CENTER: [f32; 3] = [0.0, 1.5, -1.5];
const DEFAULT_EFFECT: &str = "Flux";
const NOMINAL_FPS: u32 = 72;

pub fn run(app: &AndroidApp) {
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Info)
            .with_tag(LOG_TAG),
    );
    // Rust panics go to stderr, which Android discards. Route them to logcat
    // before anything can fail.
    std::panic::set_hook(Box::new(|info| error!("panic: {info}")));
    info!("fosfora-xr {} starting", env!("CARGO_PKG_VERSION"));

    match run_inner(app) {
        Ok(()) => info!("exited cleanly"),
        Err(e) => error!("fatal: {e:#}"),
    }
}

fn run_inner(app: &AndroidApp) -> Result<()> {
    let xr = XrContext::new(app)?;
    // Declared before `session` and `scene` so it is dropped after them:
    // OpenXR must release its Vulkan objects (swapchain images) before the
    // device goes away.
    let sdk_version = u32::try_from(app.config().sdk_version()).unwrap_or(0);
    let mut gfx = Gfx::new(&xr, sdk_version)?;
    let mut session = XrSession::new(&xr, &gfx)?;

    let dirs = crate::assets::install(app).context("installing assets")?;
    info!(
        "assets {} · config {}",
        dirs.assets.display(),
        dirs.config.display()
    );
    // Spike knobs, settable without a rebuild:
    //   adb shell setprop debug.fosfora.quality low|medium|high|ultra|max
    //   adb shell setprop debug.fosfora.scene 1280x720
    let quality = match debug_prop("debug.fosfora.quality").as_deref() {
        Some("low") => ParticleQuality::Low,
        Some("medium") => ParticleQuality::Medium,
        Some("ultra") => ParticleQuality::Ultra,
        Some("max") => ParticleQuality::Max,
        _ => ParticleQuality::High,
    };
    let (scene_w, scene_h) = debug_prop("debug.fosfora.scene")
        .and_then(|v| {
            let (w, h) = v.split_once('x')?;
            Some((w.parse().ok()?, h.parse().ok()?))
        })
        .unwrap_or((SCENE_WIDTH, SCENE_HEIGHT));
    info!("scene: {scene_w}x{scene_h}, particle quality {quality:?}");
    //   adb shell setprop debug.fosfora.effect "Flux"   (any effect name)
    let effect = debug_prop("debug.fosfora.effect").unwrap_or_else(|| DEFAULT_EFFECT.to_owned());
    let scene_dir =
        write_single_effect_scene(&dirs.config, &effect).context("writing the scene")?;
    let mut scene = XrScene::new(
        &gfx.device,
        &gfx.queue,
        scene_w,
        scene_h,
        quality,
        &scene_dir,
        NOMINAL_FPS,
    )
    .context("creating the scene")?;
    //   adb shell setprop debug.fosfora.emit <particles per second>
    if let Some(rate) = debug_prop("debug.fosfora.emit").and_then(|v| v.parse::<f32>().ok()) {
        scene.set_emit_rate(rate);
    }
    gfx.set_quad(
        &scene.quad_view,
        QUAD_WIDTH_M,
        scene_w as f32 / scene_h as f32,
        QUAD_CENTER,
    );

    let started = Instant::now();
    let mut stats = FrameStats::default();
    let mut destroyed = false;
    let mut last_t = 0.0f32;
    let mut frame_index = 0u64;

    loop {
        // While the session is running, Android events are drained without
        // blocking so the frame loop stays paced by xrWaitFrame. Otherwise block
        // briefly instead of spinning.
        let timeout = if session.is_running() {
            Duration::ZERO
        } else {
            Duration::from_millis(50)
        };
        app.poll_events(Some(timeout), |event| {
            if let PollEvent::Main(main) = event {
                // Names only: `Resume`'s Debug output dumps the whole glue state.
                let name = match main {
                    MainEvent::Destroy => {
                        destroyed = true;
                        "Destroy"
                    }
                    MainEvent::Start => "Start",
                    MainEvent::Resume { .. } => "Resume",
                    MainEvent::Pause => "Pause",
                    MainEvent::Stop => "Stop",
                    MainEvent::GainedFocus => "GainedFocus",
                    MainEvent::LostFocus => "LostFocus",
                    MainEvent::InitWindow { .. } => "InitWindow",
                    MainEvent::TerminateWindow { .. } => "TerminateWindow",
                    MainEvent::LowMemory => "LowMemory",
                    _ => return,
                };
                info!("android: {name}");
            }
        });
        if destroyed {
            // The activity is going away and the glue waits for this thread to
            // return. The session is torn down by Drop; xrDestroySession is
            // valid in any state.
            info!("android: activity destroyed, leaving the main loop");
            break;
        }

        if session.poll_events()? == Flow::Exit {
            break;
        }
        if !session.is_running() {
            continue;
        }

        let t = started.elapsed().as_secs_f32();
        let dt = (t - last_t).clamp(1.0 / 120.0, 1.0 / 30.0);
        last_t = t;
        session.frame(&gfx, background_color(t), &mut stats, || {
            scene.step(f64::from(t), dt);
        })?;
        frame_index += 1;
        if frame_index.is_multiple_of(72) {
            info!("particles alive {}", scene.alive_count());
        }
    }

    drop(session);
    drop(scene);
    drop(gfx);
    Ok(())
}

/// A slow, dim hue sweep behind the quad: shows the frame loop is alive
/// without competing with the effect.
fn background_color(t: f32) -> [f32; 4] {
    let hue = (t * 0.05).fract();
    let [r, g, b] = hsv_to_rgb(hue, 0.5, 0.12);
    [r, g, b, 1.0]
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [f32; 3] {
    let i = (h * 6.0).floor();
    let f = h * 6.0 - i;
    let p = v * (1.0 - s);
    let q = v * (1.0 - s * f);
    let t = v * (1.0 - s * (1.0 - f));
    match i as i32 % 6 {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

/// Frame pacing counters, reported to logcat once a second. The predicted
/// display period from `xrWaitFrame` is the display-rate source
/// (`docs/xr/MEASURED.md`); a "long" frame is a wait-to-wait interval over
/// 1.5x that period, i.e. at least one missed display period. CPU time is
/// the span from `xrWaitFrame` returning to `xrEndFrame`, i.e. everything
/// the app does per frame (effect step, encoding, submits).
#[derive(Default)]
pub struct FrameStats {
    window_start: Option<Instant>,
    last_frame: Option<Instant>,
    frames: u32,
    skipped_render: u32,
    long_frames: u32,
    max_interval: Duration,
    cpu_sum: Duration,
    cpu_max: Duration,
    cpu_n: u32,
    total_long: u32,
    total_frames: u32,
}

impl FrameStats {
    pub fn record(&mut self, period: Duration, should_render: bool) {
        let now = Instant::now();
        self.frames += 1;
        self.total_frames += 1;
        if !should_render {
            self.skipped_render += 1;
        }
        if let Some(last) = self.last_frame {
            let interval = now - last;
            self.max_interval = self.max_interval.max(interval);
            if interval > period.mul_f32(1.5) {
                self.long_frames += 1;
                self.total_long += 1;
            }
        }
        self.last_frame = Some(now);

        let window_start = *self.window_start.get_or_insert(now);
        let elapsed = now - window_start;
        if elapsed >= Duration::from_secs(1) {
            let hz = 1.0 / period.as_secs_f64();
            let cpu_avg = if self.cpu_n > 0 {
                self.cpu_sum.as_secs_f64() * 1e3 / f64::from(self.cpu_n)
            } else {
                0.0
            };
            info!(
                "frames {:.1}/s · display period {:.3} ms ({:.1} Hz) · max interval {:.2} ms · cpu avg {:.2} max {:.2} ms · long {} (total {}/{}) · should_render=false {}",
                f64::from(self.frames) / elapsed.as_secs_f64(),
                period.as_secs_f64() * 1e3,
                hz,
                self.max_interval.as_secs_f64() * 1e3,
                cpu_avg,
                self.cpu_max.as_secs_f64() * 1e3,
                self.long_frames,
                self.total_long,
                self.total_frames,
                self.skipped_render,
            );
            self.window_start = Some(now);
            self.frames = 0;
            self.skipped_render = 0;
            self.long_frames = 0;
            self.max_interval = Duration::ZERO;
            self.cpu_sum = Duration::ZERO;
            self.cpu_max = Duration::ZERO;
            self.cpu_n = 0;
        }
    }

    pub fn record_cpu(&mut self, cpu: Duration) {
        self.cpu_sum += cpu;
        self.cpu_max = self.cpu_max.max(cpu);
        self.cpu_n += 1;
    }
}

/// An Android system property, for spike-time knobs. Empty means unset.
fn debug_prop(name: &str) -> Option<String> {
    let out = std::process::Command::new("getprop")
        .arg(name)
        .output()
        .ok()?;
    let v = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    (!v.is_empty()).then_some(v)
}

/// A one-cue scene showing `effect` with its default parameters, written
/// under the config dir in the layout `headless::load::load_scene_dir` reads.
fn write_single_effect_scene(config: &std::path::Path, effect: &str) -> Result<std::path::PathBuf> {
    let dir = config
        .join("scenes")
        .join(effect.to_lowercase().replace(' ', "_"));
    std::fs::create_dir_all(&dir)?;
    let preset = serde_json::json!({ "layers": [{ "effect_name": effect }] });
    std::fs::write(dir.join("Cue.json"), serde_json::to_string_pretty(&preset)?)?;
    let scene = serde_json::json!({
        "version": 1,
        "name": format!("XR {effect}"),
        "loop_mode": false,
        "advance_mode": "Manual",
        "cues": [{ "preset_name": "Cue", "transition": "Cut", "label": effect }]
    });
    std::fs::write(
        dir.join("_scene.json"),
        serde_json::to_string_pretty(&scene)?,
    )?;
    Ok(dir)
}
