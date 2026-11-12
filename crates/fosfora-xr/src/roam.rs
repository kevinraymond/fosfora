//! Roam (board #5393): the app without a set-up room. The show works two
//! ways: seated in a room the headset has scanned, or roaming through
//! places it has not. In roam the show follows the wearer. Plain numbers,
//! so the rules build and test on the desktop; `app.rs` holds one of each
//! type and applies what they return.
//!
//! **When it is on** ([`RoamState`]). The wearer chooses it (the hand
//! menu's Roam control, or "roam" by voice), and the choice is saved in
//! `hand_menu.json`. Or the scan turns it on by itself: when the scene
//! query ends with no anchors after its retries, roam goes on and the scan
//! label says so for a few seconds ([`NO_ROOM_LABEL`]). That one is not
//! saved, and a later scan that finds a room turns it off again. In a
//! scanned room the default is off. `debug.fosfora.roam 0|1` forces it for
//! a launch, not saved, and a press takes over from the knob.
//!
//! **The cloud follows** ([`Follow`]). When the head is more than
//! [`ROAM_FOLLOW_M`] from the cloud's center, measured level, the center
//! glides to the head over [`GLIDE_S`] (Recenter's spot, at Recenter's
//! height), at most once every [`FOLLOW_EVERY_S`]. The sim keeps its
//! particles relative to the center, so they ride along with it: the cloud
//! drifts after the wearer, nothing jumps and nothing dies.
//!
//! **The floor follows** ([`floor_center`]). The stage floor's slab is 20 m
//! across. In roam it re-centers under the head whenever the head is more
//! than [`FLOOR_FOLLOW_M`] from its center, so there are always at least
//! 7 m of floor around the wearer. Its height stays y = 0.
//!
//! **Something to see** ([`Fill`]). The default world effect, Embers,
//! spawns only from surfaces that carry emitter weight, and with no room
//! nothing does. In roam, when the effect showing is the surface-born one
//! and nothing in reach emits, the show switches to [`FILL_EFFECT`], the
//! volume emitter, once. Turning roam off switches back, unless the wearer
//! picked another effect meanwhile.
//!
//! **Immersion gives way to walking** ([`Walk`], [`immerse_floor`],
//! [`immerse_level`]). In roam the room gets no darker than
//! [`ROAM_FLOOR`], and the immersion level is scaled down by head speed:
//! the full level below [`WALK_FULL_M_S`], none above [`WALK_NONE_M_S`].
//! The speed is the head's level travel per frame, low-passed over
//! [`WALK_SMOOTH_S`], so the room comes back while the wearer walks and
//! dims again when they stop.

use glam::{Vec2, Vec3};

use crate::label::ScanState;

/// Why roam is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    /// The wearer turned it on: the hand menu's Roam control or the voice.
    /// Saved.
    Chosen,
    /// The scan found no room. Not saved.
    NoRoom,
}

/// Whether the show follows the wearer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Roam {
    #[default]
    Off,
    On(Why),
}

impl Roam {
    pub fn is_on(self) -> bool {
        matches!(self, Self::On(_))
    }

    /// The hand menu's words, the state in the words as on the other rows.
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Roam: off",
            Self::On(Why::Chosen) => "Roam: on",
            Self::On(Why::NoRoom) => "Roam: on (no room)",
        }
    }

    /// The log's words: "off", "on (chosen)", "on (no room)".
    pub fn describe(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::On(Why::Chosen) => "on (chosen)",
            Self::On(Why::NoRoom) => "on (no room)",
        }
    }
}

/// What the scan label says when the scan found no room and roam went on
/// by itself.
pub const NO_ROOM_LABEL: &str = "No room here: roaming. Rescan from the hand menu";

/// The scan label's words for `state` with roam at `roam`: [`NO_ROOM_LABEL`]
/// for a scan that found nothing while roam is on because of it, else the
/// label's own (`label::scan_text`).
pub fn scan_text(state: ScanState, roam: Roam) -> String {
    if state == ScanState::None && roam == Roam::On(Why::NoRoom) {
        NO_ROOM_LABEL.to_owned()
    } else {
        crate::label::scan_text(state)
    }
}

/// The wearer's choice, the scan's and the launch knob's, across a run.
#[derive(Debug, Clone, Copy, Default)]
pub struct RoamState {
    /// The wearer's choice, saved.
    chosen: bool,
    /// The last scan that settled found no room.
    no_room: bool,
    /// `debug.fosfora.roam` for this launch, until a press.
    forced: Option<bool>,
    /// The scan state last seen (`None`: no room in this mode).
    scan: Option<ScanState>,
}

