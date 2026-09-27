//! The Android main loop: activity lifecycle events, OpenXR session events and
//! the frame loop, all on the `android_main` thread (see "Threads" in
//! `docs/xr/XR_DESIGN.md`).

use std::time::{Duration, Instant};

use android_activity::{AndroidApp, MainEvent, PollEvent};
use anyhow::Result;
use log::{error, info};

use crate::gfx::Gfx;
use crate::xr::{Flow, XrContext, XrSession};

/// Logcat tag. `scripts/xr/run.sh log` filters on it.
pub const LOG_TAG: &str = "fosfora_xr";

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
    // Declared before `session` so it is dropped after it: OpenXR must release
    // its Vulkan objects (swapchain images) before the device goes away.
    let gfx = Gfx::new(&xr)?;
    let mut session = XrSession::new(&xr, &gfx)?;

    let started = Instant::now();
    let mut stats = FrameStats::default();
    let mut destroyed = false;

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
        session.frame(&gfx, clear_color(t), &mut stats)?;
    }

    drop(session);
    drop(gfx);
    Ok(())
}

/// A slow hue sweep so the frame loop is visibly alive in both eyes.
fn clear_color(t: f32) -> [f32; 4] {
    let hue = (t * 0.08).fract();
    let [r, g, b] = hsv_to_rgb(hue, 0.85, 0.55);
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
/// display period from `xrWaitFrame` is the display-rate source for S1
/// (`docs/xr/MEASURED.md`); a "long" frame is a wait-to-wait interval over
/// 1.5x that period, i.e. at least one missed display period.
#[derive(Default)]
pub struct FrameStats {
    window_start: Option<Instant>,
    last_frame: Option<Instant>,
    frames: u32,
    skipped_render: u32,
    long_frames: u32,
    max_interval: Duration,
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
            info!(
                "frames {}/s · display period {:.3} ms ({:.1} Hz) · max interval {:.2} ms · long {} (total {}/{}) · should_render=false {}",
                f64::from(self.frames) / elapsed.as_secs_f64(),
                period.as_secs_f64() * 1e3,
                hz,
                self.max_interval.as_secs_f64() * 1e3,
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
        }
    }
}
