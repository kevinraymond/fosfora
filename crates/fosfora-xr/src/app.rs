//! The Android main loop: activity lifecycle events, OpenXR session events and
//! the frame loop, all on the `android_main` thread (see "Threads" in
//! `docs/xr/XR_DESIGN.md`).

use std::time::{Duration, Instant};

use android_activity::{AndroidApp, MainEvent, PollEvent};
use anyhow::{Context, Result};
use fosfora_app::settings::ParticleQuality;
use log::{error, info};

use crate::audio::LiveAudio;
use crate::gesture::{Gesture, Gestures, PinchInput};
use crate::gfx::Gfx;
use crate::hud::{Action, Controls, FrameWindow, Hud, View};
use crate::particles3d::{ObstacleBox, ObstacleSet, Params, Particles3d};
use crate::playback::{Clip, Playback, PlaybackOptions};
use crate::pose::{Behavior, HandInput, HoldTrack, Plan, Pose, Poses, Tuning};
use crate::scene::{WorldOptions, XrScene};
use crate::surfaces::SurfaceWeights;
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
/// The world effects that read the per-hand lanes (board #3314): there the
/// hand's pose picks its behavior and its pad. The others keep one behavior
/// for every hand (Flux XR World's worn-approved pad and kick).
const POSE_EFFECTS: [&str; 1] = ["Murmur XR World"];
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
/// World-effect defaults (Flux XR World, board #3276): its 2-5 mm sprites
/// are smaller and dimmer than the S5 test sim's, so the near fade starts
/// closer (the cloud stays dense at hand distance), the hand pad is wider
/// (a channel that reads) and the kick stronger. The settle drift is the
/// `mr` gravity.
const WORLD_NEAR_FADE_M: f32 = 0.15;
const WORLD_HAND_PAD_M: f32 = 0.10;
const WORLD_HAND_KICK: f32 = 0.4;
/// Where the depth-writing primer quad sits in `mr` (below the floor,
/// behind the user; 1 mm wide, so never visible).
const PRIMER_POS: [f32; 3] = [0.0, -1.0, 4.0];
/// The floor obstacle: the STAGE space's y=0 plane, from Space Setup.
const FLOOR_HALF_M: f32 = 10.0;
const FLOOR_HALF_THICKNESS_M: f32 = 0.05;
/// Seated reach: past this extension (m) the far hand shows as ghost
/// sprites of this radius and a wrist beam, at these peak alphas.
const REACH_GHOST_M: f32 = 0.03;
const REACH_GHOST_RADIUS_M: f32 = 0.008;
const REACH_GHOST_ALPHA: f32 = 0.5;
const REACH_BEAM_ALPHA: f32 = 0.15;

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
    //   adb shell setprop debug.fosfora.playperf lowlatency|none   (AAudio performance mode of the playback
    //       output; default lowlatency: 36 ms to the speaker on the Quest 3 against 140-215 ms for none)
    //   adb shell setprop debug.fosfora.playbuf <ms>   (playback output buffer; default 84 ms in lowlatency so the
    //       speaker sits behind the analysis + display chain and the tap delay lines them up; 0 = AAudio's default)
    //   adb shell setprop debug.fosfora.tapdelay <ms>  (fixed delay of the analysis tap behind the frames
    //       handed to AAudio; default: estimated from the stream's DAC timestamps, minus the detection time,
    //       so beats land on the sound; 0 = the S6 behavior that flashed 115 ms early)
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
    //   adb shell setprop debug.fosfora.gravity 0.5              (downward settle drift m/s; default 0.5 in mr and world, 0 otherwise)
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
    //       nearcull = near-fade radius around the head (default 0.15 here), handpad 0.10 and handkick 0.4 by default,
    //       gravity = settle drift; tri/pull do not apply)
    //   adb shell setprop debug.fosfora.hud 0|1                  (the debug panel on the hand menu, set and saved; unset = the menu's saved toggle)
    //   adb shell setprop debug.fosfora.hudtest 1                (diagnostic: the debug panel parked ahead of the view, untracked, for a screencap)
    //   adb shell setprop debug.fosfora.reach 0|1                (seated reach: hands' sim spheres extend Go-Go style past a comfortable reach; default on in mr/world)
    //   adb shell setprop debug.fosfora.reachat 0.30             (reach: shoulder-to-palm distance, m, within which the hand is 1:1)
    //   adb shell setprop debug.fosfora.reachgain 40             (reach: quadratic gain, 1/m; 40 puts a 0.52 m palm at 2.5 m, capped at 3 m)
    //   adb shell setprop debug.fosfora.handscare 1              (Murmur: hand predator strength 0..1; 0 = hands only push)
    //   adb shell setprop debug.fosfora.poses 0|1                (Murmur: the hand's pose picks its behavior: fist = predator with handpad/handkick,
    //       open = push only with openpad/openkick, palm to the ceiling or two palms facing close together = hold; default on;
    //       0 = every hand a predator, as before)
    //   adb shell setprop debug.fosfora.openpad 0.05             (Murmur: an open hand's pad, m)
    //   adb shell setprop debug.fosfora.openkick 0.1             (Murmur: an open hand's kick, m/s)
    //   adb shell setprop debug.fosfora.holdradius 0.25          (Murmur: a palm-up hold's radius, m; a two-hand hold's is half the palms' distance, up to this)
    //   adb shell setprop debug.fosfora.cycletest 10             (world: switch to the next world effect every 10 s,
    //       as a pinch-hold does; for measuring the switch unworn)
    //   adb shell setprop debug.fosfora.floorweight 0.5          (surface emitters, Flux XR Room: the floor's emitter weight, 0..1)
    //   adb shell setprop debug.fosfora.tableweight 1            (surface emitters: scales every table's weight; the largest
    //       table gets this, the others by top-face area)
    //   adb shell setprop debug.fosfora.ripple 0|1               (the floor ripple: rings from under the head on each beat; default on in mr/world)
    //   adb shell setprop debug.fosfora.ripplegain 1             (ripple brightness multiplier; 1 = peak alpha 0.25)
    //   adb shell setprop debug.fosfora.ripplespeed 2.5          (ripple ring speed, m/s)
    //   adb shell setprop debug.fosfora.rippletest ceiling       (diagnostic: the ripple under the room's CEILING anchor instead of on
    //       the floor, lifted toward the room, for an unworn screencap from a headset lying face up)
    //   adb shell setprop debug.fosfora.canvas 0|1               (the wall spectrum: mel bars on the wall the wearer faces; default on in mr/world)
    //   adb shell setprop debug.fosfora.canvasgain 1             (wall spectrum brightness multiplier; 1 = peak alpha 0.25)
    //   adb shell setprop debug.fosfora.canvasbars 24            (wall spectrum bar count, 1..64; fewer when the mel spectrum is shorter)
    //   adb shell setprop debug.fosfora.canvastest ceiling       (diagnostic: the wall spectrum on the room's CEILING anchor instead of a wall,
    //       for an unworn screencap from a headset lying face up)
    // Clear a knob with `setprop debug.fosfora.<name> ""`.
    let mode = match debug_prop("debug.fosfora.mode").as_deref() {
        Some("quad") => Mode::Quad,
        Some("mr" | "mixed") => Mode::Mixed,
        Some("world") => Mode::World,
        _ => Mode::Particles,
    };
    // World mode runs over the whole mixed-reality setup: passthrough, hands,
    // room, floor, occluders, primer and the wearer-centered anchor all take
    // their `mr` defaults, except where the world effect's smaller, dimmer
    // sprites need more (`WORLD_*` below).
    let mixed = matches!(mode, Mode::Mixed | Mode::World);
    let mode_name = match mode {
        Mode::Quad => "quad",
        Mode::Particles => "particles",
        Mode::Mixed => "mr",
        Mode::World => "world",
    };
    let world = mode == Mode::World;
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
        .unwrap_or(if world {
            WORLD_NEAR_FADE_M
        } else if mixed {
            MR_NEAR_CULL_M
        } else {
            0.0
        });
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
        .unwrap_or(if world {
            WORLD_HAND_PAD_M
        } else {
            MR_HAND_PAD_M
        });
    // The occluder cube hugs the joint itself (a cube already reads larger
    // than the finger inside it; +2 cm looked like a 1-inch force field).
    let hand_kick = debug_prop("debug.fosfora.handkick")
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(if world { WORLD_HAND_KICK } else { MR_HAND_KICK });
    // Seated reach (board #3308): Go-Go arm extension of the hands' sim
    // spheres, on by default wherever hands are obstacles.
    let reach_on = toggle("debug.fosfora.reach", mixed);
    let reach_threshold = debug_prop("debug.fosfora.reachat")
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(crate::reach::THRESHOLD_M);
    let reach_gain = debug_prop("debug.fosfora.reachgain")
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(crate::reach::GAIN);
    // Board #3311: how much the hands scare Murmur's flock (1 = a hawk,
    // 0 = the hands only push), for the scoop A/B and later per pose.
    let hand_scare = debug_prop("debug.fosfora.handscare")
        .and_then(|v| v.parse::<f32>().ok())
        .map_or(1.0, |v| v.clamp(0.0, 1.0));
    // Board #3314: hand poses for Murmur (`pose.rs`).
    let poses_on = toggle("debug.fosfora.poses", true);
    let knob = |name: &str, default: f32| {
        debug_prop(name)
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(default)
    };
    let open_pad = knob("debug.fosfora.openpad", crate::pose::OPEN_PAD_M);
    let open_kick = knob("debug.fosfora.openkick", crate::pose::OPEN_KICK_M_S);
    let hold_radius = knob("debug.fosfora.holdradius", crate::pose::HOLD_RADIUS_M);
    // Board #3317: surfaces as emitters (only a world sim in surface mode,
    // Flux XR Room, reads the weights).
    let surface_weights = {
        let d = SurfaceWeights::default();
        SurfaceWeights {
            table: knob("debug.fosfora.tableweight", d.table).max(0.0),
            floor: knob("debug.fosfora.floorweight", d.floor).max(0.0),
        }
    };
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
    let playback_options = PlaybackOptions {
        low_latency: debug_prop("debug.fosfora.playperf").as_deref() != Some("none"),
        tap_delay_ms: debug_prop("debug.fosfora.tapdelay").and_then(|v| v.parse::<f32>().ok()),
        buffer_ms: debug_prop("debug.fosfora.playbuf").and_then(|v| v.parse::<f32>().ok()),
    };
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
                playback = Some(
                    Playback::start(clip, unused_tap, playback_options)
                        .context("starting playback")?,
                );
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
            let p = Playback::start(clip, tap.clone(), playback_options)
                .context("starting playback")?;
            live_audio = Some(LiveAudio::from_ring(
                tap,
                p.output_rate(),
                p.callback_count(),
            ));
            playback = Some(p);
        }
        _ => {}
    }
    // The world effects a pinch-hold cycles through: every
    // `*_xr_world*.pfx` staged into the effects dir, in file-name order,
    // starting at `debug.fosfora.effect` (added if it is not one of them).
    let mut world_effects = if mode == Mode::World {
        discover_world_effects(&dirs.assets.join("effects"))
    } else {
        Vec::new()
    };
    let start_effect =
        debug_prop("debug.fosfora.effect").unwrap_or_else(|| DEFAULT_WORLD_EFFECT.to_owned());
    let mut world_index = world_effects
        .iter()
        .position(|e| *e == start_effect)
        .unwrap_or_else(|| {
            world_effects.insert(0, start_effect.clone());
            0
        });
    if mode == Mode::World {
        info!("world effects (pinch-hold cycles): {world_effects:?}");
    }
    let mut scene = None;
    // World mode: the effects not showing, by index in `world_effects`.
    let mut parked: Vec<Option<XrScene>> = Vec::new();
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
                // Every world effect is built up front (~0.5 s each on the
                // Quest 3) and parked, so a pinch-hold swaps in an instant
                // instead of stalling the frame loop for a rebuild.
                for (i, effect) in world_effects.iter().enumerate() {
                    let started = Instant::now();
                    let built = new_world_scene(
                        &gfx,
                        &dirs.config,
                        effect,
                        WorldOptions {
                            count: count_knob,
                            size_scale,
                            sim_enabled,
                            anchor: cube_center,
                        },
                    );
                    // The chosen effect must build; another that fails (a
                    // shader the device rejects, a preset mid-edit) is left
                    // out of the cycle instead of taking the app down.
                    let s = match built {
                        Ok(s) => s,
                        Err(e) if i != world_index => {
                            log::warn!("world effect '{effect}' left out of the cycle: {e:#}");
                            parked.push(None);
                            continue;
                        }
                        Err(e) => return Err(e),
                    };
                    info!(
                        "world effect '{effect}' built in {:.0} ms",
                        started.elapsed().as_secs_f64() * 1e3
                    );
                    parked.push(Some(s));
                    if i == world_index {
                        scene = parked[i].take();
                    }
                }
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
    // The hand menu: turn the left palm toward you, point the right hand at
    // it and pinch, or poke it. For now it holds one toggle, the debug panel
    // (for development; off until turned on), saved across launches until
    // turned off again. The knob, when set, overrides and is saved too. The
    // debug panel's -/+ rows drive these values from then on; with it off
    // they stay the knobs'. While the menu is up its pinches are its own.
    let hud_test = toggle("debug.fosfora.hudtest", false);
    let menu_file = dirs.config.join(HAND_MENU_FILE);
    let debug_on = match debug_prop("debug.fosfora.hud").as_deref() {
        Some("1") => {
            save_hand_menu(&menu_file, true);
            true
        }
        Some("0") => {
            save_hand_menu(&menu_file, false);
            false
        }
        _ => load_hand_menu(&menu_file),
    };
    info!(
        "hand menu: debug panel {}",
        if debug_on { "on" } else { "off" }
    );
    let mut hud =
        (hud_test || mr.hands && session.has_hands()).then(|| Hud::new(&mut gfx, debug_on));
    // The runtime's performance counters feed only the debug panel.
    let mut perf_on = hud.as_ref().is_some_and(Hud::debug);
    session.set_perf_metrics(perf_on);
    let mut controls = Controls {
        gravity,
        near_fade: near_cull,
        hand_pad,
        hand_kick,
        reach_threshold,
        reach_gain,
        hand_scare,
    };
    let mut reach = crate::reach::Reach::new(reach_threshold, reach_gain);
    // This frame's reach per hand, and the furthest (real, virtual) since
    // the last log line.
    let mut reach_now: [Option<crate::reach::HandReach>; 2] = [None; 2];
    let mut reach_max = [(0.0f32, 0.0f32); 2];
    // Hand poses: the detector, each lane row's hold, what was last logged,
    // and each hand's curl range and highest palm up-ness since the last
    // log line (the readings the thresholds are tuned on).
    let mut poses = Poses::default();
    let mut holds = [HoldTrack::default(); 2];
    let mut pose_logged = (Default::default(), false);
    let mut curl_range = [(f32::MAX, 0.0f32); 2];
    let mut up_max = [f32::MIN; 2];
    // Each hand's pose for the debug panel's hands line, and the one-line
    // summary for the log.
    let mut pose_text = [String::from("open"), String::from("open")];
    let mut pose_label = String::from("-");
    let (mut last_rms, mut last_bass) = (0.0f32, 0.0f32);
    // Board #3317: the floor ripple, on the scene floor or the stage floor.
    let ripple_ceiling = debug_prop("debug.fosfora.rippletest").as_deref() == Some("ceiling");
    let mut ripple = toggle("debug.fosfora.ripple", mixed).then(|| {
        crate::ripple::Ripple::new(
            knob("debug.fosfora.ripplespeed", crate::ripple::SPEED_M_S).max(0.1),
            knob("debug.fosfora.ripplegain", 1.0).max(0.0),
        )
    });
    info!(
        "floor ripple {} · surface weights table {} floor {}",
        ripple.as_ref().map_or("off".to_owned(), |r| format!(
            "on (speed {} m/s, gain {})",
            r.speed, r.gain
        )),
        surface_weights.table,
        surface_weights.floor
    );
    // Board #3327: the wall spectrum, on the wall the wearer faces.
    let canvas_ceiling = debug_prop("debug.fosfora.canvastest").as_deref() == Some("ceiling");
    let mut canvas = toggle("debug.fosfora.canvas", mixed).then(|| {
        crate::canvas::Canvas::new(
            debug_prop("debug.fosfora.canvasbars")
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(crate::canvas::DEFAULT_BARS),
            knob("debug.fosfora.canvasgain", 1.0).max(0.0),
        )
    });
    info!(
        "wall spectrum {}",
        canvas.as_ref().map_or("off".to_owned(), |c| format!(
            "on ({} bars, gain {}){}",
            c.bars,
            c.gain,
            if canvas_ceiling {
                " · test: on the ceiling"
            } else {
                ""
            }
        ))
    );
    let mut wall_pick = crate::canvas::WallPick::default();
    // The picked wall (room box index, face center) and the mel length,
    // for the log: logged when the pick changes and once a second.
    let mut canvas_wall: Option<(usize, glam::Vec3)> = None;
    let mut canvas_mel = 0usize;
    // A floor surface; it emits only while the room has no FLOOR anchor
    // (set per frame below), so the floor never emits twice.
    let floor_box = floor.then_some(ObstacleBox {
        center: [0.0, -FLOOR_HALF_THICKNESS_M, 0.0],
        rot: [0.0, 0.0, 0.0, 1.0],
        half: [FLOOR_HALF_M, FLOOR_HALF_THICKNESS_M, FLOOR_HALF_M],
        kind: crate::surfaces::KIND_FLOOR,
        emit: 0.0,
        hidden: false,
    });
    // I5 gestures: a pinch-drag moves the cube and the world anchor with
    // the hand, a tap toggles the S5 sprite size, a hold cycles the world
    // effects.
    let mut gestures = Gestures::default();
    let (mut anchor, mut anchor_half) = (cube_center, cube_half);
    let mut drag_total = glam::Vec3::ZERO;
    // Closest thumb-index approach per hand since the last log (meters):
    // shows near-miss pinches that never crossed the threshold.
    let mut tip_min = [f32::MAX; 2];
    let mut size_boost = false;
    let mut recentered = false;
    // A world effect to switch to after this frame (the scene is borrowed
    // by the frame while the gesture fires).
    let mut switch_to: Option<usize> = None;
    let cycle_test_s = debug_prop("debug.fosfora.cycletest")
        .and_then(|v| v.parse::<f32>().ok())
        .filter(|s| *s > 0.0);
    let mut last_switch_t = 0.0f32;
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
        let frames_window = stats.last;
        session.frame(&gfx, clear, &mut stats, particles.as_ref(), scene_mut, |input, mut scene| {
            // S7: hands and room as obstacles. I5: pinch gestures.
            for (h, d) in input.hands.tip_distance.iter().enumerate() {
                if let Some(d) = d {
                    tip_min[h] = tip_min[h].min(*d);
                }
            }
            // The panel first. While it is up its pinches are its own: no
            // cloud gestures from either hand (a pinch that just missed a
            // row moved the cloud), except one already in progress.
            let panel_up = hud.as_mut().is_some_and(|h| {
                h.record(&input.perf);
                if hud_test {
                    h.place_parked(&gfx, input.head, input.head_rot);
                    false
                } else {
                    h.place(&gfx, &input.hands, input.head, input.head_rot);
                    h.shown()
                }
            });
            let pinches = [0, 1].map(|h| PinchInput {
                pinching: input.hands.pinching[h] && !(panel_up && gestures.owner() != Some(h)),
                point: input.hands.pinch_point[h],
            });
            let mut moved = false;
            for g in gestures.step(pinches, dt) {
                match g {
                    Gesture::Tap { hand } => {
                        size_boost = !size_boost;
                        info!(
                            "gesture: tap {} · sprite size x{} {}",
                            hand_name(hand),
                            PINCH_SIZE_BOOST,
                            if size_boost { "on" } else { "off" }
                        );
                    }
                    Gesture::DragStart { hand } => {
                        drag_total = glam::Vec3::ZERO;
                        info!(
                            "gesture: drag {} began · anchor ({:.2}, {:.2}, {:.2})",
                            hand_name(hand),
                            anchor[0],
                            anchor[1],
                            anchor[2]
                        );
                    }
                    Gesture::Drag { delta, .. } => {
                        let d = glam::Vec3::from(delta);
                        drag_total += d;
                        anchor = (glam::Vec3::from(anchor) + d).to_array();
                        moved = true;
                    }
                    Gesture::DragEnd { hand } => info!(
                        "gesture: drag {} ended · moved {:.2} m · anchor ({:.2}, {:.2}, {:.2})",
                        hand_name(hand),
                        drag_total.length(),
                        anchor[0],
                        anchor[1],
                        anchor[2]
                    ),
                    Gesture::Hold { hand } => {
                        info!("gesture: hold {}", hand_name(hand));
                        if world && world_effects.len() > 1 {
                            switch_to = cycle_index(&parked, world_index, 1);
                        }
                    }
                }
            }
            if let Some(h) = hud.as_mut() {
                if hud_test || h.shown() {
                    let view = View {
                        mode: mode_name,
                        effect: world.then(|| {
                            (
                                world_effects[world_index].as_str(),
                                world_index,
                                world_effects.len(),
                            )
                        }),
                        alive: scene.as_deref().map(XrScene::alive_count),
                        frames: frames_window,
                        perf: input.perf,
                        tracked: input.hands.tracked,
                        tip_mm: input.hands.tip_distance.map(|d| d.map(|d| d * 1000.0)),
                        pinching: input.hands.pinching,
                        gesture: gestures.label(),
                        anchor,
                        room_boxes: input.room_boxes.len(),
                        rms: last_rms,
                        bass: last_bass,
                        beat: beat_env,
                        audio: &audio_source,
                        reach: reach_now.map(|r| r.map(|r| (r.real_m, r.virtual_m))),
                        pose: &pose_text,
                    };
                    for action in h.render(&gfx, &view, &mut controls) {
                        info!("debug panel: {action:?}");
                        let n = world_effects.len();
                        match action {
                            Action::NextEffect if world && n > 1 => {
                                switch_to = cycle_index(&parked, world_index, 1);
                            }
                            Action::PrevEffect if world && n > 1 => {
                                switch_to = cycle_index(&parked, world_index, -1);
                            }
                            Action::Recenter => {
                                anchor = [input.head[0], MR_CUBE_Y, input.head[2]];
                                moved = true;
                            }
                            Action::SetDebug(on) => save_hand_menu(&menu_file, on),
                            _ => {}
                        }
                    }
                }
            }
            if let Some(p) = particles.as_ref() {
                if recenter_on_wearer && !recentered {
                    recentered = true;
                    let center = [input.head[0], MR_CUBE_Y, input.head[2]];
                    (anchor, anchor_half) = (center, MR_CUBE_HALF_WEARER_M);
                    moved = true;
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
                if moved {
                    p.set_cube(anchor, anchor_half);
                    if let Some(s) = scene.as_deref_mut() {
                        s.set_anchor(anchor);
                    }
                }
                // Seated reach: each hand's joint set moves out along the
                // arm past a comfortable reach. Only the sim sees the far
                // hand; occlusion (the skinned mesh), pinch gestures and the
                // panel stay on the real one. (Without the mesh the sphere
                // occluders come from this set and would follow the far
                // hand; the mesh is on by default and always offered on
                // the Quest 3.)
                let mut offsets = [glam::Vec3::ZERO; 2];
                reach.threshold = controls.reach_threshold;
                reach.gain = controls.reach_gain;
                for h in 0..2 {
                    reach_now[h] = reach_on
                        .then(|| {
                            reach.update(
                                h,
                                input.hands.palm[h].map(|(p, _)| glam::Vec3::from(p)),
                                glam::Vec3::from(input.head),
                                glam::Quat::from_array(input.head_rot),
                                dt,
                            )
                        })
                        .flatten();
                    if let Some(r) = reach_now[h] {
                        offsets[h] = r.offset;
                        reach_max[h].0 = reach_max[h].0.max(r.real_m);
                        reach_max[h].1 = reach_max[h].1.max(r.virtual_m);
                    }
                }
                // Hand poses (board #3314): read on the real hand, acting
                // at the far hand. Only the effects that read the lanes get
                // a behavior per pose; elsewhere every hand is the hawk.
                let pose_frame = poses.step(
                    [0, 1].map(|h| HandInput {
                        palm: input.hands.palm[h]
                            .map(|(p, q)| (glam::Vec3::from(p), glam::Quat::from_array(q))),
                        tips: input.hands.finger_tips[h].map(|t| t.map(glam::Vec3::from)),
                    }),
                    dt,
                );
                let poses_apply = poses_on
                    && world
                    && POSE_EFFECTS.contains(&world_effects[world_index].as_str());
                let tuning = Tuning {
                    fist_pad: controls.hand_pad,
                    fist_kick: controls.hand_kick,
                    open_pad,
                    open_kick,
                    hold_radius,
                };
                let plans = if poses_apply {
                    let far_palm = [0, 1].map(|h| {
                        input.hands.palm[h].map(|(p, q)| {
                            (
                                glam::Vec3::from(p) + offsets[h],
                                glam::Quat::from_array(q) * glam::Vec3::NEG_Y,
                            )
                        })
                    });
                    crate::pose::decide(&pose_frame, far_palm, panel_up, &tuning)
                } else {
                    [Plan::hawk(&tuning); 2]
                };
                for h in 0..2 {
                    if let Some(c) = pose_frame.curl[h] {
                        curl_range[h] = (curl_range[h].0.min(c), curl_range[h].1.max(c));
                    }
                    if let Some(u) = pose_frame.up[h] {
                        up_max[h] = up_max[h].max(u);
                    }
                }
                let seen = (pose_frame.pose, pose_frame.together);
                if poses_apply && seen != pose_logged {
                    pose_logged = seen;
                    let reading = |v: Option<f32>| v.map_or("-".to_owned(), |v| format!("{v:.3}"));
                    info!(
                        "pose: L {} · R {} · together {} (curl L {} R {} m · up L {} R {} · palms {} m apart{})",
                        pose_name(pose_frame.pose[0]),
                        pose_name(pose_frame.pose[1]),
                        if pose_frame.together { "yes" } else { "no" },
                        reading(pose_frame.curl[0]),
                        reading(pose_frame.curl[1]),
                        reading(pose_frame.up[0]),
                        reading(pose_frame.up[1]),
                        reading(pose_frame.apart_m),
                        if panel_up { " · menu up" } else { "" }
                    );
                }
                let mut behaviors = [Behavior::before_lanes(controls.hand_kick); 2];
                for h in 0..2 {
                    let (hold, ended) = holds[h].update(plans[h].hold, dt);
                    if let Some((kind, secs, travel)) = ended {
                        info!(
                            "hold {} ({kind:?}) ended after {secs:.1} s, carried {travel:.2} m",
                            hand_name(h)
                        );
                    }
                    if let Some(hold) = hold.filter(|h| h.age_s == 0.0) {
                        info!(
                            "hold {} began: center ({:.2}, {:.2}, {:.2}) radius {:.2} m",
                            hand_name(h),
                            hold.center.x,
                            hold.center.y,
                            hold.center.z,
                            hold.radius
                        );
                    }
                    let p = plans[h];
                    behaviors[h] = Behavior {
                        scare: p.scare,
                        push: p.push,
                        kick: p.kick,
                        hold,
                    };
                }
                pose_text = [0, 1].map(|h| {
                    if !poses_apply {
                        return "open".to_owned();
                    }
                    let pose = if pose_frame.together {
                        "together"
                    } else {
                        pose_name(pose_frame.pose[h])
                    };
                    let hold = holds[h]
                        .current()
                        .map_or(String::new(), |(_, s)| format!(" hold {s:.1} s"));
                    format!("{pose}{hold}")
                });
                pose_label = if poses_apply {
                    format!("L {}   R {}", pose_text[0], pose_text[1])
                } else {
                    "off (every hand a predator)".to_owned()
                };
                let lanes = crate::pose::lane_rows(
                    u32::try_from(input.hands.sphere_count[0]).unwrap_or(0),
                    behaviors,
                    controls.hand_kick,
                    glam::Vec3::from(anchor),
                );
                // The occluder shrink is one value for every sphere: the
                // smallest pad, so no occluder is smaller than its joint.
                let min_pad = plans[0].pad.min(plans[1].pad);
                let mut set = ObstacleSet::new(
                    MR_RESTITUTION,
                    SPRITE_RADIUS_M * size_scale,
                    (min_pad - hand_occ).max(0.0),
                    controls.hand_kick,
                );
                let mut ghost = Vec::new();
                for (i, s) in input.hands.spheres.iter().enumerate() {
                    let h = usize::from(i >= input.hands.sphere_count[0]);
                    let c = glam::Vec3::new(s[0], s[1], s[2]) + offsets[h];
                    set.push_sphere([c.x, c.y, c.z, s[3] + plans[h].pad]);
                    if offsets[h].length() > REACH_GHOST_M {
                        ghost.push([c.x, c.y, c.z, REACH_GHOST_RADIUS_M]);
                    }
                }
                // The far hand: small sprites on its joints and a faint beam
                // from the real wrist to the virtual one.
                let extension = offsets[0].length().max(offsets[1].length());
                gfx.set_ghost(
                    &ghost,
                    input.head,
                    (extension / 0.3).min(1.0) * REACH_GHOST_ALPHA,
                );
                for h in 0..2 {
                    let beam = input.hands.wrist[h]
                        .filter(|_| offsets[h].length() > REACH_GHOST_M)
                        .map(|w| {
                            let w = glam::Vec3::from(w);
                            crate::gfx::Beam {
                                start: w.to_array(),
                                end: (w + offsets[h]).to_array(),
                                eye: input.head,
                                width_m: 0.004,
                                alpha: REACH_BEAM_ALPHA,
                            }
                        });
                    gfx.set_beam(crate::gfx::BEAM_REACH[h], beam);
                }
                for b in &input.room_boxes {
                    set.push_box(b);
                }
                if let Some(f) = &floor_box {
                    set.push_box(&ObstacleBox {
                        emit: crate::surfaces::synthetic_floor_emit(
                            input.room_boxes.iter().map(|b| b.kind),
                        ),
                        ..*f
                    });
                }
                p.set_obstacles(queue, &set);
                if let Some(s) = scene.as_deref_mut() {
                    s.set_world_inputs(
                        queue,
                        input.head,
                        controls.near_fade,
                        controls.gravity,
                        controls.hand_scare,
                        &set,
                        surface_weights,
                        &lanes,
                    );
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
            (last_rms, last_bass) = (f.rms, f.bass);
            if hop.beat_fired {
                beat_env = 1.0;
                if flash {
                    flash_frames = 2;
                }
                if let Some(p) = &playback {
                    // Beat timing evidence (S6): where in the clip the
                    // beat fired, against the track's known 140 BPM grid.
                    info!(
                        "beat at clip {:.4} s · out latency {:.0} ms · tap delay {:.0} ms",
                        p.position_secs(),
                        p.latency_ms(),
                        p.tap_delay_ms()
                    );
                }
            }
            beat_env *= (-dt * 6.0).exp();
            if let Some(r) = ripple.as_mut() {
                r.update(
                    t,
                    dt,
                    glam::Vec3::from(input.head),
                    hop.beat_fired,
                    f.bass.max(f.sub_bass),
                    f.bass,
                );
                // The largest scene floor, else the stage floor.
                let scene_floor = input
                    .room_boxes
                    .iter()
                    .filter(|b| b.kind == crate::surfaces::KIND_FLOOR)
                    .map(|b| {
                        crate::surfaces::TopFace::of(
                            glam::Vec3::from(b.center),
                            glam::Quat::from_array(b.rot),
                            glam::Vec3::from(b.half),
                        )
                    })
                    .max_by(|a, b| a.area().total_cmp(&b.area()));
                let stage_top = floor_box.map(|b| b.center[1] + b.half[1]);
                let quad = if ripple_ceiling {
                    input
                        .room_boxes
                        .iter()
                        .find(|b| b.kind == crate::surfaces::KIND_CEILING)
                        .map(|b| {
                            let center = glam::Vec3::from(b.center);
                            crate::ripple::Ripple::quad_under(
                                crate::surfaces::TopFace::of(
                                    center,
                                    glam::Quat::from_array(b.rot),
                                    glam::Vec3::from(b.half),
                                ),
                                center,
                            )
                        })
                } else {
                    r.quad(scene_floor, stage_top)
                };
                let rows = quad.map(|corners| r.uniform(t, corners));
                gfx.set_ripple(rows.as_ref());
            }
            if let Some(c) = canvas.as_mut() {
                c.update(dt, &hop.frame.mel);
                canvas_mel = hop.frame.mel.len();
                let head = glam::Vec3::from(input.head);
                let forward = glam::Quat::from_array(input.head_rot) * glam::Vec3::NEG_Z;
                let face_of = |b: &ObstacleBox| {
                    crate::canvas::WallFace::of(
                        glam::Vec3::from(b.center),
                        glam::Quat::from_array(b.rot),
                        glam::Vec3::from(b.half),
                        head,
                    )
                };
                // The room's walls, less the ones the runtime hides.
                let walls: Vec<(usize, crate::canvas::WallFace)> = input
                    .room_boxes
                    .iter()
                    .enumerate()
                    .filter(|(_, b)| b.kind == crate::surfaces::KIND_WALL && !b.hidden)
                    .map(|(i, b)| (i, face_of(b)))
                    .collect();
                let faces: Vec<_> = walls.iter().map(|w| w.1).collect();
                let picked = wall_pick
                    .update(head, forward, &faces, dt)
                    .map(|k| walls[k]);
                let face = if canvas_ceiling {
                    input
                        .room_boxes
                        .iter()
                        .enumerate()
                        .find(|(_, b)| b.kind == crate::surfaces::KIND_CEILING)
                        .map(|(i, b)| (i, face_of(b)))
                } else {
                    picked
                };
                let now = face.map(|(i, f)| (i, f.center));
                if now.map(|w| w.0) != canvas_wall.map(|w| w.0) {
                    match now {
                        Some((i, c)) => info!(
                            "wall spectrum: on box {i} ({} walls) at ({:.2}, {:.2}, {:.2})",
                            walls.len(),
                            c.x,
                            c.y,
                            c.z
                        ),
                        None => info!("wall spectrum: no wall in front ({} walls)", walls.len()),
                    }
                }
                canvas_wall = now;
                let rows = face.map(|(_, f)| c.uniform(f.corners()));
                gfx.set_canvas(rows.as_ref());
            }
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
        if let Some(h) = &hud
            && h.debug() != perf_on
        {
            perf_on = h.debug();
            session.set_perf_metrics(perf_on);
        }
        if hud_test
            && frame_index == 720
            && let Some(h) = &hud
            && let Err(e) = h.dump(&gfx, &dirs.config.join("hud.rgba"))
        {
            log::warn!("debug panel dump: {e:#}");
        }
        if let Some(every) = cycle_test_s
            && world
            && t - last_switch_t > every
        {
            switch_to = cycle_index(&parked, world_index, 1);
        }
        if let Some(next) = switch_to.take()
            && let Some(mut s) = parked[next].take()
        {
            // The parked effect resumes where it stopped, at today's anchor.
            last_switch_t = t;
            s.set_anchor(anchor);
            parked[world_index] = scene.replace(s);
            world_index = next;
            info!(
                "world effect switched to '{}' ({}/{})",
                world_effects[next],
                next + 1,
                world_effects.len()
            );
        }
        if frame_index.is_multiple_of(72) {
            if let Some(scene) = &scene {
                info!(
                    "particles alive {} · emitter weight {:.2}",
                    scene.alive_count(),
                    scene.emitter_weight()
                );
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
                if let Some(c) = &canvas {
                    let heights = c.heights();
                    let peak = heights.iter().copied().fold(0.0f32, f32::max);
                    let mean = heights.iter().sum::<f32>() / heights.len().max(1) as f32;
                    info!(
                        "wall spectrum: {} · {} bars from {} mel bands · top {:.2} · height mean {:.2} max {:.2}",
                        canvas_wall.map_or("no wall".to_owned(), |(i, p)| format!(
                            "box {i} at ({:.2}, {:.2}, {:.2})",
                            p.x, p.y, p.z
                        )),
                        heights.len(),
                        canvas_mel,
                        c.top(),
                        mean,
                        peak
                    );
                }
                // Seated reach: each hand's furthest real and virtual
                // shoulder-to-palm distance this second.
                if reach_on && reach_max.iter().any(|m| m.0 > 0.0) {
                    info!(
                        "reach: L {:.2} -> {:.2} m · R {:.2} -> {:.2} m (1:1 within {:.2}, gain {:.1})",
                        reach_max[0].0,
                        reach_max[0].1,
                        reach_max[1].0,
                        reach_max[1].1,
                        controls.reach_threshold,
                        controls.reach_gain
                    );
                }
                reach_max = [(0.0, 0.0); 2];
                // Hand poses: each hand's curl range and highest palm
                // up-ness this second, against the thresholds.
                if curl_range.iter().any(|r| r.0 < f32::MAX) {
                    let range = |r: (f32, f32)| {
                        if r.0 == f32::MAX {
                            "-".to_owned()
                        } else {
                            format!("{:.3}..{:.3}", r.0, r.1)
                        }
                    };
                    let up = |u: f32| {
                        if u == f32::MIN {
                            "-".to_owned()
                        } else {
                            format!("{u:.2}")
                        }
                    };
                    info!(
                        "poses: curl L {} R {} m (fist under {}, open over {}) · up max L {} R {} (palm up over {}) · {}",
                        range(curl_range[0]),
                        range(curl_range[1]),
                        crate::pose::FIST_ON_M,
                        crate::pose::FIST_OFF_M,
                        up(up_max[0]),
                        up(up_max[1]),
                        crate::pose::PALM_UP_ON,
                        pose_label
                    );
                }
                curl_range = [(f32::MAX, 0.0); 2];
                up_max = [f32::MIN; 2];
                let mm = |d: f32| {
                    if d == f32::MAX {
                        "-".to_owned()
                    } else {
                        format!("{:.0}", d * 1000.0)
                    }
                };
                // Only seconds where a pinch was attempted (tips within 5 cm).
                if tip_min.iter().any(|&d| d < 0.05) {
                    info!(
                        "pinch: closest tips L {} R {} mm (on under {:.0}) · anchor ({:.2}, {:.2}, {:.2})",
                        mm(tip_min[0]),
                        mm(tip_min[1]),
                        crate::input::PINCH_ON_M * 1000.0,
                        anchor[0],
                        anchor[1],
                        anchor[2]
                    );
                }
                tip_min = [f32::MAX; 2];
            }
            if let Some(p) = &playback {
                info!(
                    "playback: {:.1} s into the clip · {} frames played at {} Hz · out latency {:.0} ms · tap delay {:.0} ms · xruns {}",
                    p.position_secs(),
                    p.frames_played(),
                    p.output_rate(),
                    p.latency_ms(),
                    p.tap_delay_ms(),
                    p.xruns()
                );
            }
        }
    }

    drop(session);
    drop(hud);
    drop(scene);
    drop(parked);
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
    /// The last completed one-second window, for the debug panel.
    pub last: FrameWindow,
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
            self.last = FrameWindow {
                fps: (f64::from(self.frames) / elapsed.as_secs_f64()) as f32,
                display_hz: hz as f32,
                max_interval_ms: (self.max_interval.as_secs_f64() * 1e3) as f32,
                cpu_avg_ms: cpu_avg as f32,
                long_frames: self.long_frames,
            };
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

/// A pose for the log and the panel: `lost` for an untracked hand.
fn pose_name(pose: Option<Pose>) -> &'static str {
    pose.map_or("lost", Pose::label)
}

fn hand_name(h: usize) -> &'static str {
    if h == 0 { "left" } else { "right" }
}

/// An Android system property, for spike-time knobs. Empty means unset.
/// The next world effect to switch to from `from`, `step` = 1 forward or
/// -1 back, skipping slots with nothing parked (the effect showing now, and
/// any that failed to build at launch). `None` when nothing else is parked.
fn cycle_index(parked: &[Option<XrScene>], from: usize, step: isize) -> Option<usize> {
    let n = parked.len();
    (1..n)
        .map(|k| {
            let offset = (k as isize * step).rem_euclid(n as isize) as usize;
            (from + offset) % n
        })
        .find(|&i| parked[i].is_some())
}

fn debug_prop(name: &str) -> Option<String> {
    let out = std::process::Command::new("getprop")
        .arg(name)
        .output()
        .ok()?;
    let v = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    (!v.is_empty()).then_some(v)
}

/// The hand menu's saved state under the config dir.
const HAND_MENU_FILE: &str = "hand_menu.json";

/// Whether the saved hand menu has the debug panel on (off when there is
/// no file or it does not parse).
fn load_hand_menu(path: &std::path::Path) -> bool {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get("debug")?.as_bool())
        .unwrap_or(false)
}

/// Save the hand menu's debug toggle; a failure is logged, not fatal.
fn save_hand_menu(path: &std::path::Path, debug: bool) {
    let json = serde_json::json!({ "debug": debug }).to_string();
    match std::fs::write(path, json) {
        Ok(()) => info!(
            "hand menu: debug panel {} (saved)",
            if debug { "on" } else { "off" }
        ),
        Err(e) => log::warn!("hand menu: saving {}: {e}", path.display()),
    }
}

/// The world-layout effect `effect`, simulated around `options.anchor`.
fn new_world_scene(
    gfx: &Gfx,
    config: &std::path::Path,
    effect: &str,
    options: WorldOptions,
) -> Result<XrScene> {
    let scene_dir = write_single_effect_scene(config, effect).context("writing the scene")?;
    XrScene::new_world(
        &gfx.device,
        &gfx.queue,
        &scene_dir,
        effect,
        NOMINAL_FPS,
        options,
    )
    .with_context(|| format!("creating the world scene for '{effect}'"))
}

/// Names of the world-layout presets in `effects_dir` (files named
/// `*_xr_world*.pfx`), in file-name order.
fn discover_world_effects(effects_dir: &std::path::Path) -> Vec<String> {
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(effects_dir)
        .map(|dir| {
            dir.filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| {
                    p.extension().is_some_and(|x| x == "pfx")
                        && p.file_stem()
                            .and_then(|s| s.to_str())
                            .is_some_and(|s| s.contains("_xr_world"))
                })
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    files
        .iter()
        .filter_map(|p| {
            let json: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok()?;
            Some(json.get("name")?.as_str()?.to_owned())
        })
        .collect()
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