impl RoamState {
    /// At launch: the saved choice and the knob.
    pub fn new(chosen: bool, forced: Option<bool>) -> Self {
        Self {
            chosen,
            forced,
            ..Self::default()
        }
    }

    /// Roam now: the knob while it holds, else the wearer's choice, else
    /// the scan's.
    pub fn now(&self) -> Roam {
        match self.forced {
            Some(false) => Roam::Off,
            Some(true) => Roam::On(Why::Chosen),
            None if self.chosen => Roam::On(Why::Chosen),
            None if self.no_room => Roam::On(Why::NoRoom),
            None => Roam::Off,
        }
    }

    /// The wearer's choice, for `hand_menu.json`.
    pub fn chosen(&self) -> bool {
        self.chosen
    }

    /// Whether `debug.fosfora.roam` still decides.
    pub fn forced(&self) -> Option<bool> {
        self.forced
    }

    /// This frame's scan state (`Room::scan_state`, before the permission's
    /// label). A scan that ends with no anchors turns the automatic roam
    /// on, one that finds a room turns it off. Only a change of state acts,
    /// so a wearer's "stay here" holds until the next scan ends. Returns
    /// roam when it changed.
    pub fn scan(&mut self, state: Option<ScanState>) -> Option<Roam> {
        if state == self.scan {
            return None;
        }
        let before = self.now();
        self.scan = state;
        match state {
            Some(ScanState::None) => self.no_room = true,
            Some(ScanState::Found(_)) => self.no_room = false,
            _ => {}
        }
        let after = self.now();
        (after != before).then_some(after)
    }

    /// The wearer's choice, from the menu or the voice: saved by the caller
    /// ([`Self::chosen`]). Off also ends the automatic roam, until a scan
    /// ends with no room again. Either takes over from the knob. Returns
    /// roam when it changed.
    pub fn set(&mut self, on: bool) -> Option<Roam> {
        let before = self.now();
        self.chosen = on;
        if !on {
            self.no_room = false;
        }
        self.forced = None;
        let after = self.now();
        (after != before).then_some(after)
    }
}

/// The launch line: "roam: off", "roam: on (chosen)", or with the knob
/// "roam: on (debug.fosfora.roam, not saved)".
pub fn launch_line(state: &RoamState) -> String {
    let roam = state.now();
    match state.forced() {
        Some(_) => format!(
            "roam: {} (debug.fosfora.roam, not saved)",
            if roam.is_on() { "on" } else { "off" }
        ),
        None => format!("roam: {}", roam.describe()),
    }
}

/// How far the head may get from the cloud's center, level, before the
/// center follows (m).
pub const ROAM_FOLLOW_M: f32 = 1.0;
/// How long a follow glides (s).
pub const GLIDE_S: f32 = 1.5;
/// The shortest time from one follow's start to the next (s).
pub const FOLLOW_EVERY_S: f32 = 3.0;
/// How far the center may be from where the glide put it before the glide
/// counts as overridden, by a drag or Recenter (m).
const GLIDE_SLIP_M: f32 = 0.01;

/// The level part of `v`.
fn level(v: Vec3) -> Vec2 {
    Vec2::new(v.x, v.z)
}

/// 0..1 eased in and out.
fn smoothstep(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// `dt` as a usable step: finite and not negative.
fn step_s(dt: f32) -> f32 {
    if dt.is_finite() { dt.max(0.0) } else { 0.0 }
}

#[derive(Debug, Clone, Copy)]
struct Glide {
    from: Vec3,
    to: Vec3,
    t: f32,
    /// Where it put the center last.
    at: Vec3,
}

/// What a follow did this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FollowStep {
    /// The cloud's center this frame.
    pub center: Vec3,
    /// A glide started this frame, this far (m, level) from the head.
    pub started: Option<f32>,
}

/// The cloud following the wearer in roam.
#[derive(Debug, Clone, Copy)]
pub struct Follow {
    glide: Option<Glide>,
    /// Since the last glide started (s).
    since: f32,
}

impl Default for Follow {
    fn default() -> Self {
        Self {
            glide: None,
            since: FOLLOW_EVERY_S,
        }
    }
}

