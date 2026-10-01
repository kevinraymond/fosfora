# Surfaces as effects

*Design note, Sep 30, 2026, after the room editor's eighth worn pass. Kevin's
direction: every room surface becomes its own effect, drawn on the surface
and staying there, as the mockup showed (streamlines flowing across the
tabletop, rings spreading over the floor, curls on the chairs, a field over
the whiteboard, each in its own color). The particle cloud stays as one
option, not the frame everything else hangs in. This note says what we have
to build it with, three ways to build it, what each costs against the
budget, and a recommendation. Nothing here is implemented.*

## What we have

- **Surfaces with identity.** The room's anchors as oriented boxes (up to
  32), each with a kind, a UUID and a behavior lane (`lanes.rs`,
  `room_file.rs`, board #3326), assigned by pointing and pinching, saved
  per room, restored on relaunch. The editor's pick, highlight, labels and
  panel rows carry over unchanged.
- **Two surface effects already.** The floor ripple (`ripple.rs`) and the
  wall spectrum (`canvas.rs`) are each one lit quad on a face, a fragment
  shader driven by the audio uniforms, premultiplied light over
  passthrough, depth-tested against the room's occluders and the live
  depth map so a chair in front of the wall hides it, drawn before the
  sprites so embers pass over them. Both measured under the noise floor of
  a 72 Hz frame (MEASURED.md, the ripple and the spectrum rows).
- **A budget.** 13.9 ms per frame; the MR base (passthrough, anchors, hands,
  the depth occluder) takes 3 to 4 ms; Embers at 400K takes 6 ms more. With
  the cloud off, about 10 ms are free for surfaces.
- **A datum on desktop effects in XR.** The first spike (S4) rendered the
  desktop Flux into a 1280x720 texture on a quad: 30 to 50 ms a frame at the
  Low quality, 154 ms at High (MEASURED.md, "S4 notes"). The cost is the
  effect's frame (the compute raster, the resolve, the velocity feedback),
  not the quad.

## Three ways

### A. Surface shaders: a catalogue of purpose-built fragment effects

Each surface is one quad on its acting face (the top of a table, the
room-facing face of a wall), and its behavior picks a fragment shader from
a catalogue written for surfaces: **streamlines** (a curl-noise flow field
advected by the beat, the mockup's table), **rings** (the ripple, the
mockup's floor), **curls** (a tighter, faster field, the mockup's chairs),
**spectrum** (the wall canvas as it is), **pulse** (a whole-face glow on the
downbeat, for frames and lamps). Every shader takes the same inputs: the
face's uv, time, the audio uniforms, and the surface's parameters from its
lane (a color, an audio band, a speed).

- Needs: one render pipeline per shader (the ripple's family), a face quad
  per surface with its lane in a uniform row, a small per-surface parameter
  block (the lane's two spare lanes, or a second block), and a shared WGSL
  library for the fields.
- Cost: fill only. A surface costs the eye pixels it covers, once; the whole
  room is one to two full-screen fragment passes, so 1 to 2 ms with every
  surface lit. The sim cost is zero. This is the ripple and the canvas
  scaled up.
- Risk: the look. A fragment field is not a particle system; streamlines
  drawn in a shader are lines of light, not things. On a real table over
  passthrough that is what the mockup shows.

### B. Desktop effects rendered onto surfaces

Each surface runs a desktop effect (Flux, Tide, any of the 58) in its own
offscreen renderer, the texture mapped onto the face: the S4 quad path, one
per surface. It is the most literal reading of "transform each surface into
an effect", and it is out of budget by an order of magnitude on this
hardware: one Flux at 720p was 30 ms, and a surface needs at least 256 to
512 pixels a side to hold up at arm's length; ten surfaces at 256 would be
a tenth of the pixels each but the same sims, so roughly 3 ms a surface
before the mapping. It also spends the GPU on pixels the wearer never sees
(the back of a table). Not for Quest 3. Worth revisiting when a desktop
effect has a "surface" quality preset of its own.

### C. Surface-bound particles

Particles that live on a face: born there, moved by a 2D field in the
face's plane, never leaving it (the drips, the dust and the pools of the
catalogue's reserved ids 5 to 7). One sim serves every surface, the
particle carrying its box index; the draw is the sprite path we have.

- Needs: a surface-bound branch in the Flux world sim (the face frame per
  box is already computed for spawns), a 2D field per behavior, the
  existing sprite draw.
- Cost: the sprites' cost, which is the cloud's: 400K sprites are 6 ms.
- Risk: it reads as particles again, "flying around", which is what Kevin
  is steering away from; and one count for the whole room means a big
  table starves a small one.

## Recommendation

**A now, C later for the behaviors that need bodies (drips, pools), B not
on this hardware.** A is the mockup: light on surfaces, nothing flying,
every surface its own shader and color, and it costs about what the ripple
and the canvas cost today. The editor keeps working as it does; only the
catalogue behind a tap changes from five particle rules to a list of
surface shaders, and the cloud becomes one entry ("embers", the surface's
particles) rather than the world the others live in.

### The data model

- The lane stays: `x` the behavior id + 1, `y` strength, `z` and `w` two
  parameters. The catalogue grows: 0 none, 1 embers (the particle cloud
  from this surface, as today), 2 sparks, 3 spectrum, 4 rings (the ripple,
  any face), 5 streamlines, 6 curls, 7 pulse. The reserved ids move up.
- A surface parameter block (a second 32-row block after the lanes, or the
  room file's `params` widened): color as a hue (the key-tinted palette by
  default, per kind), audio band (bass, mid, high, rms), speed, density.
  Written by the room file, edited later from the panel (C's parameter
  steppers).
- Per-kind defaults: tables streamlines, floors rings, walls spectrum,
  frames and lamps pulse, storage curls, ceilings none. The first worn pass
  decides.

### The render

One `surfaces` pass in the eye pass after the occluders, one draw per lit
face: a uniform row block with every face's corners, lane and parameters,
one pipeline per shader (each a small WGSL with a shared field library),
the ripple's depth setup (no write, a bias, `LIFT_M` off the face). The
highlight stays its own quad on top. The ripple and the canvas fold into
this pass as the rings and the spectrum shaders.

### Steps

1. **D1, the pass.** The surfaces pass with two shaders, rings and
   streamlines, drawn on every acting face whose lane says so, the ripple
   and the canvas moved into it unchanged in look; the catalogue renumbered
   and the file migrated (a version 2 with the old ids mapped). Unworn: a
   screencap per shader and the cost at 1, 4 and all faces lit. Gate:
   under 2 ms with every surface lit.
2. **D2, the catalogue.** Curls, pulse, and the per-kind defaults; the
   parameter block with color and band; the editor's tap cycles the new
   list per kind. Worn gate (Kevin): the mockup's room.
3. **D3, per-surface parameters** from the panel (the C design's
   steppers), and the cloud as one entry among the others.

### Open questions for Kevin

1. Colors: one hue per surface from a palette by kind (the mockup), or the
   key-tinted palette everywhere for continuity with the desktop look?
2. The cloud's place: an entry per surface ("embers off this table"), a
   room-wide toggle, or both?
3. Whether the desktop effects should get "surface" presets later (B), or
   whether the surface catalogue is its own thing for good.

## D1 as built (Sep 30, board #3472)

**The catalogue.** Ids 0 none, 1 embers, 2 sparks, 3 spectrum, 4 rings
(the floor ripple, renamed), 5 streamlines; 6 and 7 stay reserved (curls,
pulse, D2). The room file and the `surface` knob read `ripple` as the
rings and write `rings`, so the file kept version 1. The editor's cycle
per kind: table, other and unlabeled none, embers, sparks, streamlines;
floor none, sparks, rings, streamlines; wall none, spectrum, streamlines,
rings; ceiling and frame none, rings, streamlines. The kind defaults are
unchanged (table embers, floor rings, wall spectrum, else none). A
per-kind palette (`surfaces::palette`, linear RGB): tables blue, floors
violet, walls the canvas's warm white, other green, frames and ceilings
amber. The sim is untouched (its gates read ids 1 and 2 only, and its
constant for id 4 keeps the name `XR_BEHAVIOR_RIPPLE`).

**The pass** (`surface_fx.rs`, `gfx.rs`). One pipeline with the ripple's
setup (premultiplied over passthrough, depth-tested, no depth write, the
bias), 32 uniform slots (one per box, a buffer and bind group each), one
draw per lit face, right after the occluders where the ripple drew:
before the canvas, the highlight and the sprites. A slot is 17 rows
(272 bytes): the four corners (the acting face lifted 2 cm); `face` (the
two half extents, the lift, the behavior id); `params` (strength, color
index = the kind, audio band = 0 for the rms, speed); `color` (the color,
the peak alpha); `audio` (rms, bass, beat envelope, the clock); `shape`
(the rings' origin u, v and glow; the streamlines' feature size), a row
the brief did not list; then 8 ring rows (origin u, v, radius,
intensity). Positions are in the face's (u, v), meters from its center
along its axes, which the vertex stage hands the fragment. The clock runs
at 1 + the beat envelope (integrated in f64), so the flow doubles on the
beat without jumping back as the envelope decays. Every box whose
behavior is the rings or the streamlines is lit on its acting face, the
stage floor only while the room has no scene floor (step 2d's rule, so
the two floors never double), a hidden wall never; a floor's face is
leveled and kept above the stage floor's top, as the ripple's quad was.
Logged once per change: "surfaces: 3 lit (rings 1, streamlines 2)".

**Rings** (id 4): the ripple's look, exactly, in face (u, v): the 0.15 m
ring width, the glow (0.6 m, 0.3 at full bass), the peak alpha 0.25
times `ripplegain` times the strength, 2.5 m/s (`ripplespeed`). One
`Ripple` state is kept; each ring now also keeps the smoothed head it
was born under, and its row is placed per face: on a floor the head
projected onto it (as before), elsewhere the face's point nearest the
head (a table's edge nearest the chair). On a floor they keep the
ripple's warm white, so the floor looks the same; on any other face the
kind's palette color. The one visible change on a floor: over the stage
floor the rings now cover its whole face instead of a 6 m square around
the head, so a ring fades out rather than ending at the square's edge.
`debug.fosfora.ripple 0` turns the rings off everywhere; `rippletest
ceiling` puts the rings on every ceiling for the run (the room file
untouched), in the ceiling's amber.

**Streamlines** (id 5): the contours of a stream function ψ, two octaves
of quintic value noise (0.35 m features, 0.7 m on a floor, the second
octave at twice the frequency and a quarter of the amplitude, each
drifting 0.04 cells per second of the clock); the flow is ψ's curl,
divergence-free by construction, and ψ's contours are its streamlines,
so a still frame shows thin, curving, directional lines. A contour every
0.1 in ψ (in meters) gives a median of about 10.5 lines per meter, but
the density follows the field's speed (3 to 23 between the tenth and the
ninetieth percentile over a table's field), so the contours come in
tiers: every other line thins out from 12 to 20 lines per meter, a line
between each pair fills in from 7 down to 4. That gives a median of 10
and 6.5 to 13.2 from the tenth to the ninety-fifth percentile; only
around the field's stagnation points, where the contours ring a point,
are the lines sparser. Each line
is 6 mm wide with a 1.5 mm soft edge (or a pixel's footprint, farther
away). Along the lines, streaks 8 cm long run with the flow at 0.15 m/s
of the clock (two advection phases of 0.3 m cross-faded), over a base
of 0.35. The alpha is up to 0.55 times the strength (0.30 read "too faint" worn), times the rms with
a floor of 0.6, faded over 5 cm toward the face's edges, in the kind's
palette color. The Rust twin of the field library backs the desktop
tests (divergence at a few points, the analytic gradient, the density).

**Folded in, and not yet.** The floor ripple is the rings: its pipeline,
`set_ripple`, its quad and its shader are gone. The canvas (the wall
spectrum) is not folded in: it keeps its own pipeline and draw after the
pass, and a wall on the spectrum is skipped by the pass. The highlight
stays its own quad on top.

**The sweep.** The cost at 1, 4 and every face lit (reviewer, unworn):

```
adb shell setprop debug.fosfora.surface "table=streamlines,floor=rings,wall=streamlines,other=streamlines"
```

plus `ceiling=rings,frame=streamlines` for every face; `#<k>=<behavior>`
lights one box, `clear` puts the defaults back.

**Music row** (Kevin, worn, Sep 30: every worn run had been on the
synthetic beat). A fourth hand-menu row, under Particles in both
layouts, plays and stops the bundled clip (`ember_glow_excerpt.ogg`,
the 140 BPM excerpt `debug.fosfora.audio file` loops) whatever the app
launched with. Play does what `file` does: the clip on the speakers,
the analysis on its tap. Stop ends the stream, and the analysis goes
back to the launch source: the microphones, which stay open across a
play, or the synthetic groove. `loop` keeps its analysis on the
microphones either way. The first play decodes on a worker thread (the
row reads "Music: stop" meanwhile). After that the clip is kept, and a
play resumes where the stop left it. `debug.fosfora.music 1` plays it
at launch for the unworn checks. `file` and `loop` start on "Music:
stop" as before, and nothing is saved. `music.rs` holds the row's words
and where each state puts the analysis.

## D2a as built (Oct 1, board #3488)

**The catalogue.** Ids 6 and 7 are real: 0 none, 1 embers, 2 sparks, 3
spectrum, 4 rings, 5 streamlines, 6 curls, 7 pulse; nothing is reserved,
the room file and the `surface` knob take every name. The pass draws the
rings, the streamlines, the curls and the pulse on every face that runs
them, and the spectrum on the one wall the pick chooses (below). Each
behavior has a default audio band: the bass for the rings and the pulse,
the rms for the flows. The sim is untouched (its gates read ids 1 and 2).

**Curls** (id 6): the streamlines' field tighter and faster, three
octaves of the same value noise at 0.12 m (0.25 m on a floor), each a
quarter of the last's gradient, their own seeds, drifting 0.1 cells per
second of the clock, the streaks at 0.3 m/s. The lines keep the
streamlines' contour spacing and tiers; on this field they enclose 5.5
closed curls on a 0.4 m chair seat (counted over 16 m² at two clock
times on the desktop; a test holds it in 4 to 6). A fill inside each
closed contour, the smoothed sign of ψ (`ψ / sqrt(ψ² + s²)`, s a quarter
of the feature size) at a quarter of a line's light, makes a curl a
disc of light with a bright rim, dark along ψ's zero contour between
curls. Peak alpha 0.55, the band's level with the streamlines' floor of
0.6, the 5 cm edge fade.

**Pulse** (id 7): a whole-face glow, 1 at the face's center falling
smoothly to 0.35 at its edges (the 5 cm edge fade kept), times the
downbeat envelope over a floor of 0.15 times the band's level: the
envelope jumps to 1 on `hop.downbeat_fired` and decays at 1.6/s (a
0.6 s time constant, under 0.05 after 2 s), kept in `app.rs` beside the
beat envelope. Peak alpha 0.45.

**The slot** is 34 rows (544 bytes, one per lit face, 32 slots): the
four corners; `face` (the half extents, the lift, the behavior id);
`params` (strength, the color index, the band, the speed); `color` (the
color, the peak alpha); `audio` (rms, bass, the beat envelope, the
clock); `audio2` (mid, high as the larger of presence and brilliance,
the downbeat envelope, the bar phase); `shape` (the rings' origin and
glow; the streamlines' and the curls' feature size; the spectrum's sign,
bar count, glow and bar fill); 8 ring rows; 16 rows of bar heights
(zeros on every face but the spectrum's).

**Color and band** are the lane's spare `z` and `w`, saved as the room
file's `params`. The color index: 0 the kind's own palette color (D1's
look; the rings on a floor keep the ripple's warm white and the
spectrum the canvas's on any kind), 1 blue, 2 violet, 3 warm white, 4
amber, 5 green, 6 teal, 7 rose, 8 the key's tint: the desktop's
`audio.key_hue` (core's binding source, the circle of fifths eased
toward the middle as the key's confidence falls) at full value and 0.8
saturation, computed only on a frame where a lit face asks for it. The
band: 0 rms, 1 bass, 2 mid, 3 high; an entry with no params runs the
behavior's default band, not the rms. The rings ignore the band (they
follow the ripple's own bass). The knob takes an optional suffix,
`<target>=<behavior>[:<color>[:<band>]][@<strength>]`: `table=curls:5:2`
is curls, green, the mid; a color alone keeps the band the entry has; a
kind target writes the params to every entry of the kind (the kind's
default carries none, so an anchor the room gains later runs the
defaults); a value out of range or not whole refuses the whole knob
value, nothing applied. Nothing edits them from the panel yet (D3).

**The defaults: the mockup's room.** Tables the streamlines, floors the
rings, walls the spectrum, frames the pulse, any other labeled surface
(storage, a couch, a screen, a lamp, a plant) the curls; ceilings and
unlabeled anchors none. Nothing emits by default: the Cloud row and the
`cloud` knob are unchanged, and embers or sparks on a surface is how
the cloud comes back there (decision #3474). Each kind's cycle puts its
default first after none, so one tap from any default goes to the next
look of that kind: table none, streamlines, curls, pulse, embers,
sparks; floor none, rings, streamlines, curls, sparks; wall none,
spectrum, streamlines, rings, pulse; ceiling none, rings, pulse,
streamlines; frame none, pulse, rings, streamlines; other and unlabeled
none, curls, streamlines, pulse, embers. Ceilings and unlabeled anchors
default to none, so for them the first tap lights the cycle's second
entry. The sim's own default for an unset lane (`xr_kind_behavior`) is
still the pre-D2 rule (a table on embers); the app writes every box's
row, so only an upload without the lanes sees it.

**Room file version 2.** Every save before D2 wrote all six kind
defaults, table embers and frame none among them. A version 1 file now
keeps its anchor entries (behavior and strength; its params, never
defined and always [0, 0], are dropped) and its kind defaults give way
to the built-ins, said once in the load's log ("version 1: its kind
defaults give way to the built-ins"); the next save writes version 2
with the full set. A version 2 file loads as saved, its entries' params
written only once set, so "unset" stays apart from "set to 0".

**The fold.** The wall spectrum is shader id 3 of the pass: `canvas.rs`
keeps the bars, their normalization and `WallPick`, and lost its
pipeline, its shader and its uniform; `gfx.rs` lost `set_canvas` and its
draw. Among the walls whose lane says spectrum, the pick chooses the one
the wearer faces with its hysteresis (decision #3449), and only that box
is a spectrum slot; the others draw nothing. `canvas::spectrum_face`
orders the face's axes so v is the in-plane axis closest to up, signed
up, and gives the sign of u against the wearer's right as they face the
wall (up x normal), carried in `shape.x`: the bar index runs along u
times the sign, so the low bands stay on the wearer's left however the
runtime's axes point. The look is the canvas's: warm white, the cap
line, the base glow, peak alpha 0.25 times `canvasgain` (times the
lane's strength, 1 by default). `debug.fosfora.canvas 0` leaves no
spectrum slot; `canvasbars`, `canvasgain` and `canvastest ceiling` keep
working through the slot.

**The scan label.** The room editor's label, with no hit, says where
the room scan is (`Room::scan_state`): "Scanning the room…" held while
no query has returned anchors (the first, its retries, Space Setup),
"Room: 17 surfaces" for 3 s when one does, "No room found. Rescan from
the hand menu" for 6 s when the retries give up, each fading over
0.4 s, re-posed every frame 1.2 m in front of the head along the view
turned level, at eye height. The editor's label takes its place while
one shows. The panel's edit status is untouched.

**Logs.** "surfaces: N lit (rings a, streamlines b, curls c, pulse d,
spectrum e)" on every change, "surfaces: dropped k (the pass holds 32)"
when more faces want a slot; "wall spectrum: on box i ..." as before;
"room scan: Found(17), the label reads ..." on each change of the scan
state; "room <id>: rooms/<id>.json: version 1: its kind defaults give
way to the built-ins ..." on loading an old file.

**The sweep** (reviewer). Every face lit, against `surface clear` with
the cloud off:

```
adb shell setprop debug.fosfora.surface "table=streamlines,floor=rings,wall=spectrum,other=curls,frame=pulse,ceiling=rings"
```

Per shader on one kind, e.g. `other=curls:6:2` (teal curls on the mid)
or `frame=pulse:8` (the key's tint on the bass).

**Not yet.** The desktop's single-pass effects on surfaces (D2b, ids 8
to 15). The color and band from the panel, and params on a kind's
default (D3). The worn gate and the cost rows in `MEASURED.md` (the
reviewer's, on the device).
