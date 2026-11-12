//! Immersion (board #5384): the room gives way to the music. A level from
//! 0 (the plain room) to 1 (as dark as the wearer allows) follows the
//! song's fullness; it dims the passthrough image and raises the lit
//! surfaces' alpha, so the room shows while the song is quiet and the
//! surfaces glow at their own colors over a dark room while it is full.
//! Plain numbers, so it builds and tests on the desktop; `app.rs` steps it
//! once a frame, sets the passthrough layer's style through `xr.rs` and
//! lifts the surfaces' peak alpha with [`lift_rows`].
//!
//! **What it follows** ([`target`]): `loudness_s`, the short-term loudness
//! (a 3 s window, −60..0 LUFS on 0..1), so a section's level and not its
//! beats. A smoothstep from [`LOUD_QUIET`] (−27 LUFS: a quiet intro, or
//! music heard faintly) to [`LOUD_FULL`] (−15 LUFS: a full mastered
//! section), plus [`BUILDUP_LIFT`] times `buildup`, so a riser darkens the
//! room ahead of the drop. A `drop` holds the target at 1 for
//! [`DROP_HOLD_S`] and reaches it with [`DROP_ATTACK_S`]. The bundled clip
//! reads 0.71 to 0.73 on `loudness_s` from its first second to its last,
//! `buildup` 0.14 to 0.46, and no drop fires in it (the core's
//! `--signal-dump` of `ember_glow_excerpt.ogg`), so it plays at the full
//! depth. The synthetic groove carries no `loudness_s` at all.
//!
//! **The smoothing.** One pole toward the target, with the time constant
//! [`ATTACK_S`] while it rises and [`RELEASE_S`] while it falls, so the
//! room breathes with the sections and never flickers on a beat.
//!
//! **Quiet.** With no real audio driving the analysis (the music stopped
//! over the synthetic groove), or the `rms` under [`QUIET_RMS`] for
//! [`QUIET_HOLD_S`] (the analysis's silence gate puts the energy features
//! at 0), the target is 0 with the time constant [`QUIET_RELEASE_S`]: the
//! room is back within about 1.5 s. `loudness_s` alone would take its
//! 3 s window to fall.
//!
//! **The depth** ([`Depth`]): how far the wearer lets it go, off (0), low
//! (0.5) or full (1), multiplied into the level. A change glides over
//! [`DEPTH_S`], so a press on the hand menu is seen at once but does not
//! cut.
//!
//! **What it drives.** The passthrough image's opacity,
//! `1 - level * (1 - floor)` ([`Immersion::opacity`]), with the floor the
//! darkest the room may get ([`IMMERSE_FLOOR`]; a parameter of the type, so
//! a mode that needs the room more can raise it); the style call goes to
//! the runtime only when the opacity moves by a step of 1/[`OPACITY_STEPS`]
//! ([`StyleGate`]). And the lit surfaces' peak alpha ([`peak_alpha`]): a
//! ported effect's rises from its 0.5 to [`ALPHA_FULL`], the built-in
//! behaviors' by the same ratio, capped at 1.

/// The wearer's depth setting: the hand menu's Immersion control and the
/// voice's "darker" and "lighter".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Depth {
    /// The plain room: the level stays 0.
    Off,
    /// Half way.
    Low,
    /// As dark as the floor allows.
    #[default]
    Full,
}

impl Depth {
    /// In the menu's cycle order.
    pub const ALL: [Self; 3] = [Self::Off, Self::Low, Self::Full];

    /// The multiplier on the level.
    pub fn value(self) -> f32 {
        match self {
            Self::Off => 0.0,
            Self::Low => 0.5,
            Self::Full => 1.0,
        }
    }