impl Follow {
    /// Once a frame in roam: `head`, the cloud's `center` now, the height
    /// a re-center puts it at (Recenter's) and `dt`. Returns where the
    /// center goes this frame, or `None` to leave it.
    pub fn step(&mut self, head: Vec3, center: Vec3, height: f32, dt: f32) -> Option<FollowStep> {
        let dt = step_s(dt);
        self.since += dt;
        if self
            .glide
            .is_some_and(|g| center.distance(g.at) > GLIDE_SLIP_M)
        {
            self.glide = None;
        }
        let mut started = None;
        if self.glide.is_none() {
            let off = level(head - center).length();
            if off <= ROAM_FOLLOW_M || self.since < FOLLOW_EVERY_S {
                return None;
            }
            self.since = 0.0;
            started = Some(off);
            self.glide = Some(Glide {
                from: center,
                to: Vec3::new(head.x, height, head.z),
                t: 0.0,
                at: center,
            });
        }
        let g = self.glide.as_mut()?;
        g.t += dt;
        g.at = g.from.lerp(g.to, smoothstep(g.t / GLIDE_S));
        let center = g.at;
        if g.t >= GLIDE_S {
            self.glide = None;
        }
        Some(FollowStep { center, started })
    }

    /// Stop a glide in flight (roam went off).
    pub fn cancel(&mut self) {
        self.glide = None;
    }

    pub fn gliding(&self) -> bool {
        self.glide.is_some()
    }
}

/// How far the head may get from the stage floor's center, level, before
/// the slab re-centers under it in roam (m). The slab's half extent is
/// 10 m, so the head always has at least 7 m of floor around it.
pub const FLOOR_FOLLOW_M: f32 = 3.0;

/// The stage floor's center (x, z) this frame in roam: under the head once
/// the head is more than [`FLOOR_FOLLOW_M`] from `center`, else where it
/// is. A jump, not a glide: the floor is the same plane wherever its
/// center is, and the rings spread from the head in the world, so only a
/// pattern drawn in the face's own frame moves, and only at a jump.
pub fn floor_center(center: Vec2, head: Vec3) -> Vec2 {
    let head = level(head);
    if head.distance(center) > FLOOR_FOLLOW_M {
        head
    } else {
        center
    }
}

/// The world effect that fills its volume, switched to in roam when the
/// surface-born one has nothing in reach to spawn from.
pub const FILL_EFFECT: &str = "Flux Cloud";
/// How long a found room's surfaces may all weigh nothing before the show
/// switches (s): its anchors locate one by one after the query.
pub const FILL_AFTER_S: f32 = 3.0;

/// What the fill rule sees this frame.
#[derive(Debug, Clone, Copy)]
pub struct FillInput {
    pub roam: bool,
    /// The room's scan state (`None`: no room in this mode).
    pub scan: Option<ScanState>,
    /// The world effect showing, by index.
    pub showing: usize,
    /// It spawns from surfaces (Embers).
    pub surface_born: bool,
    /// Its summed emitter weight last frame.
    pub weight: f32,
    /// [`FILL_EFFECT`]'s index, when it can show.
    pub fill: Option<usize>,
    pub dt: f32,
}

/// A switch the fill rule asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FillSwitch {
    /// To the fill effect: nothing in reach emits.
    To(usize),
    /// Roam went off: back to the effect it left.
    Back(usize),
}

/// The fill rule across frames.
#[derive(Debug, Clone, Copy, Default)]
pub struct Fill {
    /// Roam was on last frame.
    on: bool,
    /// May still switch in this roam.
    armed: bool,
    /// How long nothing has emitted (s).
    empty_s: f32,
    /// The effect the switch left.
    left: Option<usize>,
    /// The effect showing last frame, or the one a switch asked for.
    seen: Option<usize>,
}

impl Fill {
    /// Once a frame. At most one switch per roam: an effect change the rule
    /// did not make is the wearer's, and from then on it is theirs.
    pub fn step(&mut self, input: FillInput) -> Option<FillSwitch> {
        let wearer = self.seen.is_some_and(|s| s != input.showing);
        self.seen = Some(input.showing);
        if wearer {
            self.armed = false;
            self.left = None;
        }
        if !input.roam {
            let back = self
                .left
                .take()
                .filter(|_| self.on && input.fill == Some(input.showing));
            *self = Self {
                seen: back.or(Some(input.showing)),
                ..Self::default()
            };
            return back.map(FillSwitch::Back);
        }
        if !self.on {
            self.on = true;
            self.armed = true;
            self.empty_s = 0.0;
        }
        if !self.armed {
            return None;
        }
        let nothing = input.surface_born && input.weight <= 0.0;
        // No room found (or none in this mode): no anchors to wait for.
        // A room found: its anchors locate one by one, so a while.
        let wait = match input.scan {
            Some(ScanState::Found(_)) => FILL_AFTER_S,
            Some(ScanState::None | ScanState::NotAllowed) | None => 0.0,
            Some(ScanState::Scanning) => f32::INFINITY,
        };
        self.empty_s = if nothing {
            self.empty_s + step_s(input.dt)
        } else {
            0.0
        };
        match input.fill {
            Some(f) if nothing && self.empty_s >= wait && f != input.showing => {
                self.armed = false;
                self.left = Some(input.showing);
                self.seen = Some(f);
                Some(FillSwitch::To(f))
            }
            _ => None,
        }
    }
}

