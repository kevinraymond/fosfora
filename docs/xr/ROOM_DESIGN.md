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
2 sparks up from the face with the bass (Flux, today's floor behavior) ·
3 the spectrum canvas (walls) · 4 the ripple (floors) · 5 drips down the
face gathering at its bottom edge (walls, new) · 6 dust lifting off the
face on the beat (any, new) · 7 a pool that embers settle into and glow
(tables, new). The first pass ships 0 to 4, which exist as code, and
proves the plumbing; 5 to 7 come with C.

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
none), so with no file and no knob the room looks as it did. A set lane
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
right counts as a tap · edit room".
