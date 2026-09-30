//! The Android main loop: activity lifecycle events, OpenXR session events and
//! the frame loop, all on the `android_main` thread (see "Threads" in
//! `docs/xr/XR_DESIGN.md`).

use std::time::{Duration, Instant};

use android_activity::{AndroidApp, MainEvent, PollEvent};
use anyhow::{Context, Result};
use fosfora_app::settings::ParticleQuality;
use log::{error, info};

use crate::audio::LiveAudio;
use crate::env_depth::EnvDepthOptions;
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
/// The world effect a launch starts on: the surface-born one, the room
/// editor's (Kevin, Sep 30; the names as the wearer sees them since then:
/// Embers was Embers, Flux Cloud was Flux Cloud, Flock was
/// Flock; Flux Cloud Coarse, a sprite-size diagnostic, left the
/// pinch-hold cycle and stays reachable by `debug.fosfora.effect`).
const DEFAULT_WORLD_EFFECT: &str = "Embers";
/// The world effects that read the per-hand lanes (board #3314): there the
/// hand's pose picks its behavior and its pad. The others keep one behavior
/// for every hand (Flux Cloud's worn-approved pad and kick).
const POSE_EFFECTS: [&str; 1] = ["Flock"];
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
/// World-effect defaults (Flux Cloud, board #3276): its 2-5 mm sprites
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
/// The room editor's pick beam (board #3326): as wide as the reach beam,
/// brighter, since it is the thing aimed with.
const PICK_BEAM_WIDTH_M: f32 = 0.004;
const PICK_BEAM_ALPHA: f32 = 0.35;
/// `debug.fosfora.picktest`: the ray's tilt below the view.
const PICK_TEST_TILT_DEG: f32 = 20.0;
/// The room editor's status while it is off.
const EDIT_OFF: &str = "edit room off";

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
    /// C3b: a core effect's world-layout variant (Flux Cloud) instead of
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
    //   adb shell setprop debug.fosfora.rescan 1|query           (once the room is in: launch Space Setup and requery (1), or requery alone (query), replacing the anchors; the hand menu's "Rescan the room" over adb)
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
    //   adb shell setprop debug.fosfora.mode world               (C3b: Flux Cloud through render_world, over the mr setup;
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
    //   adb shell setprop debug.fosfora.floorweight 0.5          (surface emitters, Embers: the floor's emitter weight, 0..1)
    //   adb shell setprop debug.fosfora.tableweight 1            (surface emitters: scales every table's weight; the largest
    //       emitting table inside the volume gets this, the others by top-face area against it)
    //   adb shell setprop debug.fosfora.ripple 0|1               (the floor ripple: rings from under the head on each beat, on a floor whose behavior is the ripple, a floor's default; default on in mr/world)
    //   adb shell setprop debug.fosfora.ripplegain 1             (ripple brightness multiplier; 1 = peak alpha 0.25)
    //   adb shell setprop debug.fosfora.ripplespeed 2.5          (ripple ring speed, m/s)
    //   adb shell setprop debug.fosfora.rippletest ceiling       (diagnostic: the ripple under the room's CEILING anchor instead of on
    //       the floor, lifted toward the room, for an unworn screencap from a headset lying face up)
    //   adb shell setprop debug.fosfora.canvas 0|1               (the wall spectrum: mel bars on the wall the wearer faces; default on in mr/world)
    //   adb shell setprop debug.fosfora.canvasgain 1             (wall spectrum brightness multiplier; 1 = peak alpha 0.25)
    //   adb shell setprop debug.fosfora.canvasbars 24            (wall spectrum bar count, 1..64; fewer when the mel spectrum is shorter)
    //   adb shell setprop debug.fosfora.canvastest ceiling       (diagnostic: the wall spectrum on the room's CEILING anchor instead of a wall,
    //       for an unworn screencap from a headset lying face up)
    //   adb shell setprop debug.fosfora.surface "wall=none,#3=ripple,1a2b3c4d=embers@0.5"   (board #3326: each room surface's behavior,
    //       saved per room in rooms/<room id>.json under the config dir; comma-separated <target>=<behavior>[@<strength>], target = an
    //       anchor UUID (32 hex, or a prefix of 8 or more unique in the room), #<k> a box by index (the log's "room <id>:" lane table;
    //       the stage floor too) or a kind (table, floor, wall, ceiling, frame, other: its default and every anchor of it), behavior =
    //       none, embers, sparks, spectrum, ripple, strength 0..1 (default 1); "clear" drops every assignment; live: polled once a
    //       second and applied when it changes, a bad value applies nothing; unset, each room keeps its file)
    //   adb shell setprop debug.fosfora.editroom 0|1             (board #3326: the room editor on at launch, as the hand menu's "Edit room" turns it on:
    //       the right far hand's beam picks a room surface, a pinch cycles its behavior through what renders on its kind (table, other:
    //       none, embers, sparks; floor: none, sparks, ripple; wall: none, spectrum; ceiling, frame: none), a pinch-hold cycles every surface
    //       of its kind one step past it, through the same lanes and room file as debug.fosfora.surface; default 0, not saved)
    //   adb shell setprop debug.fosfora.cloud 0|1                (board #3326: the cloud at launch, as the hand menu's "Cloud" row turns it on and off:
    //       off, the world effect is hidden at once, whatever the effect, its sim stepping on so on shows it as it would have been (the
    //       pitcher's pour and the throw's bursts are its particles and hide with it), Edit room on or off; default 1, not saved)
    //   adb shell setprop debug.fosfora.picktest 3               (diagnostic: the room editor's ray from 0.5 m ahead of the head along the view tilted
    //       20 degrees down, untracked, with a synthetic right-hand tap every 3 s, for an unworn check; implies editroom 1)
    //   adb shell setprop debug.fosfora.throw 0|1                (Flux world effects: a pinch tap throws a burst where the far hand points; default on)
    //   adb shell setprop debug.fosfora.burstcount 6000          (particles a throw bursts into on impact)
    //   adb shell setprop debug.fosfora.lift 0|1                 (Flux world effects: an open far palm held still, facing down, lifts embers; default on)
    //   adb shell setprop debug.fosfora.liftradius 0.35          (the lift's radius around the far palm, m)
    //   adb shell setprop debug.fosfora.throwtest 3              (diagnostic: every 3 s a right-hand throw from 0.5 m ahead of the head
    //       along the view, untracked, for an unworn check)
    //   adb shell setprop debug.fosfora.pitcher 0|1              (board #3402, Flux world effects: the particle pitcher, a stream poured from the
    //       right far palm along its normal; default 0; the hand menu's Pitcher row turns it on and off, not saved)
    //   adb shell setprop debug.fosfora.pitcherrate 4000         (the pitcher's particles per second, 500..10000; the debug panel's "pitcher /s";
    //       above ~10000/s into a full cloud the steal path costs frames, so lower the cloud density with it)
    //   adb shell setprop debug.fosfora.pitcherspeed 1.5         (the pitcher's stream speed, m/s)
    //   adb shell setprop debug.fosfora.density 1.0              (board #3402, world mode: the cloud density, the world effect's emission rate
    //       against its preset's (scaled with the count), 0.05..1; the debug panel's "cloud density"; the pitcher's pour is not scaled)
    //   adb shell setprop debug.fosfora.space 1.5                (board #3325, world mode: the space size, the half extent in meters of the cube
    //       around the anchor the particles live in and respawn out of, 0.5..6, for every world effect; unset, each keeps its preset's;
    //       the debug panel's "space half m", live, without resetting the cloud)
    //   adb shell setprop debug.fosfora.pitchertest 1            (diagnostic: the pitcher on, pouring from 0.5 m ahead of the head, along the
    //       view tilted 30 degrees down, untracked, for an unworn cost measurement)
    //   adb shell setprop debug.fosfora.envdepth 0|1             (board #3324: the live environment depth map, XR_META_environment_depth,
    //       drawn first in the eye pass as a depth occluder, so unscanned things hide the sprites; default on in mr/world, off elsewhere:
    //       it measured at 0.1-0.3 ms, inside run-to-run noise, MEASURED.md)
    //   adb shell setprop debug.fosfora.envdepthshow 0|1         (diagnostic: the same pass drawn as gray, 0 m black to 4 m white, linear in
    //       the stored bytes, still writing depth; implies the acquire, for a screencap of orientation and alignment)
    //   adb shell setprop debug.fosfora.envdepthhands 0|1        (ask the runtime to remove the hands from the depth map; default 1, the
    //       skinned hand mesh stays the hand occluder; live: polled once a second and applied when it changes, the log line says what
    //       was asked and what the runtime answered)
    //   adb shell setprop debug.fosfora.envdepthfilter 0|1|2     (the occluder's lookup: 2, the default, bilinear over the 2x2 texels around the
    //       ray, across silhouettes too, for softer object edges (board #3402); 1 edge-aware, interpolated when the four are within
    //       0.25 m of each other, else the nearest, which cut silhouettes on the texel grid ("very chonky" worn); 0 the texel under the ray)
    //   adb shell setprop debug.fosfora.envdepthnear 0.2         (depth-map distances under this are discarded, m; the API is unreliable below ~0.2 m)
    //   adb shell setprop debug.fosfora.envdepthflipv 0|1        (diagnostic: 0 reads texture row 0 as the bottom of the view, the default; the
    //       runtime renders the map in GL order, verified by envdepthcheck against the room's boxes; 1 as the top)
    //   adb shell setprop debug.fosfora.envdepthcheck 1          (self-check: once a second, with the room's boxes located, read a 40x40 grid of each
    //       depth layer back and log how well it agrees with the boxes and the floor along the same rays, as read, rows flipped
    //       and columns mirrored; implies the acquire, draws nothing by itself)
    //   adb shell setprop debug.fosfora.depthcollide 0|1         (board #3352: the live depth map as a collision source for the world sim: each
    //       frame both layers are condensed into a small atlas that the same submit copies into the sim's obstacle texture, and
    //       particles meeting a surface in it bounce off like off a box; default on in world mode wherever the depth map is on)
    //   adb shell setprop debug.fosfora.depthcollideres 160|320  (atlas texels per layer side; 160, the default, keeps the nearest of each 2x2 block)
    //   adb shell setprop debug.fosfora.depthcollidethick 0.15   (how far behind a surface a particle still collides with it, m; deeper it is left
    //       alone, the occluder hides it)
    //   adb shell setprop debug.fosfora.depthcollideevery 2      (each particle is tested against the depth map every N frames, 1..8, each on its
    //       own phase: the collide's GPU cost divided by N, a collision caught up to N-1 frames late; default 2)
    //   adb shell setprop debug.fosfora.depthcollideupload 0|1   (diagnostic: 0 runs the atlas pass and writes the sim's rows but skips the
    //       copy into the obstacle texture, so nothing collides; splits the atlas pass's cost from the copy's)
    //   The envdepth and depthcollide knobs are read at startup (restart the app after a change), except envdepthhands; with envdepth,
    //   envdepthshow, envdepthcheck and depthcollide all 0 no depth provider is created, so the baseline is the app without it.
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
    // Board #3324: the live environment depth (`env_depth.rs`).
    let env_occlude = toggle("debug.fosfora.envdepth", mixed);
    let env_show = toggle("debug.fosfora.envdepthshow", false);
    let env_check = toggle("debug.fosfora.envdepthcheck", false);
    // Board #3352: the live depth map as a collision source for the world
    // sim, on by default wherever the depth map is.
    let env_collide = world
        && toggle(
            "debug.fosfora.depthcollide",
            env_occlude || env_show || env_check,
        );
    let env_depth = (env_occlude || env_show || env_check || env_collide).then(|| {
        let d = EnvDepthOptions::default();
        EnvDepthOptions {
            occlude: env_occlude,
            show: env_show,
            hand_removal: toggle("debug.fosfora.envdepthhands", d.hand_removal),
            near_cut_m: debug_prop("debug.fosfora.envdepthnear")
                .and_then(|v| v.parse::<f32>().ok())
                .map_or(d.near_cut_m, |v| v.max(0.0)),
            flip_v: toggle("debug.fosfora.envdepthflipv", d.flip_v),
            check: env_check,
            filter_edge_m: match debug_prop("debug.fosfora.envdepthfilter").as_deref() {
                Some("0") => 0.0,
                Some("1") => crate::env_depth::FILTER_EDGE_M,
                _ => d.filter_edge_m,
            },
            collide: env_collide,
            collide_res: match debug_prop("debug.fosfora.depthcollideres").as_deref() {
                Some("320") => 320,
                _ => d.collide_res,
            },
            collide_thickness_m: debug_prop("debug.fosfora.depthcollidethick")
                .and_then(|v| v.parse::<f32>().ok())
                .map_or(d.collide_thickness_m, |v| v.clamp(0.01, 1.0)),
            collide_upload: debug_prop("debug.fosfora.depthcollideupload").as_deref() != Some("0"),
            collide_every: debug_prop("debug.fosfora.depthcollideevery")
                .and_then(|v| v.parse::<u32>().ok())
                .map_or(d.collide_every, |v| {
                    v.clamp(1, crate::env_depth::COLLIDE_EVERY_MAX)
                }),
        }
    });
    // `envdepthhands` is live: polled once a second (below) and applied
    // when it changes.
    let mut env_hands = env_depth.map(|o| o.hand_removal);
    let mr = MrOptions {
        passthrough: toggle("debug.fosfora.passthrough", mixed),
        hands: toggle("debug.fosfora.hands", mixed),
        room: toggle("debug.fosfora.room", mixed),
        scene_capture: toggle("debug.fosfora.scenecapture", false),
        rescan_at_start: match debug_prop("debug.fosfora.rescan").as_deref() {
            Some("1") => crate::room::Rescan::Capture,
            Some("query") => crate::room::Rescan::Query,
            _ => crate::room::Rescan::Off,
        },
        env_depth,
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
    // Embers, reads the weights).
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
    // The space size: the knob's for every world effect, else the showing
    // preset's until the stepper moves (outside world mode it drives
    // nothing and shows the test sim's cube).
    let mut space = crate::space::SpaceControl::new(
        debug_prop("debug.fosfora.space").and_then(|v| v.parse::<f32>().ok()),
        scene
            .as_ref()
            .and_then(XrScene::space)
            .map_or(cube_half, |(_, preset)| preset),
    );
    let mut controls = Controls {
        gravity,
        near_fade: near_cull,
        hand_pad,
        hand_kick,
        reach_threshold,
        reach_gain,
        hand_scare,
        // The pitcher's knobs, below.
        pitcher: false,
        pitcher_rate: crate::instruments::PITCHER_RATE,
        density: knob("debug.fosfora.density", 1.0).clamp(0.05, 1.0),
        space_half: space.shown(),
        edit_room: false,
        cloud: crate::room_edit::cloud_knob(debug_prop("debug.fosfora.cloud").as_deref()),
    };
    // The cloud density each world effect's emission was last set for
    // (by index in `world_effects`; `new_world` leaves it at 1).
    let mut density_set = vec![1.0f32; world_effects.len()];
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
    // Board #3327: the hands as instruments, on the Flux world sims (the
    // effects not in POSE_EFFECTS; Murmur ignores the rows).
    let throw_on = toggle("debug.fosfora.throw", true);
    let lift_on = toggle("debug.fosfora.lift", true);
    let mut thrower = crate::instruments::Thrower::new(
        debug_prop("debug.fosfora.burstcount")
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(crate::instruments::BURST_COUNT),
    );
    let mut lifter = crate::instruments::Lifter::new(
        knob(
            "debug.fosfora.liftradius",
            crate::instruments::LIFT_RADIUS_M,
        )
        .max(0.01),
    );
    // Board #3402: the particle pitcher on the right far palm; the hand
    // menu turns it on and off, the debug panel sets its rate.
    let mut pitcher = crate::instruments::Pitcher::new(
        knob(
            "debug.fosfora.pitcherrate",
            crate::instruments::PITCHER_RATE,
        )
        .clamp(
            crate::instruments::PITCHER_RATE_MIN,
            crate::instruments::PITCHER_RATE_MAX,
        ),
        knob(
            "debug.fosfora.pitcherspeed",
            crate::instruments::PITCHER_SPEED_M_S,
        )
        .clamp(0.1, 10.0),
    );
    let pitcher_test = toggle("debug.fosfora.pitchertest", false);
    pitcher.on = pitcher_test || toggle("debug.fosfora.pitcher", false);
    (controls.pitcher, controls.pitcher_rate) = (pitcher.on, pitcher.rate);
    info!(
        "instruments: throw {} (burst {}) · lift {} (radius {} m) · pitcher {} ({}/s at {} m/s){}",
        if throw_on { "on" } else { "off" },
        thrower.burst_count,
        if lift_on { "on" } else { "off" },
        lifter.radius,
        if pitcher.on { "on" } else { "off" },
        pitcher.rate,
        pitcher.speed,
        if pitcher_test {
            " · test: pouring ahead of the view"
        } else {
            ""
        }
    );
    // Board #3326: the room editor (`room_edit.rs`), off at launch unless a
    // knob turns it on; the hand menu's "Edit room" toggles it. Its status
    // for the panel, and whether it ran last frame (to clear its beam and
    // highlight once when it goes off).
    let pick_test_s = debug_prop("debug.fosfora.picktest")
        .and_then(|v| v.parse::<f32>().ok())
        .filter(|s| *s > 0.0);
    controls.edit_room = pick_test_s.is_some() || toggle("debug.fosfora.editroom", false);
    let mut editor = crate::room_edit::RoomEditor::default();
    // Its label at the surface after each action (`label.rs`).
    let mut label = crate::label::Label::default();
    let mut label_texture = crate::label::LabelTexture::new(&mut gfx);
    let mut edit_was_on = false;
    let mut edit_status = String::from(EDIT_OFF);
    // Step 2c: what the cloud ran last frame (`room_edit::Cloud`), to log
    // its changes; on at launch unless the knob turned it off.
    let mut cloud_ran = crate::room_edit::Cloud::On;
    let mut last_pick_test = 0.0f32;
    info!(
        "edit room {}{}",
        if controls.edit_room { "on" } else { "off" },
        if pick_test_s.is_some() {
            " · test: picking ahead of the view"
        } else {
            ""
        }
    );
    // A tap this frame not taken by the hand menu (the throw needs the far
    // hand, computed below), and each hand's lift as last logged.
    let mut tapped: Option<usize> = None;
    let mut lift_logged = [false; 2];
    let throw_test_s = debug_prop("debug.fosfora.throwtest")
        .and_then(|v| v.parse::<f32>().ok())
        .filter(|s| *s > 0.0);
    let mut last_throw_test = 0.0f32;
    // A floor surface; it emits only while the room has no FLOOR anchor
    // (set per frame below), so the floor never emits twice.
    let floor_box = floor.then_some(ObstacleBox {
        center: [0.0, -FLOOR_HALF_THICKNESS_M, 0.0],
        rot: [0.0, 0.0, 0.0, 1.0],
        half: [FLOOR_HALF_M, FLOOR_HALF_THICKNESS_M, FLOOR_HALF_M],
        kind: crate::surfaces::KIND_FLOOR,
        emit: 0.0,
        hidden: false,
        uuid: crate::room_file::STAGE_FLOOR_UUID,
    });
    // Board #3326: each box's behavior (the room file and the `surface`
    // knob), and the floor the ripple is on with why, for the log (`None`
    // before the first frame, so the first pick is logged too).
    let mut surface_lanes = crate::lanes::RoomLanes::new(dirs.config.clone());
    let mut ripple_floor: Option<Option<(usize, &str)>> = None;
    // I5 gestures: a pinch-drag moves the cube and the world anchor with
    // the hand, a tap toggles the S5 sprite size, a hold cycles the world
    // effects.
    let mut gestures = Gestures::default();
    let (mut anchor, mut anchor_half) = (cube_center, cube_half);
    let mut drag_total = glam::Vec3::ZERO;
    // Board #3326: the right drag in Edit room, its start time and its
    // travel so far, for the short-drag rule.
    let mut edit_drag: Option<(f32, glam::Vec3)> = None;
    // The panel's "All: none" this frame, applied once the frame's boxes
    // are known.
    let mut all_none = false;
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
        let has_room = session.has_room();
        // The hand menu's rescan, carried out of the frame closure (the
        // session is borrowed inside it).
        let mut rescan = false;
        session.frame(&gfx, clear, &mut stats, particles.as_ref(), scene_mut, |input, mut scene| {
            // Board #3326: the boxes in the obstacle block's order (the
            // room's, then the stage floor) and each one's behavior; the
            // knob waits while the room is on but has no anchors yet.
            let lane_boxes: Vec<crate::lanes::LaneBox<'_>> = input
                .room_boxes
                .iter()
                .enumerate()
                .map(|(i, b)| crate::lanes::LaneBox {
                    uuid: b.uuid,
                    kind: b.kind,
                    label: input.room_labels.get(i).map_or("", String::as_str),
                })
                .chain(floor_box.iter().map(|f| crate::lanes::LaneBox {
                    uuid: f.uuid,
                    kind: f.kind,
                    label: "",
                }))
                .collect();
            if frame_index.is_multiple_of(72) {
                surface_lanes.poll_knob(
                    system_prop("debug.fosfora.surface").as_deref(),
                    has_room && input.room_id.is_none(),
                    &lane_boxes,
                );
            }
            if surface_lanes.update(input.room_id, &lane_boxes) {
                // The wall pick is an index into the spectrum walls,
                // which may have changed.
                wall_pick = crate::canvas::WallPick::default();
            }
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
            // Board #3326: with Edit room on, the right hand's gestures are
            // the editor's (no throw, no size toggle, no anchor drag, no
            // effect cycle); the left hand's are unchanged.
            let edit_on = controls.edit_room;
            let (mut edit_tap, mut edit_hold) = (false, false);
            for g in gestures.step(pinches, dt) {
                match g {
                    Gesture::Tap { hand: 1 } if edit_on => {
                        edit_tap = true;
                        info!("gesture: tap right · edit room");
                    }
                    Gesture::Hold { hand: 1 } if edit_on => {
                        edit_hold = true;
                        info!("gesture: hold right · edit room");
                    }
                    Gesture::DragStart { hand: 1 } if edit_on => {
                        edit_drag = Some((t, glam::Vec3::ZERO));
                        info!("gesture: drag right began · edit room: the anchor stays");
                    }
                    Gesture::Drag { hand: 1, delta } if edit_on => {
                        if let Some((_, travel)) = edit_drag.as_mut() {
                            *travel += glam::Vec3::from(delta);
                        }
                    }
                    Gesture::DragEnd { hand: 1 } if edit_on => {
                        // A quick, short drag is a tap that wobbled past
                        // the drag radius (`room_edit::short_drag_is_tap`).
                        match edit_drag.take() {
                            Some((start, travel))
                                if crate::room_edit::short_drag_is_tap(t - start, travel.length()) =>
                            {
                                edit_tap = true;
                                info!("gesture: short drag right counts as a tap · edit room");
                            }
                            _ => info!("gesture: drag right ended · edit room"),
                        }
                    }
                    Gesture::Tap { hand } => {
                        // While the menu is up its pinches are its own.
                        if !panel_up {
                            tapped = Some(hand);
                        }
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
                        room: has_room,
                        room_boxes: input.room_boxes.len(),
                        rms: last_rms,
                        bass: last_bass,
                        beat: beat_env,
                        audio: &audio_source,
                        reach: reach_now.map(|r| r.map(|r| (r.real_m, r.virtual_m))),
                        pose: &pose_text,
                        edit_status: &edit_status,
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
                            Action::RescanRoom => rescan = true,
                            Action::SetDebug(on) => save_hand_menu(&menu_file, on),
                            Action::SetEditRoom(on) => info!(
                                "edit room {}",
                                if on {
                                    "on: the right hand's pinches pick room surfaces"
                                } else {
                                    "off"
                                }
                            ),
                            // Logged with the cloud's change, below.
                            Action::SetCloud(_) => {}
                            Action::AllNone => all_none = true,
                            Action::SetPitcher(on) => info!(
                                "pitcher {} ({}/s at {} m/s)",
                                if on { "on" } else { "off" },
                                controls.pitcher_rate,
                                pitcher.speed
                            ),
                            _ => {}
                        }
                    }
                }
            }
            if all_none {
                all_none = false;
                info!("edit room: every surface -> none (the panel)");
                surface_lanes.all_none(&lane_boxes);
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
                // Board #3326: the room editor, on the throw's ray from the
                // right far pinch point. Off, it runs once more to clear.
                if edit_on || edit_was_on {
                    let head = glam::Vec3::from(input.head);
                    let pick_boxes = ray_boxes(&input.room_boxes, floor_box.as_ref());
                    let mut tap = edit_tap && !panel_up;
                    let ray = match pick_test_s {
                        Some(every) if edit_on => {
                            if t - last_pick_test >= every {
                                last_pick_test = t;
                                tap = true;
                            }
                            let rot = glam::Quat::from_array(input.head_rot)
                                * glam::Quat::from_rotation_x(-PICK_TEST_TILT_DEG.to_radians());
                            crate::room_edit::Ray::through(head, head + rot * glam::Vec3::NEG_Z * 0.5)
                        }
                        _ if panel_up || !input.hands.tracked[1] => None,
                        _ => input.hands.pinch_point[1].and_then(|p| {
                            crate::room_edit::Ray::through(head, glam::Vec3::from(p) + offsets[1])
                        }),
                    };
                    let frame = editor.step(&crate::room_edit::EditInput {
                        on: edit_on,
                        ray,
                        boxes: &pick_boxes,
                        // The panel takes the right hand's pinches, and
                        // the editor holds still, so the status cell can
                        // be read for the surface under the beam.
                        frozen: panel_up && pick_test_s.is_none(),
                        tap,
                        hold: edit_hold && !panel_up,
                        dt,
                    });
                    // The pointed box as the log names it.
                    let named = |k: usize| {
                        let (label, uuid8) = lane_boxes
                            .get(k)
                            .map_or_else(|| ("?".to_owned(), "?".to_owned()), crate::lanes::box_label);
                        format!("{} ({label} {uuid8})", crate::surfaces::friendly_name(k, &lane_boxes))
                    };
                    if frame.changed && edit_on {
                        match frame.hit {
                            Some(h) => info!(
                                "edit room: pointing at {} at ({:.2}, {:.2}, {:.2})",
                                named(h.index),
                                h.point.x,
                                h.point.y,
                                h.point.z
                            ),
                            None => info!("edit room: no surface"),
                        }
                    }
                    if let Some(g) = frame.unaimed {
                        info!("edit room: {g} with no surface under the beam, nothing changed");
                    }
                    let effective = |k: usize| surface_lanes.effective(k, &lane_boxes);
                    label.step(dt);
                    // The label for an action: its text at the hit.
                    let mut labeled = None;
                    match frame.action {
                        Some(crate::room_edit::EditAction::Cycle(k)) => {
                            let before = effective(k).map(|(b, _)| b);
                            if let Some(b) = before {
                                let next = b.next_for(lane_boxes[k].kind);
                                info!("edit room: {} -> {}", named(k), next.name());
                            }
                            match surface_lanes.cycle(k, &lane_boxes) {
                                Ok(after) => {
                                    labeled = Some(crate::label::cycle_text(
                                        &crate::surfaces::friendly_name(k, &lane_boxes),
                                        before.unwrap_or_default(),
                                        after,
                                    ));
                                }
                                Err(e) => log::warn!("edit room: {e}; nothing changed"),
                            }
                        }
                        Some(crate::room_edit::EditAction::AssignKind(k)) => {
                            let before = effective(k).map(|(b, _)| b);
                            if let Some(b) = before {
                                info!(
                                    "edit room: every {} like {} -> {}",
                                    crate::surfaces::kind_name(lane_boxes[k].kind),
                                    named(k),
                                    b.next_for(lane_boxes[k].kind).name()
                                );
                            }
                            match surface_lanes.cycle_kind_of(k, &lane_boxes) {
                                Ok(after) => {
                                    labeled = Some(crate::label::class_text(
                                        lane_boxes[k].kind,
                                        before.unwrap_or_default(),
                                        after,
                                    ));
                                }
                                Err(e) => log::warn!("edit room: {e}; nothing changed"),
                            }
                        }
                        None => {}
                    }
                    if let (Some(text), Some(h)) = (labeled, frame.hit) {
                        label.show(text, h.point, h.normal);
                    }
                    if !edit_on {
                        label.clear();
                    }
                    label_texture.show(
                        &gfx,
                        label.now().map(|l| {
                            let pose = crate::label::billboard(
                                l.point,
                                l.normal,
                                head,
                                glam::Quat::from_array(input.head_rot),
                            );
                            (l.text, l.alpha, pose)
                        }),
                    );
                    edit_status = if !edit_on {
                        EDIT_OFF.to_owned()
                    } else if let Some(h) = frame.hit {
                        let behavior = surface_lanes
                            .effective(h.index, &lane_boxes)
                            .map_or("?", |(b, _)| b.name());
                        format!(
                            "{}: {behavior}",
                            crate::surfaces::friendly_name(h.index, &lane_boxes)
                        )
                    } else {
                        "no surface".to_owned()
                    };
                    gfx.set_beam(
                        crate::gfx::BEAM_PICK,
                        frame.beam.map(|(start, end)| crate::gfx::Beam {
                            start: start.to_array(),
                            end: end.to_array(),
                            eye: input.head,
                            width_m: PICK_BEAM_WIDTH_M,
                            alpha: PICK_BEAM_ALPHA,
                        }),
                    );
                    let rows = frame.hit.map(|h| {
                        let b = pick_boxes[h.index];
                        crate::highlight::uniform(
                            &crate::surfaces::Face::across(b.center, b.rot, b.half, h.normal),
                            frame.pulse,
                        )
                    });
                    gfx.set_highlight(rows.as_ref());
                    edit_was_on = edit_on;
                }
                // Step 2c/2e: the cloud toggle. Off hides the world
                // effect's draw, Edit room or not, the sim stepping on.
                // The pitcher's pour and the throw's bursts are that
                // effect's particles, so they hide with it.
                let cloud = crate::room_edit::Cloud::of(
                    controls.cloud,
                    edit_on,
                    editor.hit().map(|h| h.index),
                );
                if world && cloud != cloud_ran {
                    info!(
                        "{}",
                        cloud.describe(|k| crate::surfaces::friendly_name(k, &lane_boxes))
                    );
                }
                if world && cloud != cloud_ran && !controls.cloud && edit_on {
                    info!("cloud off, but Edit room is on: the world effect shows while editing");
                }
                cloud_ran = cloud;
                gfx.set_world_visible(cloud.visible());
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
                // The hands as instruments: the throw aimed through the far
                // pinch point, the lift at the far palm.
                let tap = tapped.take();
                let instruments = world
                    && !POSE_EFFECTS.contains(&world_effects[world_index].as_str());
                let (instrument_rows, pour_row) = if instruments {
                    let head = glam::Vec3::from(input.head);
                    // The far pinch point of this frame's throw, if any: a
                    // tap, or the unworn diagnostic's point ahead of the
                    // view.
                    let pinch_of = |hand: usize| {
                        input.hands.pinch_point[hand].map(|p| glam::Vec3::from(p) + offsets[hand])
                    };
                    let mut throw = tap.filter(|_| throw_on).map(|h| (h, pinch_of(h)));
                    if let Some(every) = throw_test_s
                        && t - last_throw_test >= every
                    {
                        last_throw_test = t;
                        let ahead = glam::Quat::from_array(input.head_rot) * glam::Vec3::NEG_Z;
                        throw = Some((1, Some(head + ahead * 0.5)));
                    }
                    if let Some((hand, pinch)) = throw {
                        let boxes = ray_boxes(&input.room_boxes, floor_box.as_ref());
                        let room_n = input.room_boxes.len();
                        let flight =
                            pinch.and_then(|p| crate::instruments::Flight::aim(head, p, &boxes));
                        match flight {
                            Some(f) => {
                                let aim = match f.hit {
                                    Some(h) => format!(
                                        "hit box {}{} (kind {}) at ({:.2}, {:.2}, {:.2}), normal ({:.2}, {:.2}, {:.2})",
                                        h.index,
                                        if h.index >= room_n { " (stage floor)" } else { "" },
                                        boxes[h.index].kind,
                                        h.point.x,
                                        h.point.y,
                                        h.point.z,
                                        h.normal.x,
                                        h.normal.y,
                                        h.normal.z
                                    ),
                                    None => format!(
                                        "miss, bursting in the air at ({:.2}, {:.2}, {:.2})",
                                        f.to.x, f.to.y, f.to.z
                                    ),
                                };
                                info!(
                                    "throw {}: {aim} · {:.2} m from the far pinch (reach {:.2} m) · flight {:.2} s · burst {}",
                                    hand_name(hand),
                                    f.from.distance(f.to),
                                    offsets[hand].length(),
                                    f.duration_s,
                                    thrower.burst_count
                                );
                                thrower.throw(hand, f);
                            }
                            None => info!("throw {}: no pinch point, skipped", hand_name(hand)),
                        }
                    }
                    let burst = thrower.step(dt);
                    // The pitcher on the right far palm, along its normal;
                    // a throw's burst has the rows first.
                    pitcher.on = controls.pitcher;
                    pitcher.rate = controls.pitcher_rate;
                    let nozzle = if pitcher_test {
                        let rot = glam::Quat::from_array(input.head_rot);
                        let ahead = rot * glam::Vec3::NEG_Z;
                        Some((
                            head + ahead * 0.5,
                            rot * glam::Quat::from_rotation_x(-30f32.to_radians()) * glam::Vec3::NEG_Z,
                        ))
                    } else {
                        input.hands.palm[1]
                            .filter(|_| input.hands.tracked[1])
                            .map(|(p, q)| {
                                (
                                    glam::Vec3::from(p) + offsets[1],
                                    glam::Quat::from_array(q) * glam::Vec3::NEG_Y,
                                )
                            })
                    };
                    let pour = pitcher.step(nozzle, burst.is_some(), dt);
                    let lift = if lift_on {
                        lifter.step(
                            [0, 1].map(|h| crate::instruments::LiftHand {
                                pose: pose_frame.pose[h],
                                far_palm: input.hands.palm[h]
                                    .map(|(p, _)| glam::Vec3::from(p) + offsets[h]),
                                normal: input.hands.palm[h]
                                    .map(|(_, q)| glam::Quat::from_array(q) * glam::Vec3::NEG_Y),
                                blocked: h == 0 && panel_up,
                            }),
                            dt,
                        )
                    } else {
                        None
                    };
                    for h in 0..2 {
                        let on = lifter.is_on(h);
                        if on != lift_logged[h] {
                            lift_logged[h] = on;
                            match lift.filter(|l| on && l.hand == h) {
                                Some(l) => info!(
                                    "lift {} on at ({:.2}, {:.2}, {:.2}), radius {:.2} m",
                                    hand_name(h),
                                    l.at.x,
                                    l.at.y,
                                    l.at.z,
                                    l.radius
                                ),
                                None => info!("lift {} {}", hand_name(h), if on { "on (the other hand's lift wins)" } else { "off" }),
                            }
                        }
                    }
                    (
                        crate::instruments::rows(
                            burst.or(pour.map(|p| p.as_burst())),
                            lift,
                            glam::Vec3::from(anchor),
                        ),
                        crate::instruments::pour_row(pour),
                    )
                } else {
                    ([[0.0; 4]; crate::instruments::INSTRUMENT_ROWS], [0.0; 4])
                };
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
                        &instrument_rows,
                        pour_row,
                        &cloud_ran.rows(surface_lanes.rows()),
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
                // Only a floor whose behavior is the ripple carries it
                // (step 2d, decision #3459): the largest scene floor on
                // the ripple, else the stage floor on it while the room
                // has no scene floor (then it stands in for one, as its
                // emitter flag and the editor's ray take it; beside a
                // scene floor it is out of the editor's reach, and its
                // default would put the ripple back under a scene floor
                // the wearer turned to none); with none, no ripple. The
                // fallback to the largest scene floor is gone: "floor
                // ripple kept rippling when I switch to embers or none"
                // (Kevin, worn, Sep 29).
                let ripple_of = |k: usize| {
                    surface_lanes.behavior(k) == crate::surfaces::SurfaceBehavior::Ripple
                };
                let largest = input
                    .room_boxes
                    .iter()
                    .enumerate()
                    .filter(|&(k, b)| b.kind == crate::surfaces::KIND_FLOOR && ripple_of(k))
                    .map(|(k, b)| {
                        (
                            k,
                            crate::surfaces::TopFace::of(
                                glam::Vec3::from(b.center),
                                glam::Quat::from_array(b.rot),
                                glam::Vec3::from(b.half),
                            ),
                        )
                    })
                    .max_by(|a, b| a.1.area().total_cmp(&b.1.area()));
                let stage_k = input.room_boxes.len();
                let stage_ripples = floor_box.is_some()
                    && !input
                        .room_boxes
                        .iter()
                        .any(|b| b.kind == crate::surfaces::KIND_FLOOR)
                    && ripple_of(stage_k);
                let (scene_floor, pick) = match largest {
                    Some((k, f)) => (Some(f), Some((k, "a scene floor on the ripple"))),
                    None if stage_ripples => {
                        (None, Some((stage_k, "the stage floor on the ripple")))
                    }
                    None => (None, None),
                };
                if !ripple_ceiling && Some(pick) != ripple_floor {
                    match pick {
                        Some((k, why)) => info!("floor ripple: on box {k} ({why})"),
                        None => info!("floor ripple: no floor on ripple"),
                    }
                    ripple_floor = Some(pick);
                }
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
                } else if pick.is_some() {
                    r.quad(scene_floor, stage_top)
                } else {
                    None
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
                // The room's walls on the spectrum (by default every wall),
                // less the ones the runtime hides.
                let walls: Vec<(usize, crate::canvas::WallFace)> = input
                    .room_boxes
                    .iter()
                    .enumerate()
                    .filter(|&(i, b)| {
                        let spectrum = crate::surfaces::SurfaceBehavior::Spectrum;
                        b.kind == crate::surfaces::KIND_WALL
                            && !b.hidden
                            && surface_lanes.behavior(i) == spectrum
                    })
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
        if rescan {
            session.rescan_room();
        }
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
        // The cloud density: the showing world effect's emission against
        // the rate `new_world` set, applied when it changes and when a
        // pinch-hold swaps in an effect set for another. The cloud toggle
        // leaves it alone: off hides the effect (step 2e; 2c took the
        // emission to 0 here).
        if world
            && let Some(s) = scene.as_mut()
            && let Some(base) = s.base_emit_rate()
            && let Some(set) = density_set.get_mut(world_index)
            && (*set - controls.density).abs() > 1e-4
        {
            *set = controls.density;
            s.set_emit_rate(base * controls.density);
            info!(
                "cloud density {:.2}: '{}' emits {:.0}/s (preset {:.0}/s)",
                controls.density,
                world_effects[world_index],
                base * controls.density,
                base
            );
        }
        // The space size: the showing world effect's volume, applied when
        // the stepper moves and when a pinch-hold swaps in an effect set
        // for another; the stepper shows the size it runs at.
        if world
            && let Some(s) = scene.as_mut()
            && let Some((requested, preset)) = s.space()
            && let Some(half) = space.update(&mut controls.space_half, requested, preset)
        {
            s.set_space_half(half);
            info!(
                "space size: '{}' at {:.2} m half extent",
                world_effects[world_index], controls.space_half
            );
        }
        // The pitcher's last 10 s, while it is on or has poured.
        if frame_index.is_multiple_of(720) {
            let stats = pitcher.take_stats();
            if pitcher.on || stats != crate::instruments::PourStats::default() {
                info!(
                    "pitcher {} at {}/s, {} m/s: {} frames pouring, {} particles asked, {} frames skipped for a throw (10 s)",
                    if pitcher.on { "on" } else { "off" },
                    pitcher.rate,
                    pitcher.speed,
                    stats.frames,
                    stats.asked,
                    stats.skipped
                );
            }
        }
        if frame_index.is_multiple_of(72) {
            if let Some(was) = env_hands {
                let now = match system_prop("debug.fosfora.envdepthhands").as_deref() {
                    Some("0") => false,
                    Some("1") => true,
                    _ => crate::env_depth::EnvDepthOptions::default().hand_removal,
                };
                if now != was {
                    env_hands = Some(now);
                    session.set_env_depth_hand_removal(now);
                }
            }
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
                    #[expect(
                        clippy::float_cmp,
                        reason = "f32::MAX/MIN are the \"none this second\" sentinels, stored verbatim"
                    )]
                    let range = |r: (f32, f32)| {
                        if r.0 == f32::MAX {
                            "-".to_owned()
                        } else {
                            format!("{:.3}..{:.3}", r.0, r.1)
                        }
                    };
                    #[expect(
                        clippy::float_cmp,
                        reason = "f32::MAX/MIN are the \"none this second\" sentinels, stored verbatim"
                    )]
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
                #[expect(
                    clippy::float_cmp,
                    reason = "f32::MAX/MIN are the \"none this second\" sentinels, stored verbatim"
                )]
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