    /// Its name in the log, `hand_menu.json` and the voice's replies.
    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Low => "low",
            Self::Full => "full",
        }
    }

    /// The hand menu's words: the state in the words, as the other rows.
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Immersion: off",
            Self::Low => "Immersion: low",
            Self::Full => "Immersion: full",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|d| d.name() == name)
    }

    /// The menu's press: off, low, full, then off again.
    #[must_use]
    pub fn next(self) -> Self {
        match self {
            Self::Off => Self::Low,
            Self::Low => Self::Full,
            Self::Full => Self::Off,
        }
    }

    /// One step darker ("darker"), full at most.
    #[must_use]
    pub fn deeper(self) -> Self {
        match self {
            Self::Off => Self::Low,
            Self::Low | Self::Full => Self::Full,
        }
    }

    /// One step lighter ("lighter"), off at least.
    #[must_use]
    pub fn lighter(self) -> Self {
        match self {
            Self::Full => Self::Low,
            Self::Low | Self::Off => Self::Off,
        }
    }

    /// The named depth nearest a multiplier (a launch override between two).
    pub fn nearest(value: f32) -> Self {
        let off_by = |d: Self| (d.value() - value).abs();
        Self::ALL
            .into_iter()
            .min_by(|a, b| off_by(*a).total_cmp(&off_by(*b)))
            .unwrap_or_default()
    }
}

/// `loudness_s` at which the target starts to rise (−27 LUFS).
pub const LOUD_QUIET: f32 = 0.55;
/// `loudness_s` at which it reaches 1 (−15 LUFS).
pub const LOUD_FULL: f32 = 0.75;
/// How much a full `buildup` adds to the target.
pub const BUILDUP_LIFT: f32 = 0.5;
/// The time constants (s) while the level rises and while it falls.
pub const ATTACK_S: f32 = 2.0;
pub const RELEASE_S: f32 = 4.0;
/// A drop: the target held at 1 this long (s), reached with this time
/// constant (s).
pub const DROP_HOLD_S: f32 = 8.0;
pub const DROP_ATTACK_S: f32 = 0.25;
/// Quiet: the `rms` under this for [`QUIET_HOLD_S`] (s); the level then
/// falls with the time constant [`QUIET_RELEASE_S`] (s), to under 0.05 of
/// its depth in 0.3 + 3 x 0.35 = 1.35 s.
pub const QUIET_RMS: f32 = 0.02;
pub const QUIET_HOLD_S: f32 = 0.3;
pub const QUIET_RELEASE_S: f32 = 0.35;
/// A change of the depth setting glides with this time constant (s).
pub const DEPTH_S: f32 = 0.5;
/// The darkest the room may get: the passthrough image's opacity at level
/// 1 (`debug.fosfora.immersefloor`).
pub const IMMERSE_FLOOR: f32 = 0.15;
/// The style call's steps over the opacity's 0..1.
pub const OPACITY_STEPS: u32 = 64;
/// A ported effect's peak alpha at level 1 (`surface_port::PEAK_ALPHA`, 0.5,
/// is its level-0 value).
pub const ALPHA_FULL: f32 = 1.0;
/// The `color` row of the surfaces pass's and the ported effects' uniform
/// blocks (`surface_fx::rows`, `surface_port::rows`): its w is the face's
/// peak alpha.
pub const PEAK_ROW: usize = 6;

/// What the frame loop hands the level each frame, from this frame's
/// `AudioFeatures`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Input {
    /// The short-term loudness, 0..1.
    pub loudness_s: f32,
    /// The riser's tension, 0..1.
    pub buildup: f32,
    /// The drop fired.
    pub drop: bool,
    /// The plain level, 0..1 (0 under the analysis's silence gate).
    pub rms: f32,
    /// Real audio drives the analysis: the music's tap or the microphones,
    /// not the synthetic groove.
    pub playing: bool,
}

