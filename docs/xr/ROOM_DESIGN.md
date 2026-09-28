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
arm extension** (board #3308). Within a comfortable reach from the
shoulder the virtual hand is the real hand; beyond ~0.45 m it travels
quadratically, so a full stretch reaches ~2 m. The extended joint set feeds
the sim's spheres (both scaring and pushing), occlusion stays on the real
hand, and a ghost hand with a wrist beam shows where the hand acts. Subtle
gain for the first pass. Every candidate below assumes it.

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

## How they combine

1 and 2 are one system: surfaces that light up and emit. 3 sits on top of
either. 4 is a later layer. A first pass that ships 2 (cheapest, most
measured) with one surface from 1 (the floor) gives a room that answers
the music from below, and the hands already carve it.

## Decisions for Kevin

- Which two for the first pass. Recommendation: 2 (surfaces as emitters,
  desk and floor) and the floor ripple from 1.
- The look direction: the ember palette from Flux, or something built for
  a lit room (paler, thinner, more light than particle).
- Whether Murmur ships small as a room inhabitant (30–50K birds fleeing the
  hands) or is parked.
- Whether the global mesh is worth fetching now for occlusion alone, before
  any effect uses it; the passthrough-has-no-depth problem argues for it.