/// The darkest the passthrough gets in roam: the image's opacity at the
/// full immersion level.
pub const ROAM_FLOOR: f32 = 0.5;

/// The immersion floor (`Immersion::set_floor`): in roam [`ROAM_FLOOR`], or
/// the seated one where that is lighter; seated, the seated one.
pub fn immerse_floor(roam: bool, seated: f32) -> f32 {
    if roam { seated.max(ROAM_FLOOR) } else { seated }
}

/// Head speeds (m/s): the full immersion level below the first, none above
/// the second, a straight line between.
pub const WALK_FULL_M_S: f32 = 0.3;
pub const WALK_NONE_M_S: f32 = 0.8;
/// The head speed's low-pass time constant (s).
pub const WALK_SMOOTH_S: f32 = 0.5;
/// A frame's speed is taken as at most this (m/s), so a tracking jump
/// (the wearer recentering the view) does not lift the room.
pub const WALK_MAX_M_S: f32 = 2.0;

/// The share of the immersion level left at head speed `speed` (m/s).
pub fn walk_factor(speed: f32) -> f32 {
    if !speed.is_finite() {
        return 0.0;
    }
    1.0 - ((speed - WALK_FULL_M_S) / (WALK_NONE_M_S - WALK_FULL_M_S)).clamp(0.0, 1.0)
}

/// The immersion level to use: in roam scaled by the walk's factor,
/// seated as it is.
pub fn immerse_level(level: f32, roam: bool, walk: f32) -> f32 {
    if roam { level * walk } else { level }
}

/// The head's speed, low-passed.
#[derive(Debug, Clone, Copy, Default)]
pub struct Walk {
    last: Option<Vec3>,
    speed: f32,
}

impl Walk {
    /// Once a frame: the head now and `dt` since the last frame. `fake`
    /// replaces the measured speed (`debug.fosfora.roamwalk`). Returns the
    /// low-passed speed (m/s).
    pub fn step(&mut self, head: Vec3, dt: f32, fake: Option<f32>) -> f32 {
        let dt = step_s(dt);
        let measured = match self.last {
            Some(last) if dt > 0.0 => level(head - last).length() / dt,
            _ => 0.0,
        };
        self.last = Some(head);
        let raw = fake.unwrap_or(measured);
        let raw = if raw.is_finite() {
            raw.clamp(0.0, WALK_MAX_M_S)
        } else {
            0.0
        };
        self.speed += (raw - self.speed) * (1.0 - (-dt / WALK_SMOOTH_S).exp());
        self.speed
    }

    /// The low-passed speed (m/s).
    pub fn speed(&self) -> f32 {
        self.speed
    }

