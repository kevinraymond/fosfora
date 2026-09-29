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

- Which anchor the "wall the wearer faces" is when the room has four,
  and whether it follows the head or is picked once per preset.
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

