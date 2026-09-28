//! The Android main loop: activity lifecycle events, OpenXR session events and
//! the frame loop, all on the `android_main` thread (see "Threads" in
//! `docs/xr/XR_DESIGN.md`).

use std::time::{Duration, Instant};

use android_activity::{AndroidApp, MainEvent, PollEvent};
use anyhow::{Context, Result};
use fosfora_app::settings::ParticleQuality;
use log::{error, info};

use crate::audio::LiveAudio;
use crate::gfx::Gfx;
use crate::particles3d::{ObstacleBox, ObstacleSet, Params, Particles3d};
use crate::playback::{Clip, Playback};
use crate::scene::{WorldOptions, XrScene};
use crate::xr::{Flow, MrOptions, XrContext, XrSession};

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
/// C3b: the world-layout effect `mode world` runs unless
/// `debug.fosfora.effect` names another.
const DEFAULT_WORLD_EFFECT: &str = "Flux XR World";
const NOMINAL_FPS: u32 = 72;
/// S5 defaults: the test sim fills a 2 m cube centered on the quad, so half
/// the particles sit in front of it and half behind (the depth gate).
const DEFAULT_COUNT: u32 = 500_000;
const CUBE_HALF_M: f32 = 1.0;
const SPRITE_RADIUS_M: f32 = 0.004;
/// S7 defaults for mixed reality: until tracking is valid the cube sits
/// over a desk at the stage origin (center 1.1 m up, 0.9 m ahead, 1.1 m half
/// edge); on the first tracked frame it is re-centered on the wearer's head
/// (x, z) at `MR_CUBE_Y` with `MR_CUBE_HALF_WEARER_M`, so the desk and hands
/// are inside it whichever way they face. A little gravity settles
/// particles on real surfaces, bounces keep 40 % of the normal speed, and
/// the pinch toggle triples the sprite size.
const MR_CUBE_CENTER: [f32; 3] = [0.0, 1.1, -0.9];
const MR_CUBE_HALF_M: f32 = 1.1;
const MR_CUBE_Y: f32 = 1.0;
const MR_CUBE_HALF_WEARER_M: f32 = 1.5;
const MR_GRAVITY: f32 = 0.5;
/// Flow-speed multiplier in mixed reality: the S5 swirl (~1 m/s) would
/// carry particles straight through the settle-and-bounce behavior.
const MR_FLOW: f32 = 0.35;
/// Outward speed a hand gives the particles it touches (m/s).
const MR_HAND_KICK: f32 = 0.3;
/// Added to every hand joint's radius so a hand carves a visible channel
/// through the cloud (a bare 1 cm joint holds a fraction of one particle).
const MR_HAND_PAD_M: f32 = 0.06;
const MR_RESTITUTION: f32 = 0.4;
/// Sprites nearer than this to the eye are culled in mixed reality (the
/// user stands inside the cube; near sprites are pure fill-rate cost).
const MR_NEAR_CULL_M: f32 = 0.3;
const PINCH_SIZE_BOOST: f32 = 3.0;
/// Where the depth-writing primer quad sits in `mr` (below the floor,
/// behind the user; 1 mm wide, so never visible).
const PRIMER_POS: [f32; 3] = [0.0, -1.0, 4.0];
/// The floor obstacle: the STAGE space's y=0 plane, from Space Setup.
const FLOOR_HALF_M: f32 = 10.0;
const FLOOR_HALF_THICKNESS_M: f32 = 0.05;

