//! The Android main loop: activity lifecycle events, OpenXR session events and
//! the frame loop, all on the `android_main` thread (see "Threads" in
//! `docs/xr/XR_DESIGN.md`).

use std::time::{Duration, Instant};

use android_activity::{AndroidApp, MainEvent, PollEvent};
use anyhow::{Context, Result};
use fosfora_app::settings::ParticleQuality;
use log::{error, info};

use crate::audio::{LiveAudio, MicSource};
use crate::env_depth::EnvDepthOptions;
use crate::gesture::{Gesture, Gestures, PinchInput};
use crate::gfx::Gfx;
use crate::hud::{Action, Controls, FrameWindow, Hud, View};
use crate::particles3d::{ObstacleBox, ObstacleSet, Params, Particles3d};
use crate::permissions::{Change, Permissions, RECORD_AUDIO, USE_SCENE, Watch};
use crate::playback::{Playback, PlaybackOptions};
use crate::player::Music;
use crate::pose::{Behavior, HandInput, HoldTrack, Plan, Pose, Poses, Tuning};
use crate::scene::{WorldOptions, XrScene};
use crate::surfaces::SurfaceWeights;
use crate::voice::{
    Event as VoiceEvent, Note as VoiceNote, Opener as VoiceOpener, Press as VoicePress, PushToTalk,
    Voice, Window as VoiceWindow,
};
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
/// effect cycle and stays reachable by `debug.fosfora.effect`).
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
/// Board #3751: the voice path's speech model under the installed assets,
/// and its default whisper threads (`debug.fosfora.voicethreads`): the
/// spike's 0.29 s per 3 s clip on 3 threads leaves the render loop the rest.
const VOICE_MODEL: &str = "xr/models/ggml-base.en.bin";
const VOICE_THREADS: i32 = 3;
/// The longest the label's "…" waits for a transcription (s).
const VOICE_WAIT_LABEL_S: f32 = 10.0;
/// What the label says when a window closes before the model has loaded.
const VOICE_LOADING_LABEL: &str = "Voice is still loading";
/// Board #3751, V5: the on-device provider loads after the speech model,
/// or this long after launch if the speech model never does.
const LOCAL_WAIT: Duration = Duration::from_secs(15);

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
    /// the S5 test sim, over the same mixed-reality setup as `Mixed`. The
    /// default since board #3553: the room is what a launch shows.
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
    //   adb shell setprop debug.fosfora.aim 0|1                  (board #3336: the runtime's aim state chained onto the hand locate; its
    //       system-gesture flag suppresses our pinch on that hand; default on; read at startup)
    //   adb shell setprop debug.fosfora.handlead <ms>            (board #3753: the hand joints led along the runtime's velocities
    //       (XR_EXT_hand_tracking's XrHandJointVelocitiesEXT) by this many ms, 0..100, so the skinned mesh trails less on fast motion;
    //       default 0, off; read at startup; an A/B for the worn look)
    //   adb shell setprop debug.fosfora.mode world|mr|particles|quad  (default world, the room; board #3553. particles and quad are the
    //       spike's test scenes, mr the S7 setup under world)
    //   adb shell setprop debug.fosfora.audio synth|mic|micxr|aaudio|file  (default synth; mic needs RECORD_AUDIO;
    //       mic = the core's capture; micxr = an XR-owned cpal stream with debug.fosfora.micfmt i16|f32 and
    //       debug.fosfora.micrate <Hz>; aaudio = raw AAudio with debug.fosfora.micpreset
    //       unprocessed|generic|voice|camcorder and debug.fosfora.micperf lowlatency|none;
    //       file = the test clip looping on the speakers, analysis on its tap;
    //       loop = the clip on the speakers, analysis on the AAudio microphones (acoustic loopback))
    //   adb shell setprop debug.fosfora.ask 0|1        (board #3264: ask for the missing runtime permissions at launch, USE_SCENE and,
    //       for the sources that open the microphones, RECORD_AUDIO, and the shared Music/ folder's storage permission (board
    //       #5385, permissions::storage); default 1. 0 for unworn runs, where the OS dialog would sit
    //       over the view: the app still checks and logs them and picks up a later `pm grant` within 120 s)
    //   adb shell setprop debug.fosfora.file <path>   (.ogg/.mp3/.wav/.flac decoded by the core, or a raw
    //       48 kHz stereo f32 file ending in .f32, or "click" / "click:100" for a click track at that BPM;
    //       default: the bundled CC0 track under assets/audio/; board #5385: set, it is also what the Music row plays
    //       at launch on any source, looped, over the music folders and the saved track, until the row steps to a track)
    //   adb shell setprop debug.fosfora.flash 1        (whole view white for 2 frames on each beat: the
    //       audio-to-photon measurement, filmed with a phone)
    //   adb shell setprop debug.fosfora.playperf lowlatency|none   (AAudio performance mode of the playback
    //       output; default lowlatency: 36 ms to the speaker on the Quest 3 against 140-215 ms for none)
    //   adb shell setprop debug.fosfora.playbuf <ms>   (playback output buffer; default 84 ms in lowlatency so the
    //       speaker sits behind the analysis + display chain and the tap delay lines them up; 0 = AAudio's default)
    //   adb shell setprop debug.fosfora.tapdelay <ms>  (fixed delay of the analysis tap behind the frames
    //       handed to AAudio; default: estimated from the stream's DAC timestamps, minus the detection time,
    //       so beats land on the sound; 0 = the S6 behavior that flashed 115 ms early)
    //   adb shell setprop debug.fosfora.music 0|1               (board #3472: the bundled clip playing at launch, as the hand menu's Music row
    //       plays it: on the speakers, the analysis on its tap; the row's stop puts the analysis back on the launch source, synth or
    //       the microphones, and a play resumes where it stopped; board #4191: unset, it plays unless the source is mic, micxr or
    //       aaudio (synth, the default, file and loop play); 1 plays it over any source, 0 over none, file's and loop's clip
    //       included, so a quiet run needs 0; not saved; board #5385: the row plays the wearer's music folders, the bundled
    //       clip only while they are empty (or debug.fosfora.file), and unset, over a source that is no microphone, the
    //       wearer's last play or stop decides, saved in music.json under the config dir with the track)
    //   adb shell setprop debug.fosfora.quality low|medium|high|ultra|max
    //   adb shell setprop debug.fosfora.scene 1280x720
    //   adb shell setprop debug.fosfora.effect "Flux"            (mode quad; any effect)
    //   adb shell setprop debug.fosfora.emit <particles per second>   (mode quad)
    //   adb shell setprop debug.fosfora.count <particles>        (mode particles)
    //   adb shell setprop debug.fosfora.sim 0                    (freeze the S5 sim: draw cost only)
    //   adb shell setprop debug.fosfora.simcut flow,boxes        (board #3806: skip parts of the world sim's alive path to
    //       apportion its dispatch cost: flow, turb, boxes, spheres, depth, lift, edge, atomics (no alive count: the draw
    //       shows nothing), all (every one of those), path (the whole alive path: write-back and atomic only); a diagnostic,
    //       the look may break; unknown names are logged and ignored; default none; read at launch)
    //   adb shell setprop debug.fosfora.boxcells 8               (board #3808: the world sim's box cell list, N x N x N cells over the
    //       volume, each with the boxes that can touch it, so a particle tests only those; N in 2..16, 0 or 1 off (every particle
    //       against every box, the same result); default 8; read at launch)
    //   adb shell setprop debug.fosfora.flowsamples alt          (board #3810: flow-field samples per particle per frame in the world
    //       sim: 2 (two diagonals of the drift averaged, the swirl evolving in place), 1 (one diagonal: the field translates as a
    //       block, the cheap reference), alt (one, alternating the two diagonals frame to frame and particle to particle, so each
    //       particle averages them over time), altframe (one, alternating the diagonals frame to frame only, the whole cloud
    //       on the same one, which keeps a wave's texture reads local); unknown values are logged and keep the default;
    //       default altframe; read at launch)
    //   adb shell setprop debug.fosfora.size 0.5                 (sprite radius multiplier)
    //   adb shell setprop debug.fosfora.tri 0                    (6-vertex quads instead of 3-vertex sprites)
    //   adb shell setprop debug.fosfora.pull 0                   (instanced draw instead of vertex pulling)
    //   adb shell setprop debug.fosfora.hz 90                    (request a display rate)
    //   adb shell setprop debug.fosfora.eyescale 0.75            (swapchain size vs recommended)
    //   adb shell setprop debug.fosfora.faceslayer 0|1           (board #3793: the lit room surfaces in their own composition layer, a
    //       smaller per-eye swapchain between passthrough and the cloud's layer; 0 draws them in the eye pass as before; default 1 in
    //       world and mr, and only ever over passthrough; read at launch)
    //   adb shell setprop debug.fosfora.facescale 0.5            (board #3793: the faces layer's swapchain vs the recommended eye size,
    //       0.25..1.0; default 0.5; read at launch)
    //   adb shell setprop debug.fosfora.facesharpen 0|1          (board #3793: XR_FB_composition_layer_settings normal sharpening on the
    //       faces layer's upsample; default 0; read at launch)
    //   adb shell setprop debug.fosfora.mode mr                  (S7: passthrough + hands + room, no quad)
    //   adb shell setprop debug.fosfora.passthrough 0|1          (override the mode's default)
    //   adb shell setprop debug.fosfora.hands 0|1                (hand joints as obstacles + pinch)
    //   adb shell setprop debug.fosfora.micro 0|1                (board #3336: thumb microgestures, XR_META_hand_tracking_microgestures through one
    //       action set on the EXT hand interaction profile; in world mode a thumb swipe right along the index steps to the next world
    //       effect, left to the previous, either hand, not while the hand menu is up nor from the right hand in Edit room; forward
    //       and backward are logged and unassigned; the thumb tap opens the voice window (board #3751, V4, with voice on); default 1;
    //       0 creates no action set; read at launch)
    //   adb shell setprop debug.fosfora.voice 0|1                (board #3751: the voice path: hold the left fist to talk, release it (or 6 s pass)
    //       and the transcription goes through the grammar (V2, intent.rs: the hand menu's and the room editor's actions by voice), the
    //       label shows what it did or "Didn't catch that", the log both; the fist opens the window in every mode, Edit room and the hand
    //       menu included; V4: the hand menu's Voice toggle, saved in hand_menu.json ("voice", on when absent), which the knob forces
    //       and saves as debug.fosfora.hud does the debug panel's; voice needs the speech model installed
    //       (assets/xr/models/ggml-base.en.bin); on at launch, the model loads and RECORD_AUDIO is asked for on any audio source; read at launch)
    //   adb shell setprop debug.fosfora.voicethreads 3           (the transcription's whisper threads, 1..6; default 3; read at launch)
    //   adb shell setprop debug.fosfora.voicequiet 0.01          (board #3751, V4: a thumb tap opens a voice window too, either hand, which closes
    //       once speech was heard and the microphone's level (RMS of the last 50 ms, ±1 scale) has stayed under this for 0.8 s, or at 6 s,
    //       or on a second tap; default 0.01 (-40 dBFS); tune it against the room's noise; logged at launch, the closing level with each
    //       close; read at launch)
    //   adb shell setprop debug.fosfora.voicefile <path>         (unworn test: a 16 kHz mono 16-bit WAV transcribed 3 s after the model loads, as if a
    //       window had closed; the label shows it ahead of the head; read at launch)
    //   adb shell setprop debug.fosfora.voicedump 1              (board #5375: each window's audio as whisper gets it, 16 kHz mono, written by the
    //       voice thread as a 16-bit WAV to files/voice/window-<NNN>.wav, numbered from 001 per launch and logged; the voicefile clip is
    //       not dumped; default off; read at launch)
    //   adb shell "setprop debug.fosfora.say 'the desk in amber'"   (board #3751, V2: a sentence fed to the grammar as if heard, voice on or
    //       off, for the unworn gate: logged `voice: heard "…" → <intent or miss> · label "…"`, the label ahead of the head; read at launch
    //       and polled once a second, fed once per value (waiting for the room's anchors as debug.fosfora.surface does); an app cannot
    //       clear a debug property, so to say the same sentence again set it to "" first, or to another sentence)
    //   adb shell setprop debug.fosfora.agent 0|1                (board #3751, V3: a sentence the grammar cannot match goes to the language model
    //       voice.json under the config dir names (the label reads "Thinking…", then the model's sentence; the actions go through V2's
    //       path); V5: with no voice.json, or one naming no provider, the on-device model (`local.rs`) when its files are installed;
    //       board #3776: the hand menu's Agent control cycles local / the voice.json provider / off, saved, and its saved pick comes first;
    //       default 1 whenever a provider is configured or installed, 0 keeps the agent off for sweeps; read at launch; board #3776:
    //       0 is not saved, the hand menu's Agent control then reads "Agent: off" and a press turns it on from there)
    //   adb shell setprop debug.fosfora.agentpick local|anthropic|openai|off   (board #3776: the hand menu's Agent control pressed to that
    //       provider, for the unworn gate: switched and saved as a press is (hand_menu.json "agent"); read at launch and polled once a
    //       second, fed once per value as debug.fosfora.say is (set "" first to feed the same value again); a value the headset cannot
    //       pick logs `voice agent: agentpick "<v>" not available` and does nothing)
    //   adb shell setprop debug.fosfora.localthreads 3           (board #3751, V5: the on-device provider's ONNX Runtime intra-op threads, 1..6;
    //       default 3, as whisper's; read at launch)
    //   adb shell setprop debug.fosfora.localmin 0.35            (board #3751, V5: the on-device provider's confidence floor on the action kind and
    //       the value, 0..1; an answer under it is "Didn't catch that" with the grammar's hint and changes nothing; default 0.35; read
    //       at launch)
    //   adb shell setprop debug.fosfora.room 0|1                 (scene anchors as obstacles)
    //   adb shell setprop debug.fosfora.scenecapture 0|1         (no room anchors after the retries: launch Space Setup, then requery; board #5393:
    //       default on until the app has asked once, ever (hand_menu.json "space_setup_asked", saved when it launches Space Setup or its first
    //       scene query ends; a headset with a rooms/*.json from before counts as asked), off after; then only the hand menu's Rescan and the
    //       spoken rescan open it; 1 forces, 0 forbids, and a run with the knob set leaves the flag as it is)
    //   adb shell setprop debug.fosfora.roam 0|1                 (board #5393: roam forced on or off for this launch, not saved, for unworn tests where
    //       the head never moves; a press on the hand menu's Roam control or a spoken "roam" / "stay here" takes over; unset, the wearer's saved
    //       choice, else on by itself when the scan ends with no room; read at launch)
    //   adb shell setprop debug.fosfora.roamwalk 0.6             (board #5393: a fake head speed, m/s, in place of the measured one, through the same
    //       low-pass, so roam's immersion rule can be captured unworn: full immersion under 0.3, none over 0.8; logged once a second while it
    //       matters; live: polled once a second; "" back to the measured speed)
    //   adb shell setprop debug.fosfora.rescan 1|query           (once the room is in: launch Space Setup and requery (1), or requery alone (query), replacing the anchors; the hand menu's "Rescan the room" over adb)
    //   adb shell setprop debug.fosfora.anchors replay           (board #3536, a measurement aid for unworn runs: a scene query that finds no anchors
    //       replays the last located room, saved on every launch to debug/anchors.json under the config dir when its located set changes,
    //       as static boxes, until a query returns live anchors; unset or any other value off; read at launch)
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
    //       as the hand menu's effect row does; for measuring the switch unworn)
    //   adb shell setprop debug.fosfora.floorweight 0.5          (surface emitters, Embers: the floor's emitter weight, 0..1)
    //   adb shell setprop debug.fosfora.tableweight 1            (surface emitters: scales every table's weight; the largest
    //       emitting table inside the volume gets this, the others by top-face area against it)
    //   adb shell setprop debug.fosfora.ripple 0|1               (the rings, the floor ripple until D1: rings on each beat on every surface whose
    //       behavior is the rings, a floor's default, from under the head on a floor, from the face's point nearest the head elsewhere;
    //       off, no surface shows rings; default on in mr/world)
    //   adb shell setprop debug.fosfora.ripplegain 1             (rings brightness multiplier; 1 = peak alpha 0.25)
    //   adb shell setprop debug.fosfora.ripplespeed 2.5          (rings speed, m/s)
    //   adb shell setprop debug.fosfora.rippletest ceiling       (diagnostic: the rings on every CEILING anchor for the run, the room file untouched,
    //       for an unworn screencap from a headset lying face up)
    //   adb shell setprop debug.fosfora.canvas 0|1               (the wall spectrum: mel bars on the wall the wearer faces; default on in mr/world)
    //   adb shell setprop debug.fosfora.canvasgain 1             (wall spectrum brightness multiplier; 1 = peak alpha 0.25)
    //   adb shell setprop debug.fosfora.canvasbars 24            (wall spectrum bar count, 1..64; fewer when the mel spectrum is shorter)
    //   adb shell setprop debug.fosfora.canvastest ceiling       (diagnostic: the wall spectrum on the room's CEILING anchor instead of a wall,
    //       for an unworn screencap from a headset lying face up)
    //   adb shell setprop debug.fosfora.immerse 0..1             (board #5384: the immersion depth for this launch, how dark the room may get with
    //       the music, 0 off, 0.5 low, 1 full; unset, the hand menu's Immersion control as saved in hand_menu.json (full when never
    //       pressed); not saved, a press on the control takes over; read at launch)
    //   adb shell setprop debug.fosfora.immersefloor 0.15        (the passthrough image's opacity at the full level, the darkest the room gets,
    //       0..1; read at launch)
    //   adb shell setprop debug.fosfora.immersefix 0..1          (diagnostic: the immersion level pinned whatever the audio and the depth, for
    //       an unworn screencap, which includes the passthrough behind the app's layers on this OS; read at launch)
    //   adb shell setprop debug.fosfora.surface "wall=none,#3=rings,1a2b3c4d=embers@0.5"   (board #3326: each room surface's behavior,
    //       saved per room in rooms/<room id>.json under the config dir; comma-separated <target>=<behavior>[:<color>[:<band>]][@<strength>], target = an
    //       anchor UUID (32 hex, or a prefix of 8 or more unique in the room), #<k> a box by index (the log's "room <id>:" lane table;
    //       the stage floor too) or a kind (table, floor, wall, ceiling, frame, other: its default and every anchor of it), behavior =
    //       none, embers, sparks, spectrum, rings (or its old name, ripple), streamlines, curls, pulse, strength 0..1 (default 1); board #3488:
    //       an optional ":<color>[:<band>]" after the behavior, color 0..8 (0 the kind's own, 1 blue, 2 violet, 3 warm white, 4 amber,
    //       5 green, 6 teal, 7 rose, 8 the key's tint), band 0..3 (rms, bass, mid, high), e.g. "table=curls:5:2@0.8"; "clear" drops
    //       every assignment; live: polled once a
    //       second and applied when it changes, a bad value applies nothing; unset, each room keeps its file)
    //   adb shell setprop debug.fosfora.editroom 0|1             (board #3326: the room editor on at launch, as the hand menu's "Edit room" turns it on:
    //       the right far hand's beam picks a room surface, a pinch cycles its behavior through what renders on its kind (board #3488,
    //       the default first after none: table none, streamlines, curls, pulse, embers, sparks; floor none, rings, streamlines, curls,
    //       sparks; wall none, spectrum, streamlines, rings, pulse; ceiling none, rings, pulse, streamlines; frame none, pulse, rings,
    //       streamlines; other and unlabeled none, curls, streamlines, pulse, embers), a pinch-hold arms and a second within
    //       2.5 s cycles every surface of its kind one step past it (board #3336), through the same lanes and room file as
    //       debug.fosfora.surface; default 0, not saved)
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
    //   adb shell setprop debug.fosfora.gov 0|1                  (board #3789, world mode: the thermal governor steps the cloud density down by 0.1
    //       while the app GPU time is over budget for 3 s, to 0.3 at the lowest, and back up to the wearer's setting while under for 20 s;
    //       default on in world and mr (mr has no cloud density, so it idles there); read at launch)
    //   adb shell setprop debug.fosfora.govdown 0.92             (the governor's over budget, a fraction of the display period: 12.8 ms at 72 Hz)
    //   adb shell setprop debug.fosfora.govup 0.75               (the governor's under budget, a fraction of the display period: 10.4 ms at 72 Hz;
    //       held at or below govdown)
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
    //       0.25 m of each other, else the nearest, which cut silhouettes on the texel grid ("very chonky" worn); 0 the texel under the ray;
    //       0 and 1 exist on the full-screen pass only: with envdepthmesh on they log a warning and draw the full-screen pass)
    //   adb shell setprop debug.fosfora.envdepthmesh 0|1|2|4|8   (board #3813: the occluder as a mesh, one vertex per 1, 2, 4 or 8 texels of
    //       the depth map in the eye pass (320, 161, 81 or 41 per side of 320: every step-th texel and the last), the depth interpolated
    //       between them like envdepthfilter 2; the faces pass, at half the eye's resolution, at twice the step, capped at 8; default 2
    //       (faces 4), about 0.9 ms a frame under the full-screen pass, 4 about 1.3, 1 slower than it; 0 the full-screen pass, a
    //       per-pixel lookup)
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
    //   adb shell setprop debug.fosfora.liveplanes 0|1           (board #5397: live planes, surfaces found in the depth map (live_planes.rs) put into the show
    //       after the room's boxes (live_wire.rs); unset, on while roam is on and the depth provider runs; 1 also with roam off (a scanned
    //       room, for measurement), 0 never; live: polled once a second, so on and off can be A/B'd in one launch; 1 at launch creates
    //       the depth provider with every other depth knob off)
    //   adb shell setprop debug.fosfora.liveplanesgrid 64        (live planes: the read-back's cells per side of the left depth layer, 40..80; default 64;
    //       read at launch)
    //   adb shell setprop debug.fosfora.liveplaneshz 4           (live planes: read-backs a second, 1..10; default 4; read at launch)
    //   adb shell setprop debug.fosfora.liveplanesnoise 0|1      (live planes: in a scanned room, the depth map's error against the room's boxes by range,
    //       for texels whose ray meets a face more than 30 degrees off grazing, one line every 5 s ("live planes noise: ..."), to set
    //       live_planes::NOISE_K from; runs the read-back whatever roam is and creates the depth provider; default 0; read at launch)
    //   adb shell setprop debug.fosfora.liveplanesshow 0|1       (live planes: each reported plane's rectangle as an outline in its kind's color, for an
    //       unworn capture of what was found; default 0; live: polled once a second)
    //   adb shell setprop debug.fosfora.liveplanesrec 30         (board #5400: live planes: the worker records the next N seconds of read-backs the finder
    //       is given, as read back and before it takes them, to Android/data/<package>/files/debug/liveplanes-<time>-<n>.lprec for the
    //       desktop replay (live_rec.rs); read at launch, so a recording starts with the finder's first update, then polled once a second:
    //       a new value starts a new recording, unset or 0 stops one; at most 600 s; the path and count are logged when it closes)
    //   The envdepth and depthcollide knobs are read at startup (restart the app after a change), except envdepthhands; with envdepth,
    //   envdepthshow, envdepthcheck and depthcollide all 0 no depth provider is created, so the baseline is the app without it.
    // Clear a knob with `setprop debug.fosfora.<name> ""`.
    let audio_source = debug_prop("debug.fosfora.audio").unwrap_or_else(|| "synth".to_owned());
    // Board #3751: the voice path, on by default once the asset install
    // has put the speech model in place. V4: the hand menu's Voice toggle,
    // saved in the menu's file (on when the file lacks it); the knob, when
    // set, forces it and is saved, as `debug.fosfora.hud` does the debug
    // panel's.
    let voice_model = dirs.assets.join(VOICE_MODEL);
    let menu_file = dirs.config.join(HAND_MENU_FILE);
    let mut menu_saved = load_hand_menu(&menu_file);
    match debug_prop("debug.fosfora.voice").as_deref() {
        Some("1") => {
            menu_saved.voice = true;
            save_hand_menu(&menu_file, menu_saved);
        }
        Some("0") => {
            menu_saved.voice = false;
            save_hand_menu(&menu_file, menu_saved);
        }
        _ => {}
    }
    let voice_on = match (menu_saved.voice, voice_model.is_file()) {
        (false, _) => {
            info!("voice: off (the hand menu's Voice toggle, saved)");
            false
        }
        (true, false) => {
            info!("voice: off, no model at {}", voice_model.display());
            false
        }
        (true, true) => true,
    };
    // Board #3264: the runtime permissions, asked for before the session
    // so the dialog comes with the launch. USE_SCENE always (the room is
    // the product), RECORD_AUDIO when the audio source opens the
    // microphones; the missing ones in one request. The bring-up carries
    // on whichever way the wearer answers: the session, passthrough, hands
    // and the stage floor need neither, and the frame loop picks a grant
    // up without a relaunch (`permission_watch` below). Should the JNI
    // layer fail, the app runs as before it asked (everything taken as
    // granted).
    let permissions = Permissions::new(app)
        .inspect_err(|e| error!("permissions: {e:#}; carrying on as if granted"))
        .ok();
    let granted_at_launch = |name: &str| {
        permissions.as_ref().is_none_or(|p| {
            p.granted(name).unwrap_or_else(|e| {
                error!("permissions: {e:#}; taking it as granted");
                true
            })
        })
    };
    let scene_granted = granted_at_launch(USE_SCENE);
    let mic_granted = granted_at_launch(RECORD_AUDIO);
    // Board #5385: the shared Music/ folder's permission, named by the
    // app's target SDK (`permissions::storage`).
    let storage_permission = crate::permissions::storage(
        sdk_version,
        permissions.as_ref().map_or(0, |p| {
            p.target_sdk()
                .inspect_err(|e| log::warn!("permissions: {e:#}"))
                .unwrap_or(0)
        }),
    );
    let storage_granted = granted_at_launch(storage_permission);
    let ask = debug_prop("debug.fosfora.ask").as_deref() != Some("0");
    for (name, needed, granted) in [
        (USE_SCENE, true, scene_granted),
        (
            RECORD_AUDIO,
            voice_on || crate::permissions::wants_mic(&audio_source),
            mic_granted,
        ),
        (storage_permission, true, storage_granted),
    ] {
        info!(
            "permission {}: {}",
            crate::permissions::short(name),
            match (granted, needed, ask) {
                (true, _, _) => "granted".to_owned(),
                (false, false, _) => {
                    format!("missing, not needed (audio {audio_source}, voice off)")
                }
                (false, true, true) => "missing, asking".to_owned(),
                (false, true, false) => "missing, not asking (debug.fosfora.ask 0)".to_owned(),
            }
        );
    }
    let mut missing =
        crate::permissions::to_ask(&audio_source, voice_on, scene_granted, mic_granted);
    if !storage_granted {
        missing.push(storage_permission);
    }
    if ask
        && !missing.is_empty()
        && let Some(p) = &permissions
    {
        match p.request(&missing) {
            Ok(()) => info!(
                "permissions: asked for {}",
                missing
                    .iter()
                    .map(|n| crate::permissions::short(n))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Err(e) => error!("permissions: {e:#}"),
        }
    }
    let asked_at = Instant::now();
    let mut permission_watch = Watch::new(missing);
    // Board #3751: the voice path. The model loads on the voice thread
    // while the session starts; the window opens its own microphone stream
    // only with RECORD_AUDIO granted (the poll below picks a late grant up).
    // V4: a launch with the toggle off loads nothing (a sweep with
    // `debug.fosfora.voice 0` stays free of the model); the first turn on
    // from the menu loads it then, and turning it off again keeps it, so
    // the toggle is instant from then on.
    let voice_threads = debug_prop("debug.fosfora.voicethreads")
        .and_then(|v| v.trim().parse::<i32>().ok())
        .map_or(VOICE_THREADS, |n| n.clamp(1, 6));
    // Board #5375: each window's audio as whisper gets it, as a WAV under
    // the app's files, to tell a silent microphone from words whisper
    // missed.
    let voice_dump = (debug_prop("debug.fosfora.voicedump").as_deref() == Some("1"))
        .then(|| dirs.files.join(crate::voice::DUMP_DIR));
    let mut voice = if voice_on {
        let file = debug_prop("debug.fosfora.voicefile").map(std::path::PathBuf::from);
        Voice::new(&voice_model, voice_threads, file, voice_dump.clone())
            .inspect_err(|e| error!("voice: {e:#}; off"))
            .ok()
    } else {
        None
    };
    let mut voice_mic = mic_granted;
    let mut voice_mic_logged = false;
    // V4: a thumb tap's window closes on silence, under this level (RMS on
    // the ±1 scale), which the knob tunes against the room.
    let voice_quiet = debug_prop("debug.fosfora.voicequiet")
        .and_then(|v| v.trim().parse::<f32>().ok())
        .filter(|q| q.is_finite() && *q > 0.0)
        .unwrap_or(crate::voice::QUIET_LEVEL);
    info!(
        "voice: openers the left fist (the next fist closes it, or {} s) and a thumb tap (closes after {} s under {voice_quiet:.4} RMS, debug.fosfora.voicequiet, a second tap, or {} s)",
        crate::voice::FIST_MAX_S,
        crate::voice::QUIET_S,
        crate::voice::MAX_S
    );
    let mut push_to_talk = PushToTalk::new(voice_quiet);
    // V4: whether the flock last saw the left fist masked for voice, for
    // the log's one line per change.
    let mut flock_rule_logged = false;
    // The voice path's own label (the editor's is stepped and cleared by
    // Edit room), and where it floats: the left palm at the window's
    // opening, facing the head.
    let mut voice_label = crate::label::Label::default();
    let mut voice_at = (glam::Vec3::ZERO, glam::Vec3::Z);
    // V2, the grammar (`intent.rs`): the menu's actions a sentence asked
    // for, applied by the next frame's action consumer with the hand
    // menu's, and the `say` knob's last sentence fed.
    let mut voice_actions: Vec<Action> = Vec::new();
    let mut say_knob = crate::intent::SayKnob::default();
    // Board #3776: the `agentpick` knob's last value fed (the same
    // once-per-value rule as `say`).
    let mut agent_knob = crate::intent::SayKnob::default();
    // V4: "what can I say" shows the next five examples on each ask.
    let mut help_page = 0usize;
    // V3, the agent (`agent.rs`): the provider `voice.json` names, read
    // once (the key never reaches the log), and the one call in flight. A
    // newer sentence's call replaces an older one, whose answer is then
    // dropped. V5: with no `voice.json`, or one naming no provider, the
    // on-device provider (`local.rs`) when its files are installed; its
    // worker loads the model after the speech model (below). Board #3776:
    // the hand menu's Agent control picks among what is available (the
    // on-device provider, the network one `voice.json` configures, off),
    // saved in `hand_menu.json`; a saved pick that is available comes
    // first, else the default rule (local when installed). `debug.fosfora.agent 0` keeps the agent off
    // at launch without saving it, and a press turns it on from there.
    let local_files = crate::local::Files::in_dir(&dirs.assets.join(crate::local::MODELS_DIR));
    let mut available = crate::agent::Available::new(
        crate::agent::read(&dirs.config.join(crate::agent::CONFIG_FILE)),
        local_files.check(),
    );
    if let Some(why) = available.none_reason() {
        info!("voice agent: none to pick in the hand menu ({why})");
    }
    let saved_pick = menu_saved.agent.filter(|&p| available.has(p));
    let source = if saved_pick.is_some() {
        "saved"
    } else {
        "default"
    };
    let mut agent_pick = available.at_launch(menu_saved.agent);
    let mut local: Option<LocalAgent> = None;
    let mut agent: Option<std::sync::Arc<dyn crate::agent::Provider + Send + Sync>> = None;
    if debug_prop("debug.fosfora.agent").as_deref() == Some("0") {
        info!("voice agent: off (debug.fosfora.agent 0)");
        agent_pick = crate::agent::Pick::Off;
    } else {
        match (agent_pick, &available.rule) {
            (crate::agent::Pick::Off, Err(why)) if saved_pick.is_none() => {
                info!("voice agent: off ({why}) ({source})");
            }
            (crate::agent::Pick::Off, _) => info!("voice agent: off ({source})"),
            (crate::agent::Pick::Network(_), _) => {
                if let Ok(config) = &available.network {
                    info!("voice agent: on \u{b7} {} ({source})", config.summary());
                }
            }
            (crate::agent::Pick::Local, _) => {}
        }
        match switch_agent(
            agent_pick,
            &mut available,
            &mut local,
            &local_files,
            permissions.as_ref(),
        ) {
            Ok(provider) => agent = provider,
            Err(why) => {
                error!("voice agent: off ({why})");
                agent_pick = crate::agent::Pick::Off;
            }
        }
        if agent_pick == crate::agent::Pick::Local
            && let Some(l) = &local
        {
            info!("voice agent: local \u{b7} {} ({source})", l.about);
        }
    }
    let mut agent_call: Option<AgentCall> = None;
    // V5: a sentence for the on-device provider that waits for the voice
    // window to close (a `say` sentence while a window is open or its
    // transcription runs), so whisper and the cascade never run at once.
    let mut pending_call: Option<PendingCall> = None;
    // A USE_SCENE grant whose requery waits for a query or Space Setup in
    // flight, retried once a second.
    let mut room_pickup = false;
    let mode = match debug_prop("debug.fosfora.mode").as_deref() {
        Some("quad") => Mode::Quad,
        Some("mr" | "mixed") => Mode::Mixed,
        Some("particles") => Mode::Particles,
        // Board #3553: an unset knob (every launch after a reboot, and the
        // product's first) is the room, not the spike's test scene.
        _ => Mode::World,
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
    // A 0|1 knob as set, `None` unset (or any other value), for a knob
    // whose default is not a constant (`debug.fosfora.music`).
    let switch = |name: &str| match debug_prop(name).as_deref() {
        Some("0") => Some(false),
        Some("1") => Some(true),
        _ => None,
    };
    let toggle = |name: &str, default: bool| switch(name).unwrap_or(default);
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
    // Board #5397: live planes (`live_wire.rs`). The read-back is built with
    // the depth provider in the mixed modes and idles until live planes
    // run; the on/off knob is polled live, so a launch can A/B them.
    let live_grid =
        crate::live_wire::grid_knob(debug_prop("debug.fosfora.liveplanesgrid").as_deref());
    let live_hz = crate::live_wire::hz_knob(debug_prop("debug.fosfora.liveplaneshz").as_deref());
    let live_noise = toggle("debug.fosfora.liveplanesnoise", false);
    let mut live_knob = switch("debug.fosfora.liveplanes");
    let mut live_outline = toggle("debug.fosfora.liveplanesshow", false);
    let env_depth = (env_occlude
        || env_show
        || env_check
        || env_collide
        || live_noise
        || live_knob == Some(true))
    .then(|| {
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
            mesh_step: match debug_prop("debug.fosfora.envdepthmesh").as_deref() {
                Some("0") => 0,
                Some("1") => 1,
                Some("2") => 2,
                Some("4") => 4,
                Some("8") => 8,
                _ => d.mesh_step,
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
            planes_grid: if mixed { live_grid as u32 } else { 0 },
        }
    });
    // `envdepthhands` is live: polled once a second (below) and applied
    // when it changes.
    let mut env_hands = env_depth.map(|o| o.hand_removal);
    // Board #5393: Space Setup is asked for once, ever. The flag is saved
    // the first time this app launches it or its first scene query ends,
    // whatever the outcome (below, in the frame loop); after that only the
    // hand menu's Rescan and the spoken rescan open it. A rooms/*.json from
    // before the flag means the app was used, so it counts as asked. The
    // knob overrides for tests, and a run with it set leaves the flag as
    // it is.
    let capture_knob = switch("debug.fosfora.scenecapture");
    let mut space_setup_asked =
        menu_saved.space_setup_asked || crate::room_file::any_saved(&dirs.config);
    info!(
        "{}",
        match capture_knob {
            Some(on) => format!(
                "scene: Space Setup on an empty scan {} (debug.fosfora.scenecapture; the asked flag stays as it is)",
                if on { "forced on" } else { "off" }
            ),
            None if space_setup_asked => {
                "scene: Space Setup asked before: only the hand menu's Rescan and a spoken rescan open it"
                    .to_owned()
            }
            None => "scene: Space Setup not asked yet: an empty scan opens it, once".to_owned(),
        }
    );
    // Board #5393: roam, the show following the wearer (`roam.rs`): the
    // wearer's saved choice, on by itself when the scan ends with no room,
    // and forced for a launch by the knob.
    let mut roam = crate::roam::RoamState::new(menu_saved.roam, switch("debug.fosfora.roam"));
    info!("{}", crate::roam::launch_line(&roam));
    let mr = MrOptions {
        passthrough: toggle("debug.fosfora.passthrough", mixed),
        hands: toggle("debug.fosfora.hands", mixed),
        aim: toggle("debug.fosfora.aim", true),
        hand_lead_s: debug_prop("debug.fosfora.handlead")
            .and_then(|s| s.trim().parse::<f32>().ok())
            .map_or(0.0, |ms| (ms / 1000.0).clamp(0.0, 0.1)),
        room: toggle("debug.fosfora.room", mixed),
        scene_capture: capture_knob.unwrap_or(!space_setup_asked),
        rescan_at_start: match debug_prop("debug.fosfora.rescan").as_deref() {
            Some("1") => crate::room::Rescan::Capture,
            Some("query") => crate::room::Rescan::Query,
            _ => crate::room::Rescan::Off,
        },
        anchor_replay: crate::anchor_replay::replay_knob(
            debug_prop("debug.fosfora.anchors").as_deref(),
        ),
        env_depth,
        micro: toggle("debug.fosfora.micro", true),
        faces: crate::faces_layer::FacesOptions::from_knobs(
            debug_prop("debug.fosfora.faceslayer").as_deref(),
            debug_prop("debug.fosfora.facescale").as_deref(),
            debug_prop("debug.fosfora.facesharpen").as_deref(),
            mixed,
        ),
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
    let sim_cut = debug_prop("debug.fosfora.simcut").map_or(0, |v| {
        let (mask, unknown) = crate::sim_cut::parse(&v);
        for name in unknown {
            log::warn!("debug.fosfora.simcut: no part '{name}' (ignored)");
        }
        mask
    });
    let box_cells =
        debug_prop("debug.fosfora.boxcells").map_or(crate::box_cells::DEFAULT_CELLS, |v| {
            crate::box_cells::parse(&v).unwrap_or_else(|| {
                log::warn!(
                    "debug.fosfora.boxcells: '{v}' is not a cell count in 0..={} (keeping {})",
                    crate::box_cells::CELLS_MAX,
                    crate::box_cells::DEFAULT_CELLS
                );
                crate::box_cells::DEFAULT_CELLS
            })
        });
    let flow_samples = debug_prop("debug.fosfora.flowsamples").map_or_else(
        crate::box_cells::FlowSamples::default,
        |v| {
            crate::box_cells::FlowSamples::parse(&v).unwrap_or_else(|| {
                let keep = crate::box_cells::FlowSamples::default();
                log::warn!(
                    "debug.fosfora.flowsamples: '{v}' is not 2, 1, alt or altframe (keeping {})",
                    keep.describe()
                );
                keep
            })
        },
    );
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
        "mode {mode:?} · audio {audio_source} · scene {scene_w}x{scene_h} · quality {quality:?} · count {count} · sim {sim_enabled} · sim cut {} · box cells {} · flow samples {} · size x{size_scale} · tri {triangles} · pull {pull} · hz {hz:?} · eye scale {eye_scale} · mr {mr:?} · floor {floor} · gravity {gravity} · near cull {near_cull} · cube ({:.2}, {:.2}, {:.2}) half {cube_half}",
        crate::sim_cut::describe(sim_cut),
        crate::box_cells::describe(box_cells),
        flow_samples.describe(),
        cube_center[0],
        cube_center[1],
        cube_center[2]
    );

    let mut session = XrSession::new(&xr, &mut gfx, eye_scale, mr, &dirs.config)?;
    if let Some(hz) = hz {
        session.request_refresh_rate(hz);
    }

    // The launch source's analysis (the microphones; none = the synthetic
    // groove), kept open across the Music row's plays so a stop is instant.
    let mut live_audio = None;
    let playback_options = PlaybackOptions {
        low_latency: debug_prop("debug.fosfora.playperf").as_deref() != Some("none"),
        tap_delay_ms: debug_prop("debug.fosfora.tapdelay").and_then(|v| v.parse::<f32>().ok()),
        buffer_ms: debug_prop("debug.fosfora.playbuf").and_then(|v| v.parse::<f32>().ok()),
        start_frame: 0,
        looping: true,
    };
    // Board #3472: the clip on the speakers, the `file` and `loop` sources'
    // from launch, the hand menu's Music row's at runtime. Board #4191: at
    // launch unless the source is a microphone or `debug.fosfora.music 0`
    // forbids it (`music::plays_at_launch`), which holds for `file` and
    // `loop` too, so the knob's 0 means no clip whatever the source.
    // Board #5385: the row plays the wearer's music folders (`player.rs`),
    // the saved track first and the bundled clip only while they are
    // empty; `debug.fosfora.file` wins when set; unset, the knob leaves the
    // launch to the wearer's last play or stop.
    let bundled_clip = dirs.assets.join("audio").join(crate::music::CLIP);
    let file_knob = debug_prop("debug.fosfora.file").map(std::path::PathBuf::from);
    let fallback = file_knob.clone().unwrap_or_else(|| bundled_clip.clone());
    let (shared_music, own_music) = permissions.as_ref().map_or((None, None), |p| {
        p.music_dirs()
            .inspect_err(|e| error!("music: {e:#}; no music folders"))
            .unwrap_or((None, None))
    });
    if let Some(own) = &own_music
        && let Err(e) = std::fs::create_dir_all(own)
    {
        log::warn!("music: creating {}: {e}", own.display());
    }
    // Board #5400: live planes recordings go in `debug/` beside the app's
    // own `Music/` (`Android/data/<package>/files/debug`), where a USB pull
    // reaches them without run-as. The worker creates it.
    let live_rec_dir = own_music
        .as_deref()
        .and_then(std::path::Path::parent)
        .map(|files| files.join(crate::live_rec::DIR));
    let shown = |p: &Option<std::path::PathBuf>| {
        p.as_ref()
            .map_or_else(|| "none".to_owned(), |p| p.display().to_string())
    };
    info!(
        "music: folders {} ({}) and {} (the app's own)",
        shown(&shared_music),
        if storage_granted {
            "readable".to_owned()
        } else {
            format!(
                "{} missing: not scanned until it is granted",
                crate::permissions::short(storage_permission)
            )
        },
        shown(&own_music)
    );
    let saved_file = dirs.config.join(crate::playlist::SAVED_FILE);
    let saved = crate::player::load_saved(&saved_file);
    let saved_playing = saved.playing;
    let mut music = Music::new(
        fallback.clone(),
        file_knob.is_some(),
        &audio_source,
        playback_options,
        crate::player::Folders {
            shared: shared_music,
            own: own_music,
            shared_readable: storage_granted,
        },
        saved_file,
        saved,
    );
    let (music_scan, resumed) = music.scan_at_launch();
    let music_at_launch =
        crate::music::plays_at_launch(&audio_source, switch("debug.fosfora.music"), saved_playing);
    if file_knob.is_some() {
        info!(
            "music: {} tracks in the folders; debug.fosfora.file plays first: {}",
            music_scan.tracks.len(),
            fallback.display()
        );
    } else {
        info!(
            "{}",
            crate::playlist::launch_line(
                &music_scan,
                music.track_title(),
                resumed,
                music_at_launch
            )
        );
    }
    drop(music_scan);
    // Board #3264: the microphones open only with RECORD_AUDIO granted;
    // without it the synthetic groove runs (as with `synth`) and the frame
    // loop opens them, the same way, when the grant arrives.
    let mut mic_waiting = None;
    if let Some(source) = mic_source_for(&audio_source) {
        if mic_granted {
            live_audio = Some(LiveAudio::open(source)?);
        } else {
            info!("audio: RECORD_AUDIO missing: the synthetic groove until it is granted");
            mic_waiting = Some(source);
        }
    }
    match audio_source.as_str() {
        "loop" if music_at_launch => {
            let clip = std::sync::Arc::new(crate::playback::Clip::decode(&bundled_clip)?);
            let unused_tap = std::sync::Arc::new(fosfora_app::audio::capture::RingBuffer::new());
            let p = Playback::start(clip.clone(), unused_tap, playback_options)
                .context("starting playback")?;
            music.adopt(p, clip, None, bundled_clip.clone());
        }
        "file" if music_at_launch => {
            let clip = std::sync::Arc::new(crate::player::load_clip(&fallback)?);
            let tap = std::sync::Arc::new(fosfora_app::audio::capture::RingBuffer::new());
            let p = Playback::start(clip.clone(), tap.clone(), playback_options)
                .context("starting playback")?;
            let analysis = LiveAudio::from_ring(tap, p.output_rate(), p.callback_count())
                .context("analysis over the playback tap")?;
            music.adopt(p, clip, Some(analysis), fallback.clone());
        }
        "file" | "loop" => info!(
            "music: debug.fosfora.music 0: {audio_source} starts without its clip, the analysis on {} (the Music row plays the music folders, else the bundled clip)",
            crate::music::analysis(false, &audio_source).name()
        ),
        _ => {}
    }
    // The Music row's play at launch over the sources that start no clip
    // of their own, synth by default or any with `debug.fosfora.music 1`
    // (the track decodes on the music thread; the frame loop starts it).
    if music_at_launch && !music.wanted() {
        music.play_at_launch();
    }
    // The world effects the hand menu's effect row cycles through: every
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
        info!("world effects (the hand menu's effect row cycles them): {world_effects:?}");
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
                // Quest 3) and parked, so an effect switch is instant
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
                            sim_cut,
                            box_cells,
                            flow_samples,
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
    let debug_on = match debug_prop("debug.fosfora.hud").as_deref() {
        Some("1") => {
            menu_saved.debug = true;
            save_hand_menu(&menu_file, menu_saved);
            true
        }
        Some("0") => {
            menu_saved.debug = false;
            save_hand_menu(&menu_file, menu_saved);
            false
        }
        _ => menu_saved.debug,
    };
    info!(
        "hand menu: debug panel {}",
        if debug_on { "on" } else { "off" }
    );
    let mut hud =
        (hud_test || mr.hands && session.has_hands()).then(|| Hud::new(&mut gfx, debug_on));
    // Board #3789: the thermal governor (`governor.rs`) steps the cloud
    // density, so it acts in world mode only; the knob's default follows
    // the brief (on in world and mr), and mr has nothing to step.
    let gov_knob = toggle("debug.fosfora.gov", mixed);
    let gov_on = gov_knob && world;
    let gov_margins = {
        let d = crate::governor::Margins::default();
        let down = knob("debug.fosfora.govdown", d.down).clamp(0.1, 2.0);
        crate::governor::Margins {
            down,
            up: knob("debug.fosfora.govup", d.up).clamp(0.1, down),
        }
    };
    if !gov_on {
        info!(
            "governor: off{}",
            if gov_knob {
                " (world mode only: no cloud density to step)"
            } else {
                ""
            }
        );
    }
    // The runtime's performance counters feed the debug panel and the
    // governor (they cost nothing measurable, so the governor keeps them on
    // whether the panel is up or not).
    let mut perf_on = gov_on || hud.as_ref().is_some_and(Hud::debug);
    session.set_perf_metrics(perf_on);
    // The space size: the knob's for every world effect, else the showing
    // preset's until the stepper moves (outside world mode it drives
    // nothing and shows the test sim's cube).
    // Step 2h: the space size that fits the room's boxes around the
    // anchor, with how many boxes (`None` before the room is in), set each
    // frame; and the fit last applied, for the log.
    let mut room_fit: Option<(f32, usize)> = None;
    let mut fit_logged: Option<(f32, usize)> = None;
    let mut space = crate::space::SpaceControl::new(
        debug_prop("debug.fosfora.space").and_then(|v| v.parse::<f32>().ok()),
        scene
            .as_ref()
            .and_then(XrScene::space)
            .map_or(cube_half, |(_, preset)| preset),
    );
    // Board #5384: immersion (`immerse.rs`), the room dimming with the
    // music. The depth is the hand menu's saved one unless the knob gives
    // this launch another (not saved); the floor and a pinned level are
    // knobs. Without passthrough it runs, but its level is taken as 0.
    let unit_knob = |name: &str| {
        debug_prop(name)
            .and_then(|v| v.trim().parse::<f32>().ok())
            .filter(|v| v.is_finite())
            .map(|v| v.clamp(0.0, 1.0))
    };
    let immerse_knob = unit_knob("debug.fosfora.immerse");
    // Board #5393: the seated floor; roam keeps the room lighter
    // (`roam::immerse_floor`), set again on each change of roam.
    let seated_floor =
        unit_knob("debug.fosfora.immersefloor").unwrap_or(crate::immerse::IMMERSE_FLOOR);
    let mut immersion = crate::immerse::Immersion::new(
        immerse_knob.unwrap_or(menu_saved.immerse.value()),
        crate::roam::immerse_floor(roam.now().is_on(), seated_floor),
    );
    immersion.fix(unit_knob("debug.fosfora.immersefix"));
    let immerse_room = session.has_passthrough();
    info!(
        "{}",
        immersion.launch_line(immerse_knob.is_some(), immerse_room)
    );
    // What the runtime has for the passthrough style, and whether a call
    // failed (logged once).
    let mut immerse_style = crate::immerse::StyleGate::default();
    let mut immerse_failed = false;
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
        density_gov: None,
        space_half: space.shown(),
        edit_room: false,
        cloud: crate::room_edit::cloud_knob(debug_prop("debug.fosfora.cloud").as_deref()),
        music: music.wanted(),
        voice: menu_saved.voice,
        agent: available.control(agent_pick),
        immerse: immerse_knob.map_or(menu_saved.immerse, crate::immerse::Depth::nearest),
        roam: roam.now(),
    };
    // The cloud density each world effect's emission was last set for
    // (by index in `world_effects`; `new_world` leaves it at 1).
    let mut density_set = vec![1.0f32; world_effects.len()];
    // Board #3789: the governor's density under the wearer's setting, the
    // last second of app GPU time it watches and the display period its
    // launch line was logged for.
    let mut gov = crate::governor::Governor::new(gov_margins, controls.density);
    let mut gpu_window = crate::governor::GpuWindow::default();
    let mut gov_logged_period: Option<f32> = None;
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
    // Board #3317: the floor ripple's state, the rings of the surfaces pass
    // since D1 (board #3472), on every face whose behavior is the rings.
    let ripple_ceiling = debug_prop("debug.fosfora.rippletest").as_deref() == Some("ceiling");
    let mut ripple = toggle("debug.fosfora.ripple", mixed).then(|| {
        crate::ripple::Ripple::new(
            knob("debug.fosfora.ripplespeed", crate::ripple::SPEED_M_S).max(0.1),
            knob("debug.fosfora.ripplegain", 1.0).max(0.0),
        )
    });
    info!(
        "rings {}{} · surface weights table {} floor {}",
        ripple.as_ref().map_or("off".to_owned(), |r| format!(
            "on (speed {} m/s, gain {})",
            r.speed, r.gain
        )),
        if ripple_ceiling {
            " · test: on the ceiling"
        } else {
            ""
        },
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
    // Board #3488: the room scan's state on the same label while the
    // editor's shows nothing.
    let mut scan_label = crate::label::ScanLabel::default();
    // Board #5378: every surface's voice name on the surface, while the
    // editor is on, while a voice window is open and a while after, after
    // a miss on a name, and always with `debug.fosfora.labels 1`
    // (`surface_labels.rs`). Whether they showed last frame, for the log.
    let labels_knob = debug_prop("debug.fosfora.labels").as_deref() == Some("1");
    let mut surface_labels = crate::surface_labels::SurfaceLabels::default();
    let mut label_atlas = crate::surface_labels::LabelAtlas::new(&mut gfx);
    // Board #5379: where crowded labels moved last frame, so a near tie
    // keeps its order.
    let mut label_spread = crate::surface_labels::Spread::default();
    let mut labels_shown = false;
    if labels_knob {
        info!("surface labels: always shown (debug.fosfora.labels 1)");
    }
    let mut scan_logged: Option<crate::label::ScanState> = None;
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
    // knob).
    let mut surface_lanes = crate::lanes::RoomLanes::new(dirs.config.clone());
    // Board #3472: the surfaces pass's clock and its lit faces by shader
    // (rings, streamlines, curls, pulse, spectrum) and the faces past its slots, for
    // the log (`None` before the first frame, so the first count is logged
    // too).
    let mut surface_clock = crate::surface_fx::Clock::default();
    let mut surfaces_lit: Option<([usize; 6], usize)> = None;
    // Board #3489, D2b: the desktop's single-pass fragment effects on
    // surfaces (`surface_port.rs`). The eight are composed with the
    // libraries the assets carry and their pipelines built here, once;
    // `None` when that failed, and a surface on one draws nothing.
    let mut surface_ports = {
        let started = Instant::now();
        let loader = fosfora_app::effect::EffectLoader::new();
        match crate::surface_port::load(&loader) {
            Ok(effects) => {
                let sources: Vec<&str> = effects.iter().map(|e| e.source.as_str()).collect();
                if gfx.build_surface_ports(&sources, crate::surface_fx::MAX_SLOTS) {
                    info!(
                        "surface ports: {} pipelines in {:.0} ms",
                        effects.len(),
                        started.elapsed().as_secs_f64() * 1e3
                    );
                    Some(effects)
                } else {
                    None
                }
            }
            Err(e) => {
                log::warn!("surface ports: off: {e:#}");
                None
            }
        }
    };
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
    // Board #3472, D3: a surface row's step this frame (which parameter,
    // up or down), applied the same way to the surface under the beam.
    let mut surface_step: Option<(crate::lanes::Param, bool)> = None;
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
    // Board #3488: 1 on the downbeat, decaying with a 0.6 s time constant
    // (`surface_fx::downbeat_step`): the pulse's envelope.
    let mut downbeat_env = 0.0f32;
    let flash = debug_prop("debug.fosfora.flash").as_deref() == Some("1");
    let mut flash_frames = 0u32;
    // Board #5384: the last frame's immersion level (0 without
    // passthrough), with the `loudness_s` and `buildup` it followed, for the
    // style call after the frame and the log; and whether the log's last
    // line had the room dimmed.
    let mut immerse_now: Option<(f32, f32, f32)> = None;
    let mut immerse_logged = false;
    // Board #5393: roam's rules (`roam.rs`): the cloud following the head,
    // the head's speed for immersion, the switch to the effect that fills
    // its volume, and the stage floor's center (x, z), under the head in
    // roam and at the stage's origin seated. `debug.fosfora.roamwalk` fakes
    // the head's speed (polled once a second).
    let mut follow = crate::roam::Follow::default();
    let mut walk = crate::roam::Walk::default();
    let mut fill = crate::roam::Fill::default();
    let mut stage_floor_at = glam::Vec2::ZERO;
    let walk_knob = || {
        system_prop("debug.fosfora.roamwalk")
            .and_then(|v| v.trim().parse::<f32>().ok())
            .filter(|v| v.is_finite() && *v >= 0.0)
    };
    let mut roam_walk = walk_knob();
    if let Some(speed) = roam_walk {
        info!("roam: head speed faked at {speed:.2} m/s (debug.fosfora.roamwalk)");
    }
    // Board #5391, #5397: the live planes (`live_planes.rs`, wired by
    // `live_wire.rs`) are extra surface boxes found in the depth map while
    // roaming. They are chained after the room's boxes once a frame, at
    // the top of the frame below, and everything that reads the room's
    // boxes then sees them, but the space fit, the room editor, the voice
    // path's vocabulary and the surface labels. The worker thread starts
    // the first time they run; each start is a new epoch (a fresh finder).
    // The read-back exists only with the depth provider in a mixed mode.
    let live_possible = session.has_env_depth() && env_depth.is_some_and(|o| o.planes_grid > 0);
    if live_possible {
        gfx.build_live_outlines(crate::live_planes::MAX_PLANES);
    }
    let live_reading = if env_depth.is_some_and(|o| o.flip_v) {
        crate::env_depth::Reading::RowTop
    } else {
        crate::env_depth::Reading::RowBottom
    };
    let mut live_worker: Option<crate::live_wire::Worker> = None;
    let mut live_on = false;
    let mut live_epoch = 0u64;
    // The newest snapshot of this epoch, and one just in from the worker
    // (its appear and leave lines are logged in the frame).
    let mut live_snap: Option<crate::live_wire::Snapshot> = None;
    let mut live_fresh: Option<crate::live_wire::Snapshot> = None;
    let mut live_show = crate::live_wire::LiveShow::default();
    // Board #5400: `debug.fosfora.liveplanesrec`, read here so a recording
    // can start with the finder's first update, then polled once a second.
    // Its ask rides on every job and the worker writes the file; unset, the
    // jobs carry nothing.
    let mut live_rec = crate::live_rec::RecKnob::default();
    if let Some(line) = live_rec.poll(
        debug_prop(crate::live_rec::KNOB).as_deref(),
        live_rec_dir.as_deref(),
    ) {
        info!("{line}");
    }
    // The updates count at the last status line and when, for its rate.
    let mut live_rate = (Instant::now(), 0u64);
    let mut live_unavailable_logged = false;
    // The planes past the budget as last logged.
    let mut live_over_logged = 0usize;
    // The stage floor's height (`live_wire::stage_floor_y`) and the height
    // last logged.
    let mut stage_floor_y = 0.0f32;
    let mut stage_floor_logged = 0.0f32;
    // The room's boxes for the noise calibration, refreshed each frame
    // while it is on.
    let mut noise_boxes: Vec<crate::env_depth::Obb> = Vec::new();
    if live_noise {
        if live_possible {
            live_worker = spawn_live_worker();
            session.set_live_planes(Some(live_hz));
            info!(
                "live planes noise: on (debug.fosfora.liveplanesnoise): grid {live_grid} of the left layer at {live_hz} Hz against the room's boxes, a line every {} s",
                crate::live_wire::NOISE_EVERY_S
            );
        } else {
            info!("live planes noise: no depth read-back in this run");
        }
    }

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
        // Board #5393: the scan's end turns the automatic roam on (no
        // anchors) or off (a room found).
        let room_scan = session.scan_state();
        if let Some(now) = roam.scan(room_scan) {
            roam_changed(
                now,
                if now.is_on() {
                    "the scan found no room"
                } else {
                    "the scan found a room"
                },
                &mut immersion,
                seated_floor,
                &mut controls,
            );
        }
        // Board #5393: Space Setup asked for once, ever: saved the first
        // time this app launches it or its first scene query ends.
        let asked_now = if space_setup_asked || capture_knob.is_some() {
            None
        } else if session.space_setup_requested() {
            Some("Space Setup launched")
        } else {
            match room_scan {
                Some(crate::label::ScanState::Found(_)) => Some("the first scan found a room"),
                Some(crate::label::ScanState::None) => Some("the first scan ended with no room"),
                _ => None,
            }
        };
        if let Some(why) = asked_now {
            space_setup_asked = true;
            menu_saved.space_setup_asked = true;
            save_hand_menu(&menu_file, menu_saved);
            info!(
                "scene: {why}: Space Setup asked for once (saved); from now on only the hand menu's Rescan and a spoken rescan open it"
            );
        }
        // Board #3264: while USE_SCENE is awaited, a room without anchors
        // reads "Allow spatial data to see the room".
        let scan_state =
            crate::label::with_scene_permission(room_scan, permission_watch.waiting_for(USE_SCENE));
        if scan_state != scan_logged {
            if let Some(state) = scan_state {
                info!(
                    "room scan: {:?}, the label reads \"{}\"",
                    state,
                    crate::roam::scan_text(state, roam.now())
                );
            }
            scan_logged = scan_state;
        }
        // Board #5397: live planes run while roam is on (or the knob says),
        // where there is a depth read-back. Each start is a fresh finder;
        // off, the read-back stops and the planes fade out of the show.
        let live_want = crate::live_wire::runs(roam.now().is_on(), live_knob);
        if live_want && !live_possible && !live_unavailable_logged {
            live_unavailable_logged = true;
            info!(
                "live planes: no depth read-back in this run (no depth provider, or not a mixed mode)"
            );
        }
        let live_want = live_want && live_possible;
        if live_want != live_on {
            live_on = live_want;
            live_snap = None;
            live_fresh = None;
            if live_on {
                live_epoch += 1;
                live_show.clear();
                if live_worker.is_none() {
                    live_worker = spawn_live_worker();
                }
                live_rate = (Instant::now(), 0);
                info!("{}", crate::live_wire::start_line(live_grid, live_hz));
            } else {
                info!(
                    "live planes: off ({})",
                    crate::live_wire::why(roam.now().is_on(), live_knob)
                );
            }
            session.set_live_planes((live_on || live_noise).then_some(live_hz));
        }
        if let Some(w) = live_worker.as_mut()
            && let Some(snap) = w.poll()
            && live_on
            && snap.epoch == live_epoch
        {
            // Board #5400: the runtime corrected its tracking with the
            // planes in view; they moved with the world.
            if let Some(c) = snap.correction {
                info!(
                    "live planes: the tracking origin moved {:.2} m and turned {:.1}°; the planes moved with it",
                    c.moved_m, c.turned_deg
                );
            }
            live_fresh = Some(snap);
        }
        // The hand menu's rescan, carried out of the frame closure (the
        // session is borrowed inside it).
        let mut rescan = false;
        // Board #3789: this frame's app GPU time, for the governor.
        let mut gpu_frame: Option<f32> = None;
        session.frame(&gfx, clear, &mut stats, particles.as_ref(), scene_mut, |input, mut scene| {
            // Board #3789: the governor's signal, taken after the frame.
            gpu_frame = input.perf.app_gpu_ms;
            // Board #5397: a snapshot just in from the live planes' worker:
            // the planes it reports first and the ones gone, then it is the
            // frame's.
            if let Some(fresh) = live_fresh.take() {
                let before = live_snap.as_ref().map_or(&[][..], |s| s.planes.as_slice());
                let head = glam::Vec3::from(input.head);
                for line in crate::live_wire::change_lines(before, &fresh.planes, head) {
                    info!("{line}");
                }
                live_snap = Some(fresh);
            }
            // Board #5393: in roam the stage floor's slab re-centers under
            // the head once the head is 3 m from its center
            // (`roam::floor_center`); seated, it is at the stage's origin.
            // Board #5397: in roam it stands at the live planes' floor
            // height, following it (`live_wire::stage_floor_y`); seated,
            // and in roam until the finder has a floor, at y = 0.
            let roam_on = roam.now().is_on();
            let floor_at = if roam_on {
                crate::roam::floor_center(stage_floor_at, glam::Vec3::from(input.head))
            } else {
                glam::Vec2::ZERO
            };
            if floor_box.is_some() && floor_at.distance(stage_floor_at) > 1e-4 {
                info!(
                    "roam: the stage floor at ({:.2}, {:.2}){}",
                    floor_at.x,
                    floor_at.y,
                    if roam_on { ", under the head" } else { ", the stage's origin" }
                );
            }
            stage_floor_at = floor_at;
            let floor_y = crate::live_wire::stage_floor_y(
                stage_floor_y,
                roam_on,
                live_snap.as_ref().and_then(|s| s.floor_y),
                dt,
            );
            let back_to_zero = floor_y.abs() < 1e-6 && stage_floor_logged.abs() >= 1e-6;
            if floor_box.is_some() && ((floor_y - stage_floor_logged).abs() >= 0.05 || back_to_zero) {
                info!(
                    "roam: the stage floor at height {floor_y:.2}{}",
                    if roam_on { ", the live planes' floor" } else { ", seated" }
                );
                stage_floor_logged = floor_y;
            }
            stage_floor_y = floor_y;
            let floor_box = floor_box.map(|f| ObstacleBox {
                center: [floor_at.x, f.center[1] + stage_floor_y, floor_at.y],
                ..f
            });
            // Board #5391, #5397: the room's boxes and the live planes after
            // them, the one list every user of the room's boxes below reads,
            // but the space fit, the room editor, the voice path's
            // vocabulary and the surface labels, which take the room's alone
            // or pass the live ones by. A live plane on a scanned box or on
            // the stage floor is left out; the rest take the boxes the room
            // and the stage floor leave of the 32, by id, fading in and out
            // (`live_wire::LiveShow`).
            let scanned_n = input.room_boxes.len();
            let live_planes = live_snap.as_ref().map_or(&[][..], |s| s.planes.as_slice());
            let scanned: Vec<crate::env_depth::Obb> = if live_planes.is_empty() {
                Vec::new()
            } else {
                input.room_boxes.iter().map(obb_of).collect()
            };
            let stage_obb = floor_box.as_ref().map(obb_of);
            let live_room = crate::live_wire::room_for_live(scanned_n, floor_box.is_some());
            let shown = live_show.step(dt, live_planes, &scanned, stage_obb.as_ref(), live_room);
            let live_levels: Vec<f32> = shown.iter().map(|l| l.level).collect();
            let room_boxes: Vec<ObstacleBox> = input
                .room_boxes
                .iter()
                .copied()
                .chain(shown.iter().map(|l| live_box(&l.b)))
                .collect();
            let over = live_show.counts().over_budget;
            if over != live_over_logged {
                live_over_logged = over;
                info!(
                    "live planes: {over} over budget (the room's {scanned_n} boxes{} leave {live_room} of {})",
                    if floor_box.is_some() {
                        " and the stage floor"
                    } else {
                        ""
                    },
                    crate::surfaces::SURFACE_LANE_ROWS
                );
            }
            let live_range = crate::live_wire::unpickable(scanned_n, live_levels.len());
            if live_noise {
                noise_boxes.clear();
                noise_boxes.extend(input.room_boxes.iter().filter(|b| !b.hidden).map(obb_of));
            }
            // Board #5397, `debug.fosfora.liveplanesshow`: every reported
            // plane's rectangle outlined in its kind's color, the ones left
            // out on scanned boxes too.
            if live_possible {
                let head = glam::Vec3::from(input.head);
                let rows: Vec<[[f32; 4]; crate::highlight::UNIFORM_ROWS]> = if live_outline {
                    live_planes
                        .iter()
                        .map(|p| {
                            let face = crate::surfaces::acting_face(p.center, p.rot, p.half, head);
                            crate::highlight::outline(&face, crate::surfaces::palette(p.kind))
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                gfx.set_live_outlines(&rows);
            }
            // Board #3326: the boxes in the obstacle block's order (the
            // room's, then the stage floor) and each one's behavior; the
            // knob waits while the room is on but has no anchors yet.
            let lane_boxes: Vec<crate::lanes::LaneBox<'_>> = room_boxes
                .iter()
                .enumerate()
                .map(|(i, b)| crate::lanes::LaneBox {
                    uuid: b.uuid,
                    kind: b.kind,
                    label: if i < scanned_n {
                        input.room_labels.get(i).map_or("", String::as_str)
                    } else {
                        crate::live_wire::label(b.kind)
                    },
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
            // the editor's (no throw, no size toggle, no anchor drag); the
            // left hand's are unchanged.
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
                        // The sprite size toggle is the S5 test sim's, for
                        // the sweeps (board #3336): in mr and world it drew
                        // nothing and logged on every throw.
                        if mode == Mode::Particles {
                            size_boost = !size_boost;
                            info!(
                                "gesture: tap {} · sprite size x{} {}",
                                hand_name(hand),
                                PINCH_SIZE_BOOST,
                                if size_boost { "on" } else { "off" }
                            );
                        } else {
                            info!("gesture: tap {}", hand_name(hand));
                        }
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
                    // Board #3336: a hold no longer cycles the world effect
                    // (a still pinch, aiming a throw or a hand at rest, fired
                    // it unasked); the hand menu's effect row does.
                    Gesture::Hold { hand } => info!(
                        "gesture: hold {} (unassigned in the world since board #3336)",
                        hand_name(hand)
                    ),
                }
            }
            // Board #3336: thumb microgestures (`microgestures.rs`). A
            // swipe right steps to the next world effect and left to the
            // previous, either hand, by the path the menu's `<` `>` take;
            // the gesture block's rules hold (nothing while the hand menu
            // is up, nothing from the right hand in Edit room).
            {
                let micro = &input.micro;
                let effects = if world { world_effects.len() } else { 0 };
                let outcomes = [0, 1].map(|h| {
                    micro.swipe[h].map(|s| {
                        (s, crate::microgestures::outcome(s, h, panel_up, edit_on, effects))
                    })
                });
                let right_steps = outcomes[1].is_some_and(|(_, o)| o.step().is_some());
                for (h, swiped) in outcomes.iter().enumerate() {
                    if let Some((s, o)) = swiped {
                        let overruled = h == 0 && right_steps && o.step().is_some();
                        info!(
                            "microgesture: swipe {} ({} hand) -> {}{}",
                            s.label(),
                            hand_name(h),
                            o.label(),
                            if overruled { " (the right hand's swipe counts this frame)" } else { "" }
                        );
                    }
                    if micro.tap[h] {
                        info!("microgesture: tap thumb {}", hand_name(h));
                    }
                }
                if let Some(step) = crate::microgestures::consume(micro, panel_up, edit_on, effects) {
                    switch_to = cycle_index(&parked, world_index, if step > 0 { 1 } else { -1 });
                }
            }
            // The hand menu's actions this frame, after the voice path's
            // from the last (board #3751): one consumer, so a spoken
            // toggle saves, logs and acts as the menu's press does.
            let mut actions = std::mem::take(&mut voice_actions);
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
                        room_boxes: room_boxes.len(),
                        rms: last_rms,
                        bass: last_bass,
                        beat: beat_env,
                        audio: crate::music::header(music.playing(), &audio_source),
                        track: music.title(),
                        reach: reach_now.map(|r| r.map(|r| (r.real_m, r.virtual_m))),
                        pose: &pose_text,
                        edit_status: &edit_status,
                        // Last frame's hit (the editor steps below), held
                        // while the panel is up; the file as it is now.
                        surface_params: editor
                            .hit()
                            .filter(|_| controls.edit_room)
                            .map(|h| surface_lanes.params_of(h.index, &lane_boxes)),
                    };
                    for action in h.render(&gfx, &view, &mut controls) {
                        info!("debug panel: {action:?}");
                        actions.push(action);
                    }
                }
            }
            for action in actions {
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
                    Action::SetDebug(on) => {
                        menu_saved.debug = on;
                        save_hand_menu(&menu_file, menu_saved);
                    }
                    // Board #3751, V4: the Voice toggle, saved; the model
                    // loads on the first turn on (a launch with it off
                    // loaded nothing) and stays loaded when it goes off.
                    Action::SetVoice(on) => {
                        menu_saved.voice = on;
                        save_hand_menu(&menu_file, menu_saved);
                        info!("voice: {} (menu)", if on { "on" } else { "off" });
                        if !on {
                            // Off: an open window is dropped unheard and
                            // the label shows nothing.
                            push_to_talk.cancel();
                            if let Some(v) = voice.as_mut() {
                                v.cancel();
                            }
                            voice_label.clear();
                        }
                        if on && voice.is_none() {
                            if voice_model.is_file() {
                                voice = Voice::new(
                                    &voice_model,
                                    voice_threads,
                                    None,
                                    voice_dump.clone(),
                                )
                                    .inspect_err(|e| error!("voice: {e:#}; off"))
                                    .ok();
                            } else {
                                info!("voice: no model at {}, nothing listens", voice_model.display());
                            }
                        }
                        if on
                            && voice.is_some()
                            && !voice_mic
                            && ask
                            && let Some(p) = &permissions
                        {
                            match p.request(&[RECORD_AUDIO]) {
                                Ok(()) => info!("permissions: asked for RECORD_AUDIO (voice on)"),
                                Err(e) => error!("permissions: {e:#}"),
                            }
                        }
                    }
                    // Board #3776: the Agent control (or the `agentpick`
                    // knob), saved, through the one switch. A call in
                    // flight or waiting is dropped, so no stale answer
                    // lands on the room afterwards, and its "Thinking…"
                    // goes with it.
                    Action::SetAgent(pick) if available.has(pick) => {
                        menu_saved.agent = Some(pick);
                        save_hand_menu(&menu_file, menu_saved);
                        let in_flight = agent_call.take().is_some();
                        let waiting = pending_call.take().is_some();
                        if (in_flight || waiting)
                            && voice_label
                                .now()
                                .is_some_and(|l| l.text.contains(crate::agent::THINKING_LABEL))
                        {
                            voice_label.clear();
                        }
                        let fresh = local.is_none();
                        match switch_agent(
                            pick,
                            &mut available,
                            &mut local,
                            &local_files,
                            permissions.as_ref(),
                        ) {
                            Ok(provider) => {
                                agent = provider;
                                agent_pick = pick;
                                match (pick, &available.network, &local) {
                                    (crate::agent::Pick::Local, _, Some(l)) if fresh => {
                                        info!("voice agent: local (menu) \u{b7} {}", l.about);
                                    }
                                    (crate::agent::Pick::Network(_), Ok(config), _) => {
                                        info!("voice agent: {} (menu)", config.summary());
                                    }
                                    _ => info!("voice agent: {} (menu)", pick.name()),
                                }
                            }
                            Err(why) => {
                                error!("voice agent: off ({why})");
                                agent = None;
                                agent_pick = crate::agent::Pick::Off;
                            }
                        }
                        controls.agent = available.control(agent_pick);
                    }
                    Action::SetAgent(pick) => {
                        info!("voice agent: {} not available", pick.name());
                    }
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
                    Action::SurfaceParam(param, up) => surface_step = Some((param, up)),
                    Action::SetMusic(on) => music.set(on),
                    // Board #5385: the Music row's `<` `>` and the voice's
                    // songs.
                    Action::PrevTrack => music.step(-1),
                    Action::NextTrack => music.step(1),
                    Action::PlayTrack(i) => music.play_track(i),
                    Action::ScanMusic => music.rescan(),
                    // Board #3789: the wearer's own density is the
                    // governor's new ceiling, its debt cleared.
                    Action::SetDensity => {
                        if gov.governed().is_some() {
                            info!(
                                "governor: the wearer set the density {:.2} (was governed at {:.2} under {:.2})",
                                controls.density,
                                gov.density(),
                                gov.ceiling()
                            );
                        }
                        gov.set_ceiling(controls.density);
                    }
                    Action::SetPitcher(on) => info!(
                        "pitcher {} ({}/s at {} m/s)",
                        if on { "on" } else { "off" },
                        controls.pitcher_rate,
                        pitcher.speed
                    ),
                    // Board #5384: the Immersion control or a spoken
                    // "darker", saved; the level glides to the new depth.
                    Action::SetImmersion(depth) => {
                        info!(
                            "immersion: {} (depth {:.2} -> {:.2})",
                            depth.name(),
                            immersion.depth(),
                            depth.value()
                        );
                        immersion.set_depth(depth.value());
                        menu_saved.immerse = depth;
                        save_hand_menu(&menu_file, menu_saved);
                    }
                    // Board #5393: the Roam control or a spoken "roam" /
                    // "stay here", the wearer's choice, saved; it takes
                    // over from the knob.
                    Action::SetRoam(on) => {
                        let change = roam.set(on);
                        menu_saved.roam = roam.chosen();
                        save_hand_menu(&menu_file, menu_saved);
                        match change {
                            Some(now) => roam_changed(
                                now,
                                "asked",
                                &mut immersion,
                                seated_floor,
                                &mut controls,
                            ),
                            None => {
                                controls.roam = roam.now();
                                info!("roam: {} already", roam.now().describe());
                            }
                        }
                    }
                    _ => {}
                }
            }
            if all_none {
                all_none = false;
                info!("edit room: every surface -> none (the panel)");
                surface_lanes.all_none(&lane_boxes);
            }
            // Board #3472, D3: a surface row's step, on the surface under
            // the beam (the editor holds its hit while the panel is up, so
            // it is the one the rows show), through the lanes' typed
            // writer: the same entry fields the knob's
            // `:<color>:<band>@<strength>` writes, saved and logged the
            // same way. The label at the hit names the result.
            if let Some((param, up)) = surface_step.take() {
                let what = match param {
                    crate::lanes::Param::Color => "color",
                    crate::lanes::Param::Band => "band",
                    crate::lanes::Param::Strength => "strength",
                };
                let dir = if up { "up" } else { "down" };
                match editor.hit().filter(|_| controls.edit_room) {
                    Some(h) => {
                        let k = h.index;
                        let name = crate::surfaces::friendly_name(k, &lane_boxes);
                        let edit = crate::lanes::ParamEdit::step(
                            param,
                            up,
                            surface_lanes.params_of(k, &lane_boxes),
                        );
                        info!("edit room: {name} {what} {dir} (the panel)");
                        if surface_lanes.set_params(k, &lane_boxes, edit) {
                            let behavior = surface_lanes
                                .effective(k, &lane_boxes)
                                .map(|(b, _)| b)
                                .unwrap_or_default();
                            label.show(
                                crate::label::param_text(
                                    &name,
                                    behavior,
                                    surface_lanes.params_of(k, &lane_boxes),
                                ),
                                h.point,
                                h.normal,
                            );
                        }
                    }
                    None => info!(
                        "edit room: {what} {dir} with no surface under the beam, nothing changed"
                    ),
                }
            }
            // The left fist this frame, for the voice path (below).
            let mut left_fist = false;
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
                // Board #5393: in roam the cloud follows the wearer, by
                // Recenter's path (the anchor moved, `moved` set), gliding
                // there over 1.5 s once the head is a meter from it, at
                // most every 3 s (`roam::Follow`). The sim keeps its
                // particles relative to the anchor, so the cloud drifts
                // with it: none jumps, none dies.
                if roam_on && recenter_on_wearer && recentered {
                    if let Some(step) = follow.step(
                        glam::Vec3::from(input.head),
                        glam::Vec3::from(anchor),
                        MR_CUBE_Y,
                        dt,
                    ) {
                        if let Some(d) = step.started {
                            info!(
                                "roam: the cloud follows the wearer, {d:.2} m from the head, gliding over {} s",
                                crate::roam::GLIDE_S
                            );
                        }
                        anchor = step.center.to_array();
                        moved = true;
                    }
                } else {
                    follow.cancel();
                }
                if moved {
                    p.set_cube(anchor, anchor_half);
                    if let Some(s) = scene.as_deref_mut() {
                        s.set_anchor(anchor);
                    }
                }
                // The room fit for this frame's boxes and anchor (the
                // stage floor is not a room box); applied after the frame.
                // Board #5397: the room's boxes alone: with the live planes
                // the space would change size as the wearer looks around.
                room_fit = crate::space::fit_room(
                    glam::Vec3::from(anchor),
                    input.room_boxes.iter().map(|b| {
                        (
                            glam::Vec3::from(b.center),
                            glam::Quat::from_array(b.rot),
                            glam::Vec3::from(b.half),
                        )
                    }),
                )
                .map(|half| (half, scanned_n));
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
                left_fist = pose_frame.pose[0] == Some(Pose::Fist);
                let poses_apply = poses_on
                    && world
                    && POSE_EFFECTS.contains(&world_effects[world_index].as_str());
                // Board #3751, V4: with voice on the left fist toggles the
                // voice window everywhere (board #5380), so the flock sees
                // an open left hand; the right fist keeps its predator.
                // Logged once per change.
                let voice_fist = controls.voice && voice.is_some();
                let flock_rule = poses_apply && voice_fist;
                if flock_rule != flock_rule_logged {
                    flock_rule_logged = flock_rule;
                    info!(
                        "{}",
                        if flock_rule {
                            "pose: voice on, the left fist toggles the voice window and is no predator; the right fist is"
                        } else {
                            "pose: the left fist is a predator again"
                        }
                    );
                }
                let flock_frame = crate::voice::flock_poses(&pose_frame, voice_fist);
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
                    crate::pose::decide(&flock_frame, far_palm, panel_up, &tuning)
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
                    let pick_boxes = ray_boxes(&room_boxes, scanned_n, floor_box.as_ref());
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
                    // Board #5397: the live planes are not offered.
                    let frame = editor.step_except(&crate::room_edit::EditInput {
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
                    }, live_range.clone());
                    // The pointed box as the log names it.
                    let named = |k: usize| {
                        let (label, uuid8) = lane_boxes
                            .get(k)
                            .map_or_else(|| ("?".to_owned(), "?".to_owned()), crate::lanes::box_label);
                        format!("{} ({label} {uuid8})", crate::surfaces::friendly_name(k, &lane_boxes))
                    };
                    if frame.changed && edit_on {
                        match frame.hit {
                            // Which face is tinted (step 2h): a volume's
                            // top, a plane's face toward the head.
                            Some(h) => info!(
                                "edit room: pointing at {} ({}) at ({:.2}, {:.2}, {:.2})",
                                named(h.index),
                                if crate::surfaces::is_plane(pick_boxes[h.index].half) {
                                    "face"
                                } else {
                                    "top"
                                },
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
                    // Step 2h: whether box `k`'s top face lies outside the
                    // surface-born effect's volume, so it would weigh 0
                    // and emit nothing whatever it runs.
                    let space_half = scene
                        .as_deref()
                        .filter(|s| s.surface_born())
                        .and_then(XrScene::emitter_half);
                    let outside = |k: usize| {
                        space_half.zip(pick_boxes.get(k)).is_some_and(|(half, b)| {
                            !crate::surfaces::reaches(
                                b.center - glam::Vec3::from(anchor),
                                b.rot,
                                b.half,
                                half,
                            )
                        })
                    };
                    // The label's text, and a log line when the result
                    // cannot emit there.
                    let placed = |k: usize, text: String, after: crate::surfaces::SurfaceBehavior| {
                        let out = outside(k);
                        if out && after.emits() {
                            info!(
                                "edit room: {} is outside the space ({:.2} m half extent): its {} emit nothing",
                                named(k),
                                space_half.unwrap_or_default(),
                                after.name()
                            );
                        }
                        crate::label::space_text(text, after, out)
                    };
                    label.step(dt);
                    // The label for an action: its text at the hit, and
                    // how long it shows (the arm's for the whole arm).
                    let mut labeled = None;
                    let mut label_s = crate::label::LABEL_S;
                    match frame.action {
                        Some(crate::room_edit::EditAction::Cycle(k)) => {
                            let before = effective(k).map(|(b, _)| b);
                            if let Some(b) = before {
                                let next = b.next_for(lane_boxes[k].kind);
                                info!("edit room: {} -> {}", named(k), next.name());
                            }
                            match surface_lanes.cycle(k, &lane_boxes) {
                                Ok(after) => {
                                    let text = crate::label::cycle_text(
                                        &crate::surfaces::friendly_name(k, &lane_boxes),
                                        before.unwrap_or_default(),
                                        after,
                                    );
                                    labeled = Some(placed(k, text, after));
                                }
                                Err(e) => log::warn!("edit room: {e}; nothing changed"),
                            }
                        }
                        // Board #3336: a first hold only arms the class
                        // cycle; the label says what a second hold within
                        // `ARM_S` would do.
                        Some(crate::room_edit::EditAction::Arm(k)) => {
                            if let Some((b, _)) = effective(k) {
                                let kind = lane_boxes[k].kind;
                                let next = b.next_for(kind);
                                info!(
                                    "edit room: armed, every {} like {} -> {} on a second hold within {} s",
                                    crate::surfaces::kind_name(kind),
                                    named(k),
                                    next.name(),
                                    crate::room_edit::ARM_S
                                );
                                labeled = Some(crate::label::arm_text(
                                    &crate::surfaces::friendly_name(k, &lane_boxes),
                                    kind,
                                    next,
                                ));
                                label_s = crate::room_edit::ARM_S;
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
                                    let text = crate::label::class_text(
                                        lane_boxes[k].kind,
                                        before.unwrap_or_default(),
                                        after,
                                    );
                                    labeled = Some(placed(k, text, after));
                                }
                                Err(e) => log::warn!("edit room: {e}; nothing changed"),
                            }
                        }
                        None => {}
                    }
                    if let (Some(text), Some(h)) = (labeled, frame.hit) {
                        label.show_for(text, h.point, h.normal, label_s);
                    }
                    if !edit_on {
                        label.clear();
                    }
                    edit_status = if !edit_on {
                        EDIT_OFF.to_owned()
                    } else if let Some(h) = frame.hit {
                        // Board #3336: " · armed" while the class cycle is
                        // armed on it.
                        crate::label::status_text(
                            &crate::surfaces::friendly_name(h.index, &lane_boxes),
                            surface_lanes.effective(h.index, &lane_boxes),
                            frame.armed.is_some(),
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
                    // Step 2h: the face the behavior acts on, whichever
                    // face the ray entered (the pick and the label's
                    // placement keep the hit's).
                    let rows = frame.hit.map(|h| {
                        let b = pick_boxes[h.index];
                        crate::highlight::uniform(
                            &crate::surfaces::acting_face(b.center, b.rot, b.half, head),
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
                for b in &room_boxes {
                    set.push_box(b);
                }
                if let Some(f) = &floor_box {
                    set.push_box(&ObstacleBox {
                        // Board #5397: the room's kinds alone; a live floor
                        // is left out on the stage floor.
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
                        let boxes = ray_boxes(&room_boxes, scanned_n, floor_box.as_ref());
                        let room_n = room_boxes.len();
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
                        // Board #5397: the live planes' strengths faded.
                        &cloud_ran.rows(&crate::live_wire::fade_rows(
                            surface_lanes.rows(),
                            scanned_n,
                            live_levels.iter().copied(),
                        )),
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
            // The label: the room editor's after an action at its hit,
            // else the room scan's (board #3488), held in front of the
            // head: "Scanning the room…" until the query returns anchors,
            // "Room: 17 surfaces" when it does, "No room found..." when
            // the retries give up.
            // Board #3751: the voice path's window, toggled by the left
            // fist (hand 0, board #5380: a fist opens it, the next closes
            // it), or (V4) opened by a thumb tap on either hand, while the
            // hand menu's Voice toggle is on. It opens whatever else the
            // hands are doing: with Edit room on, with the hand menu up, on
            // Flock, where the left fist is then no predator (above).
            voice_label.step(dt);
            // The sentence heard this frame, for the grammar below.
            let mut sentence: Option<String> = None;
            if let Some(v) = voice.as_mut() {
                let head = glam::Vec3::from(input.head);
                // V4: a thumb tap on either hand is the second opener;
                // with the hand menu's Voice toggle off neither the fist
                // nor the tap does anything for voice.
                let tap_hand = (0..2).find(|&h| input.micro.tap[h]);
                if (left_fist || tap_hand.is_some())
                    && controls.voice
                    && !voice_mic
                    && !voice_mic_logged
                {
                    voice_mic_logged = true;
                    info!("voice: RECORD_AUDIO missing, the window stays closed");
                }
                // V5: while the on-device provider decides, no window
                // opens (and so no transcription starts), so whisper's
                // threads and the cascade's never run at once.
                let local_busy =
                    agent_pick == crate::agent::Pick::Local && agent_call.is_some();
                let listens = voice_mic && controls.voice && !local_busy;
                let press = VoicePress {
                    fist: left_fist && listens,
                    tap: tap_hand.is_some() && listens,
                    level: v.level(),
                };
                match push_to_talk.step_with(press, dt) {
                    Some(VoiceEvent::Opened(opener)) => {
                        // V4: at the left palm, or 1 m ahead of the head
                        // with the palm out of view (a tap from the right
                        // hand, the left one down).
                        voice_at = crate::voice::label_anchor(
                            input.hands.palm[0].map(|(p, _)| glam::Vec3::from(p)),
                            head,
                            glam::Quat::from_array(input.head_rot),
                        );
                        let how = match opener {
                            VoiceOpener::Fist => "left fist".to_owned(),
                            VoiceOpener::Tap => {
                                format!("thumb tap, {}", hand_name(tap_hand.unwrap_or(0)))
                            }
                        };
                        match v.open(&how) {
                            // Up for the whole window: a fist's runs to
                            // 20 s (board #5380).
                            Ok(()) => voice_label.show_for(
                                crate::voice::LISTENING_LABEL.to_owned(),
                                voice_at.0,
                                voice_at.1,
                                crate::voice::max_s(opener) + 1.0,
                            ),
                            Err(e) => {
                                log::warn!("voice: the window's microphone did not open: {e:#}");
                                voice_label.clear();
                            }
                        }
                    }
                    Some(VoiceEvent::Closed { seconds, why }) => {
                        if v.close(seconds, why) {
                            voice_label.show_for(
                                crate::voice::WAITING_LABEL.to_owned(),
                                voice_at.0,
                                voice_at.1,
                                VOICE_WAIT_LABEL_S,
                            );
                        } else {
                            // Nothing to wait for: the model is still
                            // loading, or the stream never opened.
                            push_to_talk.finish();
                            if v.ready() {
                                voice_label.clear();
                            } else {
                                voice_label.show(
                                    VOICE_LOADING_LABEL.to_owned(),
                                    voice_at.0,
                                    voice_at.1,
                                );
                            }
                        }
                    }
                    None => {}
                }
                let idle = push_to_talk.window() == VoiceWindow::Closed && !local_busy;
                let note = v.step(dt, idle);
                // V4: the microphone's level under "Listening…", this
                // frame's.
                if matches!(push_to_talk.window(), VoiceWindow::Open { .. }) {
                    voice_label.meter(Some(v.level()));
                }
                match note {
                    Some(VoiceNote::FileQueued { .. }) => {
                        // The unworn path: the label ahead of the head.
                        push_to_talk.begin_closing();
                        voice_at = crate::voice::label_anchor(
                            None,
                            head,
                            glam::Quat::from_array(input.head_rot),
                        );
                        voice_label.show_for(
                            crate::voice::WAITING_LABEL.to_owned(),
                            voice_at.0,
                            voice_at.1,
                            VOICE_WAIT_LABEL_S,
                        );
                    }
                    Some(VoiceNote::Heard { text, .. }) => {
                        push_to_talk.finish();
                        let text = crate::voice::spoken(&text);
                        if !controls.voice {
                            // V4: turned off while it transcribed.
                            info!("voice: off (menu), the sentence is dropped");
                        } else if text.is_empty() {
                            voice_label.show(
                                crate::voice::NOTHING_LABEL.to_owned(),
                                voice_at.0,
                                voice_at.1,
                            );
                        } else {
                            sentence = Some(text.to_owned());
                        }
                    }
                    None => {}
                }
            }
            // Board #3776, `debug.fosfora.agentpick`: the Agent control's
            // press for the unworn gate, fed once per value as the `say`
            // knob is; it goes through the menu's one switch (next
            // frame's action consumer) and saves as a press does.
            if frame_index.is_multiple_of(72)
                && let Some(value) =
                    agent_knob.poll(system_prop("debug.fosfora.agentpick").as_deref(), false)
            {
                match crate::agent::Pick::from_name(&value).filter(|&p| available.has(p)) {
                    Some(pick) => {
                        info!("voice agent: debug.fosfora.agentpick \"{value}\"");
                        voice_actions.push(Action::SetAgent(pick));
                    }
                    None => info!("voice agent: agentpick \"{value}\" not available"),
                }
            }
            // `debug.fosfora.say`: a sentence as if heard, for the unworn
            // gate (fed once per value; it waits for the room's anchors as
            // the `surface` knob does), its label ahead of the head.
            if frame_index.is_multiple_of(72)
                && let Some(said) = say_knob.poll(
                    system_prop("debug.fosfora.say").as_deref(),
                    has_room && input.room_id.is_none(),
                )
            {
                info!("voice: debug.fosfora.say \"{said}\"");
                voice_at = crate::voice::label_anchor(
                    None,
                    glam::Vec3::from(input.head),
                    glam::Quat::from_array(input.head_rot),
                );
                sentence = Some(said);
            }
            // Board #3751, V2: the sentence through the grammar, generated
            // from what the frame holds (the world effects, the behaviors,
            // the lane boxes by their friendly names, the surface under the
            // editor's beam), applied by `apply_intent` (`lane_boxes` being
            // this frame's). With the agent on (V3), a sentence no template
            // fits, or one naming an unknown behavior, goes to the model
            // instead of the miss's label.
            if let Some(sentence) = sentence.take() {
                let vocab = crate::intent::Vocabulary {
                    effects: &world_effects,
                    behaviors: &crate::surfaces::SurfaceBehavior::ALL,
                    // Board #5397: not the live planes.
                    surfaces: crate::live_wire::offered(&lane_boxes),
                    pointed: editor
                        .hit()
                        .filter(|_| controls.edit_room)
                        .map(|h| h.index),
                    tracks: music.tracks(),
                };
                // V4: the sentence as clauses ("amber on the desk and the
                // walls on the bass"), applied in order; "what can I say"
                // shows examples; with the agent on, what misses the way
                // the agent takes goes to it (`intent::respond`), after the
                // matched clauses applied, so it sees the room after them.
                let clauses = crate::intent::parse_clauses(&sentence, &vocab);
                let response = crate::intent::respond(
                    &sentence,
                    &clauses,
                    &vocab,
                    agent.is_some(),
                    &mut help_page,
                    |intent| {
                        apply_intent(
                            intent,
                            &vocab,
                            &mut surface_lanes,
                            &lane_boxes,
                            &mut controls,
                            &mut voice_actions,
                            &mut switch_to,
                        )
                    },
                );
                if let (Some(asked), Some(provider)) = (&response.agent, &agent) {
                    let head = glam::Vec3::from(input.head);
                    let sizes: Vec<[f32; 2]> = room_boxes
                        .iter()
                        .chain(floor_box.iter())
                        .map(|b| {
                            let face = crate::surfaces::acting_face(
                                glam::Vec3::from(b.center),
                                glam::Quat::from_array(b.rot),
                                glam::Vec3::from(b.half),
                                head,
                            );
                            [2.0 * face.half[0], 2.0 * face.half[1]]
                        })
                        .collect();
                    let room = crate::agent::room_state(
                        &vocab,
                        (mode == Mode::World).then_some(world_index),
                        &sizes,
                        &surface_lanes,
                        &lane_boxes,
                    );
                    let call = PendingCall {
                        sentence: asked.clone(),
                        room,
                        pointed: vocab.pointed,
                        at: voice_at,
                    };
                    // V5: the on-device provider waits while a window is
                    // open or transcribing; a newer sentence replaces a
                    // waiting one.
                    if agent_pick == crate::agent::Pick::Local
                        && push_to_talk.window() != VoiceWindow::Closed
                    {
                        info!("voice local: waiting for the voice window to close");
                        pending_call = Some(call);
                    } else {
                        pending_call = None;
                        agent_call = Some(call.ask(provider.as_ref()));
                    }
                }
                info!(
                    "{}",
                    crate::intent::clauses_log_line(&sentence, &clauses, &response.label)
                );
                // Board #5378: a name the room lacks, or one several
                // surfaces share, shows every surface's name for a while.
                if clauses
                    .iter()
                    .any(|c| c.outcome.as_ref().is_err_and(|m| m.reason.names_surfaces()))
                {
                    surface_labels.miss();
                }
                voice_label.show_for(response.label, voice_at.0, voice_at.1, response.seconds);
            }
            // V5: the on-device provider loads once the speech model has
            // (at once with voice off; after LOCAL_WAIT whatever whisper
            // does), so the two loads never share the CPU; its outcome is
            // logged once, and a failed load turns the agent off.
            if let Some(l) = local.as_mut() {
                if !l.asked
                    && (voice.as_ref().is_none_or(Voice::ready) || started.elapsed() > LOCAL_WAIT)
                {
                    l.handle.load();
                    l.asked = true;
                }
                match l.loaded.try_recv() {
                    Ok(Ok(loaded)) => {
                        info!(
                            "voice agent: on \u{b7} local \u{b7} {} \u{b7} {} ms",
                            crate::local::MODEL_NAME,
                            loaded.ms
                        );
                        info!(
                            "voice local: ONNX Runtime from {}{}",
                            loaded.library,
                            if loaded.reused {
                                " \u{b7} the session parked by the last launch"
                            } else {
                                ""
                            }
                        );
                    }
                    // Board #3776: a failed load also takes the on-device
                    // provider out of the Agent control's cycle; when it
                    // is the pick, the agent goes off (not saved: the
                    // wearer's pick stays theirs).
                    Ok(Err(e)) => {
                        error!("voice agent: off (local: {e})");
                        available.local = Err(format!("the load failed: {e}"));
                        local = None;
                        if agent_pick == crate::agent::Pick::Local {
                            agent = None;
                            agent_pick = crate::agent::Pick::Off;
                        }
                        controls.agent = available.control(agent_pick);
                    }
                    Err(_) => {}
                }
            }
            if pending_call.is_some()
                && push_to_talk.window() == VoiceWindow::Closed
                && let (Some(call), Some(provider)) = (pending_call.take(), &agent)
            {
                agent_call = Some(call.ask(provider.as_ref()));
            }
            // V3: the agent's answer, polled (never waited for). Its
            // actions map onto intents through the grammar's resolution
            // and apply through the grammar's path, in order; the label
            // shows the model's sentence (the actions' own labels are in
            // the log), or what was skipped, or the error. V5: the
            // on-device provider's miss shows as the grammar's own
            // "Didn't catch that" with its hint.
            if let Some(call) = &agent_call {
                use std::sync::mpsc::TryRecvError;
                let answer = match call.reply.try_recv() {
                    Ok(answer) => Some(answer),
                    Err(TryRecvError::Empty)
                        if call.started.elapsed() > crate::agent::TIMEOUT + Duration::from_secs(1) =>
                    {
                        Some(Err(crate::agent::AgentError::Timeout))
                    }
                    Err(TryRecvError::Empty) => None,
                    Err(TryRecvError::Disconnected) => Some(Err(
                        crate::agent::AgentError::Http(0, "the call's thread ended".to_owned()),
                    )),
                };
                if let Some(answer) = answer {
                    let at = call.at;
                    let sentence = call.sentence.clone();
                    let pointed = call.pointed.filter(|&p| p < lane_boxes.len());
                    agent_call = None;
                    let (text, seconds) = match answer {
                        Ok(crate::agent::Reply {
                            miss: Some(miss), ..
                        }) => {
                            if miss.reason.names_surfaces() {
                                surface_labels.miss();
                            }
                            let vocab = crate::intent::Vocabulary {
                                effects: &world_effects,
                                behaviors: &crate::surfaces::SurfaceBehavior::ALL,
                                surfaces: crate::intent::surfaces(&lane_boxes),
                                pointed,
                                tracks: crate::intent::Tracks::default(),
                            };
                            let text = format!(
                                "{}\n{}",
                                crate::intent::miss_text(&miss),
                                crate::intent::hint(&miss.heard, &vocab)
                            );
                            info!(
                                "voice agent: heard \"{}\" \u{2192} miss {:?} \u{b7} label \"{}\"",
                                sentence.trim(),
                                miss.reason,
                                text.replace('\n', " | ")
                            );
                            (text, crate::intent::MISS_S)
                        }
                        Ok(reply) => {
                            let vocab = crate::intent::Vocabulary {
                                effects: &world_effects,
                                behaviors: &crate::surfaces::SurfaceBehavior::ALL,
                                surfaces: crate::intent::surfaces(&lane_boxes),
                                pointed,
                                tracks: crate::intent::Tracks::default(),
                            };
                            let outcomes: Vec<Result<crate::intent::Intent, crate::intent::Miss>> =
                                reply
                                    .actions
                                    .iter()
                                    .map(|a| crate::intent::Intent::from_json(a, &vocab))
                                    .collect();
                            let mut applied = Vec::new();
                            let mut missed = Vec::new();
                            for outcome in &outcomes {
                                match outcome {
                                    Ok(intent) => applied.push(apply_intent(
                                        intent,
                                        &vocab,
                                        &mut surface_lanes,
                                        &lane_boxes,
                                        &mut controls,
                                        &mut voice_actions,
                                        &mut switch_to,
                                    )),
                                    Err(miss) => missed.push(miss.clone()),
                                }
                            }
                            info!(
                                "{}",
                                crate::agent::heard_line(&sentence, &outcomes, &reply.say)
                            );
                            if let Some(line) = crate::agent::dropped_line(&outcomes) {
                                info!("{line}");
                            }
                            if missed.iter().any(|m| m.reason.names_surfaces()) {
                                surface_labels.miss();
                            }
                            crate::agent::label(&reply.say, &applied, &missed)
                        }
                        Err(e) => {
                            info!(
                                "voice agent: heard \"{}\" \u{2192} {e} \u{b7} label \"{}\"",
                                sentence.trim(),
                                e.label()
                            );
                            (e.label().to_owned(), crate::agent::LABEL_S)
                        }
                    };
                    // Board #5380: a fist window runs to 20 s, and
                    // "Listening…" stays up all of it. A reply that lands
                    // meanwhile is in the log above, not on the label.
                    if matches!(push_to_talk.window(), VoiceWindow::Open { .. }) {
                        info!("voice agent: the reply's label is skipped, a voice window is open");
                    } else {
                        voice_label.show_for(text, at.0, at.1, seconds);
                    }
                }
            }
            // Board #5393: "No room here: roaming…" when the scan's end
            // turned roam on.
            let roam_now = roam.now();
            scan_label.step_with(scan_state, dt, |s| crate::roam::scan_text(s, roam_now));
            {
                let head = glam::Vec3::from(input.head);
                let head_rot = glam::Quat::from_array(input.head_rot);
                // The voice path's label first: it answers the wearer's
                // own fist.
                let shown = voice_label
                    .now()
                    .or_else(|| label.now())
                    .map(|l| {
                        let pose = crate::label::billboard(l.point, l.normal, head, head_rot);
                        (l.text, l.alpha, l.level, pose)
                    })
                    .or_else(|| {
                        scan_label.now().map(|(text, alpha)| {
                            (text, alpha, None, crate::label::scan_billboard(head, head_rot))
                        })
                    });
                label_texture.show(&gfx, shown);
                // Board #5378: the surface labels. Only while they show
                // are the names, the behaviors and the poses worked out;
                // the atlas is redrawn only when a tag or the fade's stage
                // changed.
                surface_labels.step(
                    crate::surface_labels::Reasons {
                        editing: controls.edit_room,
                        listening: push_to_talk.window() != VoiceWindow::Closed,
                        knob: labels_knob,
                    },
                    dt,
                );
                let stage = surface_labels.stage();
                if stage > 0 {
                    use crate::surface_labels::{MAX_LABELS, Tag, placed, pose};
                    // The lane boxes' names as the grammar has them, and
                    // their boxes in the same order (board #5397: the live
                    // planes left out, as the grammar leaves them).
                    let offered = crate::live_wire::offered(&lane_boxes);
                    let tags: Vec<Tag> = offered
                        .iter()
                        .take(MAX_LABELS)
                        .map(|s| Tag {
                            behavior: crate::label::behavior_word(
                                surface_lanes.effective(s.index, &lane_boxes),
                            ),
                            name: s.name.clone(),
                        })
                        .collect();
                    let boxes: Vec<&ObstacleBox> = room_boxes.iter().chain(floor_box.iter()).collect();
                    let poses = offered.iter().enumerate().filter_map(|(k, s)| {
                        let b = boxes.get(s.index)?;
                        let (center, half) = (glam::Vec3::from(b.center), glam::Vec3::from(b.half));
                        Some((k, pose(center, glam::Quat::from_array(b.rot), half, head, head_rot)))
                    });
                    // Board #5379: labels that crowd on the screen move
                    // apart and point back at their surfaces.
                    let in_view = label_spread.place(&placed(poses, head, head_rot), head, head_rot);
                    if !labels_shown {
                        info!(
                            "surface labels: shown ({}) \u{b7} {} surfaces, {} in view, {} moved apart",
                            surface_labels.why(),
                            tags.len(),
                            in_view.len(),
                            in_view.iter().filter(|s| s.leader.is_some()).count()
                        );
                    }
                    label_atlas.show(&gfx, &tags, stage, &in_view, head);
                } else {
                    if labels_shown {
                        info!("surface labels: hidden");
                    }
                    label_atlas.hide(&gfx);
                }
                labels_shown = stage > 0;
            }
            // The Music row: a decode that finished starts the clip; a
            // failed decode or start turns the row back to "Music: play".
            music.poll();
            controls.music = music.wanted();
            // This frame's audio: the music's tap, the headset microphones
            // or the synthetic 120 BPM groove.
            let hop = match music.analysis.as_mut().or(live_audio.as_mut()) {
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
                if let Some(p) = &music.playback {
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
            downbeat_env = crate::surface_fx::downbeat_step(downbeat_env, hop.downbeat_fired, dt);
            surface_clock.advance(dt, beat_env);
            if let Some(r) = ripple.as_mut() {
                r.update(
                    t,
                    dt,
                    glam::Vec3::from(input.head),
                    hop.beat_fired,
                    f.bass.max(f.sub_bass),
                    f.bass,
                );
            }
            // Board #3327: the wall spectrum's bars and its wall. Among the
            // room's walls whose lane says spectrum (by default every
            // wall), less the ones the runtime hides, the one the wearer
            // faces, with the pick's hysteresis (decision #3449);
            // `canvastest ceiling` puts it on the room's ceiling instead.
            // Since D2 (board #3488) it draws as a slot of the surfaces
            // pass below, on that one box.
            let spectrum_box = match canvas.as_mut() {
                Some(c) => {
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
                    let walls: Vec<(usize, crate::canvas::WallFace)> = room_boxes
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
                        room_boxes
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
                    now.map(|w| w.0)
                }
                None => None,
            };
            // Board #5384: immersion. The level follows the song while real
            // audio drives the analysis (the music's tap or the
            // microphones; the synthetic groove is no music); it dims the
            // passthrough after the frame and lifts the surfaces' alpha
            // below. Without passthrough there is no room to dim, and the
            // surfaces stay as they were.
            let immerse_level = immersion.step(
                dt,
                crate::immerse::Input {
                    loudness_s: f.loudness_s,
                    buildup: f.buildup,
                    drop: f.drop > 0.5,
                    rms: f.rms,
                    playing: music.analysis.is_some() || live_audio.is_some(),
                },
            );
            // Board #5393: in roam the level gives way to walking: the
            // head's level speed, low-passed, scales it from all of it
            // under 0.3 m/s to none over 0.8 m/s (`roam::Walk`).
            walk.step(glam::Vec3::from(input.head), dt, roam_walk);
            let immerse_level =
                crate::roam::immerse_level(immerse_level, roam.now().is_on(), walk.factor());
            let immerse_level = if immerse_room { immerse_level } else { 0.0 };
            immerse_now = Some((immerse_level, f.loudness_s, f.buildup));
            // Board #3472, D1: the surfaces pass. Every box whose behavior
            // is a surface shader (the rings, the streamlines, since D2,
            // board #3488, the curls and the pulse, and since D2b, board
            // #3489, the ported desktop effects) is lit on the face the
            // behavior acts on, the stage floor too while the room has no
            // scene floor (then it stands in for one, as its emitter flag
            // and the editor's ray take it; beside a scene floor it would
            // double it, step 2d); a hidden wall never. A floor's face is
            // leveled and kept above the stage floor, as the ripple's quad
            // was. The rings need the ripple state (`debug.fosfora.ripple`);
            // `rippletest ceiling` puts them on every ceiling for the run,
            // the room file untouched. The spectrum is lit on the one box
            // picked above (`debug.fosfora.canvas 0`: none), on its face
            // oriented for the bars; another box on the spectrum is dark.
            {
                use crate::surfaces::{KIND_CEILING, KIND_FLOOR, SurfaceBehavior};
                let head = glam::Vec3::from(input.head);
                let stage_k = room_boxes.len();
                // Board #5397: the room's floor, not a live one (a live
                // floor on the stage floor is left out).
                let scene_floor = input.room_boxes.iter().any(|b| b.kind == KIND_FLOOR);
                // Board #5397: the live planes' strengths faded.
                let lane_rows = crate::live_wire::fade_rows(
                    surface_lanes.rows(),
                    scanned_n,
                    live_levels.iter().copied(),
                );
                let stage_top = floor_box.map(|b| b.center[1] + b.half[1]);
                let mut audio = crate::surface_fx::Audio {
                    rms: f.rms,
                    bass: f.bass,
                    beat: beat_env,
                    clock: surface_clock.seconds(),
                    mid: f.mid,
                    high: f.presence.max(f.brilliance),
                    downbeat: downbeat_env,
                    bar_phase: f.bar_phase,
                    key_tint: [1.0; 3],
                };
                let mut counts = [0usize; 6];
                let mut slots = Vec::new();
                // The ported effects' rates integrate every frame, lit or
                // not, one state per effect: two walls on aurora drift in
                // step, and an effect picks up where the clock is.
                for e in surface_ports.iter_mut().flatten() {
                    e.advance(dt);
                }
                // The ported faces this frame: effect, face, kind,
                // behavior, strength and color index.
                let mut ported = Vec::new();
                let mut spectrum_sign = 1.0;
                for (k, b) in room_boxes.iter().chain(floor_box.iter()).enumerate() {
                    // The lane's behavior, strength, color index and band
                    // (board #3488; a box past the lanes' rows: the
                    // defaults).
                    let row = lane_rows.get(k).copied();
                    let lane = surface_lanes.behavior(k);
                    let test_rings = ripple_ceiling && b.kind == KIND_CEILING;
                    let (behavior, strength) = if spectrum_box == Some(k) {
                        let strength = row
                            .filter(|_| lane == SurfaceBehavior::Spectrum)
                            .map_or(1.0, |r| r[1]);
                        (SurfaceBehavior::Spectrum, strength)
                    } else if test_rings {
                        (SurfaceBehavior::Rings, 1.0)
                    } else {
                        (lane, row.map_or(1.0, |r| r[1]))
                    };
                    let (color, band) = match row {
                        Some(r) if behavior == lane => crate::surfaces::lane_params(r, behavior),
                        _ => (0, behavior.default_band()),
                    };
                    let spectrum = behavior == SurfaceBehavior::Spectrum;
                    if spectrum != (spectrum_box == Some(k))
                        || (!spectrum && !behavior.draws_on_face())
                        || b.hidden
                        || (k == stage_k && scene_floor)
                        || (behavior == SurfaceBehavior::Rings && ripple.is_none())
                        || (behavior.port().is_some() && surface_ports.is_none())
                    {
                        continue;
                    }
                    let mut face = crate::surfaces::acting_face(
                        glam::Vec3::from(b.center),
                        glam::Quat::from_array(b.rot),
                        glam::Vec3::from(b.half),
                        head,
                    );
                    if spectrum {
                        (face, spectrum_sign) = crate::canvas::spectrum_face(face);
                    } else if b.kind == KIND_FLOOR {
                        face = crate::surface_fx::level_floor(face, stage_top);
                    }
                    // A ported desktop effect (board #3489): its own
                    // slot, the face turned upright for its frame, the
                    // effect's uniform for this face's size, built below
                    // once the key's tint is known. Since D3 (board
                    // #3472) the lane's color index tints it
                    // (`surface_port::tint`: index 0 leaves its colors its
                    // own); the band is not read, a port follows its own
                    // audio bindings.
                    if let (Some(i), Some(_)) = (behavior.port(), surface_ports.as_ref()) {
                        counts[5] += 1;
                        ported.push((
                            i,
                            crate::surface_port::port_face(face),
                            b.kind,
                            behavior,
                            strength,
                            color,
                        ));
                        continue;
                    }
                    match behavior {
                        SurfaceBehavior::Rings => counts[0] += 1,
                        SurfaceBehavior::Streamlines => counts[1] += 1,
                        SurfaceBehavior::Curls => counts[2] += 1,
                        SurfaceBehavior::Pulse => counts[3] += 1,
                        _ => counts[4] += 1,
                    }
                    slots.push(crate::surface_fx::Slot {
                        face,
                        kind: b.kind,
                        behavior,
                        strength,
                        color,
                        band,
                    });
                }
                // The key's tint only when a lit face asks for it (a
                // ported one too, since D3): the desktop's key hue from
                // core's binding sources, which builds a map of every
                // source.
                if slots.iter().any(|s| s.color == crate::surfaces::COLOR_KEY)
                    || ported
                        .iter()
                        .any(|&(.., color)| color == crate::surfaces::COLOR_KEY)
                {
                    let hue = fosfora_app::bindings::sources::collect_audio(&f)
                        .get("audio.key_hue")
                        .map_or(0.5, |v| v.0);
                    audio.key_tint = crate::surfaces::key_tint(hue);
                }
                let mut port_faces: Vec<_> = surface_ports
                    .as_ref()
                    .map(|effects| {
                        ported
                            .iter()
                            .map(|&(i, face, kind, behavior, strength, color)| {
                                let effect = &effects[i];
                                crate::gfx::PortFace {
                                    effect: i,
                                    uniforms: crate::surface_port::uniforms(
                                        &f,
                                        audio.clock,
                                        dt,
                                        frame_index,
                                        face.half,
                                        effect.params(),
                                    ),
                                    rows: crate::immerse::lift_rows(
                                        crate::surface_port::rows(
                                            &face,
                                            strength,
                                            effect.overlay,
                                            behavior.id(),
                                            crate::surface_port::tint(color, kind, audio.key_tint),
                                        ),
                                        immerse_level,
                                    ),
                                }
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let spectrum = canvas.as_ref().map(|c| crate::surface_fx::Spectrum {
                    canvas: c,
                    sign: spectrum_sign,
                });
                let faces: Vec<_> = slots
                    .iter()
                    .map(|slot| crate::surface_fx::rows(slot, audio, ripple.as_ref(), spectrum, t))
                    .map(|rows| crate::immerse::lift_rows(rows, immerse_level))
                    .collect();
                // One budget: a ported face and a `SurfaceFx` face both
                // count against the pass's slots, the ported ones last.
                let max = crate::surface_fx::MAX_SLOTS;
                let lit = faces.len() + port_faces.len();
                let dropped = lit.saturating_sub(max);
                port_faces.truncate(max.saturating_sub(faces.len()));
                if Some((counts, dropped)) != surfaces_lit {
                    info!(
                        "surfaces: {lit} lit (rings {}, streamlines {}, curls {}, pulse {}, spectrum {}, ports {})",
                        counts[0],
                        counts[1],
                        counts[2],
                        counts[3],
                        counts[4],
                        counts[5]
                    );
                    if dropped > 0 {
                        info!(
                            "surfaces: dropped {dropped} (the pass holds {})",
                            crate::surface_fx::MAX_SLOTS
                        );
                    }
                    surfaces_lit = Some((counts, dropped));
                }
                gfx.set_surfaces(&faces);
                gfx.set_surface_ports(&port_faces);
            }
            if let Some(scene) = scene {
                match music.analysis.as_mut().or(live_audio.as_mut()) {
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
        // Board #5397: a depth read-back that landed this frame goes to the
        // live planes' worker, with the time it was acquired at, for the
        // finder (live planes on) and the noise bins (their knob).
        if let Some(grid) = session.take_depth_grid()
            && let Some(w) = live_worker.as_mut()
        {
            let noise = (live_noise && !noise_boxes.is_empty()).then(|| noise_boxes.clone());
            if live_on || noise.is_some() {
                w.submit(crate::live_wire::Job {
                    epoch: live_epoch,
                    time_s: grid
                        .acquired
                        .saturating_duration_since(started)
                        .as_secs_f32(),
                    grid,
                    reading: live_reading,
                    fit: live_on,
                    noise,
                    record: live_rec.ask(),
                });
            }
        }
        if rescan {
            session.rescan_room();
        }
        // Board #5384: the passthrough style, only when the opacity moves
        // by a step (the gate), and never without passthrough; a failed
        // call is logged once.
        if immerse_room
            && let Some((level, ..)) = immerse_now
            && let Some(opacity) = immerse_style.update(immersion.opacity(level))
            && let Err(e) = session.set_passthrough_opacity(opacity)
            && !immerse_failed
        {
            immerse_failed = true;
            log::warn!("immersion: the passthrough style failed (logged once): {e:#}");
        }
        let perf_want = gov_on || hud.as_ref().is_some_and(Hud::debug);
        if perf_want != perf_on {
            perf_on = perf_want;
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
        // Board #5393: in roam, the surface-born effect with nothing in
        // reach to spawn from gives way, once, to the effect that fills its
        // volume; roam off switches back unless the wearer picked another
        // meanwhile (`roam::Fill`). A switch asked for this frame wins.
        if world {
            let fill_index = world_effects
                .iter()
                .position(|e| e == crate::roam::FILL_EFFECT)
                .filter(|&i| i == world_index || parked.get(i).is_some_and(Option::is_some));
            let asked = fill.step(crate::roam::FillInput {
                roam: roam.now().is_on(),
                scan: session.scan_state(),
                showing: world_index,
                surface_born: scene.as_ref().is_some_and(XrScene::surface_born),
                weight: scene.as_ref().map_or(0.0, XrScene::emitter_weight),
                fill: fill_index,
                dt,
            });
            if switch_to.is_none() {
                match asked {
                    Some(crate::roam::FillSwitch::To(i)) => {
                        info!(
                            "roam: nothing in reach emits on '{}': '{}' fills the space",
                            world_effects[world_index], world_effects[i]
                        );
                        switch_to = Some(i);
                    }
                    Some(crate::roam::FillSwitch::Back(i)) => {
                        info!("roam off: back to '{}'", world_effects[i]);
                        switch_to = Some(i);
                    }
                    None => {}
                }
            }
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
        // Board #3789: the thermal governor, once a frame in world mode:
        // the last second's mean app GPU time against the display period,
        // or without the counters the frame window's fps and long frames.
        if gov_on {
            if (controls.density - gov.ceiling()).abs() > 1e-4 {
                gov.set_ceiling(controls.density);
            }
            if let Some(ms) = gpu_frame {
                gpu_window.push(t, ms);
            }
            let frames = stats.last;
            let period_ms = if frames.display_hz > 0.0 {
                1000.0 / frames.display_hz
            } else {
                0.0
            };
            let (over_ms, under_ms) = gov.margins().limits_ms(period_ms);
            if period_ms > 0.0 && gov_logged_period.is_none_or(|p| (p - period_ms).abs() > 0.05) {
                gov_logged_period = Some(period_ms);
                info!(
                    "governor: on \u{b7} down over {over_ms:.1} ms for {:.0} s \u{b7} up under {under_ms:.1} ms for {:.0} s \u{b7} floor {:.2} (display {:.1} Hz)",
                    crate::governor::GOV_DOWN_S,
                    crate::governor::GOV_UP_S,
                    crate::governor::GOV_FLOOR,
                    frames.display_hz
                );
            }
            let gpu = gpu_window.mean(t);
            let signal = gpu.map(crate::governor::Signal::Gpu).or_else(|| {
                (period_ms > 0.0).then_some(crate::governor::Signal::Frames {
                    fps: frames.fps,
                    long: frames.long_frames,
                })
            });
            match signal {
                Some(signal) => {
                    if let Some(c) = gov.step(t, signal, period_ms) {
                        let load = match (signal, c.down) {
                            (crate::governor::Signal::Gpu(ms), true) => {
                                format!("gpu {ms:.1} ms over {over_ms:.1}")
                            }
                            (crate::governor::Signal::Gpu(ms), false) => {
                                format!("gpu {ms:.1} ms under {under_ms:.1}")
                            }
                            (crate::governor::Signal::Frames { fps, long }, _) => {
                                format!("frames {fps:.1}/s with {long} long (no gpu time)")
                            }
                        };
                        let what = if c.level == crate::governor::Level::Starved {
                            format!(
                                "density {:.2} at the floor: starved (the next stage, board #3788, is not built)",
                                c.to
                            )
                        } else if (c.from - c.to).abs() < 1e-4 {
                            format!("no longer starved \u{b7} density {:.2}", c.to)
                        } else {
                            format!("density {:.2} \u{2192} {:.2}", c.from, c.to)
                        };
                        info!("governor: {load} for {:.0} s \u{b7} {what}", c.held_s);
                    }
                }
                None => gov.idle(),
            }
            controls.density_gov = gov.governed();
        }
        // The cloud density: the showing world effect's emission against
        // the rate `new_world` set, applied when it changes and when an
        // effect switch swaps in one set for another. The cloud toggle
        // leaves it alone: off hides the effect (step 2e; 2c took the
        // emission to 0 here). The governor's density, at most the
        // wearer's (`controls.density`; the same with it off).
        let density = if gov_on {
            gov.density()
        } else {
            controls.density
        };
        if world
            && let Some(s) = scene.as_mut()
            && let Some(base) = s.base_emit_rate()
            && let Some(set) = density_set.get_mut(world_index)
            && (*set - density).abs() > 1e-4
        {
            *set = density;
            s.set_emit_rate(base * density);
            info!(
                "cloud density {:.2}{}: '{}' emits {:.0}/s (preset {:.0}/s)",
                density,
                if gov.governed().is_some() {
                    " (gov)"
                } else {
                    ""
                },
                world_effects[world_index],
                base * density,
                base
            );
        }
        // The space size: the showing world effect's volume, applied when
        // the stepper moves, when an effect switch swaps in one set for
        // another, and for the surface-born effect when the room fit
        // changes (step 2h: the boxes or the anchor moved) while no size
        // is asked; the stepper shows the size it runs at.
        if world
            && let Some(s) = scene.as_mut()
            && let Some((requested, preset)) = s.space()
        {
            let fit = room_fit.filter(|_| s.surface_born());
            if let Some(half) = space.update(
                &mut controls.space_half,
                requested,
                preset,
                fit.map(|f| f.0),
            ) {
                s.set_space_half(half);
                info!(
                    "space size: '{}' at {:.2} m half extent",
                    world_effects[world_index], controls.space_half
                );
            }
            let applied = fit.filter(|_| space.asked().is_none());
            if let Some((half, boxes)) = applied
                && applied != fit_logged
            {
                info!("space fit to the room: {half:.2} m half extent ({boxes} boxes)");
            }
            fit_logged = applied;
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
            // Board #3264: the runtime permissions' grants while any is
            // missing (the result callback never reaches native code).
            let granted_now = |name: &str| {
                permissions.as_ref().is_some_and(|p| {
                    p.granted(name).unwrap_or_else(|e| {
                        log::warn!("permissions: {e:#}");
                        false
                    })
                })
            };
            // Board #3751, V4: voice turned on from the menu after a launch
            // that did not ask for the microphone: its grant, once a second.
            if controls.voice && voice.is_some() && !voice_mic && granted_now(RECORD_AUDIO) {
                voice_mic = true;
                info!("permission RECORD_AUDIO granted: the voice window can open");
            }
            for change in permission_watch.poll(asked_at.elapsed().as_secs_f32(), granted_now) {
                match change {
                    Change::Granted { name, after_s } if name == USE_SCENE => {
                        let then = if !session.has_room() {
                            "no room in this mode"
                        } else if matches!(
                            session.scan_state(),
                            Some(crate::label::ScanState::Found(_))
                        ) {
                            "the room is already in"
                        } else {
                            room_pickup = true;
                            "querying the room"
                        };
                        info!("permission USE_SCENE granted after {after_s:.0} s: {then}");
                    }
                    // Board #5385: the shared Music/ folder joins the
                    // playlist.
                    Change::Granted { name, after_s } if name == storage_permission => {
                        info!(
                            "permission {} granted after {after_s:.0} s: scanning Music/",
                            crate::permissions::short(name)
                        );
                        music.storage_granted();
                    }
                    Change::Granted { name, after_s } => {
                        // Board #3751: the voice window's stream needs only
                        // the grant.
                        voice_mic |= name == RECORD_AUDIO;
                        match mic_waiting.take() {
                            Some(source) => {
                                info!(
                                    "permission {} granted after {after_s:.0} s: opening the microphones",
                                    crate::permissions::short(name)
                                );
                                match LiveAudio::open(source) {
                                    Ok(a) => live_audio = Some(a),
                                    Err(e) => log::warn!(
                                        "audio: the microphones did not open after the grant: {e:#}; the synthetic groove stays"
                                    ),
                                }
                            }
                            None => info!(
                                "permission {} granted after {after_s:.0} s",
                                crate::permissions::short(name)
                            ),
                        }
                    }
                    Change::GaveUp(still) => info!(
                        "permissions: {} still missing after {:.0} s: polling stopped, the fallbacks stay",
                        still
                            .iter()
                            .map(|n| crate::permissions::short(n))
                            .collect::<Vec<_>>()
                            .join(", "),
                        crate::permissions::WATCH_S
                    ),
                }
            }
            // The requery waits while a query or Space Setup is in flight;
            // anchors that arrive meanwhile make it moot.
            if room_pickup
                && (matches!(
                    session.scan_state(),
                    Some(crate::label::ScanState::Found(_))
                ) || session.requery_room_after_grant())
            {
                room_pickup = false;
            }
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
            // Board #5397: the live planes' knobs, live, and their line
            // once a second while they run.
            let knob = switch_now("debug.fosfora.liveplanes");
            if knob != live_knob {
                live_knob = knob;
                info!(
                    "live planes: debug.fosfora.liveplanes {}",
                    knob.map_or("unset", |on| if on { "1" } else { "0" })
                );
            }
            let show = switch_now("debug.fosfora.liveplanesshow") == Some(true);
            if show != live_outline {
                live_outline = show;
                info!(
                    "live planes: outlines {} (debug.fosfora.liveplanesshow)",
                    if show { "on" } else { "off" }
                );
            }
            if let Some(line) = live_rec.poll(
                system_prop(crate::live_rec::KNOB).as_deref(),
                live_rec_dir.as_deref(),
            ) {
                info!("{line}");
            }
            if live_on {
                match &live_snap {
                    Some(snap) => {
                        let (since, updates) = live_rate;
                        let secs = since.elapsed().as_secs_f32().max(1e-3);
                        let hz = snap.updates.saturating_sub(updates) as f32 / secs;
                        live_rate = (Instant::now(), snap.updates);
                        info!(
                            "{}",
                            crate::live_wire::status_line(&crate::live_wire::Status {
                                planes: &snap.planes,
                                tracked: snap.stats.tracked,
                                floor_y: snap.floor_y,
                                mean_ms: snap.mean_ms,
                                max_ms: snap.max_ms,
                                hz,
                                counts: live_show.counts(),
                            })
                        );
                    }
                    None => info!("live planes: on, no update yet"),
                }
            }
            // Board #5384: the immersion level once a second while the room
            // is dimmed, and once when it is back.
            if let Some((level, loudness_s, buildup)) = immerse_now {
                let dimmed = level > 0.005;
                if dimmed || immerse_logged {
                    info!(
                        "immersion: level {level:.2} · passthrough {:.2} · loudness_s {loudness_s:.2} · buildup {buildup:.2} · depth {:.2}",
                        immersion.opacity(level),
                        immersion.depth()
                    );
                }
                immerse_logged = dimmed;
            }
            // Board #5393: the faked head speed, live; and in roam, while
            // the head's speed takes from the immersion level, by how much.
            let knob = walk_knob();
            if knob.map(f32::to_bits) != roam_walk.map(f32::to_bits) {
                roam_walk = knob;
                match knob {
                    Some(speed) => {
                        info!("roam: head speed faked at {speed:.2} m/s (debug.fosfora.roamwalk)");
                    }
                    None => info!("roam: head speed measured again"),
                }
            }
            if roam.now().is_on() && (roam_walk.is_some() || walk.factor() < 0.999) {
                info!(
                    "roam: head speed {:.2} m/s{} · immersion x{:.2} (level {:.2} of {:.2}) · passthrough floor {:.2}",
                    walk.speed(),
                    if roam_walk.is_some() {
                        " (debug.fosfora.roamwalk)"
                    } else {
                        ""
                    },
                    walk.factor(),
                    immerse_now.map_or(0.0, |(level, ..)| level),
                    immersion.level(),
                    immersion.floor()
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
            if let Some(p) = &music.playback {
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
    drop(music);
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
fn ray_boxes(
    room: &[ObstacleBox],
    scanned: usize,
    floor: Option<&ObstacleBox>,
) -> Vec<crate::instruments::RayBox> {
    let ray_box = |b: &ObstacleBox| crate::instruments::RayBox {
        center: glam::Vec3::from(b.center),
        rot: glam::Quat::from_array(b.rot),
        half: glam::Vec3::from(b.half),
        kind: b.kind,
    };
    let mut boxes: Vec<_> = room.iter().map(ray_box).collect();
    // Board #5397: whether the room has a floor of its own, the live
    // planes (after the first `scanned`) aside.
    if !room[..scanned.min(room.len())]
        .iter()
        .any(|b| b.kind == crate::surfaces::KIND_FLOOR)
        && let Some(f) = floor
    {
        boxes.push(ray_box(f));
    }
    boxes
}

/// Board #5397: a live plane's box (`live_planes::to_obstacle_box`) as the
/// obstacle block takes it.
fn live_box(b: &crate::live_planes::PlaneBox) -> ObstacleBox {
    ObstacleBox {
        center: b.center,
        rot: b.rot,
        half: b.half,
        kind: b.kind,
        emit: b.emit,
        hidden: b.hidden,
        uuid: b.uuid,
    }
}

// The live planes' budget counts the lane rows, one per obstacle box.
const _: () = assert!(crate::particles3d::MAX_BOXES == crate::surfaces::SURFACE_LANE_ROWS);

/// An obstacle box as the self-check's and the overlap test's oriented box.
fn obb_of(b: &ObstacleBox) -> crate::env_depth::Obb {
    crate::env_depth::Obb {
        center: glam::Vec3::from(b.center),
        rot: glam::Quat::from_array(b.rot),
        half: glam::Vec3::from(b.half),
    }
}

/// Board #5397: the live planes' worker thread; `None`, logged, when it
/// does not start.
fn spawn_live_worker() -> Option<crate::live_wire::Worker> {
    match crate::live_wire::Worker::spawn() {
        Ok(w) => Some(w),
        Err(e) => {
            log::warn!("live planes: the worker thread did not start: {e}");
            None
        }
    }
}

/// Board #3751, V3: the agent's call in flight: its answer's channel, the
/// sentence, the lane box pointed at when it was said (a "this" means
/// that one), where the label floats, and when it was asked.
struct AgentCall {
    reply: std::sync::mpsc::Receiver<Result<crate::agent::Reply, crate::agent::AgentError>>,
    sentence: String,
    pointed: Option<usize>,
    at: (glam::Vec3, glam::Vec3),
    started: Instant,
}

/// Board #3751, V5: a call not yet asked: the sentence, the room as the
/// agent sees it, the lane box pointed at, and where the label floats.
struct PendingCall {
    sentence: String,
    room: crate::agent::RoomState,
    pointed: Option<usize>,
    at: (glam::Vec3, glam::Vec3),
}

impl PendingCall {
    /// Ask `provider` now.
    fn ask(self, provider: &(dyn crate::agent::Provider + Send + Sync)) -> AgentCall {
        AgentCall {
            reply: provider.answer(&self.sentence, &self.room),
            sentence: self.sentence,
            pointed: self.pointed,
            at: self.at,
            started: Instant::now(),
        }
    }
}

/// Board #3751, V5: the on-device provider: its handle, the load's
/// outcome (once), whether the load was asked for, and the launch log's
/// tail (the model, when it loads, the knobs).
struct LocalAgent {
    handle: std::sync::Arc<crate::local::Local>,
    loaded: std::sync::mpsc::Receiver<Result<crate::local::Loaded, String>>,
    asked: bool,
    about: String,
}

/// Board #3751, V5: start the on-device provider's worker with the
/// `localthreads` and `localmin` knobs; the frame loop asks it to load once
/// the speech model has (or after `LOCAL_WAIT`). Err: why it is off.
fn start_local(
    files: &crate::local::Files,
    permissions: Option<&Permissions>,
) -> Result<LocalAgent, String> {
    let threads = debug_prop("debug.fosfora.localthreads")
        .and_then(|v| v.trim().parse::<usize>().ok())
        .map_or(crate::local::THREADS, |n| n.clamp(1, 6));
    let floor = debug_prop("debug.fosfora.localmin")
        .and_then(|v| v.trim().parse::<f32>().ok())
        .filter(|p| (0.0..=1.0).contains(p))
        .unwrap_or(crate::local::FLOOR);
    // The runtime by name (the app's native library directory is on the
    // dynamic linker's search path, as for the OpenXR loader), then by the
    // Activity's nativeLibraryDir.
    let mut library = vec![std::path::PathBuf::from(crate::local::LIBRARY)];
    if let Some(dir) = permissions.and_then(|p| {
        p.native_library_dir()
            .inspect_err(|e| log::warn!("voice local: {e:#}"))
            .ok()
    }) {
        library.push(std::path::Path::new(&dir).join(crate::local::LIBRARY));
    }
    let options = crate::local::Options {
        threads,
        floors: crate::local::Floors::both(floor),
        library,
    };
    let (handle, loaded) = crate::local::Local::start(files.clone(), options)
        .map_err(|e| format!("local: no worker thread: {e}"))?;
    Ok(LocalAgent {
        handle: std::sync::Arc::new(handle),
        loaded,
        asked: false,
        about: format!(
            "{} loads after the speech model ({threads} threads, floor {floor:.2})",
            crate::local::MODEL_NAME
        ),
    })
}

/// Board #3776: the provider `pick` names, the one place the agent is set
/// (at launch, and on the hand menu's Agent control or the `agentpick`
/// knob): the network one from its config (cheap, no I/O); the on-device
/// one's handle, its worker started on the first pick and kept when the
/// pick moves away, so switching back is instant; none for off. Err: why
/// the pick cannot answer (a worker that would not start also leaves the
/// cycle).
fn switch_agent(
    pick: crate::agent::Pick,
    available: &mut crate::agent::Available,
    local: &mut Option<LocalAgent>,
    files: &crate::local::Files,
    permissions: Option<&Permissions>,
) -> Result<Option<std::sync::Arc<dyn crate::agent::Provider + Send + Sync>>, String> {
    match pick {
        crate::agent::Pick::Off => Ok(None),
        crate::agent::Pick::Network(_) => {
            let config = available.network.as_ref().map_err(Clone::clone)?;
            let provider: std::sync::Arc<dyn crate::agent::Provider + Send + Sync> =
                config.provider();
            Ok(Some(provider))
        }
        crate::agent::Pick::Local => {
            if local.is_none() {
                let started = start_local(files, permissions)
                    .inspect_err(|why| available.local = Err(why.clone()))?;
                *local = Some(started);
            }
            Ok(local.as_ref().map(|l| {
                let provider: std::sync::Arc<dyn crate::agent::Provider + Send + Sync> =
                    l.handle.clone();
                provider
            }))
        }
    }
}

/// Board #3751: one intent applied, the grammar's (V2) and the agent's
/// (V3) alike, and its label. The menu's intents become the `Action`s the
/// hand menu's presses produce, consumed by the next frame's action loop
/// (a toggle sets its `controls` field first, as the menu does; naming a
/// world effect sets `switch_to`); the surface intents write through the
/// lanes' typed writers now (`intent::apply`).
fn apply_intent(
    intent: &crate::intent::Intent,
    vocab: &crate::intent::Vocabulary<'_>,
    lanes: &mut crate::lanes::RoomLanes,
    boxes: &[crate::lanes::LaneBox<'_>],
    controls: &mut Controls,
    actions: &mut Vec<Action>,
    switch_to: &mut Option<usize>,
) -> String {
    use crate::intent::Intent;
    match *intent {
        Intent::NextEffect => actions.push(Action::NextEffect),
        Intent::PrevEffect => actions.push(Action::PrevEffect),
        // The effect showing has nothing parked, so naming it switches
        // nothing.
        Intent::Effect(i) => *switch_to = Some(i),
        Intent::EditRoom(on) => {
            controls.edit_room = on;
            actions.push(Action::SetEditRoom(on));
        }
        Intent::Cloud(on) => {
            controls.cloud = on;
            actions.push(Action::SetCloud(on));
        }
        Intent::Pitcher(on) => {
            controls.pitcher = on;
            actions.push(Action::SetPitcher(on));
        }
        Intent::Music(on) => {
            controls.music = on;
            actions.push(Action::SetMusic(on));
        }
        // Board #5385: the Music row's steps and the songs by name.
        Intent::NextTrack => actions.push(Action::NextTrack),
        Intent::PrevTrack => actions.push(Action::PrevTrack),
        Intent::Track(i) => {
            controls.music = true;
            actions.push(Action::PlayTrack(i));
        }
        Intent::ScanMusic => actions.push(Action::ScanMusic),
        Intent::Rescan => actions.push(Action::RescanRoom),
        Intent::Recenter => actions.push(Action::Recenter),
        Intent::AllNone => actions.push(Action::AllNone),
        // Board #5393: the Roam control's press, by voice.
        Intent::Roam(on) => actions.push(Action::SetRoam(on)),
        // Board #5384: a step from the depth the control shows; the label
        // names where it came to.
        Intent::Immersion(step) => {
            controls.immerse = step.resolve(controls.immerse);
            actions.push(Action::SetImmersion(controls.immerse));
            return crate::intent::immersion_reply(controls.immerse);
        }
        _ => {}
    }
    crate::intent::apply(intent, vocab, lanes, boxes)
        .unwrap_or_else(|| crate::intent::reply(intent, vocab))
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

/// How `audio_source` (`debug.fosfora.audio`) opens the microphones, from
/// its knobs; `None` for a source that does not listen. Read once at
/// launch, so the retry after a late `RECORD_AUDIO` grant (board #3264)
/// opens them as the launch would have. The sources match
/// `permissions::MIC_SOURCES`.
fn mic_source_for(audio_source: &str) -> Option<MicSource> {
    let sample_rate = || {
        debug_prop("debug.fosfora.micrate")
            .and_then(|v| v.parse().ok())
            .unwrap_or(48_000)
    };
    match audio_source {
        "mic" => Some(MicSource::Core),
        "micxr" => Some(MicSource::Cpal {
            format: match debug_prop("debug.fosfora.micfmt").as_deref() {
                Some("f32") => cpal::SampleFormat::F32,
                _ => cpal::SampleFormat::I16,
            },
            sample_rate: sample_rate(),
        }),
        "aaudio" | "loop" => {
            use ndk::audio::AudioInputPreset;
            Some(MicSource::AAudio {
                preset: match debug_prop("debug.fosfora.micpreset").as_deref() {
                    Some("generic") => AudioInputPreset::Generic,
                    Some("voice") => AudioInputPreset::VoiceRecognition,
                    Some("camcorder") => AudioInputPreset::Camcorder,
                    _ => AudioInputPreset::Unprocessed,
                },
                sample_rate: sample_rate(),
                low_latency: debug_prop("debug.fosfora.micperf").as_deref() != Some("none"),
            })
        }
        _ => None,
    }
}

fn debug_prop(name: &str) -> Option<String> {
    let out = std::process::Command::new("getprop")
        .arg(name)
        .output()
        .ok()?;
    let v = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    (!v.is_empty()).then_some(v)
}

/// A 0|1 knob polled from the frame loop ([`system_prop`]): `None` unset or
/// any other value.
fn switch_now(name: &str) -> Option<bool> {
    match system_prop(name).as_deref() {
        Some("0") => Some(false),
        Some("1") => Some(true),
        _ => None,
    }
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

/// The saved hand menu's toggles (`palm_panel::MenuFile`): the debug
/// panel off, voice on, no agent pick and immersion full when there is no
/// file, it does not parse, or a field is missing.
fn load_hand_menu(path: &std::path::Path) -> crate::palm_panel::MenuFile {
    std::fs::read_to_string(path)
        .map(|s| crate::palm_panel::MenuFile::parse(&s))
        .unwrap_or_default()
}

/// Save the hand menu's toggles; a failure is logged, not fatal.
fn save_hand_menu(path: &std::path::Path, menu: crate::palm_panel::MenuFile) {
    let on_off = |on: bool| if on { "on" } else { "off" };
    match std::fs::write(path, menu.to_json()) {
        Ok(()) => info!(
            "hand menu: debug panel {} · voice {}{} · immersion {} · roam {} (saved)",
            on_off(menu.debug),
            on_off(menu.voice),
            menu.agent
                .map(|p| format!(" · agent {}", p.name()))
                .unwrap_or_default(),
            menu.immerse.name(),
            on_off(menu.roam)
        ),
        Err(e) => log::warn!("hand menu: saving {}: {e}", path.display()),
    }
}

/// Board #5393: roam changed to `now`, because of `why`: the menu shows
/// it, the passthrough's floor follows it (`roam::immerse_floor` from the
/// seated one), and the log says so.
fn roam_changed(
    now: crate::roam::Roam,
    why: &str,
    immersion: &mut crate::immerse::Immersion,
    seated_floor: f32,
    controls: &mut Controls,
) {
    controls.roam = now;
    immersion.set_floor(crate::roam::immerse_floor(now.is_on(), seated_floor));
    info!(
        "roam: {} · {why} · passthrough floor {:.2}",
        now.describe(),
        immersion.floor()
    );
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