/// The boxes a ray from a hand is cast against (the throw's and the room
/// editor's): the room's, then the stage floor while the room has no
/// floor, in the lanes' order.
fn ray_boxes(room: &[ObstacleBox], floor: Option<&ObstacleBox>) -> Vec<crate::instruments::RayBox> {
    let ray_box = |b: &ObstacleBox| crate::instruments::RayBox {
        center: glam::Vec3::from(b.center),
        rot: glam::Quat::from_array(b.rot),
        half: glam::Vec3::from(b.half),
        kind: b.kind,
    };
    let mut boxes: Vec<_> = room.iter().map(ray_box).collect();
    if !room.iter().any(|b| b.kind == crate::surfaces::KIND_FLOOR)
        && let Some(f) = floor
    {
        boxes.push(ray_box(f));
    }
    boxes
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

/// A system property read in place through bionic, without spawning
/// `getprop` as [`debug_prop`] does (milliseconds of the frame thread), so
/// a knob can be polled from the frame loop.
fn system_prop(name: &str) -> Option<String> {
    let name = std::ffi::CString::new(name).ok()?;
    let mut value = [0 as libc::c_char; libc::PROP_VALUE_MAX as usize];
    // SAFETY: `name` is a NUL-terminated string and `value` holds
    // PROP_VALUE_MAX bytes, the most the call writes (the NUL included);
    // both outlive the call.
    let len = unsafe { libc::__system_property_get(name.as_ptr(), value.as_mut_ptr()) };
    let len = usize::try_from(len)
        .ok()
        .filter(|&n| n > 0)?
        .min(value.len());
    // `c_char` is u8 on arm64 and i8 elsewhere: take the byte either way.
    let bytes: Vec<u8> = value[..len].iter().map(|&c| c.to_ne_bytes()[0]).collect();
    let v = String::from_utf8_lossy(&bytes).trim().to_owned();
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