/// What the frame renders, from `debug.fosfora.mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// S4: one core effect on the quad.
    Quad,
    /// S5: world-space particles around a static quad.
    Particles,
    /// S7: world-space particles over passthrough, hands and room as
    /// obstacles, no quad.
    Mixed,
    /// C3b: a core effect's world-layout variant (Flux XR World) instead of
    /// the S5 test sim, over the same mixed-reality setup as `Mixed`.
    World,
}

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

    let dirs = crate::assets::install(app).context("installing assets")?;
    info!(
        "assets {} · config {}",
        dirs.assets.display(),
        dirs.config.display()
    );
    // Spike knobs, settable without a rebuild:
    //   adb shell setprop debug.fosfora.mode quad|particles     (default particles)
    //   adb shell setprop debug.fosfora.audio synth|mic|micxr|aaudio|file  (default synth; mic needs RECORD_AUDIO;
    //       mic = the core's capture; micxr = an XR-owned cpal stream with debug.fosfora.micfmt i16|f32 and
    //       debug.fosfora.micrate <Hz>; aaudio = raw AAudio with debug.fosfora.micpreset
    //       unprocessed|generic|voice|camcorder and debug.fosfora.micperf lowlatency|none;
    //       file = the test clip looping on the speakers, analysis on its tap;
    //       loop = the clip on the speakers, analysis on the AAudio microphones (acoustic loopback))
    //   adb shell setprop debug.fosfora.file <path>   (.ogg/.mp3/.wav/.flac decoded by the core, or a raw
    //       48 kHz stereo f32 file ending in .f32, or "click" / "click:100" for a click track at that BPM;
    //       default: the bundled CC0 track under assets/audio/)
    //   adb shell setprop debug.fosfora.flash 1        (whole view white for 2 frames on each beat: the
    //       audio-to-photon measurement, filmed with a phone)
    //   adb shell setprop debug.fosfora.quality low|medium|high|ultra|max
    //   adb shell setprop debug.fosfora.scene 1280x720
    //   adb shell setprop debug.fosfora.effect "Flux"            (mode quad; any effect)
    //   adb shell setprop debug.fosfora.emit <particles per second>   (mode quad)
    //   adb shell setprop debug.fosfora.count <particles>        (mode particles)
    //   adb shell setprop debug.fosfora.sim 0                    (freeze the S5 sim: draw cost only)
    //   adb shell setprop debug.fosfora.size 0.5                 (sprite radius multiplier)
    //   adb shell setprop debug.fosfora.tri 0                    (6-vertex quads instead of 3-vertex sprites)
    //   adb shell setprop debug.fosfora.pull 0                   (instanced draw instead of vertex pulling)
    //   adb shell setprop debug.fosfora.hz 90                    (request a display rate)
    //   adb shell setprop debug.fosfora.eyescale 0.75            (swapchain size vs recommended)
    //   adb shell setprop debug.fosfora.mode mr                  (S7: passthrough + hands + room, no quad)
    //   adb shell setprop debug.fosfora.passthrough 0|1          (override the mode's default)
    //   adb shell setprop debug.fosfora.hands 0|1                (hand joints as obstacles + pinch)
    //   adb shell setprop debug.fosfora.room 0|1                 (scene anchors as obstacles)
    //   adb shell setprop debug.fosfora.scenecapture 1           (no room anchors: launch Space Setup, then requery)
    //   adb shell setprop debug.fosfora.floor 0|1                (the stage floor as an obstacle; default on in mr)
    //   adb shell setprop debug.fosfora.gravity 0.3              (downward drift m/s; default 0.3 in mr, 0 otherwise)
    //   adb shell setprop debug.fosfora.handpad 0.06             (m added to each hand joint's obstacle radius)
    //   adb shell setprop debug.fosfora.handocc 0.0              (m added to each joint's depth-occluder cube beyond the joint radius)
    //   adb shell setprop debug.fosfora.handkick 0.3             (outward speed, m/s, a hand gives the particles it touches)
    //   adb shell setprop debug.fosfora.handmesh 0|1             (the runtime's skinned hand mesh as the depth occluder instead of joint spheres; default on)
    //   adb shell setprop debug.fosfora.handmeshtest "0,0,-0.5" (diagnostic: draw the left hand mesh in bind pose at this offset from the head, in the head's frame, untracked, for a screencap)
    //   adb shell setprop debug.fosfora.flow 0.35                (flow speed multiplier; default 0.35 in mr, 1 otherwise)
    //   adb shell setprop debug.fosfora.occluders 0|1            (obstacles drawn depth-only so real objects hide sprites; default on in mr)
    //   adb shell setprop debug.fosfora.cube "0,1.1,-0.9,1.1"    (sim cube center x,y,z and half edge)
    //   adb shell setprop debug.fosfora.nearcull 0.3             (cull sprites nearer than this, m; default 0.3 in mr, 0 otherwise)
    //   adb shell setprop debug.fosfora.quad 0|1                 (the static test quad; default on in particles, off in mr)
    //   adb shell setprop debug.fosfora.quadpos "0,1.5,-1.5"     (where the static quad sits; diagnostic for the S7 quad finding)
    //   adb shell setprop debug.fosfora.depthtest 0              (sprites drawn with depth compare Always; diagnostic)
    //   adb shell setprop debug.fosfora.primer 0|1               (mr without the quad: keep a 1 mm depth-writing quad in the pass; default on)
    //   adb shell setprop debug.fosfora.mode world               (C3b: Flux XR World through render_world, over the mr setup;
    //       count = particles (default: the preset's 300K), size = sprite radius multiplier, sim 0 = freeze after warmup,
    //       effect = another world-layout preset, cube = anchor x,y,z (half edge ignored: the preset sets the volume),
    //       nearcull = near-fade radius around the head; tri/pull do not apply)
    // Clear a knob with `setprop debug.fosfora.<name> ""`.
    let mode = match debug_prop("debug.fosfora.mode").as_deref() {
        Some("quad") => Mode::Quad,
        Some("mr" | "mixed") => Mode::Mixed,
        Some("world") => Mode::World,
        _ => Mode::Particles,
    };
    // World mode runs over the whole mixed-reality setup: passthrough, hands,
    // room, floor, occluders, primer and the wearer-centered anchor all take
    // their `mr` defaults.
    let mixed = matches!(mode, Mode::Mixed | Mode::World);
    let toggle = |name: &str, default: bool| match debug_prop(name).as_deref() {
        Some("0") => false,
        Some("1") => true,
        _ => default,
    };
    let mr = MrOptions {
        passthrough: toggle("debug.fosfora.passthrough", mixed),
        hands: toggle("debug.fosfora.hands", mixed),
        room: toggle("debug.fosfora.room", mixed),
        scene_capture: toggle("debug.fosfora.scenecapture", false),
    };
    let floor = toggle("debug.fosfora.floor", mixed);
    let gravity = debug_prop("debug.fosfora.gravity")
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(if mixed { MR_GRAVITY } else { 0.0 });
    let near_cull = debug_prop("debug.fosfora.nearcull")
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(if mixed { MR_NEAR_CULL_M } else { 0.0 });
    let cube_knob = debug_prop("debug.fosfora.cube").and_then(|v| {
        let n: Vec<f32> = v.split(',').filter_map(|s| s.trim().parse().ok()).collect();
        (n.len() == 4).then(|| ([n[0], n[1], n[2]], n[3]))
    });
    // An explicit cube stays where it is put; otherwise mr follows the wearer.
    let recenter_on_wearer = mixed && cube_knob.is_none();
    let flow_mul = debug_prop("debug.fosfora.flow")
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(if mixed { MR_FLOW } else { 1.0 });
    let occluders = toggle("debug.fosfora.occluders", mixed);
    let hand_pad = debug_prop("debug.fosfora.handpad")
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(MR_HAND_PAD_M);
    // The occluder cube hugs the joint itself (a cube already reads larger
    // than the finger inside it; +2 cm looked like a 1-inch force field).
    let hand_kick = debug_prop("debug.fosfora.handkick")
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(MR_HAND_KICK);
    let hand_occ = debug_prop("debug.fosfora.handocc")
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(0.0);
    // Draw path check without a wearer: the left mesh, bind pose, parked in
    // front of the eyes wherever the headset points, so a screencap shows
    // its silhouette carved out of the cloud.
    let hand_mesh_test = debug_prop("debug.fosfora.handmeshtest").and_then(|v| {
        let n: Vec<f32> = v.split(',').filter_map(|s| s.trim().parse().ok()).collect();
        (n.len() == 3).then(|| [n[0], n[1], n[2]])
    });
    let (cube_center, cube_half) = cube_knob.unwrap_or(if mixed {
        (MR_CUBE_CENTER, MR_CUBE_HALF_M)
    } else {
        (QUAD_CENTER, CUBE_HALF_M)
    });
    let audio_source = debug_prop("debug.fosfora.audio").unwrap_or_else(|| "synth".to_owned());
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
    let count_knob = debug_prop("debug.fosfora.count")
        .and_then(|v| v.parse::<u32>().ok())
        .map(|c| c.max(1));
    let count = count_knob.unwrap_or(DEFAULT_COUNT);
    let sim_enabled = debug_prop("debug.fosfora.sim").as_deref() != Some("0");
    let size_scale = debug_prop("debug.fosfora.size")
        .and_then(|v| v.parse::<f32>().ok())
        .filter(|s| (0.01..=10.0).contains(s))
        .unwrap_or(1.0);
    // Defaults are the cheapest form measured in S5 (MEASURED.md).
    let triangles = debug_prop("debug.fosfora.tri").as_deref() != Some("0");
    let pull = debug_prop("debug.fosfora.pull").as_deref() != Some("0");
    let hz = debug_prop("debug.fosfora.hz").and_then(|v| v.parse::<f32>().ok());
    let eye_scale = debug_prop("debug.fosfora.eyescale")
        .and_then(|v| v.parse::<f32>().ok())
        .filter(|s| (0.1..=2.0).contains(s))
        .unwrap_or(1.0);
    info!(
        "mode {mode:?} · audio {audio_source} · scene {scene_w}x{scene_h} · quality {quality:?} · count {count} · sim {sim_enabled} · size x{size_scale} · tri {triangles} · pull {pull} · hz {hz:?} · eye scale {eye_scale} · mr {mr:?} · floor {floor} · gravity {gravity} · near cull {near_cull} · cube ({:.2}, {:.2}, {:.2}) half {cube_half}",
        cube_center[0], cube_center[1], cube_center[2]
    );

    let mut session = XrSession::new(&xr, &mut gfx, eye_scale, mr)?;
    if let Some(hz) = hz {
        session.request_refresh_rate(hz);
    }

    let mut live_audio = None;
    let mut playback = None;
    match audio_source.as_str() {
        "mic" => live_audio = Some(LiveAudio::mic()),
        "micxr" => {
            let format = match debug_prop("debug.fosfora.micfmt").as_deref() {
                Some("f32") => cpal::SampleFormat::F32,
                _ => cpal::SampleFormat::I16,
            };
            let rate = debug_prop("debug.fosfora.micrate")
                .and_then(|v| v.parse().ok())
                .unwrap_or(48_000);
            live_audio = Some(LiveAudio::mic_with(format, rate).context("XR mic stream")?);
        }
        "aaudio" | "loop" => {
            use ndk::audio::AudioInputPreset;
            let preset = match debug_prop("debug.fosfora.micpreset").as_deref() {
                Some("generic") => AudioInputPreset::Generic,
                Some("voice") => AudioInputPreset::VoiceRecognition,
                Some("camcorder") => AudioInputPreset::Camcorder,
                _ => AudioInputPreset::Unprocessed,
            };
            let rate = debug_prop("debug.fosfora.micrate")
                .and_then(|v| v.parse().ok())
                .unwrap_or(48_000);
            let low_latency = debug_prop("debug.fosfora.micperf").as_deref() != Some("none");
            live_audio =
                Some(LiveAudio::mic_aaudio(preset, rate, low_latency).context("AAudio mic")?);
            if audio_source == "loop" {
                let clip = Clip::decode(&dirs.assets.join("audio").join("ember_glow_excerpt.ogg"))?;
                let unused_tap =
                    std::sync::Arc::new(fosfora_app::audio::capture::RingBuffer::new());
                playback = Some(Playback::start(clip, unused_tap).context("starting playback")?);
            }
        }
        "file" => {
            let path = debug_prop("debug.fosfora.file")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| dirs.assets.join("audio").join("ember_glow_excerpt.ogg"));
            let clip = if let Some(bpm) = path
                .to_str()
                .and_then(|p| p.strip_prefix("click"))
                .map(|rest| rest.trim_start_matches(':').parse::<f32>().unwrap_or(120.0))
            {
                Clip::click(bpm, 30.0, 48_000)
            } else if path.extension().is_some_and(|e| e == "f32") {
                Clip::from_raw_f32_stereo(&path, 48_000)?
            } else {
                Clip::decode(&path)?
            };
            let tap = std::sync::Arc::new(fosfora_app::audio::capture::RingBuffer::new());
            let p = Playback::start(clip, tap.clone()).context("starting playback")?;
            live_audio = Some(LiveAudio::from_ring(
                tap,
                p.output_rate(),
                p.callback_count(),
            ));
            playback = Some(p);
        }
        _ => {}
    }
    let mut scene = None;
    let mut particles = None;
    let mut static_quad = None;
    match mode {
        Mode::Quad => {
            //   adb shell setprop debug.fosfora.effect "Flux"   (any effect name)
            let effect =
                debug_prop("debug.fosfora.effect").unwrap_or_else(|| DEFAULT_EFFECT.to_owned());
            let scene_dir =
                write_single_effect_scene(&dirs.config, &effect).context("writing the scene")?;
            let mut s = XrScene::new(
                &gfx.device,
                &gfx.queue,
                scene_w,
                scene_h,
                quality,
                &scene_dir,
                NOMINAL_FPS,
            )
            .context("creating the scene")?;
            if let Some(rate) = debug_prop("debug.fosfora.emit").and_then(|v| v.parse::<f32>().ok())
            {
                s.set_emit_rate(rate);
            }
            if let Some((_, view)) = &s.quad {
                gfx.set_quad(
                    view,
                    QUAD_WIDTH_M,
                    scene_w as f32 / scene_h as f32,
                    QUAD_CENTER,
                );
            }
            scene = Some(s);
        }
        Mode::Particles | Mode::Mixed | Mode::World => {
            // S5: a static, opaque quad at the S4 placement, the depth
            // reference the particles are judged against. It costs nothing
            // to render, so the sweep numbers are the particles'. In mixed
            // reality the real room is the reference, so no quad unless
            // asked for.
            if toggle("debug.fosfora.quad", !mixed) {
                let quad_pos = debug_prop("debug.fosfora.quadpos")
                    .and_then(|v| {
                        let n: Vec<f32> =
                            v.split(',').filter_map(|s| s.trim().parse().ok()).collect();
                        (n.len() == 3).then(|| [n[0], n[1], n[2]])
                    })
                    .unwrap_or(QUAD_CENTER);
                let (tex, view) = static_quad_texture(&gfx.device, &gfx.queue);
                gfx.set_quad(&view, QUAD_WIDTH_M, 16.0 / 9.0, quad_pos);
                static_quad = Some(tex);
            } else if toggle("debug.fosfora.primer", mixed) {
                // S7 finding (MEASURED.md): with a depth-writing opaque draw
                // in the eye pass, 750K sprites cost ~10 ms; with the sprite
                // draw alone, ~31 ms, whether or not the quad is in view or
                // covers anything. Until the mechanism is understood, keep a
                // depth-writing draw in the pass: a 1 mm quad below the
                // floor, behind the user.
                let (tex, view) = static_quad_texture(&gfx.device, &gfx.queue);
                gfx.set_quad(&view, 0.001, 1.0, PRIMER_POS);
                static_quad = Some(tex);
            }
            if mode == Mode::World {
                let effect = debug_prop("debug.fosfora.effect")
                    .unwrap_or_else(|| DEFAULT_WORLD_EFFECT.to_owned());
                let scene_dir = write_single_effect_scene(&dirs.config, &effect)
                    .context("writing the scene")?;
                let s = XrScene::new_world(
                    &gfx.device,
                    &gfx.queue,
                    &scene_dir,
                    &effect,
                    NOMINAL_FPS,
                    WorldOptions {
                        count: count_knob,
                        size_scale,
                        sim_enabled,
                        anchor: cube_center,
                    },
                )
                .context("creating the world scene")?;
                scene = Some(s);
            }
            // In world mode the effect is the particles: the test sim keeps
            // only its obstacle block and occluders (count 0).
            let mut p = Particles3d::new(
                &gfx.device,
                if mode == Mode::World { 0 } else { count },
                crate::xr::EYE_COUNT,
                Params {
                    triangles,
                    pull,
                    cube_center,
                    cube_half,
                    base_size: SPRITE_RADIUS_M * size_scale,
                    flow_scale: 1.0,
                    lifetime: 12.0,
                    gravity,
                    near_cull,
                    depth_test: toggle("debug.fosfora.depthtest", true),
                },
            );
            p.sim_enabled = sim_enabled;
            p.occluders = occluders;
            // The skinned hand mesh occludes when the runtime delivered one
            // per hand; otherwise (or with the knob off) the joint spheres.
            p.hand_mesh = toggle("debug.fosfora.handmesh", true);
            p.set_hand_meshes(&gfx.device, session.hand_meshes());
            particles = Some(p);
        }
    }
    let floor_box = floor.then_some(ObstacleBox {
        center: [0.0, -FLOOR_HALF_THICKNESS_M, 0.0],
        rot: [0.0, 0.0, 0.0, 1.0],
        half: [FLOOR_HALF_M, FLOOR_HALF_THICKNESS_M, FLOOR_HALF_M],
    });
    // Pinch toggles a visible parameter (the sprite size); either hand.
    let mut size_boost = false;
    let mut recentered = false;
    let mut obstacle_log = (0u32, 0u32, [false; 2], [false; 2]);

    let started = Instant::now();
    let mut stats = FrameStats::default();
    let mut destroyed = false;
    let mut last_t = 0.0f32;
    let mut frame_index = 0u64;
    let mut beat_env = 0.0f32;
    let flash = debug_prop("debug.fosfora.flash").as_deref() == Some("1");
    let mut flash_frames = 0u32;

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
        // Drain Android input events (controller buttons, touch). Nothing
        // consumes them yet (hands and controllers come through OpenXR in
        // S7), but an undrained queue makes Android flag the app as not
        // responding as soon as a wearer generates any input.
        if let Ok(mut events) = app.input_events_iter() {
            while events.next(|_| android_activity::InputStatus::Unhandled) {}
        }
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
        let queue = &gfx.queue;
        // Over passthrough the frame starts fully transparent so the camera
        // image shows wherever nothing is drawn.
        let clear = if flash_frames > 0 {
            flash_frames -= 1;
            [1.0, 1.0, 1.0, 1.0]
        } else if session.has_passthrough() {
            [0.0, 0.0, 0.0, 0.0]
        } else {
            background_color(t)
        };
        let scene_mut = scene.as_mut();
        session.frame(&gfx, clear, &mut stats, particles.as_ref(), scene_mut, |input, mut scene| {
            // S7: hands and room as obstacles, pinch as the toggle.
            if input.hands.pinch_began.iter().any(|&b| b) {
                size_boost = !size_boost;
                info!(
                    "pinch toggle: sprite size x{} {}",
                    PINCH_SIZE_BOOST,
                    if size_boost { "on" } else { "off" }
                );
            }
            if let Some(p) = particles.as_ref() {
                if recenter_on_wearer && !recentered {
                    recentered = true;
                    let center = [input.head[0], MR_CUBE_Y, input.head[2]];
                    p.set_cube(center, MR_CUBE_HALF_WEARER_M);
                    if let Some(s) = scene.as_deref_mut() {
                        s.set_anchor(center);
                    }
                    info!(
                        "cube (and world anchor) re-centered on the wearer: ({:.2}, {:.2}, {:.2}) half {MR_CUBE_HALF_WEARER_M} (head at ({:.2}, {:.2}, {:.2}))",
                        center[0],
                        center[1],
                        center[2],
                        input.head[0],
                        input.head[1],
                        input.head[2]
                    );
                }
                let mut set = ObstacleSet::new(
                    MR_RESTITUTION,
                    SPRITE_RADIUS_M * size_scale,
                    (hand_pad - hand_occ).max(0.0),
                    hand_kick,
                );
                for s in &input.hands.spheres {
                    set.push_sphere([s[0], s[1], s[2], s[3] + hand_pad]);
                }
                for b in &input.room_boxes {
                    set.push_box(b);
                }
                if let Some(f) = &floor_box {
                    set.push_box(f);
                }
                p.set_obstacles(queue, &set);
                if let Some(s) = scene.as_deref_mut() {
                    s.set_world_inputs(queue, input.head, near_cull, &set);
                }
                if let Some(offset) = hand_mesh_test {
                    let mut skins = input.hands.skins;
                    let rot = glam::Quat::from_array(input.head_rot);
                    let at = glam::Vec3::from(input.head) + rot * glam::Vec3::from(offset);
                    let parked = glam::Mat4::from_rotation_translation(rot, at).to_cols_array();
                    if frame_index.is_multiple_of(360) {
                        info!(
                            "hand mesh test: left mesh parked at ({:.2}, {:.2}, {:.2}), head ({:.2}, {:.2}, {:.2})",
                            at.x, at.y, at.z, input.head[0], input.head[1], input.head[2]
                        );
                    }
                    skins[0] = [parked; crate::particles3d::HAND_JOINTS];
                    p.set_hand_skins(queue, &skins, [true, input.hands.mesh_ready[1]]);
                } else {
                    p.set_hand_skins(queue, &input.hands.skins, input.hands.mesh_ready);
                }
                obstacle_log = (
                    set.sphere_count(),
                    set.box_count(),
                    input.hands.tracked,
                    input.hands.mesh_ready,
                );
            }
            // This frame's audio: the headset microphones or the
            // synthetic 120 BPM groove.
            let hop = match live_audio.as_mut() {
                Some(a) => a.frame(dt, f64::from(t)),
                None => synth_hop_for(frame_index, t),
            };
            let f = hop.frame.features;
            if hop.beat_fired {
                beat_env = 1.0;
                if flash {
                    flash_frames = 2;
                }
                if let Some(p) = &playback {
                    // Beat timing evidence (S6): where in the clip the
                    // beat fired, against the track's known 140 BPM grid.
                    info!("beat at clip {:.4} s", p.position_secs());
                }
            }
            beat_env *= (-dt * 6.0).exp();
            if let Some(scene) = scene {
                match live_audio.as_mut() {
                    Some(a) => scene.step(f64::from(t), dt, &hop, a.waveform()),
                    None => {
                        let hop = scene.synth(f64::from(t));
                        let wave = scene.synth_waveform().to_vec();
                        scene.step(f64::from(t), dt, &hop, &wave);
                    }
                }
            }
            if let Some(p) = particles.as_ref() {
                // bass drives the flow speed, rms and the beat pulse
                // the sprite size.
                let boost = if size_boost { PINCH_SIZE_BOOST } else { 1.0 };
                p.update(
                    queue,
                    t,
                    dt,
                    (0.6 + 1.2 * f.bass) * flow_mul,
                    (0.7 + 0.9 * f.rms) * (1.0 + 0.5 * beat_env) * boost,
                );
            }
        })?;
        frame_index += 1;
        if frame_index.is_multiple_of(72) {
            if let Some(scene) = &scene {
                info!("particles alive {}", scene.alive_count());
            }
            if session.has_hands() || session.has_room() {
                info!(
                    "obstacles: {} spheres (hands L {} R {} · mesh L {} R {}) · {} boxes (room {} + floor {}) · anchors [{}] · size boost {}",
                    obstacle_log.0,
                    obstacle_log.2[0],
                    obstacle_log.2[1],
                    obstacle_log.3[0],
                    obstacle_log.3[1],
                    obstacle_log.1,
                    obstacle_log
                        .1
                        .saturating_sub(u32::from(floor_box.is_some())),
                    floor_box.is_some(),
                    session.room_summary().unwrap_or_default(),
                    size_boost
                );
            }
            if let Some(p) = &playback {
                info!(
                    "playback: {:.1} s into the clip · {} frames played at {} Hz",
                    p.position_secs(),
                    p.frames_played(),
                    p.output_rate()
                );
            }
        }
    }

    drop(session);
    drop(scene);
    drop(playback);
    drop(live_audio);
    drop(particles);
    drop(static_quad);
    drop(gfx);
    Ok(())
}

