# The room as the visualizer

*Design note, Sep 28, 2026. Kevin's pivot after the Murmur port: the XR
product is not the desktop effects moved into a headset, it is the wearer's
room reacting to the music. This note lists what the measured stack gives us
to build with, four candidate room effects on that stack, what each costs
against the budget, and the questions Kevin decides. It replaces the Tide
brief.*

## What we have measured and working

| Primitive | State on the Quest 3 | Where |
|---|---|---|
| Passthrough with a premultiplied projection layer | ~1.5 ms with a depth-writing draw in the pass | S7 |
| Scene anchors: 5 tables, 3 storage units, floor, ceiling, 4 walls, door and window frames, as oriented boxes | 16 boxes reach the sim every frame; particles land on them and slide off | S7, C3b |
| The global mesh, 87,530 triangles | Returned by Space Setup; not yet drawn or used as an obstacle | S7 |
| Hands: 26 joints per hand as spheres, the skinned hand mesh as a depth occluder | Cloud parts around the hands, joints kick particles | S7, #3263 |
| Gestures: pinch-drag moves the anchor, pinch-hold cycles effects, an opt-in palm panel | 28 of 28 pinches engaged worn | #188 |
| World-space sprites: `prepare_world` / `draw_world` inside the eye pass, additive or alpha over passthrough | 400K at 9.3–10.7 ms; 500K at 8.6 ms for the plain test sim | S5, C3b |
| Audio: playback tap timed to the speaker | Beat flash within 6–12 ms of the sound | #3253 |
| Budget | 13.9 ms per frame at 72 Hz; the room, hands and passthrough take ~3–4 ms of it | #3203 |

Not measured yet, needed by some candidates below: drawing a lit quad per
anchor plane inside the eye pass (a handful of triangles, so cheap, but
the depth interaction with passthrough is untested), and the global mesh as
a drawn surface.

## A constraint on everything: the wearer is seated