    /// [`walk_factor`] at the low-passed speed.
    pub fn factor(&self) -> f32 {
        walk_factor(self.speed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FPS: f32 = 72.0;
    const DT: f32 = 1.0 / FPS;

    #[test]
    fn a_scanned_room_is_seated_and_no_room_roams_by_itself() {
        let mut s = RoamState::new(false, None);
        assert_eq!(s.now(), Roam::Off);
        assert_eq!(s.scan(Some(ScanState::Scanning)), None);
        // A room found: still seated.
        assert_eq!(s.scan(Some(ScanState::Found(17))), None);
        assert_eq!(s.now(), Roam::Off);
        // A launch whose scan ends with nothing.
        let mut s = RoamState::new(false, None);
        s.scan(Some(ScanState::Scanning));
        assert_eq!(s.scan(Some(ScanState::None)), Some(Roam::On(Why::NoRoom)));
        // The same state again changes nothing.
        assert_eq!(s.scan(Some(ScanState::None)), None);
        assert!(!s.chosen(), "the automatic roam is not saved");
        // A rescan that finds a room turns it off.
        assert_eq!(s.scan(Some(ScanState::Scanning)), None);
        assert_eq!(s.scan(Some(ScanState::Found(3))), Some(Roam::Off));
        // No room in this mode: nothing turns it on.
        let mut s = RoamState::new(false, None);
        assert_eq!(s.scan(None), None);
        assert_eq!(s.now(), Roam::Off);
    }

    #[test]
    fn the_wearers_choice_wins_and_is_what_is_saved() {
        let mut s = RoamState::new(true, None);
        assert_eq!(s.now(), Roam::On(Why::Chosen));
        // A room found leaves a chosen roam on.
        assert_eq!(s.scan(Some(ScanState::Found(17))), None);
        assert_eq!(s.now(), Roam::On(Why::Chosen));
        assert_eq!(s.set(false), Some(Roam::Off));
        assert!(!s.chosen());
        // "roam" while it roams by itself makes it the wearer's.
        let mut s = RoamState::new(false, None);
        s.scan(Some(ScanState::None));
        assert_eq!(s.set(true), Some(Roam::On(Why::Chosen)));
        assert!(s.chosen());
        // "stay here" while it roams by itself holds until a scan ends
        // with no room again.
        let mut s = RoamState::new(false, None);
        s.scan(Some(ScanState::None));
        assert_eq!(s.set(false), Some(Roam::Off));
        assert_eq!(s.scan(Some(ScanState::None)), None);
        assert_eq!(s.now(), Roam::Off);
        s.scan(Some(ScanState::Scanning));
        assert_eq!(s.scan(Some(ScanState::None)), Some(Roam::On(Why::NoRoom)));
        // A press that changes nothing reports nothing.
        let mut s = RoamState::new(false, None);
        assert_eq!(s.set(false), None);
    }

    #[test]
    fn the_knob_forces_a_launch_until_a_press() {
        let mut s = RoamState::new(false, Some(true));
        assert_eq!(s.now(), Roam::On(Why::Chosen));
        assert!(!s.chosen(), "the knob is not saved");
        assert_eq!(launch_line(&s), "roam: on (debug.fosfora.roam, not saved)");
        // A room found does not turn the forced roam off.
        assert_eq!(s.scan(Some(ScanState::Found(4))), None);
        let mut s = RoamState::new(true, Some(false));
        assert_eq!(s.now(), Roam::Off);
        assert_eq!(s.scan(Some(ScanState::None)), None);
        assert_eq!(launch_line(&s), "roam: off (debug.fosfora.roam, not saved)");
        // A press takes over.
        assert_eq!(s.set(true), Some(Roam::On(Why::Chosen)));
        assert_eq!(s.forced(), None);
        // Without the knob, the state's own words.
        assert_eq!(launch_line(&RoamState::new(false, None)), "roam: off");
        assert_eq!(
            launch_line(&RoamState::new(true, None)),
            "roam: on (chosen)"
        );
    }

    #[test]
    fn the_menu_and_the_label_say_why() {
        assert_eq!(Roam::Off.label(), "Roam: off");
        assert_eq!(Roam::On(Why::Chosen).label(), "Roam: on");
        assert_eq!(Roam::On(Why::NoRoom).label(), "Roam: on (no room)");
        assert_eq!(Roam::On(Why::NoRoom).describe(), "on (no room)");
        assert_eq!(
            scan_text(ScanState::None, Roam::On(Why::NoRoom)),
            NO_ROOM_LABEL
        );
        // Roam off by the knob or the wearer: the scan's own words.
        assert_eq!(
            scan_text(ScanState::None, Roam::Off),
            crate::label::scan_text(ScanState::None)
        );
        assert_eq!(
            scan_text(ScanState::Found(17), Roam::On(Why::Chosen)),
            "Room: 17 surfaces"
        );
    }

    /// Step `f` at 72 frames a second for `secs` with the head at `head(t)`
    /// and the center moved to wherever the follow puts it.
    fn walk_follow(
        f: &mut Follow,
        center: &mut Vec3,
        secs: f32,
        head: impl Fn(f32) -> Vec3,
    ) -> Vec<(f32, FollowStep)> {
        let mut out = Vec::new();
        for i in 0..(secs * FPS).round() as u32 {
            let t = i as f32 / FPS;
            if let Some(step) = f.step(head(t), *center, 1.0, DT) {
                *center = step.center;
                out.push((t, step));
            }
        }
        out
    }

    #[test]
    fn the_cloud_stays_while_the_head_is_within_a_meter() {
        let mut f = Follow::default();
        let mut center = Vec3::new(0.0, 1.0, 0.0);
        // Standing up 0.6 m over the center and leaning 0.9 m away.
        let steps = walk_follow(&mut f, &mut center, 10.0, |_| Vec3::new(0.9, 1.6, 0.0));
        assert!(steps.is_empty());
        assert_close!(center.to_array(), [0.0, 1.0, 0.0]);
    }

    #[test]
    fn the_cloud_glides_to_the_head_over_a_second_and_a_half() {
        let mut f = Follow::default();
        let mut center = Vec3::new(0.0, 1.2, 0.0);
        let head = Vec3::new(2.0, 1.6, -1.0);
        let steps = walk_follow(&mut f, &mut center, 3.0, |_| head);
        let (t0, first) = steps[0];
        assert_close!(t0, 0.0);
        assert_close!(first.started.unwrap(), 5.0f32.sqrt());
        assert!(steps[1..].iter().all(|(_, s)| s.started.is_none()));
        // No jump: every frame moves a small part of the way.
        let mut last = Vec3::new(0.0, 1.2, 0.0);
        for (_, s) in &steps {
            assert!(s.center.distance(last) < 0.05, "{:?}", s.center);
            last = s.center;
        }
        // Half way at half the glide, there at its end, at Recenter's
        // height, then still.
        let at = |secs: f32| steps[(secs * FPS) as usize - 1].1.center;
        assert!(
            (at(GLIDE_S * 0.5).x - 1.0).abs() < 0.05,
            "{}",
            at(GLIDE_S * 0.5)
        );
        assert_close!(center.to_array(), [head.x, 1.0, head.z]);
        let frames = (GLIDE_S * FPS).round() as usize;
        assert!(steps.len().abs_diff(frames) <= 1, "{}", steps.len());
        assert!(!f.gliding());
    }

    #[test]
    fn a_walk_follows_every_few_seconds_at_most() {
        let mut f = Follow::default();
        let mut center = Vec3::new(0.0, 1.0, 0.0);
        // 1 m/s along -z for 20 s.
        let steps = walk_follow(&mut f, &mut center, 20.0, |t| Vec3::new(0.0, 1.6, -t));
        let starts: Vec<f32> = steps
            .iter()
            .filter(|(_, s)| s.started.is_some())
            .map(|(t, _)| *t)
            .collect();
        assert!(starts.len() >= 5, "{starts:?}");
        for w in starts.windows(2) {
            assert!(w[1] - w[0] >= FOLLOW_EVERY_S - 1e-3, "{starts:?}");
        }
        // The cloud stays within reach of the wearer.
        let gap = (center.z + 20.0).abs();
        assert!(gap < 5.0, "{gap}");
    }

    #[test]
    fn a_drag_or_recenter_during_a_glide_ends_it() {
        let mut f = Follow::default();
        let head = Vec3::new(3.0, 1.6, 0.0);
        let mut center = Vec3::new(0.0, 1.0, 0.0);
        let s = f.step(head, center, 1.0, DT).expect("a glide");
        center = s.center;
        f.step(head, center, 1.0, DT).expect("gliding");
        // Recenter put the center under the head.
        assert!(f.step(head, Vec3::new(3.0, 1.0, 0.0), 1.0, DT).is_none());
        assert!(!f.gliding());
        f.cancel();
        assert!(!f.gliding());
    }

    #[test]
    fn the_floor_recenters_under_the_head_past_three_meters() {
        let c = Vec2::ZERO;
        assert_eq!(floor_center(c, Vec3::new(2.9, 1.6, 0.0)), c);
        assert_eq!(floor_center(c, Vec3::new(0.0, 0.2, -2.0)), c);
        assert_eq!(
            floor_center(c, Vec3::new(2.5, 1.6, -2.0)),
            Vec2::new(2.5, -2.0)
        );
        // A walk of 30 m keeps the head within FLOOR_FOLLOW_M of the
        // center, so at least 7 m of the 10 m half extent is around it.
        let mut c = Vec2::ZERO;
        for i in 0..=3000 {
            let head = Vec3::new(i as f32 * 0.01, 1.6, i as f32 * -0.005);
            c = floor_center(c, head);
            assert!(level(head).distance(c) <= FLOOR_FOLLOW_M + 1e-4);
        }
    }

    fn fill_input(roam: bool, showing: usize, weight: f32) -> FillInput {
        FillInput {
            roam,
            scan: Some(ScanState::None),
            showing,
            // Embers is 1, Flux Cloud 0.
            surface_born: showing == 1,
            weight,
            fill: Some(0),
            dt: DT,
        }
    }

    #[test]
    fn no_room_switches_embers_to_the_volume_and_back() {
        let mut fill = Fill::default();
        // Seated: nothing.
        assert_eq!(fill.step(fill_input(false, 1, 0.0)), None);
        // Roam on with no room: at once.
        assert_eq!(fill.step(fill_input(true, 1, 0.0)), Some(FillSwitch::To(0)));
        // The switch lands; nothing more while roaming.
        for _ in 0..1000 {
            assert_eq!(fill.step(fill_input(true, 0, 0.0)), None);
        }
        // Roam off: back to Embers.
        assert_eq!(
            fill.step(fill_input(false, 0, 0.0)),
            Some(FillSwitch::Back(1))
        );
        assert_eq!(fill.step(fill_input(false, 1, 0.0)), None);
    }

    #[test]
    fn a_surface_that_emits_keeps_embers() {
        let mut fill = Fill::default();
        for _ in 0..1000 {
            assert_eq!(fill.step(fill_input(true, 1, 0.8)), None);
        }
        // Flock, or Flux Cloud already: nothing to switch.
        let mut fill = Fill::default();
        for _ in 0..1000 {
            assert_eq!(fill.step(fill_input(true, 2, 0.0)), None);
            assert_eq!(fill.step(fill_input(true, 2, 0.0)), None);
        }
    }

    #[test]
    fn a_found_room_gets_time_for_its_anchors_and_a_scan_none() {
        let found = |weight: f32| FillInput {
            scan: Some(ScanState::Found(17)),
            ..fill_input(true, 1, weight)
        };
        let mut fill = Fill::default();
        let frames = (FILL_AFTER_S * FPS) as usize;
        for _ in 0..frames - 2 {
            assert_eq!(fill.step(found(0.0)), None);
        }
        // The desk located and emits: the wait starts over.
        assert_eq!(fill.step(found(0.5)), None);
        for _ in 0..frames - 2 {
            assert_eq!(fill.step(found(0.0)), None);
        }
        let switched = (0..10).find_map(|_| fill.step(found(0.0)));
        assert_eq!(switched, Some(FillSwitch::To(0)));
        // While the scan runs, never.
        let mut fill = Fill::default();
        for _ in 0..10_000 {
            let scanning = FillInput {
                scan: Some(ScanState::Scanning),
                ..fill_input(true, 1, 0.0)
            };
            assert_eq!(fill.step(scanning), None);
        }
    }

    #[test]
    fn the_wearers_effect_is_left_alone() {
        // On Flock when roam went on, then Embers by hand: no switch.
        let mut fill = Fill::default();
        assert_eq!(fill.step(fill_input(true, 2, 0.0)), None);
        for _ in 0..1000 {
            assert_eq!(fill.step(fill_input(true, 1, 0.0)), None);
        }
        // Switched to Flux Cloud, then the wearer went to Flock: roam off
        // stays on Flock.
        let mut fill = Fill::default();
        assert_eq!(fill.step(fill_input(true, 1, 0.0)), Some(FillSwitch::To(0)));
        assert_eq!(fill.step(fill_input(true, 0, 0.0)), None);
        assert_eq!(fill.step(fill_input(true, 2, 0.0)), None);
        assert_eq!(fill.step(fill_input(false, 2, 0.0)), None);
        // No fill effect to show: nothing.
        let mut fill = Fill::default();
        let none = FillInput {
            fill: None,
            ..fill_input(true, 1, 0.0)
        };
        assert_eq!(fill.step(none), None);
        // The next roam may switch again.
        let mut fill = Fill::default();
        assert_eq!(fill.step(fill_input(true, 1, 0.0)), Some(FillSwitch::To(0)));
        assert_eq!(
            fill.step(fill_input(false, 0, 0.0)),
            Some(FillSwitch::Back(1))
        );
        assert_eq!(fill.step(fill_input(false, 1, 0.0)), None);
        assert_eq!(fill.step(fill_input(true, 1, 0.0)), Some(FillSwitch::To(0)));
    }

    #[test]
    fn the_walk_factor_is_full_below_0_3_and_none_above_0_8() {
        assert_close!(
            [0.0, 0.2, 0.3, 0.55, 0.8, 1.5].map(walk_factor),
            [1.0, 1.0, 1.0, 0.5, 0.0, 0.0]
        );
        assert_close!(walk_factor(f32::NAN), 0.0);
        // Seated, the level is untouched; in roam it is scaled.
        assert_close!(immerse_level(0.8, false, 0.0), 0.8);
        assert_close!(immerse_level(0.8, true, 0.5), 0.4);
        // The floor: 0.5 in roam, a lighter seated one kept.
        assert_close!(immerse_floor(true, 0.15), ROAM_FLOOR);
        assert_close!(immerse_floor(false, 0.15), 0.15);
        assert_close!(immerse_floor(true, 0.7), 0.7);
    }

    /// Step `w` at 72 frames a second for `secs` with the head at `head(t)`;
    /// returns the factor after each frame.
    fn walk(w: &mut Walk, t0: f32, secs: f32, head: impl Fn(f32) -> Vec3) -> Vec<f32> {
        (0..(secs * FPS).round() as u32)
            .map(|i| {
                w.step(head(t0 + i as f32 / FPS), DT, None);
                w.factor()
            })
            .collect()
    }

    /// The first time (s) `levels` meets `pred`.
    fn first(levels: &[f32], pred: impl Fn(f32) -> bool) -> Option<f32> {
        levels
            .iter()
            .position(|&l| pred(l))
            .map(|i| (i + 1) as f32 / FPS)
    }

    #[test]
    fn walking_brings_the_room_back_and_stopping_dims_it_again() {
        let mut w = Walk::default();
        // Seated, a head that sways a few centimeters: full immersion.
        let sway = walk(&mut w, 0.0, 5.0, |t| {
            Vec3::new(0.03 * (t * 3.0).sin(), 1.2, 0.02 * (t * 2.0).cos())
        });
        assert!(sway.iter().all(|&f| f > 0.99), "{:?}", w.speed());
        // Walking at 1.2 m/s: the room is back within a second.
        let mut w = Walk::default();
        let walking = walk(&mut w, 0.0, 3.0, |t| Vec3::new(0.0, 1.6, -1.2 * t));
        let back = first(&walking, |f| f <= 0.0).expect("back");
        assert!((0.4..1.0).contains(&back), "{back}");
        assert!((w.speed() - 1.2).abs() < 0.01, "{}", w.speed());
        // Stopping: dimmed again within a second and a half.
        let stopped = walk(&mut w, 3.0, 3.0, |_| Vec3::new(0.0, 1.6, -3.6));
        let dim = first(&stopped, |f| f >= 0.99).expect("dim");
        assert!((0.5..1.5).contains(&dim), "{dim}");
        // A slow stroll at 0.5 m/s: part of the way.
        let mut w = Walk::default();
        walk(&mut w, 0.0, 5.0, |t| Vec3::new(0.5 * t, 1.6, 0.0));
        assert!((w.factor() - 0.6).abs() < 0.02, "{}", w.factor());
        // Standing up is not walking: only level travel counts.
        let mut w = Walk::default();
        walk(&mut w, 0.0, 1.0, |t| Vec3::new(0.0, 1.2 + 0.4 * t, 0.0));
        assert_close!(w.factor(), 1.0);
    }

    #[test]
    fn a_tracking_jump_does_not_lift_the_room() {
        // The view recentered: the head jumps 5 m in one frame.
        let mut w = Walk::default();
        walk(&mut w, 0.0, 1.0, |_| Vec3::new(0.0, 1.6, 0.0));
        let after = walk(&mut w, 1.0, 3.0, |_| Vec3::new(5.0, 1.6, 0.0));
        assert!(after.iter().all(|&f| f > 0.99), "{:?}", after.first());
    }

    #[test]
    fn the_fake_speed_drives_the_same_low_pass() {
        let mut w = Walk::default();
        for _ in 0..(3.0 * FPS) as u32 {
            w.step(Vec3::new(0.0, 1.6, 0.0), DT, Some(1.0));
        }
        assert!((w.speed() - 1.0).abs() < 0.01, "{}", w.speed());
        assert_close!(w.factor(), 0.0);
        // Past the cap, the cap.
        for _ in 0..(3.0 * FPS) as u32 {
            w.step(Vec3::new(0.0, 1.6, 0.0), DT, Some(50.0));
        }
        assert!(w.speed() <= WALK_MAX_M_S + 1e-4);
        for _ in 0..(3.0 * FPS) as u32 {
            w.step(Vec3::new(0.0, 1.6, 0.0), DT, Some(0.0));
        }
        assert!(w.factor() > 0.99, "{}", w.factor());
    }
}