/// A 256x256 opaque test card for the S5 quad: a gray ramp with a bright
/// border, so occlusion by the quad is visible in a screencap and the quad
/// itself is easy to find.
fn static_quad_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> (wgpu::Texture, wgpu::TextureView) {
    const N: u32 = 256;
    let mut pixels = vec![0u8; (N * N * 4) as usize];
    for y in 0..N {
        for x in 0..N {
            let border = x < 8 || y < 8 || x >= N - 8 || y >= N - 8;
            let checker = ((x / 32) + (y / 32)) % 2 == 0;
            let v = if border {
                220
            } else if checker {
                90
            } else {
                40
            };
            let i = ((y * N + x) * 4) as usize;
            pixels[i..i + 4].copy_from_slice(&[v, v, v + 20, 255]);
        }
    }
    let size = wgpu::Extent3d {
        width: N,
        height: N,
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("xr-static-quad"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    queue.write_texture(
        texture.as_image_copy(),
        &pixels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(N * 4),
            rows_per_image: Some(N),
        },
        size,
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

/// The S4 synthetic groove as a hop, for the particles mode without a
/// scene.
fn synth_hop_for(frame_index: u64, t: f32) -> fosfora_app::audio::hop::HopOutput {
    crate::scene::synth_hop(frame_index as u32, NOMINAL_FPS, f64::from(t))
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