/// `x` in 0..1, and 0 for anything that is not a number.
fn unit(x: f32) -> f32 {
    if x.is_finite() {
        x.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// The level the song asks for, before the smoothing and the depth: the
/// smoothstep of `loudness_s` from [`LOUD_QUIET`] to [`LOUD_FULL`], plus
/// [`BUILDUP_LIFT`] times `buildup`, at most 1.
pub fn target(input: &Input) -> f32 {
    let x = ((unit(input.loudness_s) - LOUD_QUIET) / (LOUD_FULL - LOUD_QUIET)).clamp(0.0, 1.0);
    (x * x * (3.0 - 2.0 * x) + BUILDUP_LIFT * unit(input.buildup)).min(1.0)
}

/// `from` moved toward `to` over `dt` with time constant `tau` (s).
fn approach(from: f32, to: f32, dt: f32, tau: f32) -> f32 {
    from + (to - from) * (1.0 - (-dt / tau).exp())
}

/// The immersion level.
#[derive(Debug, Clone, Copy)]
pub struct Immersion {
    /// The wearer's setting, the multiplier on the level.
    depth: f32,
    /// The depth as it glides toward `depth`.
    shown_depth: f32,
    /// The passthrough image's opacity at level 1.
    floor: f32,
    /// The music's level, smoothed, before the depth.
    music: f32,
    /// What [`Self::step`] returned last.
    level: f32,
    /// The drop's hold left (s).
    drop_s: f32,
    /// How long it has been quiet (s).
    quiet_s: f32,
    /// `debug.fosfora.immersefix`: the level pinned, whatever the audio
    /// and the depth.
    fixed: Option<f32>,
}

impl Immersion {
    /// At `depth` (0..1) with the room at `floor` (0..1) at most dark.
    pub fn new(depth: f32, floor: f32) -> Self {
        let depth = unit(depth);
        Self {
            depth,
            shown_depth: depth,
            floor: unit(floor),
            music: 0.0,
            level: 0.0,
            drop_s: 0.0,
            quiet_s: 0.0,
            fixed: None,
        }
    }

    pub fn depth(&self) -> f32 {
        self.depth
    }

    /// The wearer changed the depth: the level glides to it.
    pub fn set_depth(&mut self, depth: f32) {
        self.depth = unit(depth);
    }

    pub fn floor(&self) -> f32 {
        self.floor
    }

    pub fn set_floor(&mut self, floor: f32) {
        self.floor = unit(floor);
    }

    /// Pin the level (`Some`, 0..1) whatever the audio and the depth, or
    /// let it follow the music again (`None`).
    pub fn fix(&mut self, level: Option<f32>) {
        self.fixed = level.map(unit);
    }

    pub fn fixed(&self) -> Option<f32> {
        self.fixed
    }

    /// The level [`Self::step`] returned last.
    pub fn level(&self) -> f32 {
        self.level
    }

    /// Once a frame, `dt_s` after the last: the level, 0..1, the depth
    /// applied.
    pub fn step(&mut self, dt_s: f32, input: Input) -> f32 {
        let dt = if dt_s.is_finite() { dt_s.max(0.0) } else { 0.0 };
        self.quiet_s = if !input.playing {
            QUIET_HOLD_S
        } else if unit(input.rms) < QUIET_RMS {
            self.quiet_s + dt
        } else {
            0.0
        };
        let quiet = self.quiet_s >= QUIET_HOLD_S;
        if quiet {
            self.drop_s = 0.0;
        } else if input.drop {
            self.drop_s = DROP_HOLD_S;
        }
        let (goal, tau) = if quiet {
            (0.0, QUIET_RELEASE_S)
        } else if self.drop_s > 0.0 {
            self.drop_s = (self.drop_s - dt).max(0.0);
            (1.0, DROP_ATTACK_S)
        } else {
            let goal = target(&input);
            (
                goal,
                if goal > self.music {
                    ATTACK_S
                } else {
                    RELEASE_S
                },
            )
        };
        self.music = approach(self.music, goal, dt, tau);
        self.shown_depth = approach(self.shown_depth, self.depth, dt, DEPTH_S);
        self.level = self.fixed.unwrap_or(self.music * self.shown_depth);
        self.level
    }

    /// The passthrough image's opacity at `level`: 1 at 0, the floor at 1.
    pub fn opacity(&self, level: f32) -> f32 {
        1.0 - unit(level) * (1.0 - self.floor)
    }

    /// The launch log line: "immersion: full · floor 0.15 · attack 2.0 s ·
    /// release 4.0 s", the depth named when it is one of [`Depth`]'s,
    /// marked when `debug.fosfora.immerse` set it (`from_knob`), with the
    /// pinned level and a missing passthrough said.
    pub fn launch_line(&self, from_knob: bool, passthrough: bool) -> String {
        let depth = Depth::ALL
            .into_iter()
            .find(|d| (d.value() - self.depth).abs() < 1e-4)
            .map_or_else(|| format!("{:.2}", self.depth), |d| d.name().to_owned());
        let knob = if from_knob {
            " (debug.fosfora.immerse)"
        } else {
            ""
        };
        let pinned = self
            .fixed
            .map(|l| format!(" \u{b7} pinned at {l:.2} (debug.fosfora.immersefix)"))
            .unwrap_or_default();
        let room = if passthrough {
            ""
        } else {
            " \u{b7} no passthrough: the room and the surfaces stay as they are"
        };
        format!(
            "immersion: {depth}{knob} \u{b7} floor {:.2} \u{b7} attack {ATTACK_S:.1} s \u{b7} release {RELEASE_S:.1} s{pinned}{room}",
            self.floor,
        )
    }
}

/// The step of `opacity` on the style call's grid of [`OPACITY_STEPS`].
pub fn opacity_step(opacity: f32) -> u32 {
    (unit(opacity) * OPACITY_STEPS as f32).round() as u32
}

/// What the runtime was last told: the passthrough opacity's step, sent
/// only when it changes. It starts at the runtime's own default, opacity
/// 1, so a run the level never leaves 0 makes no call.
#[derive(Debug, Clone, Copy)]
pub struct StyleGate {
    sent: u32,
}

impl Default for StyleGate {
    fn default() -> Self {
        Self {
            sent: OPACITY_STEPS,
        }
    }
}

impl StyleGate {
    /// The opacity to send this frame, on the grid, or `None` when the
    /// runtime already has it.
    pub fn update(&mut self, opacity: f32) -> Option<f32> {
        let step = opacity_step(opacity);
        (step != self.sent).then(|| {
            self.sent = step;
            step as f32 / OPACITY_STEPS as f32
        })
    }
}

/// A lit surface's peak alpha at `level`, from `base`, its level-0 value:
/// `base` times the ported effects' ratio (`ALPHA_FULL / 0.5`, 2 at level
/// 1), so a port's 0.5 becomes `0.5 + 0.5 * level` and the built-in
/// behaviors rise in the same proportion; capped at 1, and never under
/// `base`, so level 0 is `base` exactly (a rings or spectrum gain over 4
/// keeps its peak over 1 as before).
pub fn peak_alpha(base: f32, level: f32) -> f32 {
    let lift = ALPHA_FULL / crate::surface_port::PEAK_ALPHA - 1.0;
    (base * (1.0 + lift * unit(level))).min(1.0).max(base)
}

/// A face's uniform rows (`surface_fx::rows` or `surface_port::rows`)
/// with the peak alpha in [`PEAK_ROW`] lifted to `level` ([`peak_alpha`]).
pub fn lift_rows<const N: usize>(mut rows: [[f32; 4]; N], level: f32) -> [[f32; 4]; N] {
    const { assert!(N > PEAK_ROW) };
    rows[PEAK_ROW][3] = peak_alpha(rows[PEAK_ROW][3], level);
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    const FPS: f32 = 72.0;

    /// A section of the song: the inputs as a function of the time since
    /// its start.
    type Section<'a> = &'a dyn Fn(f32) -> Input;

    /// Step `im` at 72 frames a second for `secs` with `input`; returns the
    /// level after each frame.
    fn run(im: &mut Immersion, secs: f32, input: Section<'_>) -> Vec<f32> {
        let frames = (secs * FPS).round() as u32;
        (0..frames)
            .map(|i| im.step(1.0 / FPS, input(i as f32 / FPS)))
            .collect()
    }

    fn playing(loudness_s: f32, buildup: f32) -> Input {
        Input {
            loudness_s,
            buildup,
            drop: false,
            rms: 0.6,
            playing: true,
        }
    }

    /// The bundled clip as the analysis reads it: `loudness_s` 0.72,
    /// `buildup` about 0.3.
    fn full(_: f32) -> Input {
        playing(0.72, 0.3)
    }

    /// A quiet intro: −36 LUFS, a little tension.
    fn quiet(_: f32) -> Input {
        playing(0.4, 0.05)
    }

    /// Silence after a full section: the rms at 0 (the silence gate),
    /// `loudness_s` still high in its 3 s window.
    fn silence(_: f32) -> Input {
        Input {
            rms: 0.0,
            ..playing(0.7, 0.3)
        }
    }

    /// The first time (s) the levels fall under `under`.
    fn first_under(levels: &[f32], under: f32) -> Option<f32> {
        levels
            .iter()
            .position(|&l| l < under)
            .map(|i| (i + 1) as f32 / FPS)
    }

    #[test]
    fn the_map_follows_the_sections_loudness_and_the_buildup() {
        assert_close!(target(&playing(0.3, 0.0)), 0.0);
        assert_close!(target(&playing(LOUD_QUIET, 0.0)), 0.0);
        assert_close!(target(&playing(0.65, 0.0)), 0.5);
        assert_close!(target(&playing(LOUD_FULL, 0.0)), 1.0);
        assert_close!(target(&playing(0.95, 0.0)), 1.0);
        // The buildup lifts it, at most to 1.
        assert_close!(target(&playing(0.65, 0.4)), 0.7);
        assert_close!(target(&playing(0.72, 0.5)), 1.0);
        // The bundled clip without its buildup sits near the top.
        let clip = target(&playing(0.72, 0.0));
        assert!((0.9..1.0).contains(&clip), "{clip}");
        // Not a number reads as nothing.
        assert_close!(target(&playing(f32::NAN, f32::INFINITY)), 0.0);
    }

    #[test]
    fn quiet_music_stays_near_zero() {
        let mut im = Immersion::new(1.0, IMMERSE_FLOOR);
        let levels = run(&mut im, 60.0, &quiet);
        assert!(levels.iter().all(|&l| l < 0.05), "{:?}", levels.last());
        assert!(im.opacity(im.level()) > 0.95);
    }

    #[test]
    fn a_full_section_reaches_the_depth_with_the_attack() {
        let mut im = Immersion::new(1.0, IMMERSE_FLOOR);
        let levels = run(&mut im, 10.0, &full);
        // One time constant: about 63 % of the way; three: 95 %.
        let at = |s: f32| levels[(s * FPS) as usize - 1];
        assert!((0.55..0.7).contains(&at(ATTACK_S)), "{}", at(ATTACK_S));
        assert!(at(3.0 * ATTACK_S) > 0.94, "{}", at(3.0 * ATTACK_S));
        assert!(im.level() > 0.99, "{}", im.level());
        assert_close!(im.opacity(1.0), IMMERSE_FLOOR);
        // Back to a quiet section: it releases over the slower constant.
        let levels = run(&mut im, 2.0 * RELEASE_S, &quiet);
        let after_one = levels[(RELEASE_S * FPS) as usize - 1];
        assert!((0.3..0.45).contains(&after_one), "{after_one}");
    }

    #[test]
    fn a_drop_reaches_the_depth_fast_and_holds_it() {
        let mut im = Immersion::new(1.0, IMMERSE_FLOOR);
        run(&mut im, 10.0, &quiet);
        // A drop on the first frame of a section the map puts half way.
        let levels = run(&mut im, DROP_HOLD_S, &|t| Input {
            drop: t < 0.5 / FPS,
            ..playing(0.65, 0.0)
        });
        let at = |s: f32| levels[(s * FPS) as usize];
        assert!(at(0.75) > 0.9, "{}", at(0.75));
        assert!(at(1.0) > 0.95, "{}", at(1.0));
        // Held at the depth through the hold, not back to the map's 0.5.
        assert!(levels[(FPS as usize)..].iter().all(|&l| l > 0.95));
        // Then the map's level again, over the release.
        let levels = run(&mut im, 6.0 * RELEASE_S, &|_| playing(0.65, 0.0));
        assert!((levels[levels.len() - 1] - 0.5).abs() < 0.01);
    }

    #[test]
    fn a_beat_rate_wobble_hardly_moves_the_level() {
        // 140 BPM, the bundled clip's tempo. Within one of its beats
        // `loudness_s` moves by 0.005 at most and `buildup` by 0.11 (the
        // `--signal-dump`); here they swing by 0.02 and 0.2 on every beat,
        // and the rms from 0.05 to 0.95, its auto-leveled range.
        let beat_hz = 140.0 / 60.0;
        let wobble = |t: f32| {
            let s = (std::f32::consts::TAU * beat_hz * t).sin();
            Input {
                loudness_s: 0.65 + 0.01 * s,
                buildup: 0.25 + 0.1 * s,
                drop: false,
                rms: 0.5 + 0.45 * s,
                playing: true,
            }
        };
        let mut im = Immersion::new(1.0, IMMERSE_FLOOR);
        let levels = run(&mut im, 40.0, &wobble);
        let tail = &levels[levels.len() - 10 * FPS as usize..];
        let (lo, hi) = tail
            .iter()
            .fold((f32::MAX, f32::MIN), |(lo, hi), &l| (lo.min(l), hi.max(l)));
        assert!(hi - lo < 0.02, "{lo}..{hi}");
        // On the steady inputs' level, a little over it: the attack is
        // faster than the release, so a wobble ratchets it up.
        let steady = target(&playing(0.65, 0.25));
        assert!((0.0..0.05).contains(&(lo - steady)), "{lo} vs {steady}");
        // The passthrough's step moves by one at most.
        assert!(opacity_step(im.opacity(hi)).abs_diff(opacity_step(im.opacity(lo))) <= 1);
    }

    #[test]
    fn silence_or_stopped_music_brings_the_room_back_within_1_5_s() {
        let back = |input: Section<'_>| {
            let mut im = Immersion::new(1.0, IMMERSE_FLOOR);
            run(&mut im, 10.0, &full);
            assert!(im.level() > 0.99);
            first_under(&run(&mut im, 3.0, input), 0.05)
        };
        // Silence: the rms at 0 while `loudness_s` is still in its window.
        let t = back(&silence).expect("back");
        assert!(t <= 1.5, "{t}");
        // The music stopped: the synthetic groove's features, no hold.
        let t = back(&|_| Input {
            playing: false,
            ..playing(0.0, 0.0)
        })
        .expect("back");
        assert!(t <= 1.5 - QUIET_HOLD_S, "{t}");
        // A short dip of the rms (a break) is not silence.
        let mut im = Immersion::new(1.0, IMMERSE_FLOOR);
        run(&mut im, 10.0, &full);
        run(&mut im, 0.25, &silence);
        let levels = run(&mut im, 1.0, &full);
        assert!(levels.iter().all(|&l| l > 0.98), "{:?}", levels.first());
    }

    #[test]
    fn the_depth_scales_the_ceiling() {
        for (depth, ceiling) in [(Depth::Full, 1.0), (Depth::Low, 0.5), (Depth::Off, 0.0)] {
            let mut im = Immersion::new(depth.value(), IMMERSE_FLOOR);
            let levels = run(&mut im, 20.0, &full);
            assert!(
                levels.iter().all(|&l| l <= ceiling + 1e-4),
                "{depth:?}: {:?}",
                levels.last()
            );
            assert!(
                (im.level() - ceiling).abs() < 0.01,
                "{depth:?}: {}",
                im.level()
            );
        }
        // A change glides: seen within a frame, settled within 1.5 s.
        let mut im = Immersion::new(1.0, IMMERSE_FLOOR);
        run(&mut im, 20.0, &full);
        im.set_depth(Depth::Low.value());
        let levels = run(&mut im, 1.5, &full);
        assert!(levels[0] < 0.99 && levels[0] > 0.9, "{}", levels[0]);
        assert!((im.level() - 0.5).abs() < 0.03, "{}", im.level());
    }

    #[test]
    fn the_floor_is_the_darkest_and_a_parameter() {
        let mut im = Immersion::new(1.0, IMMERSE_FLOOR);
        assert_close!(
            [im.opacity(0.0), im.opacity(0.5), im.opacity(1.0)],
            [1.0, 1.0 - 0.5 * (1.0 - IMMERSE_FLOOR), IMMERSE_FLOOR]
        );
        im.set_floor(0.6);
        assert_close!([im.opacity(1.0), im.floor()], [0.6, 0.6]);
        assert_close!(im.opacity(2.0), 0.6);
    }

    #[test]
    fn a_pinned_level_ignores_the_audio_and_the_depth() {
        let mut im = Immersion::new(0.0, IMMERSE_FLOOR);
        im.fix(Some(0.8));
        for input in [quiet(0.0), full(0.0), silence(0.0)] {
            assert_close!(im.step(1.0 / FPS, input), 0.8);
        }
        im.fix(Some(3.0));
        assert_close!(im.step(1.0 / FPS, quiet(0.0)), 1.0);
        im.fix(None);
        assert!(im.step(1.0 / FPS, quiet(0.0)) < 0.01);
    }

    #[test]
    fn the_style_goes_out_only_when_its_step_changes() {
        let mut gate = StyleGate::default();
        // The runtime's default is opacity 1: nothing to send at level 0.
        assert_eq!(gate.update(1.0), None);
        assert_eq!(gate.update(0.996), None);
        let sent = gate.update(0.9).expect("a new step");
        assert_close!(sent, 58.0 / 64.0);
        assert_eq!(gate.update(0.9), None);
        assert_eq!(gate.update(0.903), None);
        assert!(gate.update(0.88).is_some());
        // A full ramp over the attack sends at most one call per step.
        let mut im = Immersion::new(1.0, IMMERSE_FLOOR);
        let mut gate = StyleGate::default();
        let calls = run(&mut im, 20.0, &full)
            .into_iter()
            .filter_map(|l| gate.update(im.opacity(l)))
            .count();
        let steps = opacity_step(1.0) - opacity_step(IMMERSE_FLOOR);
        assert!(calls as u32 <= steps, "{calls} calls for {steps} steps");
        assert!(calls > 10);
    }

    #[test]
    fn the_peak_alpha_rises_with_the_level_and_stays_at_level_zero() {
        use crate::surface_fx::{CURL_PEAK_ALPHA, PULSE_PEAK_ALPHA, STREAM_PEAK_ALPHA};
        use crate::surface_port::PEAK_ALPHA;
        // Level 0: every base exactly as it was, a gain over 4 included.
        for base in [
            PEAK_ALPHA,
            STREAM_PEAK_ALPHA,
            PULSE_PEAK_ALPHA,
            0.0,
            0.125,
            1.3,
        ] {
            assert_eq!(peak_alpha(base, 0.0).to_bits(), base.to_bits(), "{base}");
        }
        // A port: from 0.5 to ALPHA_FULL along the level.
        assert_close!(
            [peak_alpha(PEAK_ALPHA, 0.5), peak_alpha(PEAK_ALPHA, 1.0)],
            [0.75, ALPHA_FULL]
        );
        // The built-in behaviors by the same ratio, capped at 1.
        assert_close!(peak_alpha(PULSE_PEAK_ALPHA, 1.0), 0.9);
        assert_close!(peak_alpha(STREAM_PEAK_ALPHA, 1.0), 1.0);
        assert_close!(peak_alpha(CURL_PEAK_ALPHA, 0.5), 0.825);
        assert_close!(peak_alpha(crate::ripple::PEAK_ALPHA, 1.0), 0.5);
        assert_close!(peak_alpha(1.3, 1.0), 1.3);
    }

    #[test]
    fn the_rows_carry_the_lifted_peak_alpha_in_the_color_row() {
        use crate::surface_fx::{Audio, PULSE_PEAK_ALPHA, Slot};
        use crate::surfaces::{Face, KIND_FRAME, SurfaceBehavior};
        use glam::Vec3;
        let face = Face {
            normal: Vec3::Z,
            center: Vec3::new(0.0, 1.5, -2.0),
            axes: [Vec3::X, Vec3::Y],
            half: [0.4, 0.3],
        };
        // The surfaces pass's pulse: the color row's w.
        let slot = Slot {
            face,
            kind: KIND_FRAME,
            behavior: SurfaceBehavior::Pulse,
            strength: 0.7,
            color: 0,
            band: 0,
        };
        let rows = crate::surface_fx::rows(&slot, Audio::default(), None, None, 0.0);
        assert_close!(rows[PEAK_ROW][3], PULSE_PEAK_ALPHA);
        assert_eq!(lift_rows(rows, 0.0), rows);
        let lifted = lift_rows(rows, 1.0);
        assert_close!(lifted[PEAK_ROW][3], 0.9);
        // Only the peak alpha moves: the strength, the color and the rest
        // are the pass's.
        let mut back = lifted;
        back[PEAK_ROW][3] = rows[PEAK_ROW][3];
        assert_eq!(back, rows);
        // A port's.
        let rows = crate::surface_port::rows(&face, 1.0, true, 9, [1.0; 3]);
        assert_close!(rows[PEAK_ROW][3], crate::surface_port::PEAK_ALPHA);
        assert_close!(lift_rows(rows, 1.0)[PEAK_ROW][3], ALPHA_FULL);
        assert_eq!(lift_rows(rows, 0.0), rows);
    }

    #[test]
    fn the_depth_steps_names_and_nearest() {
        use Depth::{Full, Low, Off};
        assert_eq!(Depth::default(), Full);
        assert_eq!([Off.next(), Low.next(), Full.next()], [Low, Full, Off]);
        assert_eq!(
            [Off.deeper(), Low.deeper(), Full.deeper()],
            [Low, Full, Full]
        );
        assert_eq!(
            [Off.lighter(), Low.lighter(), Full.lighter()],
            [Off, Off, Low]
        );
        for d in Depth::ALL {
            assert_eq!(Depth::from_name(d.name()), Some(d));
            assert!(d.label().starts_with("Immersion: "), "{}", d.label());
            assert_eq!(Depth::nearest(d.value()), d);
        }
        assert_eq!(Depth::from_name("dark"), None);
        assert_eq!(
            [0.2, 0.3, 0.74, 0.76].map(Depth::nearest),
            [Off, Low, Low, Full]
        );
    }

    #[test]
    fn the_launch_line_says_the_depth_the_floor_and_the_time_constants() {
        let im = Immersion::new(1.0, IMMERSE_FLOOR);
        assert_eq!(
            im.launch_line(false, true),
            "immersion: full \u{b7} floor 0.15 \u{b7} attack 2.0 s \u{b7} release 4.0 s"
        );
        let mut im = Immersion::new(0.75, 0.3);
        im.fix(Some(0.8));
        assert_eq!(
            im.launch_line(true, false),
            "immersion: 0.75 (debug.fosfora.immerse) \u{b7} floor 0.30 \u{b7} attack 2.0 s \u{b7} release 4.0 s \u{b7} pinned at 0.80 (debug.fosfora.immersefix) \u{b7} no passthrough: the room and the surfaces stay as they are"
        );
    }
}