The app is stationary: every interaction has to work from a chair, and a
seated arm reaches about 0.7 m. The 40K Murmur flock passed its worn gate
(out of the face, splits around a hand) only after standing and stepping a
meter toward it. So reach is a product feature, not an effect's: **Go-Go
arm extension** (board #3308, PR #189, worn pass). Within a comfortable
reach from the shoulder the virtual hand is the real hand; beyond ~0.30 m
it travels quadratically, so Kevin's full stretch reaches ~2.5 m. The
extended joint set feeds the sim's spheres (both scaring and pushing),
occlusion stays on the real hand, and a ghost hand with a wrist beam
shows where the hand acts. Poses read on the real hand pick the behavior
at the far hand (board #3314: fist scares, open hand parts, palm up or
two closing hands hold a group). Every candidate below assumes both.

## Four candidates

Each is one effect on the existing plumbing (`render_world`, the aux
obstacle block, the hand joints, the timed audio). Costs are estimates from
the S5/S7/C3b numbers, unworn, to be replaced by sweeps.

### 1. Surfaces as canvases

The desk, the walls, the floor and the window frame, as the anchors give
them, become lit surfaces: a world-space quad per anchor plane with an
audio-driven fragment shader. A kick ripples across the real floor from
the wearer's feet; the spectrum climbs the wall in front; a window frame
pulses on the downbeat; the desk shows a caustic that follows the bass.

- Needs: a quad draw per anchor plane in the eye pass (new, small), the
  anchor poses (have), one shader per surface kind, alpha over passthrough
  so the real texture shows through the light.
- Cost: a few triangles per surface; the fragment cost is the fill of the
  visible surfaces, well under 2 ms at eye resolution.
- Risk: passthrough has no depth, so a lit wall behind a real chair draws
  over the chair unless the global mesh or the chair's anchor occludes it.
  The same problem the particles had; the same fix (depth occluders).

### 2. Surfaces as emitters

Particles come *from* the room. Sparks rise off the floor with the bass,
embers pour off the edge of the desk on the beat and settle on the floor,
drops run down a wall and gather on the window sill, dust lifts from every
table top on the snare. The obstacle block is already there, so what
leaves one surface lands on another.

- Needs: emission from anchor faces (a sim variant of the Flux world sim
  where `emit_particle` samples a chosen anchor's top face instead of the
  volume), per-surface emission rates driven by audio bands.
- Cost: the Flux world numbers apply: 300–400K sprites at 7.6–10.7 ms.
- Risk: low. This is the Flux port with the emitter moved; the settle
  drift and the hand channel come for free.

### 3. Hands as instruments

The wearer plays the room. A pinch throws a cloud that flies to where the
hand points and bursts on the wall it hits; an open palm pulls light out
of the nearest surface; a slow drag conducts the flock or drags the
spectrum across the wall. The pinch, drag and hold gestures exist; this
adds emission and force fields anchored to the hands.

- Needs: per-hand emitter and attractor lanes in the aux block (rows are
  free), a hit test against the anchor boxes for the throw (the collide
  code has it), a visible cue on the hand (the palm panel's beam is one).
- Cost: nothing on the GPU beyond the particles already budgeted.
- Risk: feel. Gesture timing against a beat is the whole effect, and it
  is only judged worn.

### 4. The global mesh

The room's scanned mesh (87K triangles) as a surface the whole room lights
up through: a wireframe that pulses with the beat, a scan line that sweeps
the real furniture, or nothing visible until the bass paints it. It also
occludes and collides for furniture the anchors miss.

- Needs: the mesh downloaded from the runtime (the S7 session listed it
  but did not fetch it), a draw of 87K triangles per eye, and the mesh as
  an obstacle (a triangle soup does not fit the box block; a coarse voxel
  or SDF of it would).
- Cost: 87K triangles per eye is cheap to draw; an SDF build is a one-off.
- Risk: the highest of the four. An unfetched extension path, a new
  obstacle representation, and a look that depends on the scan quality.
- Since board #3324 the live depth map (`XR_META_environment_depth`) covers
  the "occluder first" role for what the scan misses (unscanned furniture,
  people, the wearer's body, moved objects). Collision from it is phase 2.
- Phase 2 is in (board #3352): the Flux world sims collide with the live
  depth map too, so embers land on a person or an unscanned chair and
  slide off; see MEASURED.md, "Live depth as a collision source".

## The design (Kevin, Sep 28, board #3315)

In one sentence: you sit in your room with music playing and the room
answers it. Surfaces light up and shed particles, a few things live in
the air, and your hands, long when you stretch, play all of it.

### Three layers, not one effect

The desktop unit is an effect. The room's unit is a **room preset**: a set
of behaviors assigned to the room's parts, switched with the pinch-hold
that today cycles effects. The desktop effect list is not exposed on the
Quest.

1. **Surfaces.** The anchors give five tables, the floor, walls, a door
   and a window. Two behaviors: *canvas* (a lit quad on the surface,
   driven by audio) and *emitter* (particles born on the surface). The
   floor ripples on the kick from under the chair. The desk sheds embers
   on the beat that slide off its edge and pool on the floor, which the
   settle drift already does. The wall the wearer faces carries the
   spectrum climbing it. The window frame pulses on the downbeat.
2. **Inhabitants.** Things in the air: the Flux ember cloud and the 40K
   Murmur flock. They react to surfaces and hands as they do now.
3. **Hands.** With Go-Go and the poses: a pinch-release throws a burst
   that lands where it hits the wall; an open palm held still lifts
   embers off the nearest surface toward it; a drag moves a cloud's home,
   which the anchor drag already is; fist, open hand and palm-up act on
   the flock per #3314.

### Look: three materials, one per layer

Over passthrough, additive glow reads as light falling on real surfaces
and alpha silhouettes read as objects in the room. So: **light on
surfaces, embers in the air, dark birds.** The air keeps Flux's ember
palette with the key-tinted accent, for continuity with desktop. Surfaces
are paler and thinner than the particles, more light than material, so
the real room stays the subject. This is an eye call, taken on the
recommendation and revisited once Kevin wears it.

### Budget shapes the presets

The MR base (passthrough, anchors, hands, occluder) costs 3–4 ms; 300K
sprites 6 ms; the 40K flock 6 ms; surfaces well under 1 ms. A preset can
therefore carry embers or the flock at full size, not both: an *embers*
room and a *flock* room, with the hands in every one.

### The first pass

Surfaces as emitters (the desk and the floor) plus the floor ripple.

- Emitters: shader-only on the Flux world sim. The box block already
  holds every table's center, rotation and half extents in the aux rows,
  so `emit_particle` samples the chosen anchor's top face instead of the
  volume, with per-surface rates driven by audio bands. No app work, no
  new plumbing; it inherits the settle drift, the hand channel and the
  beat timing.
- The floor ripple: one quad on the floor plane inside the eye pass, the
  smallest new draw and the one that makes the room itself light. Its
  origin is **under the chair**: the head position projected onto the
  floor plane and smoothed, so it needs no anchor and stays right when
  the wearer turns.
- Audio: the **playback tap**, timed to the speaker (#3253). The Quest's
  microphones cancel the headset's own output and beamform toward the
  mouth, so room music barely registers (#3247, #3248); the mic is
  ambience only.

### Sequencing

1. Emitters (desk, floor) and the floor ripple: one session each.
2. The wall spectrum and the hand instruments, on Go-Go and the poses.
3. The global mesh last, and as an occluder first: passthrough has no
   depth, so a lit wall draws over a real chair without it.

### Still open

- ~~Which anchor the "wall the wearer faces" is~~: answered in the second
  pass. The pick follows the head with hysteresis (a 0.15 margin held for
  1 s), and held on one wall through a 6 min worn session.
- Whether Murmur's small-flock preset (40K, #3307) needs its own surface
  behaviors or shares the embers room's.

### Follow-ups from the first worn pass (Kevin, Sep 28)

The first pass passed worn ("a good v1"). Asked for next, on the board:

- A "rescan room" action on the hand menu that relaunches Space Setup and
  requeries the anchors (today only a knob does it, and only when no
  anchors are found). Note that the particles collide with the anchors'
  boxes, not the global mesh, which is still unused.
- Reacting to the live environment, not only the scanned room: the
  environment depth extension as a depth occluder first, then as a
  collision source.
- A "space size" control: the emitter cube's half extent as a stepper, a
  conceptual zoom of the volume the particles live in.
- The room preset editor: point at a surface and attach a behavior to it,
  or assign a behavior to a class (all vertical, all horizontal surfaces).


## The room preset editor (board #3326): options

Kevin's target (Sep 28): point at a surface with the Go-Go far hand or the
beam and attach a behavior to it ("effect A on this vertical surface"), or
assign a behavior to a class ("effect B on all horizontal surfaces"), and
keep the assignment per room. This section lays out what exists, three
ways to build it, and a recommendation. Nothing here is implemented.

### What exists (verified in the code, Sep 29)

- **Surfaces.** The room's anchors reach the sim as up to 32 oriented boxes
  in the aux block, with a semantic kind (`box_center.w`: table, floor,
  wall, ceiling, frame, other) and an emitter weight (`box_half.w`, 0..1)
  in the two spare lanes (#3319). The world sims read them; Flux XR Room
  emits from the weighted top faces (#3317). No lane is free for a
  per-surface behavior today.
- **Identity.** `xrRetrieveSpaceQueryResultsFB` returns each anchor's UUID
  next to its `XrSpace`; `Room` keeps the space and drops the UUID. Space
  Setup keeps the same UUIDs across sessions and a rescan returns the
  same handles for unchanged anchors (#3335), so a UUID is a stable key
  for "this desk" and "this wall".
- **Pointing.** `instruments::cast` intersects a ray with the boxes and
  returns the box index, the hit point and the face normal (the throw's
  aim). The reach extension puts a far hand and a beam where the wearer
  points (#3308), and the pinch is the click (#3296).
- **Behaviors that already bind to a surface.** Flux XR Room's surface
  emitters (tables and floor, by kind and weight); the wall spectrum
  (`canvas.rs`, on the wall the wearer faces, picked with hysteresis); the
  floor ripple (the scene floor); the lift and the throw (the hands, any
  surface). Each picks its surface by a fixed rule.
- **Persistence.** The hand menu saves a JSON in the app's config dir
  (`hand_menu.json`); the same place can hold a room file.
- **The panel.** Two-column egui rows on the palm, pointer hit-testing per
  cell, toggles and steppers (#3402); an "Edit room" toggle and a picker
  fit the existing pattern.

### The three options

**A. Assign from the panel, no pointing.** An "Edit room" page on the
panel lists the room's surfaces (kind and size, e.g. "table 1.2 x 0.6")
with a behavior picker per row and one per kind.
- Needs: a per-surface behavior lane, the panel page, the room file.
- Cost: nothing at runtime beyond the lane.
- Risk: the list does not say which table is which; with 5 tables and 4
  walls the wearer guesses. Hands-first holds. Effort: one session.
- Verdict: a fallback, not the ask.

**B. Point and assign (the ask).** "Edit room" on the panel. While on, the
beam from the right far hand casts against the boxes; the hit surface is
highlighted (its face tinted through the occluder like the ripple decal);
a pinch cycles that surface's behavior through the catalogue (or opens a
short picker row on the panel); a panel toggle "apply to all <kind>"
writes the same behavior to every box of the hit's kind. The assignment
is saved per room on change and restored when the room's anchors come
back (by UUID), with the kind defaults for anchors the file does not
know.
- Needs: the behavior lane (32 rows after the pour row, one `vec4` per
  box: behavior id, strength, two parameters), the sim gates by lane
  instead of by kind (a shader change in the XR assets, the kind gate
  kept as the default), the highlight decal (one quad on the face,
  the ripple's pipeline), the UUID kept per anchor, the room file, the
  panel page, and pins for the spectrum ("this wall") and the ripple
  ("this floor") through the same lane.
- Cost: the highlight is one quad; the lane adds 32 aux rows and one
  read per spawn. Under 0.1 ms.
- Risk: the pick on thin walls from an angle (the 4 cm slab; the throw's
  cast handles it), and a rescan renumbering boxes (the file is by UUID,
  the lane is rebuilt from the file every locate, so renumbering is
  safe). Effort: two sessions (lane + sim + file; then the interaction).
- Verdict: the recommendation.

**C. B plus room presets and parameters.** On top of B: named presets per
room (a "dinner" room, a "show" room), cycled by the pinch-hold instead
of the effect list (#3315 wanted the pinch-hold to cycle ROOM PRESETS),
and per-surface parameters (rate, tint, audio band) as steppers when a
surface is selected.
- Needs: everything in B, a preset list in the room file, the pinch-hold
  rebound, a parameter block per surface (the lane's two spare lanes, or
  a second block).
- Cost: as B.
- Risk: scope; the parameter UI on the palm gets deep. Effort: B plus
  one or two sessions.
- Verdict: the second pass, once B is worn and the behavior catalogue has
  settled.

### The behavior catalogue (the lane's values)

0 none · 1 embers off the top face (Flux, today's table behavior) ·
2 sparks up from the face with the bass (Flux, the floor's behavior
until step 2d) · 3 the spectrum canvas (walls) · 4 the ripple (floors;
their default since step 2d) · 5 drips down the
face gathering at its bottom edge (walls, new) · 6 dust lifting off the
face on the beat (any, new) · 7 a pool that embers settle into and glow
(tables, new). The first pass ships 0 to 4, which exist as code, and
proves the plumbing; 5 to 7 come with C. (Amended Oct 1: the surfaces
design took 4 to 7 for its shaders, 4 the rings (the ripple), 5 the
streamlines, 6 the curls, 7 the pulse; see "D2a" at the end and
`SURFACES_DESIGN.md`.)

### The data model

- Aux: `XR_AUX_SURFACE` block of 32 rows after the pour row (`WORLD_AUX_ROWS`
  181 to 213): x behavior id, y strength 0..1, z and w parameters (band,
  rate scale). Built every locate from the room file and the kind defaults.
- Room file: `rooms/<room-id>.json` in the config dir, where the room id
  is the sorted list of anchor UUIDs hashed; entries `{uuid, kind,
  behavior, strength, params}` plus `{kind_defaults}`; written on every
  change, read when the anchors are located.

**Implemented, step 1 (Sep 29).** The lane block is aux 181..213, one
row per box in the obstacle block's order (the room's boxes, then the
stage floor). `x` is the behavior id + 1, so a zero row means unset and
the box runs its kind's default through the old kind gate: an upload
without the block behaves exactly as before. The kind defaults are the
old fixed rule (table embers, floor sparks, wall spectrum, the rest
none), so with no file and no knob the room looks as it did. (Amended
Sep 29, decision #3459, step 2d: the floor's default is now the ripple,
and an unset lane runs its kind's default behavior through the same
gate a set lane does, not the old kind gate; an unset table sheds
embers as before, an unset floor spawns nothing.) A set lane
gates the box's spawns by its behavior (embers on the beat gate, sparks
on the bass gate, the rest closed) times its strength; the spawn stays
on the upward face. The room id is FNV-1a 64 over the anchors' UUIDs
sorted bytewise, 16 hex digits; the file is `rooms/<room id>.json`,
version 1, `{"version": 1, "kind_defaults": {"table": "embers", ...},
"anchors": [{"uuid", "kind", "behavior", "strength", "params"}]}`,
loaded when the room id changes and saved on every change, never on
load; the stage floor's entry is kept under the all-zero UUID. The knob
`debug.fosfora.surface` takes comma-separated
`<target>=<behavior>[@<strength>]`, the target a UUID (or a unique
prefix of at least 8 hex digits), a box index `#<k>` or a kind name
(the class assignment: the kind's default and every anchor of that
kind), the behavior one of `none embers sparks spectrum ripple`, the
strength 0..1 (default 1); `clear` drops every assignment. The wall
spectrum picks among the visible walls whose behavior is the spectrum
(none: no canvas); the ripple goes to the largest floor on the ripple
(the stage floor included), else the largest scene floor, else the
stage floor, and pinning it to a floor turns that floor's sparks off.
(Amended Sep 29, decision #3459, step 2d: the fallback is gone; only a
floor on the ripple carries it, and with none there is no ripple. See
step 2d below.)

### The interaction, hands-first

Edit room on (panel toggle) → the right hand's beam is on and the hit
surface is tinted → pinch: the tint pulses and the surface's behavior
advances one step through the catalogue, the panel's status row names it
("desk: embers") → hold the pinch: apply to all of that kind → Edit room
off: the beam goes, the file is saved. The left hand keeps the panel;
the reach extension keeps far walls reachable from the chair.

### Recommendation and first-pass gate

B, in two steps: (1) the lane, the sim gate, the UUID, the room file and
kind defaults, verified unworn by a knob that assigns a behavior to a
UUID from adb and a GPU test that a lane value moves the emission to the
right box; (2) the beam pick, the highlight and the panel page, gated
worn: point at the desk and turn its embers off and on, put the spectrum
on the side wall, relaunch and find the room as left.

### Open questions for Kevin

1. Cycle behaviors with pinches on the surface, or pick from a panel row?
   (Default taken: pinch cycles; the panel row shows the name.)
2. Is a room identified by its anchor set, or does one file per headset
   suffice for now? (Default: by anchor set, since Space Setup keeps one
   room per space.)
3. Does the pinch-hold keep cycling effects, or switch to room presets in
   C? (Deferred to C.)

**Implemented, step 2 (Sep 29).** Kevin chose the pinch on the surface
(question 1) and the pinch-hold as the class assignment on a surface,
the effect cycle elsewhere (question 3, for the editor). "Edit room"
is a second hand menu row under the pitcher and debug toggles, in both
layouts, beside a status cell; it is off at launch and not saved. While
it is on the right hand's gestures are the editor's (`room_edit.rs`):
the ray is the throw's, from the head through the right far pinch point
(the seated reach applies), cast up to 8 m against the room's boxes and
the stage floor while the room has none; a tap cycles the pointed
surface's behavior one step (none, embers, sparks, spectrum, ripple,
none, from its effective behavior, so an unassigned table goes to
sparks, its strength kept), a hold (0.7 s) moves every surface of its
kind, and the kind default, one step past it at its strength (the class
cycle: first built as "apply this surface's behavior to its kind", which
did nothing visible when the kind already ran it, Kevin, worn). The right
tap no longer throws or toggles the sprite size, the right hold no
longer cycles the effect, the right drag no longer moves the anchor;
the left hand, the lift and the pitcher are unchanged. While the panel
is up its pinches are its own and the editor is frozen: the hit, the
highlight and the status cell hold, the beam goes, nothing fires (first
built with the hit clearing, so the status read "no surface" whenever
the wearer turned the palm up to read it, Kevin, worn); a gesture
without a ray or a hit changes nothing and is logged. The hit has
hysteresis: another box counts after 0.15 s under the ray, the hit
clears after 0.3 s off it, and the gestures act on the hit as shown.
The assignments go through step 1's lanes, file and log unchanged. The
look: a beam (4 mm, alpha 0.35) from the far pinch point to the hit,
or at the held hit's distance through a short miss, else 3 m out; the
hit face (`Face::across`, the box face across the hit normal) lifted
1 cm and tinted in the ripple's warm white, a fill at alpha 0.10 under
a 2.5 cm border at 0.30 (the border is what reads with one eye), drawn
after the ripple and the canvas, behind hands and furniture, under the
embers; a cycle pulses it once and a class assignment twice (+0.25,
0.3 s each). The status cell names the surface in words, with the box
index when two share a name ("table 3: embers", "wall 12: spectrum"),
"no surface" or "edit room off". Knobs: `debug.fosfora.editroom 0|1`
(the mode at launch) and `debug.fosfora.picktest <s>` (the ray from
0.5 m ahead of the head along the view tilted 20 degrees down,
untracked, a synthetic tap every `<s>` seconds; implies the mode).
Logs: "edit room: pointing at wall 12 (WALL_FACE 8f7ada5c) at (x, y,
z)", "edit room: no surface", "edit room: table 0 (TABLE dc83ba94) ->
sparks" followed by the lanes' line, "edit room: every wall like ..."
for a class, and the mode's changes. Worn gate (Kevin): point at the
desk and turn its embers off and on, put the spectrum on the side wall,
relaunch and find the room as left. Step 2b (Sep 29), after two worn
passes: every tap and hold landed in the log, but most steps of the full
catalogue looked the same on a given surface (a wall on embers, sparks,
ripple or none shows nothing, a table on spectrum, ripple or none is
dark) and the only naming of the result was the palm panel, which the
wearer is not looking at while pointing, so "it's not very obvious that
anything is happening on either tap or hold" (Kevin). Kevin chose to
narrow the cycle now and to make every behavior render on every surface
later (C). A tap and a hold now step through the kind's own catalogue
(`SurfaceBehavior::catalogue`, `next_for`), what renders on it, `none`
first: table, other and unlabeled anchors none, embers, sparks; floor
none, sparks, ripple; wall none, spectrum; ceiling and frame none only.
(These lists grew with the surface shaders in D1 and were redone in D2a
with each kind's default first after none; see "D2a" at the end.)
A behavior off it (a knob put the spectrum on a table) goes to the first
entry after `none`; the knob still takes any behavior on any kind. After
each action a label floats at the hit for 1.5 s, fading over its last
0.4 s, naming the result: "desk: sparks", "all tables: none", "ceiling:
nothing to change". It is white text on a dark rounded ground at alpha
0.85 (the fade scales both; text on a ground, so it reads with one eye),
egui into a 512 x 96 texture of its own, shown 0.28 m wide as a
billboard 8 cm out along the face normal and 6 cm up from the hit,
facing the head with the head's right, drawn with the panel's pipeline
right after the panel (`label.rs`). And a quick, short right drag counts
as a tap: of Kevin's right pinches, 16 in one pass and 2 in the next
moved past the 2.5 cm drag radius before opening and did nothing, so a
drag in Edit room that ends under 0.5 s from its start and under 10 cm
from where it started is the editor's tap, logged "gesture: short drag
right counts as a tap · edit room". Step 2c (Sep 29), after the third worn pass:
the labels changed but "further away is just a blob of pixels", and "the
actual effects are NOT cycling ... Half the time I don't even know
what's happening because the giant particle cloud is everywhere"
(Kevin). The log showed the edits landing; they were lost in the mass.
In that pass the summed emitter weight was 1.85 with the floor on ripple,
every table shedding into 160,000 particles a second, so a table going
from embers to sparks to none moved where a share of them were born
while 400K living sprites drifted on for their 12 s lifetime. Unworn
earlier, with the headset on the desk, the sum was 0.99 with the floor
at 0.5: the largest table sat outside the 1.5 m volume, and the tables
in reach shared 0.49 as slivers of it. Three changes. The label's width
now follows its distance from the head, 0.11 m per meter clamped to
0.28..1.4 m, so it holds about 6 degrees across from the chair to the
far wall (0.28 m up to 2.5 m away, 0.33 m at 3 m, 0.88 m at 8 m), from a
1024 x 192 texture with the text starting at 88 px and shrinking to fit
down to 40 px, so it is sharp up close too; the fade and the billboard
are unchanged. The table weights are taken against the largest emitting
table inside the volume, not the room's largest (`emitter_weights`), so
the nearest big table weighs 1.0 and a room whose only emitting table is
a side table gives it 1.0; other kinds follow the same reference, the
floor keeps its own weight. And a third hand menu row, under Edit room
in both layouts, "Cloud: on/off" (on at launch, not saved; knob
`debug.fosfora.cloud 0|1`). Off, the world effect's emission goes to 0
through the density path, the density kept so on restores it. Off with
Edit room on, the pointed surface is soloed: the emission stays at the
density, but the lane rows uploaded to the sim carry strength 0 for every
box but the editor's hit (`room_edit::solo`, on a copy each frame, never
on the lanes' rows or the file), so only that surface spawns, and with
no hit nothing does; the wearer points at the desk and sees its embers,
its sparks or nothing, alone, and the label names it. The wall spectrum,
the ripple, the pitcher and the throw are unchanged. The particle system
has no way to clear the living cloud without a core change, so turning
the cloud off, or moving the solo, leaves what is alive to die over its
lifetime (12 s for Flux XR Room). Logs: "cloud off: emission 0, the
cloud fades over the lifetime", "cloud off, edit room: solo desk",
"cloud off, edit room: solo, no surface (nothing spawns)", "cloud on",
and the density line with "(cloud off)".

**Implemented, step 2d (Sep 29, decision #3459).** Worn: "floor ripple
kept rippling when I switch to embers or none" (Kevin). The ripple fell back to the largest floor whenever no floor
was on it, and the floor's default was sparks, so a floor could not be
left dark. Kevin chose: a floor's default is the ripple
(`SurfaceBehavior::default_for`, `xr_kind_behavior`), and the ripple
draws only on a floor whose behavior is the ripple, the largest scene
floor on it, else the stage floor on it while the room has no scene
floor (then the stage floor stands in, as its emitter flag and the
editor's ray already take it; beside a scene floor it is out of the
editor's reach, and its default would put the ripple back under a scene
floor turned to none). With neither there is no ripple, logged "floor
ripple: no floor on ripple". A floor sparks only when assigned, and
none on a floor means nothing on it. In the sim an unset lane now runs
its kind's default behavior through the behavior gate at full strength
(`xr_box_gate`), the kind gate is gone: an unset table sheds embers as
before, an unset floor, wall or other box spawns nothing (the old gate
gave walls and other kinds a fixed 0.3, which the app never weighted).
The floor's cycle is unchanged, none, sparks, ripple; its default is
the last, so a tap from it goes to none, then sparks, then back to the
ripple. The room file needs no migration, with one catch: every save
writes all six kind defaults, so a room saved before 2d names "floor":
"sparks" and keeps it, as a wearer's own assignment would; a room with
no file takes the ripple. A hold on a floor (the class cycle from
sparks) or the knob `floor=ripple` puts such a room on the new default.
And the solo now runs at the full rate: with Cloud off in Edit room and
a hit that spawns (embers or sparks), every other box's row goes to the
sim as none at strength 0, not just strength 0, so the weights see one
emitting box and every draw lands on it; in 2c the others kept their
behaviors and so their share of the weight, and the pointed surface
spawned about 1.0 / 1.85 of the rate in Kevin's room. With no hit, or a
hit that spawns nothing (a floor on the ripple, a wall), the 2c rule
stays (behaviors kept, strengths 0): turning every row to none there
would leave no weight anywhere, and the sim would fall back to spawning
through the whole volume.
 An "All: none" button beside the Cloud toggle
(both layouts) puts every kind default and every anchor, the stage floor
too, on none in one save: the quiet room from which one assignment can be
judged (Kevin's fourth pass, Sep 29).

**Implemented, step 2e (Sep 29).** Worn, fifth pass: "The whole entire
scene, clouds, glow, flock, whatever persist no matter what I'm trying"
(Kevin). Two causes. "All: none" filled the room: the sim fell back to
the volume emitter whenever no box carried weight (step 1's rule for a
room with no anchors), and the weights follow the behaviors, so a room
whose every surface was explicitly none spawned the full cloud from
nowhere. And Cloud off only took the emission to 0: the sprites alive
drew on until they died (12 s for Flux XR Room, 15 s for Murmur's
birds), and the other world effects (Flux XR World, Coarse, Murmur)
ignore the lanes anyway. Now Cloud off hides the world effect's draw at
once, whatever the effect (`Gfx::set_world_visible`, from
`Cloud::visible`): the eye pass skips it for both eyes while its sim
steps on, so on is instant and the room is as it would have been. The
depth occluders, the ripple, the wall spectrum, the highlight, the
label, the beams and the panel still draw; the pitcher's pour and the
throw's bursts are the world effect's particles and hide with it. The
emission-0 path is gone, and the density path takes the density alone.
Solo is unchanged, shown, and what was alive before a solo switch still
fades over its lifetime (an instant kill needs a core method, a separate
decision). In the sim, the volume fallback runs only when the
surface-emit preset has no boxes at all: with boxes and no weight
anywhere nothing spawns, so a room all on none is silent, and a run with
no room at all keeps the volume cloud. The XR app always pushes the
stage floor as a box in mr and world modes, and its default is the
ripple, so a Flux XR Room run with no anchors now spawns nothing unless
the stage floor's lane emits: the unworn cost sweeps set
`debug.fosfora.surface "#0=sparks"`. Logs: "cloud off: the world effect
is hidden", "cloud on".

**Implemented, step 2f (Sep 30).** Kevin's sixth worn pass: "cloud: off,
all: none, and still there are particles flying all around me." The log
showed two things. The world effect was Flux XR World, the default in
`mode world`, a volume emitter that ignores the lanes; the room editor
steers only Flux XR Room. And Cloud off with Edit room on was the solo,
which showed the effect. So Cloud off now hides the effect whatever Edit
room is doing; the solo is not reachable from the toggle and returns as
an explicit mode once the living particles can be cleared at once
(board #3464).

**Names (Sep 30).** The world effects carry the names the wearer sees:
**Embers** (was Flux XR Room, the surface-born one the editor steers,
now the default in `mode world`), **Flux Cloud** (was Flux XR World, the
volume emitter) and **Flock** (was Murmur XR World). Flux Cloud Coarse
(was Flux XR World Coarse, a sprite-size diagnostic) left the pinch-hold
cycle (its file no longer matches `*_xr_world*`) and stays reachable by
`debug.fosfora.effect`. MEASURED.md keeps the old names in its history.

**Implemented, step 2g (Sep 30).** Kevin's seventh worn pass, on Embers:
"Cloud: off to get rid of the default effect, room edit, only floor:
ripple and wall: spectrum." The log showed the surfaces emitting (weight
2.19, 120K particles alive from the tables he set) and all of it hidden
by the toggle: the embers painted on a surface are the effect's own
particles. So while Edit room is on the effect always shows, whatever
the Cloud toggle says (its button now reads "Particles: on/off", and "Particles: off (shown:
editing)" in that case: the word cloud read as the ambient mass alone),
and on Embers the way to a quiet room is All: none, which since 2e
spawns nothing.

**Implemented, step 2h (Sep 30).** Kevin's eighth worn pass, two
reports with two causes. "It's not obvious that the top and sides of
table 14 are connected: I can highlight each surface separately, but they
don't get effects separately": the behavior is one per box, but the
highlight tinted whichever face the ray entered. Now it tints the face
the behavior acts on, whichever face the ray entered
(`surfaces::acting_face`): a volume's top face (a table, storage, the
stage floor's slab; the face the sim spawns on) and a plane's face toward
the head (a wall's room side, a floor's top, the ceiling's underside; a
plane is a box with a half extent at `PLANE_HALF_THICKNESS_M`, 2 cm). The
pick, its hysteresis and the label's placement at the hit are unchanged,
and the log's "pointing at" line says "(top)" or "(face)". "Table 13
worked for a pinch-hold but not a single pinch": the tap assigned it, but
its top face lay outside Embers' volume, the cube of the preset's 1.5 m
half extent around the anchor recentered on the wearer, so its emitter
weight was 0 and a particle born there would respawn at once; the hold
copied the behavior to every table, and table 14, inside the cube, lit.
Now the surface-born effect's space fits the room (`space::fit_room`):
the smallest cube around the anchor holding every room box's corners
(the stage floor excluded, it is 20 m across) plus a 0.25 m margin,
rounded up to the stepper's 0.25 m and clamped to its 0.5 to 6 m,
applied whenever the boxes or the anchor change and no size was asked.
The knob or the stepper still wins; the stepper shows the fit and steps
from it. Flux Cloud and Flock keep their presets' sizes (the flock was
tuned at 1.5 m). Log: "space fit to the room: 3.75 m half extent (17
boxes)". And the label says when a tap or a hold leaves a surface on
embers or sparks outside the volume, "table 13: sparks (outside the
space)", by the reach test the weights use: rare with the fit, it stays
for an asked size too small for the room.

**Amended, D2a (Oct 1, board #3488).** The surfaces design
(`SURFACES_DESIGN.md`, "D2a as built") changed what this editor cycles
through and what a room runs with no file. The kind defaults are the
mockup's room: tables the streamlines, floors the rings, walls the
spectrum, frames the pulse, any other labeled surface the curls,
ceilings and unlabeled anchors none; nothing emits by default, and
embers or sparks on a surface brings the cloud back there (decision
#3474). Each kind's cycle puts its default first after none: table none,
streamlines, curls, pulse, embers, sparks; floor none, rings,
streamlines, curls, sparks; wall none, spectrum, streamlines, rings,
pulse; ceiling none, rings, pulse, streamlines; frame none, pulse,
rings, streamlines; other and unlabeled none, curls, streamlines, pulse,
embers. The room file is version 2: a version 1 file keeps its anchor
entries and its kind defaults give way to the built-ins (every save
before wrote all six, table embers among them), and an entry's `params`
are the lane's color index and audio band, written only once set. The
knob takes `<target>=<behavior>[:<color>[:<band>]][@<strength>]`. The
sim's own default for an unset lane (`xr_kind_behavior`, a table on
embers) is unchanged; the app writes every box's row, so only an upload
without the lanes sees it.

**Amended, D2b (Oct 3, board #3489).** Eight desktop effects join the
catalogue as ids 8 to 15 (`SURFACES_DESIGN.md`, "D2b as built"), and
each kind's cycle gains the ones that suit it after its own entries, so
the defaults and the first taps above are unchanged: a table adds
shards and prism; a floor tessera, prism and astrolabe; a wall aurora,
fenestra and bezel; a ceiling aurora and astrolabe; a frame bezel,
reticle and fenestra; other and unlabeled shards, reticle and tessera.
The knob and the room file take the eight names on any kind.

**Amended, D3 (Oct 8, board #3472).** The hand menu's rows, top to
bottom: Pitcher and Debug panel; Edit room and its status; with Edit
room on, three rows for the surface under the beam (`SURFACES_DESIGN.md`,
"D3 as built"): Color, Band and Strength, each a `<` and a `>` around
the name and the value ("Color  amber", "Band  mid", "Strength  0.7");
Particles and All: none; Music. The menu grows from four rows to seven
upward when Edit room turns on, its bottom rows staying under the
pointer, and the debug panel's block from 11 rows to 14. The rows act
on the editor's hit, which holds while the panel is up, so pointing at
the desk and turning the palm up is how the desk is chosen; with no
surface under the beam they say "Point at a surface" and press nothing.
Each step is saved to the room file through the lanes, as the knob's
`:<color>:<band>@<strength>` is, and the label at the hit names the
result ("desk: curls · amber · mid · 0.7"). (Amended Oct 20, board
#4192: over Pitcher and Debug the menu now has the world effect's `<`
`>` row (board #3336), Recenter and Rescan (board #4192) and the Agent
row (board #3776), so it grows from seven rows to ten, 324 to 444
points, and the debug panel's block from 12 to 15.)

**Amended, the arm (Oct 8, board #3336).** The editor's hold is now hold
to arm, hold again within 2.5 s to apply to the kind: a first right hold on
a surface changes nothing, pulses the highlight twice and labels the hit
with what a second would do ("desk: hold again for all tables ->
streamlines"), and the status cell reads "desk: curls · armed"; a second
hold on the same surface within 2.5 s runs the class cycle as before (every
surface of its kind one step past it, "all tables: streamlines"). The time
running out, a tap (which cycles the surface as before), a gesture with no
surface under the beam, the beam settling on another surface and Edit room
off each disarm without firing; a hold on another surface arms that one.
The panel up holds the surface, and the arm's clock runs on, so the status
cell shows it armed. The reason is the gesture audit (`XR_DESIGN.md`,
"Hands: the gesture vocabulary"): the class cycle rewrites a whole kind from
a gesture a hand can make by accident. In the same change the hand menu
gained the world effect's `<` `>` row on top, since the bare pinch-hold no
longer cycles the effect, so the menu is five rows, eight with Edit room
on. (Amended Oct 20, board #4192: seven rows, ten with Edit room on, since
the Agent row (board #3776) and Recenter and Rescan (board #4192) went in
under the effect's; the hand menu's Rescan is the one the scan label's "No
room found. Rescan from the hand menu" points to.)

**Amended, the voice path's grammar (Oct 8, board #3751).** The editor's
assignments can be spoken (`VOICE_DESIGN.md`, "V2 as built"): hold the
left fist, say "the desk in amber", "every table streamlines", "wall 5 on
the bass", "nothing on the ceiling", "the desk dimmer" or "what is this",
and release. A surface is named by the status cell's words ("desk",
"table 14", "window"), by its kind for all of it ("every table", "the
walls"), or not at all, in which case the surface under the beam is meant,
as the rows' is. The writes go through the same lanes, room file and log
as the tap, the hold and the rows (a kind's behavior through
`RoomLanes::set_kind`, a set where the hold steps), and the voice label
names the result in the editor's words ("desk: amber", "all tables:
streamlines"). Nothing the voice does is out of the hands' reach: a
behavior a kind does not cycle through is refused ("embers doesn't run on
a wall").

## Surface labels (board #5378)

Voice commands name surfaces ("wall 6", "table 3", "desk"), but nothing in
the room showed those names. In Kevin's worn pass on Nov 12, "make the wall
blue" came back `Ambiguous(["wall 1", "wall 6", "wall 8", "wall 14"])` and
"make wall 16 blue" came back "No surface called wall 16". The wearer had no
way to see which wall was which. Kevin: "what I need is some way to see the
registered surfaces, like labels on the walls." The code is
`surface_labels.rs`.

**What a label says.** Two lines on the editor's label's dark rounded
ground. The first is the surface's name exactly as the grammar takes it, the
`name` the voice vocabulary holds (`intent::surfaces`). The second is what
the surface runs, in the status cell's word: "aurora", "embers", "none".
The status cell and the labels read that word from one function
(`label::behavior_word`).

**Which surfaces.** Every lane box gets a label: the planes and volumes the
scene query returned, then the stage floor. Next to a scene floor the stage
floor is "floor N", as the grammar names it. The stage floor is a 10 cm
slab, a volume, so its label stands just off the slab at the stage's origin
as the head sees it. A hidden wall
(`INVISIBLE_WALL_FACE`) is labeled like any other, because the grammar takes
its name. There are 32 cells, one per lane box, the same 32 boxes the lanes
have rows for.

**Where** (board #5379). Every label faces the head and stays upright. Its
right edge is level, so its text never tilts, even when the head rolls. It
sits at its surface's center as the head sees it: on the line from the center
of the plane's rect or the volume's box to the head, just in front of the
face that line leaves by. Its nearest corner stands 4 cm off that face
(`CLEAR_M`), so it never z-fights with a wall and never sinks into a table.
That matters because the room's boxes and the depth map hide whatever is
behind them. Square on, the label is about 4 cm out. On a wall seen at a
slant it has to come nearer the head to clear the wall, and it shrinks by as
much, so its size on the screen stays the same. It comes at most halfway to
the head, so a wall seen nearly edge-on hides part of its label. Looking
almost straight down at a label, the head's right takes over from the level
right, so the label reads the right way round and does not spin as the head
passes over it. Each label is `label::label_w_m` wide for its face's distance
from the head, as the editor's label is, and 0.375 of that high (the cell's
aspect). A label whose center is behind the head, or farther than 6 m from
it, does not draw. The first build laid a plane's label flat on the plane,
upright on a wall and turned on a floor. After wearing it Kevin asked for
every label to face the wearer and sit at its surface's center (Nov 12).

**Crowding** (board #5379). Each frame the labels in view are projected onto
the head's level view, a screen with no roll. Two labels whose rects overlap
move apart along its vertical, half each, until they are their heights and
15% apart (`SPREAD_GAP`). The one whose surface is higher on the screen goes
up. Near a tie they keep last frame's order until the other surface is higher
by half the gap they need (`SPREAD_FLIP`), so two labels do not swap back and
forth as the head moves. Two labels side by side push only as far as they
overlap across, reaching the full push at a quarter of the narrower one's
width (`SPREAD_RAMP`), so the push grows from nothing as they slide together.
The relaxation makes at most 32 passes over the pairs and stops once nothing
moves (`SPREAD_PASSES`). 32 labels packed into one band of the view need
about 30. A label that moved more than half its height off its surface's
center gets a leader: a thin line from the middle of its edge nearer that
point (the bottom when it went up) to the point, in the labels' ink at their
alpha, 5% of the label's height wide. A head that only rolls moves nothing.
The spread is rects in and offsets out (`spread`), with desktop tests.

**When.** While Edit room is on. While a voice window is open, listening or
transcribing, and for 4 s after it closes. For 4 s after a miss on a name,
`UnknownSurface` or `Ambiguous` (`Reason::names_surfaces`), whether the
grammar, the agent's actions or the on-device provider missed. Always while
`debug.fosfora.labels` is `1` (read at launch). They fade in over 0.4 s
(`label::FADE_S`). When the last reason goes they fade out over 0.4 s, and a
4 s hold fades over its last 0.4 s, as a label's life ends in
`label::fade_for`. `SurfaceLabels` decides when they show, in plain numbers
with desktop tests.

**Rendering.** One 2048 x 1536 atlas, a 4 x 8 grid of 512 x 192 px cells.
egui paints every label into its cell in one pass, through the
`TextureCanvas` the editor's label now uses too. The atlas is redrawn only
when a name, a behavior or the fade's stage changes. The fade has 8 stages,
so a fade in or out redraws it 8 times. Each label is a posed quad of the
panel's pipeline that reads its own cell. For that the panel's uniform gained
a texture rect (64 bytes, was 48), which the hand menu and the editor's label
set to the whole width and their top part. The labels draw farthest first,
before the hand menu and the editor's label. Turning the head changes which
quads draw and never redraws the atlas. The leaders are strips of the beams'
pipeline, a second one with a slot per label, drawn just before the labels.
With no label showing, the frame checks one counter and draws nothing, and the
atlas is not touched.

**Describe.** Asking for a kind now lists its surfaces by the same names,
each with its behavior ("walls: wall 1 aurora · wall 6 aurora" over "wall 8
none · wall 14 none"), so the voice label and the labels in the room agree
(`VOICE_DESIGN.md`, "What the wearer sees").

**Log.** "surface labels: shown (edit room) · 17 surfaces, 9 in view, 2
moved apart" when they appear, the reason being "edit room", "voice window",
"a voice window closed", "a miss on a name" or "debug.fosfora.labels". The
last count is the labels with a leader. "surface labels: hidden" when they
go. A launch with the knob on says "surface labels: always shown
(debug.fosfora.labels 1)".

**Unworn check.** `debug.fosfora.say "make wall 16 blue"` on a room without a
wall 16 misses on the name, so the labels show for 4 s, long enough for a
screencap.

## Roam (board #5393)

The app works two ways: seated in a room the headset has scanned, or roaming
through places it has not. Before roam the second way was broken at launch.
With no room the default effect, Embers, had nothing to spawn from, Space
Setup was pushed at the wearer on every launch that found no anchors, the
cloud stayed where the wearer stood at launch, and the stage floor was a
20 m slab around the stage origin. The rules are in `roam.rs`, with desktop
tests.

**When it is on.** `Roam { Off, On }`, with why: `Chosen` by the wearer or
`NoRoom`. The wearer's choice is saved in `hand_menu.json` (`"roam": true`).
In a scanned room the default is off. When the scene query ends with no
anchors after its retries, roam goes on by itself, and the scan label reads
"No room here: roaming. Rescan from the hand menu" for its usual 6 s. That
one is not saved. A later scan that finds a room turns it off again. Only a
change of the scan's state acts, so a wearer who turns roam off in a place
with no room keeps it off until the next scan ends with none. With the room
off (`debug.fosfora.room 0`) there is no scan, and roam is on only when
chosen or forced.

**Space Setup, once.** The app asks for Space Setup once, ever. The flag
`"space_setup_asked": true` in `hand_menu.json` is saved the first time the
app launches Space Setup or its first scene query ends, whatever the
outcome. After that Space Setup opens only from the hand menu's Rescan and
the spoken rescan. A headset with a `rooms/*.json` from before the flag
counts as asked (`room_file::any_saved`). `debug.fosfora.scenecapture 0|1`
still overrides for tests, and a run with it set leaves the flag alone.

**The cloud follows.** In roam the cloud's center glides to the head when
the head is more than `ROAM_FOLLOW_M` (1.0 m) from it, measured level. The
glide takes `GLIDE_S` (1.5 s), eased in and out, and a new one starts at
most every `FOLLOW_EVERY_S` (3 s). It goes where Recenter puts the center,
at Recenter's height, by Recenter's path (the anchor moves, the test sim's
cube and the world effect's anchor with it). The sim keeps its particles
relative to the anchor, so they ride along: none jumps, none dies, and the
bounds move with them. That reads as the cloud drifting after the wearer.
Keeping the particles still in the world while the bounds move would take a
change to the sim. Particles resting on the floor slide with the cloud. A
drag or a Recenter during a glide ends it. With `debug.fosfora.cube` set
the cloud stays where the knob puts it.

**The floor follows.** The stage floor's slab is 20 m across. In roam it
re-centers under the head whenever the head is more than `FLOOR_FOLLOW_M`
(3 m) from its center, so the head always has at least 7 m of floor around
it. The slab jumps, it does not glide. The floor is the same plane wherever
its center is, so collisions and the depth occluder do not change, and the
rings spread from under the head in the world. Only a pattern drawn in the
face's own frame (streamlines, curls, a ported effect assigned to the stage
floor) moves, and only at a jump. In roam its height follows the live
planes' floor once the finder has one ("Live planes", "Wiring"), and stays
y = 0 until then. Seated, the slab is at the stage's origin at y = 0, as
before.

**Something to see.** In roam, when the effect showing is the surface-born
one (Embers) and nothing in reach carries emitter weight, the show switches
to Flux Cloud, the effect that fills its volume. With no room found it
switches at once. In a found room it waits `FILL_AFTER_S` (3 s) of zero
weight, since anchors locate one by one, and a surface that emits keeps
Embers. It switches once per roam. An effect change the rule did not make
is the wearer's, and from then on the effect is theirs. Turning roam off
switches back to the effect it left, unless the wearer picked another
meanwhile. This was the smallest honest way: the effect row names what
shows, nothing is written to the room file, and a scanned room with roam
off looks as before. Giving the stage floor an emitting default in roam
would have spawned sparks under a floor whose label says rings.

**Immersion gives way to walking.** In roam the passthrough floor is
`ROAM_FLOOR` (0.5), through `Immersion`'s floor, or the seated floor where
that is lighter. The immersion level is scaled by head speed: all of it
below `WALK_FULL_M_S` (0.3 m/s), none above `WALK_NONE_M_S` (0.8 m/s), a
straight line between. The speed is the head's level travel per frame,
taken as at most 2 m/s and low-passed with a 0.5 s time constant
(`WALK_SMOOTH_S`). Walking at 1.2 m/s the room is back within about 0.6 s.
Stopping, it dims again within about 0.7 s. Standing up is not walking, and
a tracking jump does not lift the room. Seated, immersion is unchanged.

**The control and the words.** The hand menu's Recenter row holds it, in
both layouts: Recenter and Rescan share the left cell, a half each, and the
right cell reads "Roam: off", "Roam: on" or "Roam: on (no room)". A press
turns it off from on and on from off, and saves the choice. The debug
panel's header leaves no room for another row, so no row was added. By
voice, "roam", "roam mode", "follow me", "roam on" and "start roaming" turn
it on; "stay here", "seated", "stop roaming", "roam off" and "seated mode"
turn it off. These are grammar phrases only. The on-device model and the
agents' actions do not know them.

**Live planes.** Surface boxes found in the live depth map while roaming
(board #5391, wired by #5397) are chained after the room's boxes once a
frame in `app.rs`, into the one list every reader of the room's boxes uses.
The space fit, the room editor, the voice path's vocabulary and the surface
labels pass them by. "Live planes" below has the rules.

**Knobs.** `debug.fosfora.roam 0|1` forces roam for a launch, not saved,
for unworn tests where the head never moves; a press or a spoken roam takes
over. `debug.fosfora.roamwalk <m/s>` fakes the head speed through the same
low-pass, polled once a second, so the immersion rule can be captured
unworn.

**Log.** At launch "roam: off", "roam: on (chosen)" or "roam: on
(debug.fosfora.roam, not saved)", and the Space Setup line ("scene: Space
Setup not asked yet: an empty scan opens it, once"). On each change "roam:
on (no room) · the scan found no room · passthrough floor 0.50". Per follow
"roam: the cloud follows the wearer, 1.24 m from the head, gliding over
1.5 s". Per floor jump "roam: the stage floor at (4.10, -2.30), under the
head". The fill "roam: nothing in reach emits on 'Embers': 'Flux Cloud'
fills the space" and "roam off: back to 'Embers'". Once a second in roam
while walking counts, "roam: head speed 0.62 m/s · immersion x0.36 (level
0.30 of 0.84) · passthrough floor 0.50".

## Live planes (board #5391)

The effects draw on the room's surfaces, and today those come only from the
room scan. A wearer who walks somewhere the scan never covered has nothing to
draw on. The live depth map works anywhere but sees only the current view and
keeps nothing. `live_planes.rs` turns it into a small, stable set of planes in
world space. It is plain numbers with desktop tests. Since board #5397 the
headset's depth map feeds it ("Wiring", below).

**What it takes.** One observation per update: the depth camera's pose and
fov, and a row-major grid of world points, row 0 at the top, `None` where the
map has no depth. `unproject_grid` builds that grid from stored depth values
the way the 40x40 read-back samples them, with `texel_dir` and
`decode_distance`, so it reads the texture as the occluder does. The grid size
is a parameter. Points nearer than 0.3 m (hands, the wearer's body) or farther
than 4.5 m are ignored.

**The fit.** Region growing over the grid, not RANSAC. The grid already says
which samples touch, so a hand in front of a wall does not join it and two
coplanar surfaces with a gap between them stay apart. It is one pass with no
random sampling and gives the same answer for the same input. The noise model
is one deviation along the ray of 1% of the range at 1 m and 4% at 4 m, and
every distance threshold scales with it. Each cell gets a normal from a window
as wide as that noise needs: 3x3 to 1.6 m, 5x5 to 3.8 m, 7x7 beyond on the
40x40 grid. A cell beside a crease or a silhouette takes the window on its own
surface. A region takes a neighbor within 30 degrees of its normal and 2.5
noise deviations of its plane. A patch needs 10 cells, 0.05 m² and about 3
cells of width. Cells that hold a blend of two depths at a silhouette are left
out first. The plane fit takes the model's noise out of the scatter, because
noise along slanted rays tilts a small far plane: a 0.5 m panel at 3.3 m seen
at 45 degrees leans 12.4 degrees in a plain fit and 0.8 in this one.

**Tracking.** A patch joins a tracked plane when the normals agree within 10
degrees, the plane passes within 8 cm of the patch and the rectangles overlap
or come within 5 cm. The plane is the weighted fit of every point it has taken,
near points counting more. Its rectangle is upright on a wall. Each edge
follows the median of its last 8 observations and moves only by more than a
cell's width at its range. An edge shrinks only while it is in view, since an
edge out of view is where the view ended. Planes that come to overlap merge.
Ids are never reused. A plane farther than 8 m from the camera is dropped.

**Stability.** A plane is reported once it had patches in 70% of its updates
over its first 0.5 s. A candidate that misses is held back, still taking
patches, until the camera moves 0.5 m or turns 30 degrees, so a surface found
only now and then does not pop in while the wearer stands still. A plane first
seen after the camera has been still for half a second needs patches in 90% of
its updates over 2 s. The reported rectangle follows the estimate behind a
dead band (a cell's width for an edge, 0.5 degrees, 5 mm) at up to 1 m/s and
10 degrees/s while the camera moves, and 8 mm/s and 0.5 degrees/s while it is
still. Each update checks a 4x4 grid of points on every plane against the map.
A plane in view that the map sees through, with no patch, for 2 s is dropped.
A plane out of view or behind something nearer is kept as it was. At most 24
are reported, by confidence, area and nearness, and a reported plane keeps its
place unless a newcomer scores twice as high.

**Kinds.** Normal within 15 degrees of up: the floor within 15 cm of the floor
estimate, a table 0.3 to 1.3 m above it, else other. Normal down, 1.9 m above
the floor: the ceiling. Normal within 15 degrees of horizontal: a wall when its
top reaches 1.2 m above the floor, else other (a box side, a cabinet front).
The floor estimate follows the lowest large upward plane at least 0.9 m below
the camera, because `y = 0` need not be the floor when the wearer roams. A
reported plane changes kind only after the new kind has held for 1 s.
`to_obstacle_box` gives an `ObstacleBox`'s fields as a plain `PlaneBox`
(`particles3d` is Android-only), half thickness 2 cm like a scene plane. Its
UUID starts with `fslive` and two zero bytes and ends with the id. The zero
version and variant bits are a layout no runtime anchor has, and it is never
the stage floor's all-zero UUID.

**Measured on the desktop.** An update on the 40x40 grid takes 0.34 ms (0.36
at most) on one core of an i9-13900K, and 1.3 ms on 80x80. After warm-up its
buffers do not grow. Standing still, the floor, the walls and the table are
reported at 0.6 s at 5 updates a second and at the second update at 2 and 1,
within 1.2 degrees and 1.7 cm. With noise alone for 10 s the ids hold from the
first second and no center moves faster than 0.1 cm/s. A box taken away in
view is dropped after 1.8 s. Over 100 noise seeds the scripted tests failed
once in 900 runs: on the 80x80 grid, one of 40 panels was reported 10.5
degrees off.

**Not yet.** The wiring reads 64x64 by default (below). At 40x40 a cell is
8 cm at 1.5 m and 16 cm at 3 m, so a table
top 2.3 m from a standing wearer spans two rows and is not found, and 40
separate patches in one view need the 80x80 grid. The noise model is assumed, not measured. On
noise-free depth the correction leans slanted walls 2.5 degrees the other way,
so the Quest's real noise has to be measured before the model is trusted.
Planes are not saved, and their ids last one session, so the room file must
not key behaviors to them. Horizontal planes take their yaw from their first
patch, not from the walls. No meshes, no curved surfaces.

### Wiring (board #5397)

`live_wire.rs` feeds the finder from the headset and puts its planes into
the show. Its rules are plain numbers with desktop tests, and so is its
worker thread.

**The read-back.** The self-check's shape, made for the finder. A compute
pass loads a 64x64 grid of the left depth layer into a buffer, and the
buffer is mapped a few frames later. The pass and its copy ride in the
frame's one submission, since a submit of its own cost about 0.8 ms. Nothing
waits on the map: the frame loop polls it. One layer, the left, because the
two layers see nearly the same room and one costs half as much. It runs 4
times a second while live planes are on, with or without room boxes. Each
grid carries the pose and fov its image was acquired with, its near and far,
and when. The finder unprojects with those, not with the current frame's.
The self-check runs as it did.

**The worker.** The frame loop hands each read-back to a thread through a
slot that holds one job, so a newer one replaces one the thread has not
taken. The thread runs `unproject_grid` and `LivePlanes::update` and sends
back the reported planes, `floor_y`, the `Stats` and how long the update
took, with a mean and a max over the last 16. The frame loop never waits
on it. It hands a job over only when the thread is not holding the slot,
else it keeps the job for the next frame, and it drains the results with
`try_recv`.

**When it runs.** While roam is on and the depth provider runs. Off,
nothing is read back and the thread sleeps. Each time it turns on, the
finder starts fresh. `debug.fosfora.liveplanes 1` also runs it with roam
off, to measure in a scanned room, and 0 keeps it off.

**Into the show.** Each frame the reported planes become obstacle boxes
after the room's, so the surfaces pass, the emitters and the collisions
treat them as room surfaces. Each runs its kind's default behavior.

- A live plane that lies on a scanned box is left out, so a scanned wall is
  not lit twice. It lies on one when its normal is within 15 degrees of a
  face's, its center within 10 cm of the face's plane, and more than half
  of its rectangle over the face. The stage floor counts as such a box. In
  roam it stands at the finder's floor height, so it already is that
  surface, 20 m across.
- The room's boxes come first. Live boxes fill what is left of the 32 in
  the finder's order (by id), with the stage floor's slot kept, since it is
  pushed last and would be the one dropped. The count past the budget is
  logged when it changes.
- A live plane fades in over 0.6 s from when it is first shown, and out
  over the same time when it leaves, with its last box. The fade scales the
  strength in its lane row. The surfaces pass applies that strength to the
  light and the sim already scales its emitters by it, so nothing in the
  sim changed. A plane past the budget waits at zero and fades in once it
  has a box. Collisions do not fade.
- The space fit takes the room's boxes alone. With the live planes in it,
  the space would change size as the wearer looks around.

**Never saved.** A live plane's UUID is never written to a room file and
never enters the room id. Its id lasts one session, so an entry under it
would name nothing at the next launch, and a room id with it would change
as the wearer looks around. Every place a box's UUID is kept or hashed, and
what it does with a live one:

- `room_file::room_id` leaves it out.
- `RoomFile::assign` writes nothing for it and `assign_kind` skips it.
  `to_json` never writes one, and a file that names one skips it with a
  line.
- The lanes' writers refuse it. `#<k>` and a UUID never name one, a kind
  passes it by (it follows the kind's new default), and the hand menu's
  surface rows refuse it.
- The room editor's ray passes through live planes
  (`RoomEditor::step_except`). The voice path's vocabulary, its agent and
  the surface labels leave them out and name the room's surfaces among
  themselves.
- The lanes' build key hashes every box's UUID, live ones included. It is a
  key in memory, so the rows rebuild when a live plane comes or goes.
- The anchor replay (`debug/anchors.json`) saves the room's located
  anchors, which never include a live plane.

A live box's label is "live wall", "live table" and so on, which no anchor
has, so a scanned surface's name never changes because of one.

**The floor.** In roam, once the finder has a floor, the stage floor's slab
and rings stand at its height. The height is already filtered, so the slab
follows it directly, at up to 0.25 m/s, which turns a first value well off
y = 0 into a glide. Seated, it stays at y = 0. Whether the room has a floor
of its own (the stage floor's emitter flag, its rings and the editor's
pick) is decided by the room's boxes alone, so a live floor never makes the
stage floor stand down.

**Noise calibration.** With `debug.fosfora.liveplanesnoise 1` in a scanned
room the read-back runs whatever roam does. Each texel's ray is cast
against the room's boxes as the self-check casts it. For a texel whose ray
meets the nearest face more than 30 degrees off grazing, the depth error
along the ray goes into a 0.5 m bin by the box's range, from 0.5 to 4.5 m.
Every 5 s one line gives each bin's count, median and spread (1.4826 times
the median absolute deviation) in centimeters, and the noise constant that
spread implies at the bin's middle range. `NOISE_K` stays at 0.01 in this
step, to be set from those lines.

**Seeing the planes unworn.** `debug.fosfora.liveplanesshow 1` outlines
every reported plane's rectangle in its kind's color, the ones left out on
scanned boxes too, with the room editor's highlight shader.

**Recording (board #5400).** The finder's tests use a synthetic sensor,
and the headset's depth is quieter and smoother than that sensor.
`debug.fosfora.liveplanesrec <seconds>` has the worker append each
read-back it is given, before the finder takes it, to one file in
`Android/data/<package>/files/debug/`, beside the app's own `Music/`, so a
USB pull reaches it without run-as. The file holds a header (magic,
version, the grid's size, the reading) and, per read-back, its time, the
layer's pose and fov, near and far, the map's size and the depth values as
read back. `live_rec.rs` has the layout. The knob is read at launch, so a
recording can start with the finder's first update, and then polled once
a second. A new value starts a recording of that many seconds (at most
600), counted from the first read-back written; unset or 0 stops it. A
recording is of one run of the finder and closes when the finder starts
over. Unset, the jobs carry nothing and the worker only checks that.
`live_rec::replay` runs a recording through the worker's own `Fitter`.
`live_rec::doubles` names pairs of reported planes that are one surface
twice: normals within 15 degrees, each center within 20 cm of the other's
plane, and more than 30% of the smaller rectangle over the other.
Recordings are of real rooms and stay out of the repository. The ignored
test `replay_the_recording_in_live_planes_rec` reads one from
`LIVE_PLANES_REC`.

**Knobs.** `debug.fosfora.liveplanes 0|1` (polled once a second, so on and
off can be compared in one launch; 1 at launch also creates the depth
provider), `liveplanesgrid` 40..80 (default 64, at launch), `liveplaneshz`
1..10 (default 4, at launch), `liveplanesnoise 0|1` (at launch),
`liveplanesshow 0|1` (polled once a second) and `liveplanesrec <seconds>`
(at launch, then polled once a second).

**Log.** At start "live planes: on · grid 64 · 4 Hz · layer left", and
"live planes: off (roam off)" when it stops. Once a second while it runs
"live planes: 7 reported (floor 1, wall 3, table 1, other 2) · 12 tracked ·
floor y -0.02 · update 1.9 ms (max 3.4) · 4.0 Hz · 3 shown, 2 on scanned
boxes, 1 on the stage floor, 0 over budget". A line when a plane is first
reported, "live planes: + #12 wall 1.20 x 2.10 m, 1.8 m away", and the
same with "-" when it leaves. When the count past the budget changes, "live
planes: 2 over budget (the room's 29 boxes and the stage floor leave 2 of
32)". The stage floor's height when it moves 5 cm,
"roam: the stage floor at height -0.02, the live planes' floor". The noise
line, "live planes noise: 20 reads, error along the ray (cm) by range (m),
over 30° off grazing: 0.5 n 210 med +0.40 sd 0.60 k 0.0107 · ...". A
recording's lines: "live planes rec: 30 s asked for
(debug.fosfora.liveplanesrec), from the next read-back the finder is
given, into .../files/debug" when the knob changes, "live planes rec:
recording 30 s to .../liveplanes-1762950000-1.lprec (grid 64, rows
bottom-up)" when the file opens, and "live planes rec: closed
.../liveplanes-1762950000-1.lprec · 120 read-backs over 29.8 s, 1.9 MiB
(its time is up)" when it closes.

**Not measured yet.** The read-back's cost, the update's time on the Quest
and the depth noise are for the device.

## Footprints (board #5403)

The plane finder describes flat surfaces. A room also holds things that are
not flat: a chair, a pile of boxes, toys, a person, a pet. No plane fits
them, and a box around one is wrong everywhere above the floor. What can be
measured from any viewpoint is where a thing stands on the floor. The show
gives each such thing a low box around its footprint, and its real shape is
left to particles landing on the depth surface. `footprints.rs` finds and
tracks the footprints. It is plain numbers with desktop tests, and nothing
draws them yet.

**What it takes.** Per update the time, the finder's own `Observation`, the
floor height and the reported planes as boxes (`Plane`, from a `LivePlane`
or a `PlaneBox`). With no floor height (`None`) it uses a fallback the
caller passes. The caller should pass the stage floor's current height, the
one `live_wire::stage_floor_y` keeps: 0 in the floor-level space until the
finder has a floor, and the live floor after.

**Which points count.** Bare floor is within 6 cm of the floor height. Stuff
is from 8 cm to 1.8 m above it, farther than 0.5 m (level) from the camera,
within 4.5 m, and not on a reported plane. On a plane means within 6 cm of
its box, or 3 noise deviations if more, and inside its rectangle grown by
10 cm. Planes win, so a bookcase found as planes leaves no footprint, and a
floor plane explains nothing. Both height bands widen by the noise's
vertical part, since the noise runs along the ray: at 3.8 m stuff starts
about 12 cm up, so far floor the noise lifts is not a thing. Three kinds of
point are left out first:

- Blends of two depths at a silhouette. A point is one when its neighbors
  on one axis jump in range and it lies between them, and its neighbors on
  the other axis do not agree with it. For a jump above and below, neither
  neighbor left or right may agree; that keeps a top seen in a single row
  from a low eye, which runs along the row. For a jump left and right, the
  two neighbors above and below must both agree. A point near the floor
  needs the plane finder's larger jump above and below, because a grazing
  floor's rows are far apart.
- Walls. An upright surface 2 m above the floor is a wall's, a wardrobe's
  or a curtain's. Each grid column is followed down from there while its
  points stay on that vertical plane. This keeps walls out before the
  finder reports them, and where it misses them.
- Stuff with no neighbor (not a blend) within two grid cells' width. A blend
  that got through hangs alone in the air behind a silhouette.

A stuff point stands for the level segment half way to its neighbors left
and right on its surface, so the segment follows a turned face. On a top it
also stands for the strip of the top its grid cell sees. The strip runs
toward and away from the camera, up to 25 cm each way, never past the
points of the grid cells above and below. At a top's back edge it runs at
most 10 cm. From a low eye a top is one row of the grid, and the strip is
what gives its depth. Bare floor is spread the same way, so far floor
between sparse rows is seen.

**The floor grid.** Cells of 10 cm in world x and z, made only by stuff.
Each keeps an occupancy score, its lowest and highest stuff, its extreme
samples in eight directions and a probe, a stuff point it held. The score
rises by the update's time step for each update with stuff in it, up to
1 s. A cell is occupied at 0.3 s, the second update at 4 Hz. Beside an
occupied cell near a moving footprint it is occupied at once, which is the
moving thing's leading edge. Elsewhere that would let a silhouette's blends
join a thing. Stuff in a cell in an update wins over anything else in it.
Without stuff, a cell is free at once in two cases:

- A ray reached the floor in it lower than its lowest stuff. The floor seen
  under a chair's seat says nothing about the seat.
- The map is farther than its probe on all four grid cells around it. The
  probe is seen through.

Its score falls by the time step when points on a plane land in it, so a
thing found later as planes leaves. It also falls when the map around its
probe is farther on some grid cells and on it on none, unless the probe's
own grid cell is a blend in that update. That is a blend that got through,
hanging in the air. A cell not observed keeps its state. Cells farther than
8 m are dropped, and free cells with no stuff for 5 s are recycled.

**Groups.** Occupied cells join an 8-neighbor, or a cell one over across an
empty one, when their samples come within 15 cm and their height ranges
within 20 cm. Groups grow only from cells with stuff below 0.4 m. A hanging
lamp or a wall shelf stands on nothing, and a shelf beside a low cabinet
stays out of the cabinet's footprint. A group's rectangle is the
minimum-area one over yaw of its cells' extreme samples. Every point it is
fitted to is a sample, so a turned face does not add the corners a box
around its samples would. It keeps the previous yaw unless another is 10%
smaller, so a square thing does not spin.

**Tracking.** A group goes to the footprint whose estimate, moved by its
velocity and grown by 15 cm, holds most of its cells. A footprint is
reported once it had a group in 70% of its updates over 0.5 s, with a
rectangle of 0.04 m² whose short side counts as at least 15 cm. A thing
seen from one side shows only its near surface: a person seen from the
front is a thin arc, and anything standing has that much behind it. A
table's leg is still far too small. Ids are never reused.

Still, a reported footprint fits only the cells seen for 0.7 s, so a blend
seen once or twice does not move it. Its rectangle follows inside dead
bands and rate limits like the planes'. An edge moves only past 12 cm, at
most 8 mm/s while the camera is still and 0.5 m/s while it moves. The yaw
turns only past 3 degrees, at most 0.5 and 10 degrees/s.

Moved and noise are told apart this way. A footprint has moved when its
estimate is more than 15 cm from it and neither rectangle holds 75% of the
other. It has also moved when its estimate's velocity, smoothed over 0.5 s,
passes 0.2 m/s and it has left by 7.5 cm. Noise moves an estimate both ways
by an edge cell at a time, so the smoothed velocity stays low. A growth or
a shrink keeps one rectangle inside the other and follows at the still
limits. A moving footprint follows fast (gain 8, up to 2 m/s) while its
estimate keeps going and 0.5 s after. A moving footprint that found no
group takes an untaken group within 30 cm of where its velocity puts it.
The cells it leaves are cleared by bare floor or by being seen through. A
footprint with no group for 0.5 s is dropped. Out of view its cells keep
their state, so it is kept.

**Budget.** At most 8 are reported. Over that, the nearest small neighbors
are merged first, within 1 m, last update's pairs favored, under the larger
one's id. The rest are ranked by area and nearness, reported ones favored.

**Output.** `footprints()` gives each footprint's id, center on the floor,
yaw, half extents, highest stuff above the floor, confidence, whether it is
moving and when it was first reported. `to_obstacle_box` gives a
`PlaneBox`: 12 cm tall, standing on the floor, kind other. Its UUID starts
with `fsfoot` and two zero bytes and ends with the id. That is never a
plane's `fslive`, the zero version and variant bits are a layout no
runtime anchor has, and it is never the stage floor's all-zero UUID.
`nearest(point)` answers from every footprint remembered, in view or not,
with the id it is reported under (its own when it is over the budget) and
its level distance (0 inside).

**The synthetic sensor.** The tests' sensor is built on the headset's
figures for the error along the ray: median +2 cm and spread 0.14 cm at
2.5 m and nearer, +0.9 cm and 3 cm at 3.5 m, linear between, the spread
growing at that rate beyond (5.9 cm at 4.5 m). The noise is smoothed over
5x5 grid cells, as the runtime filters it. Silhouette cells are blended at
a random fraction half the time, as in the finder's tests, and 2% of cells
are holes. The camera is a 100 degree pinhole on a 64x64 grid at 4 Hz.

**Measured on the desktop.** An update on the 64x64 grid takes 0.25 ms
(0.26 at most) on one core of an i9-13900K, 0.12 ms on 40x40 and 0.36 ms on
80x80. Its buffers do not grow after warm-up. The numbers below are for the
default seeds.

- A 0.5 m cube 2 m away is reported at 0.75 s. Square to the view it
  measures 0.49 x 0.42, 0.51 x 0.48 and 0.47 x 0.53 m from eyes 0.76, 1.2
  and 1.7 m. Turned 30 degrees: 0.48 x 0.50 (7.0 degrees off), 0.50 x 0.56
  (4.1) and 0.54 x 0.53 m (0.0). Centers are within 3.3 cm. With no floor
  height, on the fallback, square: 0.49 x 0.44, 0.44 x 0.51 and
  0.48 x 0.59 m.
- An L-shaped pile with a 1 x 1 m bound gives one footprint of
  0.94 x 1.07 m, a corner at most 7.3 cm outside it.
- Two 0.4 m cubes 0.5 m apart stay two. 0.1 m apart they become one of
  0.85 to 0.88 x 0.37 to 0.38 m.
- Noise alone for 10 s: ids hold from the first second and no center moves.
- A 0.4 m box sliding at 0.5 m/s keeps one id. The footprint stays within
  21 cm of it, and no cell is left more than 25 cm behind it. At 1 m/s: one
  id, within 29 cm, cells up to 50 cm behind.
- A box taken away in view is dropped 0.25 s later. Taken away out of view
  it is kept, and dropped by the time the camera is back on its spot.
- No footprint for a hanging cube and a wall shelf, a table whose top is a
  plane, four bare legs, a bookcase whose front is a plane, the wearer's own
  feet, or walls not passed as planes (a full turn at 1.6 and 1.2 m).
- Turned away and back: the same id throughout, and `nearest` answers with
  it at every update.
- Ten 0.4 m boxes 0.5 m apart: 10 tracked, 8 reported with 2 merged pairs.
- With the plane finder run on the same input: a person (a cylinder) is a
  footprint of its visible front, and nothing is reported on the walls.

Over 13 noise seeds (`FOOTPRINTS_SEED`), 8 of 273 test runs failed:

- In 4 runs, two of the ten boxes, one behind the other as seen, joined
  after 4 to 8 s. Blends between the near box's top edge and the far box's
  face pile up in the gap the camera cannot see.
- In 2 runs the L pile's rectangle came out diagonal, a corner 12 and 27 cm
  outside.
- In 1 run a table leg was reported for 0.75 s.
- In 1 run the turned cube came out 20 degrees off from 0.76 and 1.2 m.

**Not yet.** The thresholds are set for the synthetic sensor, whose blends
are drawn fresh at every update. The headset's are filtered and may be
steadier. The ignored test `replay_footprints_in_live_planes_rec` runs the
finder and the footprints on a recording (`LIVE_PLANES_REC`), as the worker
would, and has not been run on one yet. Other limits:

- A thing seen from one side is only its visible part. A person seen from
  the front is a 10 to 15 cm deep arc until the wearer walks around.
- A box seen with almost no top (a 0.5 m cube from a 0.76 m eye) has its
  depth from a single row of the grid, often 5 to 8 cm short.
- A turned box seen as a V of two faces with little top fits the
  rectangle along its faces or along the chord between them at the same
  area, so its yaw can come out up to 45 degrees off.
- A 0.3 m box 3.2 m away is about 2.5 grid columns wide on the 64x64 grid.
  Its estimate is 0.14 to 0.24 m wide, under the minimum, and it is often
  not reported.
- A thing partly explained by planes (a box whose top is found but not its
  sides) leaves a thin footprint along its unexplained faces.
- A table whose top is not found as a plane is a footprint of the whole
  table.

**What the wiring needs.** The worker runs `Footprints::update` after
`LivePlanes::update` on the same points. It passes the finder's planes
(`Plane::from_live`) and, in a scanned room, the room's boxes
(`Plane::from_box`). It also passes `floor_y()` and the stage floor's height
as the fallback, and sends `footprints()` back with the planes. The frame
loop draws each footprint as `to_obstacle_box`, with sides leaning inward
and fading out above. Footprints and live planes share what the room's
boxes leave of the 32. The stage floor's slot is kept first, then the
planes in the finder's order, then the footprints by id, at most 8. Planes
go first because they carry the surface effects, and a footprint is the
fallback for what no plane fits.
