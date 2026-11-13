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
and where each state puts the analysis. (Amended Oct 20, board #4191: a
first-time wearer heard nothing and saw the synthetic beat until they
found the row, so the clip now plays at launch unless the launch source
is a microphone. `debug.fosfora.music` has three states: unset, it
plays over `synth`, the default, and `file` and `loop` as before, and
not over `mic`, `micxr` or `aaudio`, which are chosen to measure what
the microphones hear; `1` plays it over any source; `0` over none,
`file`'s and `loop`'s own clip included, so a quiet (no music) run sets
`0`. The row, its stop and where the stop puts the analysis are
unchanged; `music::plays_at_launch` holds the rule.)

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
one shows. The panel's edit status is untouched. (Amended Oct 20, board
#4192: the query after Space Setup gets the first query's retries, where
it had one try, so a room Space Setup just captured is not given up on
at the first empty answer; and Rescan is in the hand menu, under the
effect's row, not only in the debug panel.)

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

## D2b as built (Oct 3, board #3489)

**The eight.** Catalogue ids 8 to 15 are desktop effects, drawn on the
face by the surfaces pass, each named by its `.pfx`: 8 aurora, 9 prism,
10 shards, 11 astrolabe, 12 bezel, 13 fenestra, 14 reticle, 15 tessera.
All are a single fragment pass with no feedback and no texture read.
The core is unchanged and nothing is copied out of it: `surface_port.rs`
reads each shader and `.pfx` from `assets/` with `include_str!` and
goes through the core's public API (`EffectLoader::prepend_library`,
`ParamStore`, `effect::rates`, `mirror_audio_features`). The knob and
the room file take the names on any kind; the sim never sees them (its
gates read ids 1 and 2).

**The composition.** The effect's fragment entry becomes a plain
function: its header, `@fragment fn fs_main(@builtin(position)
frag_coord: vec4f) -> @location(0) vec4f {`, which the eight share to
the character, is replaced by `fn effect_main(frag_coord: vec4f) ->
vec4f {`, and the load fails unless that hits exactly once and no other
`fs_main` is left. The core's loader prepends the uniform block and the
libraries, and the wrapper is appended. Each composed module validates
under naga with one vertex and one fragment entry, and the fragment
entry reads two globals, `u` and the port block: of the desktop's seven
bindings in group 0 only the uniform is used, so the layout has binding
0 alone and nothing stands in for the previous frame or the audio
textures. A desktop GPU test builds the eight pipelines on those layouts
and draws them (`cargo test -p fosfora-xr --lib surface_port --
--ignored`).

**The wrapper** (`WRAPPER_WGSL`). Group 0 binding 0 is the effect's
uniform, one buffer per lit face. Group 1 is the eye camera (binding 0)
and the port block (binding 1), 8 rows: the four corners, `face` (the
half extents, the lift, the behavior id), `params` (the strength),
`color` (the peak alpha, 0.5), `mode` (x 1 for an overlay). The vertex
stage is the surfaces pass's. The fragment maps the face to the effect's
frame: 0..1 across the face from the (u, v) in meters, v flipped (a
frame's y runs down, the face's v up), times `u.resolution`. An overlay
effect (`alpha: true` in its `.pfx`, read from the parsed file:
astrolabe, bezel, fenestra, reticle, tessera) keeps its own
premultiplied coverage; an opaque one (aurora, prism, shards) is covered
by its luminance, so its blacks are the real surface and its highlights
glow. Either way the light is the effect's own color, clipped at 1,
times the peak alpha, the lane's strength and the pass's 5 cm edge fade.
The lane's color index and band are not read (D3 may tint).

Two things differ from the brief here. It listed tessera as opaque; its
`.pfx` says overlay, and the flag is the file's. And its formula for an
opaque effect multiplied the color by the luminance a second time, which
put a 0.3 mid-tone at 0.09 of the peak: shards' cells and prism's body
all but vanished in the desktop renders. The color is written as it is
and only the coverage is the luminance, which is also what the overlay
branch does with its alpha.

**Which way is up.** On a wall (a face steeper than 45°) the frame's up
is the wall's in-plane axis closest to up and its left is the wearer's
left, from the side the wearer is on. A level face keeps the box's own
axes, so the frame does not turn as the wearer walks around a table,
with u signed so the frame is not mirrored seen from above (or from
below, for a ceiling). Checked on the desktop GPU with a probe effect
that writes its uv (the frame's top left lands on the face's -u +v
corner) and with bezel, whose sweep arc sits at the top center of its
frame and lands at the top of an upright face.

**The uniform fill.** One `ShaderUniforms` (448 bytes) per lit ported
face per frame: this frame's `AudioFeatures` through
`mirror_audio_features`, as the desktop fills it; `time` the surfaces'
clock (so the motion runs faster on the beat, like the other surface
shaders); `delta_time` the frame's dt; `frame_index` the frame;
`resolution` the face's extents at 512 virtual pixels per meter, each
axis clamped to 256..2048, so the effect's aspect is the face's and a
2 m table runs at 1024 across; `params` the `.pfx` defaults as
`ParamStore` packs them, once at startup, with the rates integrated into
the slots after them every frame (aurora's curtain speed in slot 3,
prism's rotation in slot 7; the other six have none). One rate state
per effect, advanced every frame whatever is lit: two walls on aurora
drift in step. Everything else is zero.

One input does not run at its `.pfx` default: tessera's `scrim` is 0.
The scrim is a black cover at 0.85 over every tile not yet revealed,
which on the desktop hides the layer beneath and on a table would darken
the real surface under a rectangle. Without it tessera is its tiles'
strokes and embers, and the opening in the middle is the surface.

**The draw.** A pipeline per effect, built once at launch after the
assets are unpacked, with the surfaces pass's state (depth-tested, no
depth write, the bias, premultiplied over), and 32 slots shared by the
eight: per slot the effect's uniform, the port block and a group 1 bind
group per eye. Ported faces draw right after the pass's own faces and
count against the same 32. If the pipelines do not validate on the
device the ports stay off (logged) and a surface on one draws nothing,
since an invalid pipeline in the eye pass would cost the whole frame.

**The mapping.** Chosen by looking at each of the eight rendered through
the wrapper on the desktop GPU, on a 1.2 by 0.8 m face over a dim gray,
at four points of a bar on the synthetic features
(`FOSFORA_PORT_DUMP=<dir>` with the test above writes the frames). Each
effect is on two kinds' lists (other and unlabeled share one), no list
passes 8 entries, and the D2a entries keep their order in front, so the
defaults and the first taps are unchanged:

| kind | cycle |
| --- | --- |
| table | none, streamlines, curls, pulse, embers, sparks, shards, prism |
| floor | none, rings, streamlines, curls, sparks, tessera, prism, astrolabe |
| wall | none, spectrum, streamlines, rings, pulse, aurora, fenestra, bezel |
| ceiling | none, rings, pulse, streamlines, aurora, astrolabe |
| frame | none, pulse, rings, streamlines, bezel, reticle, fenestra |
| other, unlabeled | none, curls, streamlines, pulse, embers, shards, reticle, tessera |

- **aurora** (wall, ceiling): horizontal curtains stacked by band; they
  want an upright or overhead expanse wider than it is tall.
- **prism** (table, floor): a kaleidoscope about the face's center; it
  reads the same from every side of a level face.
- **shards** (table, other): Voronoi cells that fill a face of any size
  and aspect, with no up.
- **astrolabe** (floor, ceiling): one dial assembling ring by ring out to
  half the face's height; it needs room around the center.
- **bezel** (frame, wall): chrome along the face's edges, for what has
  edges worth framing. Its outer brackets sit 3.5 % of the face's height
  in from the edge, so on a face under 1.4 m high they fall inside the
  5 cm edge fade and dim (at 0.8 m to about 0.6 of their light).
- **fenestra** (wall, frame): panels with a header bar along their top;
  the one effect that needs an up.
- **reticle** (frame, other): small marks that take a new place every
  bar; they read on a small face.
- **tessera** (floor, other): a grid of tiles opening from the center; a
  floor's tiles.

The knob puts any of the eight on any kind.

**Excluded.** `iris` and `beam` read the previous frame; `limn` needs
the backdrop under it; `intarsia` is two passes; every other effect is
multi-pass or a particle system. None fits one draw on a face with the
uniform alone.

**Logs.** "surface ports: 8 pipelines in N ms" at launch (or "surface
ports: off: ..." with the reason); "surfaces: N lit (rings a,
streamlines b, curls c, pulse d, spectrum e, ports f)" on every change.

**The sweep** (reviewer). Every kind on a ported effect of its own list,
all eight between the two values, each within the 91 bytes a property
holds:

```
adb shell setprop debug.fosfora.surface "table=shards,floor=tessera,wall=aurora,other=reticle,frame=bezel,ceiling=astrolabe"
adb shell setprop debug.fosfora.surface "table=prism,floor=astrolabe,wall=fenestra,other=tessera,frame=reticle,ceiling=aurora"
```

**Not yet.** The device: the launch time of the eight pipelines, the
cost rows in `MEASURED.md` and the worn gate are the reviewer's. A
tint from the lane's color index, and the effects' parameters from the
panel (D3).

## D2c: the surfaces in their own layer (Oct 9, board #3793)

**Why.** The worn A/B (`MEASURED.md`, "Worn A/B of the frame cost", and
board #3791) put the cost of a lit room surface in the face draw itself,
per covered eye pixel, whatever shader runs on it: three aurora walls and
a floor cost 3.0 ms worn at 640 MHz, and the built-in pulse and rings on
the same faces 1.4 ms of a 2.2 ms unworn delta. A face covers a large
share of the view and is smooth light, so fewer shaded face pixels is the
lever, and the compositor's upsample is nearly free for it.

**What moved.** Draws 4 and 5 of the eye pass, the surfaces pass's slots
and the ported effects, leave the eye pass for a second per-eye
`R8G8B8A8_SRGB` swapchain at `round(recommended x facescale)` (default
0.5: 840x880 per eye on a Quest 3), created and wrapped as the eye
swapchain is, with its own `Depth32Float` depth. Per eye, ahead of the
eye pass, the "xr-faces" pass clears color to transparent black (stored)
and depth to 1.0 (discarded), then draws the env-depth occluder, the
particles' occluders (room boxes, then the hand mesh or joint spheres)
and the two draw groups, with their pipelines and depth state unchanged.
Nothing else moves: the highlight, the sprites, the cloud, the ghosts,
the beams, the panel and the label stay in the eye pass, which keeps its
own occluder draws so the sprites still hide behind real surfaces and
hands. `Gfx::draw_faces` is the one draw code for the two groups; the
knob only picks the pass it runs in. With nothing lit the faces pass is
only its clear.

**Submission.** `xrEndFrame` takes `[passthrough, faces, main]`, bottom
first. The faces layer is a projection layer with the same pose and fov
per view as the main layer, its sub-image rect 0,0 to the faces extent,
array index 0, `BLEND_TEXTURE_SOURCE_ALPHA`. Its images are acquired and
waited beside the eyes' and released after the render. The runtime
composites it at full resolution over passthrough and under the cloud.

**The knobs** (read at launch). `debug.fosfora.faceslayer 0|1`, default
1 in world and mr: 0 keeps the two groups in the eye pass, for the worn
A/B (`scripts/xr/worn-ab.sh`, condition `layer0`). `debug.fosfora.facescale`,
0.25 to 1.0, default 0.5. `debug.fosfora.facesharpen 0|1`, default 0:
`XR_FB_composition_layer_settings`' `NORMAL_SHARPENING` chained on the
faces layer (the extension is enabled whenever the runtime offers it; the
flag is chained only when asked). Without passthrough (the quad and
particles test modes) no faces layer is created and the surfaces draw in
the eye pass as before, whatever the knob. The launch log says which:
`faces layer: 840x880 per eye (scale 0.50) · <n> images · sharpen off`, or
`faces layer: off (...)`.

**Depth.** The faces pass rebuilds the occlusion the surfaces were tested
against in the eye pass: the env-depth occluder at the faces target's
size, and the particles' occluders. The occluder's fragment-to-NDC step
reads `EyeReprojection::target_size`, so `EnvDepth` keeps a second
uniform set per eye (the WGSL is unchanged), written only while the
faces layer exists, and its bind groups per depth image become per
target and per eye (`OccluderTarget::Eye`, `OccluderTarget::Faces`). The
camera uniforms do not depend on the target size and are shared. The
surfaces' depth bias (`SURFACE_DEPTH_BIAS` -16, `SURFACE_DEPTH_SLOPE_BIAS`
-2.0) was tuned at full resolution and is kept. The constant term is in
the depth format's units, not pixels, so it means the same at any size;
the slope term multiplies the depth slope per window pixel, which doubles
at half size, so the same face is pulled toward the camera by up to
twice the slope term's depth there. The occluders' edges are resolved at
the faces layer's resolution, so a hand's or a desk's silhouette over a
lit face is half as sharp as the cloud's.

**Alpha.** The faces are premultiplied light blended over the pass's
transparent clear, exactly as they were over the eye pass's clear (also
transparent black over passthrough, except the two white flash frames),
so the faces layer over passthrough is the same light as before. The main
layer is then composited over it by its own alpha. Where the cloud's
sprites or the panel cover a face, that differs from the single pass
only for the additive sprites: they accumulate alpha, so a face under
them is now scaled by one minus that alpha before the sprite's light is
added, where before the light was added on top of the face. The
alpha-blended sprites and the panel give the same result either way.

**The cost to measure.** The faces pass's shading drops to the faces
layer's pixel count (a quarter at 0.5); its clear, its occluder draws
and the second layer's composition are new. The app GPU time (`App=`)
should fall; the compositor's (`TW=` on the runtime's frame line, which
`worn-ab.sh` now reports as `tw_med` and `tw_p90`) should rise by the
second layer's composition. `full` against `layer0` in one sitting
answers it, with `facescale` and `facesharpen` as follow-ups.

## D3 as built (Oct 8, board #3472)

**The writer.** `RoomLanes::set_params` (`lanes.rs`) is the panel's
path to the fields the knob's `<target>=<behavior>:<color>:<band>@<strength>`
writes: a box index (the pointed surface) and a `ParamEdit` of an
optional color, band and strength, `None` leaving a field alone. A
surface without an entry first gets one at its effective behavior and
strength (the room file's `set_color` writes nothing without an entry),
then the params, through the same write, save and log as every
assignment ("room <id>: DESK 11a1a2a3 (table) -> streamlines@1.00 color
4", the revision bumped). The color is a whole number 0 to 8, the band 0
to 3, the strength 0..1, stored at one decimal; anything out of range,
or a box past the frame's, refuses the edit whole with one warning, as
a bad knob value does. A strength alone leaves unset params unset; a
band goes in with the color the surface shows, as the file keeps the
two together. An edit to the values a surface already has writes
nothing. `params_of` reads a surface back as the pass does (its lane
row through `lane_params` and the row's strength), so the panel shows
what the face shows: an unset color is 0, an entry without params runs
its behavior's default band. The panel and the knob write the same
fields; the knob's grammar is unchanged.

**The rows.** With Edit room on, the hand menu and the debug panel's
menu block gain three rows under the editor's: `<` and `>` at the ends,
"Color  amber", "Band  mid", "Strength  0.7" between (`surfaces::color_name`:
kind, blue, violet, warm white, amber, green, teal, rose, key;
`surface_fx::band_name`: rms, bass, mid, high). They act on the surface
under the beam: the editor holds its hit while the panel is up, so the
wearer points at the desk, turns the left palm up and steps it, and the
right hand's pinches are the panel's, never an editor tap. Color and
band wrap (the key's tint is one step below the kind's own); strength
moves by tenths and clamps at 0 and 1, and a held Strength `<` or `>`
repeats like a stepper; color and band do not. With no surface under
the beam the first row reads "Point at a surface" over two empty rows,
nothing presses, and the menu keeps its height. The press is a step,
not a value: the app applies it after the frame's boxes exist, as All:
none, from the file as it is then, and on a change the label at the hit
says "desk: curls · amber · mid · 0.7" (`label::param_text`). The
status cell is unchanged. The values the rows paint are one frame late
after a press (the panel renders before the editor steps).

**The layout.** The menu's row count follows Edit room: 4 rows or 7
(`panel_grid::menu_rows`, `menu_h`: 204 or 324 points), the debug
panel's 11 or 14 (`debug_rows`), 14 being the headroom its header
leaves. No row or font shrinks. The quad follows the state the last
render laid out and keeps its bottom edge, so it grows upward when Edit
room turns on and the rows under the pointer stay put. (Amended Oct 20,
board #4192: the menu is 7 rows or 10, `menu_h` 324 or 444 points, and
the debug panel's 12 or 15. Three rows joined the menu over Pitcher and
Debug since: the world effect's `<` `>` on top (board #3336), Recenter
and Rescan under it (board #4192, the debug panel's row until then, so
its count held) and the Agent's under those (board #3776, one more in
the debug panel). 15 is the headroom the debug header leaves now.)

**What each shader does with the three.**

| behavior | color | band | strength |
| --- | --- | --- | --- |
| streamlines, curls | the light's color | the level under the flow | scales the light |
| pulse | the glow's color | the envelope's floor | scales the glow |
| rings | the rings' color (0: the ripple's warm white on a floor) | not read: the ripple's own bass, by design | scales the light |
| spectrum | the bars' color (0: the canvas's warm white) | not read: the bars are every band, by design | scales the light |
| the eight ports | a tint (0: the effect's own colors) | not read: the effect's own audio bindings | scales the light |
| embers, sparks | not read | not read | scales the sim's gate |

**The port tint.** The port block's `color.rgb`, written white and never
read in D2b, carries the tint: white for index 0, so the default look is
bit-identical to D2b's (a multiply by 1), else the color the index picks
(`surface_port::tint`, the same `surface_color` the pass's own faces
resolve through `color_of`, the key's tint included; the key's tint is
now computed when a ported face asks for it too). The wrapper multiplies
the effect's rgb by it after the coverage is taken, for overlays and
opaque effects alike, so a tint never changes an alpha: an opaque
effect's luminance coverage is its untinted color's, and a tinted effect
covers the surface where it did, in the tint's color. A desktop GPU run
of the eight pipelines with the tint passes (`cargo test -p fosfora-xr
--lib surface_port -- --ignored`).

**Logs.** "edit room: desk color up (the panel)" for each press, then
the lanes' "room <id>: ... -> curls@0.70 color 4 band 2, saved
rooms/<id>.json" on a change; "edit room: color up with no surface
under the beam, nothing changed" without a hit; "room <id>: color 9 is
not from 0 to 8; nothing changed" for a refused edit (none reachable
from the panel).

**Not yet.** Speed and density per surface: the room file's `params` is
exactly two numbers, and widening it is a version 3 with a migration, a
later step. Parameter edits for a whole kind (a hold on a row, or
similar). The worn gate (point at the desk, step it to amber on the mid,
lower the strength, relaunch and find it kept), the screencaps of the
rows and the cost of tinted ports against untinted in `MEASURED.md` are
the reviewer's, on the device.

## Immersion (Nov 12, board #5384; reworked, board #5409)

**Why.** The lit surfaces were light at half coverage at most over a
passthrough image at full brightness, so the room always won and the
surfaces looked washed out. Nothing set a passthrough style. Now the room
gives way to the music: it shows while the song is quiet, it goes dark
while the song is full, and the surfaces glow at their own colors then.

**Why the rework.** Worn seated for 13 minutes with real songs, the first
version's dimming was there but not visible as it happened. In the log of
that sitting the level was over 0.8 for 721 of 789 s, between 0.2 and 0.8
for 55 s and under 0.2 for 13 s. `loudness_s` averaged 0.79, and the first map was
full at 0.75. Mastered music is loud all the way through, so an absolute
map saturates: the room went dark in the first seconds and stayed dark.
Its constants came from the bundled clip, which reads flat for its whole
length. The rework follows the song against its own recent range, with
constants chosen from signal dumps of real songs (`--signal-dump
--feat-bus`); the tests pin the shapes with synthetic traces.

**The opening** (`immerse::Opening`). At launch the level is 0: the room
fully visible. When real audio first drives the analysis (the music's tap
or the microphones, heard: `loudness_m` over the quiet threshold) the
in-world label says "Sit back and relax" for `OPEN_HOLD_S` (4 s) with the
room still plain, then the level glides to `OPEN_LEVEL` (0.35) over
`OPEN_GLIDE_S` (3 s), a smoothstep. That is the start of the show; from
there the level follows the song from 0.35, blending into the followed
level as the history fills. It plays once per launch. If the music stops
during the message it starts over at the next play; after it, a stop, a
silence or a pause of the session brings no message, and the level
glides from where the room is to the song's level in about 2 s
(`RESUME_S`: the time constant is a third of it). On the synthetic groove
nothing shows and the room stays plain until the music first plays.

The message is the label's last choice. It shows where the scan label
does, 1.2 m ahead at eye height, only while no other label is up: the
voice's, then the editor's, then the scan's and the permission's ("Allow
spatial data to see the room"). It waits while any of them shows, and its
4 s hold counts only while it is the one shown, so a scan that ends with
"Room: 17 surfaces" plays out first and a fist mid-message covers it
without eating it. `debug.fosfora.immerseopen 0`, a pinned level
(`immersefix`) and a launch without passthrough skip it; the music's
start then glides to 0.35 in about 2 s. At depth off there is no message
and no dimming; at low the opening settles at 0.175.

**The level: the song against itself** (`immerse::History`, `position`,
`follow`). The history keeps `loudness_s` at `HISTORY_HZ` (4 Hz) over
`WINDOW_S` (30 s), recorded only while the song is heard and from
`LOUDNESS_WARM_S` (3 s) after it came back: the 3 s window reads low until
it holds the song again. Its range is the 10th and 90th percentiles, so a
few seconds of a spike or a dip do not stretch it. Where the song is in
that range is the position:

- The range is at least `MIN_SPAN` (0.05, 3 dB) wide, widened downward
  from its high, so a flat song sits at the top of its range and parks
  closed, and the noise within a section is a small share of the span.
- The top `TOP_SHARE` (a quarter) of the range counts as full, so the
  wobble within a full section keeps the room closed.
- The position maps onto `LOW_LEVEL` (0.42) to 1. A breakdown falls under
  the range's low, which follows it down within about 3 s, so the song
  sits at the bottom and, after the release, the room is about half
  dimmed, for as long as the full section's high stays in the window
  (27 s). The first rework had 0.2, which brought about three quarters of
  the room back; worn, that was too bright. 0.42 is over the opening's
  0.35, so the music never shows the room as much as the opening does,
  and the blend from 0.35 as the history fills only darkens.
  `debug.fosfora.immerselow` sets it live for tuning.
- `buildup` over `BUILD_FROM` (0.4) adds up to `BUILDUP_LIFT` (1) to the
  level, scaled by the position. Measured on real songs, `buildup` is high
  wherever the bass is out, in breakdowns as much as before drops, and
  added as the first version did it held breakdowns closed. Scaled by
  the position it adds nothing at the bottom of the range and closes the
  room in on the climb out of a breakdown toward the drop.
- A `drop` holds the target at 1 for `DROP_HOLD_S` (8 s), reached with
  `DROP_ATTACK_S` (0.25 s), as before.

One pole smooths it: `ATTACK_S` 2 s rising, `RELEASE_S` 4 s falling, the
first version's values. On the real songs they open a breakdown well
before its middle, let a short dip before a drop open the room only part
way, and close it within a few seconds of the full section's return. Nothing forces movement: a song that stays full
stays closed. At launch the history is empty, so the target blends from
0.35 into the followed level as it fills over `HISTORY_FILL_S` (15 s). On
a track change (the player counts tracks put on the speakers from their
top; a resume of the same track is not one) the history carries over,
but the range takes only the samples since the change, and at least the
latest `TRACK_ADAPT_S` (4 s), until the window is the new song's: a
quieter song after a loud one closes again within 12 s in the tests
instead of sitting open for half a minute.

**Quiet.** The level follows the song only while real audio drives the
analysis. Stopped music, or `loudness_m` under `QUIET_LOUDNESS` (0.15,
−51 LUFS, 4 dB over the analysis's silence gate) for 0.3 s, brings the
room back: the level falls with 0.35 s and the room is back within about
1.35 s. The first version read the `rms` under 0.02, but the `rms` is
auto-leveled and dips under any fixed threshold between hits inside real
music, so it snapped the room open mid-song. `loudness_m` under 0.15 for
0.3 s happens in the dumps only before a song starts and after it ends.

**The room.** Each frame the passthrough layer's opacity is
`1 - level * (1 - floor)`, through `xrPassthroughLayerSetStyleFB` with
the edges off. The seated floor (`IMMERSE_FLOOR`) is now 0: level 1 at
full depth is a black room behind the visuals. It stays a field of the
type, and roam keeps its own floor, 0.5, and its walk rule. The `openxr`
0.22 wrapper has no style call, so `room.rs` calls the extension's
function pointer. The call goes out after the frame, only when the
opacity moves by a 1/64 step, the first one only once the level leaves 0,
so a run that stays quiet makes no call and looks exactly as before. The
passthrough layer is the bottom of `[passthrough, faces, main]` (D2c) and
the blend mode is OPAQUE, so what shows under a dimmed image is the
compositor's black. While the image is dimmed the passthrough layer goes
out with `BLEND_TEXTURE_SOURCE_ALPHA`, so the composition reads its
opacity. At full opacity its flags stay empty, as they have been since S7.

**The hands at opacity 0.** The app draws no hands. The hand occluders
(the runtime's skinned hand mesh, or joint spheres with
`debug.fosfora.handmesh 0`) write depth only, with color writes off, over
a frame cleared to transparent, so where a hand is the passthrough image
shows: the wearer sees their own hands in the camera image. At opacity 0
that image is black. Against the dark room the hands vanish entirely;
in front of the particles, a lit surface or the hand menu they show as
black silhouettes cut out of the light. The hand menu itself, its cursor,
the beams and the labels are the app's own quads and stay visible. With
the old floor the hands kept 15 % of the image. Worn, seated, the hands
at opacity 0 were fine.

**The surfaces.** Every lit face's peak alpha rises with the same level.
A port's goes from its 0.5 to 1.0, `0.5 + 0.5 * level`. The built-in
behaviors rise by the same ratio, capped at 1: the streamlines' and the
curls' 0.55 reach 1.0, the pulse's 0.45 reaches 0.9, the rings' and the
spectrum's 0.25 times their gain reach 0.5 times it. One helper
(`immerse::lift_rows`) applies it to the `color` row of both blocks, so
the constants stay the level-0 values and level 0 is bit for bit the old
look. The lane's strength still multiplies it. Without passthrough
(the quad and particles test modes) the level is taken as 0: there is no
room to dim.

**The depth.** How far the wearer lets it go: off (0), low (0.5) or full
(1), full by default. It multiplies the level, the opening's included,
and a change glides over 0.5 s. The hand menu's Agent row holds it in its
right cell, "Immersion: full", each press the next of off, low and full;
a new row would push the debug panel's fullest block past its header. It
is saved in `hand_menu.json` as `"immerse"`. By voice, "darker" and
"lighter" step it ("dim the lights", "lights down", "lights up", "more
immersion"), "lights on" sets it off, "lights off", "lights out" and
"full dark" full, and "immersion low" (or off, full) sets that depth. The
label says where it came to: "immersion full". "darker" alone is the
room's now; "the desk darker" still dims the desk. The agent and the
on-device model have no immersion action: the model's kinds are a closed
set.

**A song plan, not built.** `immerse::SongPlan` (a range and a sorted
list of timed cues: build start, drop, breakdown start and end) is the
seam for a pass over the whole track before it plays. The step takes
`Option<&SongPlan>` and the track's position; a plan's range replaces
the history's. Nothing makes a plan yet, nothing reads the cues, and
`app.rs` passes none.

**The knobs** (read at launch). `debug.fosfora.immerse 0..1` is the depth
for this launch, not saved; a press takes over. `debug.fosfora.immersefloor`
sets the seated floor (default 0). `debug.fosfora.immersefix 0..1` pins
the level whatever the audio and the depth, and skips the opening, for
unworn screencaps (on this OS version `adb exec-out screencap -p`
includes the passthrough behind the app's layers).
`debug.fosfora.immerseopen 0` skips the opening.
`debug.fosfora.immerselow 0..1` sets the low level; it is also polled
once a second, so it can be tuned while worn, and a change is logged
(`immersion: low level 0.45 (debug.fosfora.immerselow)`; unset again,
`immersion: low level 0.42 (the default)`). The level moves to a new
value over the attack or the release.

**Measurement runs.** A launch with the music on now spends about 7 s on
the opening and fills the history for about 15 s more: on the bundled
clip, a flat loud master, the level reaches 0.96 20 s after the music
starts and 1 by 30 s, where the first version was at 1 by 7 s. `worn-ab.sh` (20 s warmup) and
`gpu-stages.sh` (25 s) start capturing after both; `sweep.sh` in mr or
world mode with the music on measures from 5 s and sees the ramp. For a
level that matches the first version's runs, set `immersefix=1` through
their `--with` or `--set`; `immerseopen=0` alone saves the 7 s but not
the fill.

**Logs.** At launch: `immersion: full · floor 0.00 · opening "Sit back
and relax" 4.0 s, then 0.35 over 3.0 s · follows loudness_s within its
last 30 s (p10..p90, span 0.05 at least) onto 0.42..1 · attack 2.0 s ·
release 4.0 s`, with the knobs (the low level's as "onto 0.45..1
(debug.fosfora.immerselow)"), a skipped opening ("no opening
(debug.fosfora.immerseopen 0)"), the pinned level and a missing
passthrough said when they apply. When the message goes up:
`immersion: opening, "Sit back and relax" for 4.0 s, then 0.35 over 3.0
s`; at depth off `immersion: no opening, the depth is off`. When the
glide ends: `immersion: the show starts at 0.35 · history 4.0 s`. On a
drop: `immersion: drop, to 1 for 8.0 s · loudness_s 0.81 in 0.62..0.80`.
On a change of the depth: `immersion: low (depth 1.00 -> 0.50)`, then the
menu file's save line. Once a second while the room is dimmed, and once
when it is back: `immersion: level 0.73 · passthrough 0.27 · loudness_s
0.74 in 0.70..0.80 at 0.53 · buildup 0.30 · depth 1.00`, the range and
the song's position in it, and "opening: the glide" while it runs. A
failed style call warns once.

**Worn.** The rework passed worn, seated, with real songs: the opening
played, the level moved with the song, the black room at level 1 was
good, and the hands at opacity 0 were fine. The one change asked for was
a darker open state, hence the low level at 0.42 and its knob.

**Not yet.** The device: the worn verdict on 0.42, the label order at
launch with a permission prompt, the cost of the style calls, and the
microphones, whose quiet room may sit over the quiet threshold, so a
stopped speaker leaves the room at the low level instead of fully back.

## Themes (Nov 13, board #5415)

**Why.** There are 16 surface behaviors and a list per kind of surface,
but the wearer picks them one surface at a time, in the room editor or by
voice. Nothing changed the whole room's look at once, so most of what
exists was never seen. A theme sets the whole room in one press, and it
wins over what was assigned by hand. The app is used seated in a scanned
room first, so each theme has to look complete there.

**What a theme is** (`themes.rs`). A name and, for each kind of surface
(table, floor, wall, ceiling, frame, other), a behavior, a color index, an
audio band, a strength and, since board #5431, how the edge bars draw that
kind's edges. The module is plain data: the table, lookup by
name, the next theme in the cycle, what a room file shows, and the knob's
launch rule. No theme adds a shader or a behavior. Every pairing below is
on its kind's own list (`SurfaceBehavior::catalogue`), so none of the
pairings first proposed needed changing.

**The five.** Each cell is behavior, color, band, strength. The first
four draw the edge bars on the face for every kind; Mirror's edge mode is
in its cells.

| kind | Classic | Drift | Tide | Aurora | Mirror |
| --- | --- | --- | --- | --- | --- |
| table | streamlines, own (blue), rms, 1 | streamlines, teal, rms, 1 | curls, rose, rms, 1 | streamlines, teal, rms, 1 | none, blue, rms, 1, edges in |
| floor | rings, own (violet), bass, 1 | rings, blue, bass, 1 | curls, violet, rms, 1 | streamlines, green, rms, 1 | rings, blue, bass, 1, edges on the face |
| wall | spectrum, own (warm white), rms, 1 | streamlines, blue, rms, 1 | pulse, violet, bass, 1 | aurora, the key's tint, rms, 1 | none, warm white, rms, 1, edges in |
| ceiling | none, own (amber), rms, 1 | rings, blue, bass, 1 | pulse, violet, bass, 1 | aurora, green, rms, 1 | none, warm white, rms, 1, edges in |
| frame | pulse, own (amber), bass, 1 | pulse, teal, bass, 0.6 | rings, rose, bass, 1 | pulse, teal, bass, 1 | none, blue, rms, 1, edges in |
| other | curls, own (green), rms, 1 | curls, teal, rms, 1 | pulse, rose, bass, 1 | curls, teal, rms, 1 | none, blue, rms, 1, edges in |

Classic is today's defaults (`default_for`), each kind's own color, so
applying it gives exactly a fresh room. It is the only theme with the wall
spectrum, since the edge bars carry the spectrum on every edge in every
theme. Drift is the calm one. The other three color by one rule: the
room's planes (walls, floor, ceiling) take one color and what stands in it
(tables, frames, other surfaces) the other, so a table reads against the
floor and a frame against its wall. Aurora puts the key's tint on the
walls and gives the floor and the ceiling green. Every band is the
behavior's own, the rms for the flows and the bass for the rings and the
pulse, which is Drift's rule as asked; picking a band per theme waits for
signal dumps of real songs. Every strength is 1 except Drift's frames at
0.6, because the pulse jumps on the downbeat and is the one restless look
in the calm theme. The rings and the ported Aurora do not read the band
(D3's table), so it only matters there if a hand edit later changes the
behavior.

**Mirror** (board #5431). The owner wore the edge bars and the themes and
saw every theme draw the same edges, so a theme now says what the edges do
per kind of surface: off, on the face, standing or in (`edge_bars::Mode`,
`XR_DESIGN.md`, Edge bars). Mirror is the theme the edges carry, after
Aurora in the cycle. Walls, the ceiling, tables, frames and other surfaces
run no behavior and their edges go in: each face a window into a tunnel of
rungs, the LED infinity mirror the owner asked for. From the chair the
walls and the ceiling fill the view, so that is where the tunnels are. The
floor keeps the rings with the bars on its face, as a pit under a seated
wearer's feet is left for the owner to ask for. Warm white on the walls and
the ceiling, blue on the rest, the floor's rings included.

**The edges per kind.** The edge bars draw each shape in its kind's mode
under the room's saved theme (`themes::edge_mode`), edited since or not: a
hand edit changes behaviors and colors, not the theme's edges. A room with
no theme saved, fresh or custom, draws every kind on the face. The mode
comes from the saved name alone, so nothing new is written to the room
file. A build without Mirror reads a room saved on it as one naming an
unknown theme: no theme, Mirror's looks kept, every kind on the face. A
live plane takes its kind's mode, a hidden wall and the stage floor
are still not outlined, and an unlabeled anchor draws on the face. Faces on
the face and standing are both strips, so a frame of mixed modes is one
buffer write and at most two draws per eye, the strips and the tunnels.

**The Edges control over the theme.** The hand menu's Edges control (the
right half of the cloud row's right cell) now steps theme, off, face,
stand, in, and back to theme. "Edges theme", the default, follows the
theme; each of the others forces that mode on every kind. The knob
`debug.fosfora.edgebars` does the same: unset follows the theme, 0 to 3
force (0 off, 1 face, 2 stand, 3 in). The control's choice holds until
the knob's value changes, as before, and neither is saved: a relaunch
follows the theme again unless the knob is set.

**The theme wins** (`RoomLanes::apply_theme`). Applying a theme sets every
kind default and every surface of the room to the kind's look, hand
assignments included, through the lanes' one writing path, in one save
and one log line, as All: none does. Every entry of the six kinds takes
the look whole, and every surface in the room without an entry gets one
(the stage floor under its zero UUID). A live plane never gets an entry,
so it follows the theme through its kind's default, which therefore now
carries a strength and params besides its behavior. A hidden wall is a
wall: it gets the walls' look and is still not drawn. An unlabeled anchor
has no look and is left as it is, as All: none leaves it. A look whose
color is the kind's own and whose band is the behavior's own writes no
params, so Classic leaves the room exactly as a fresh one, and a behavior
changed by hand afterwards runs its own band as it would without the
theme.

**What is saved** (`room_file.rs`). The room file stays version 2 and
gains two optional fields: `theme`, the name of the theme last applied,
and `kind_looks`, the strength and params of each kind's default where a
theme set them. Both are written only when set, so a room that never had
a theme saves as it did. A new entry starts from its kind's look, so an
edit by hand after a theme changes only what it names.

What the control shows is worked out from the file each time, not kept as
a flag. A room whose kind defaults and entries all run the saved theme's
looks shows the theme. Once something was edited by hand it shows the
theme as edited ("Drift*"), and the edit and the saved name both stay.
Undoing the edit unmarks it, and a reload shows the same. A room saved
before themes has no theme: it shows Classic when it runs Classic (no
entries and the built-in defaults, or entries that equal them) and custom
otherwise, and nothing changes until the wearer picks. This is the
brief's rule of custom with entries and Classic without, except that a
room whose entries equal the defaults shows Classic, and one with no
entries but changed kind defaults shows custom. Older files (version 1,
and version 2 from before themes) read as before. A file written now also
reads in the build before themes, whose reader ignores fields it does not
know: the kind defaults and the entries, with their params, read as
written, so every scanned surface keeps its look, and only the name and
the kind looks are lost until that build saves the room.

**The control.** "Theme" over the theme's name ("Drift", "Drift*",
"custom") in the right half of the music row's right cell, with Voice in
the left half, in both layouts (debug panel on and off). Each press steps
to the next theme (Classic, Drift, Tide, Aurora, Mirror, Classic; from
custom, to Classic) and applies it once the frame's boxes are in, as All: none does.
It works with the ray and the pinch like every other cell. Adding a row
would push the debug panel's fullest block into its header, and every full
cell holds a control the wearer reaches for, so the control shares a cell
as Recenter and Rescan do. Voice fits a half at the button's 16 points (68
of 90 points) and is a saved setting, rarely pressed. "Theme: Aurora*" on
one line needs 113 points at 16, and at 12 it would leave 3 points a side,
so the half shows the word over the name, as a stepper's label sits over
its value, with the name at 16 points like every other control. No row
count or height changed.

**By voice** (`intent.rs`). "theme drift", "drift theme", "set the theme
to aurora", "change the theme to tide", "use the classic theme" name a
theme. "next theme", "another theme", "change the theme" step it as a
press does. The reply names the theme it came to ("theme: Aurora"). A name
no theme has replies with the list: No theme called "sunset": Classic,
Drift, Tide, Aurora or Mirror. "mirror theme" and "set the theme to
mirror" work through the same grammar, with no new phrase. The word
"theme" has to start or end the sentence,
so "drift" or "aurora" alone keep meaning what they did. The agent's
actions have no theme, and an unknown theme is not forwarded to it.

**The knob.** `debug.fosfora.theme <name>` (classic, drift, tide, aurora,
mirror) is polled once a second after the lanes' update and applies that theme
when its value changes, once. At launch, a value naming the room's own
theme, edited or not, applies nothing, so a value left over from an
earlier run neither rewrites the room nor wipes an edit. Like the menu, it
writes the saved room. To apply the same theme again, set it to "" first.
It waits for the room's anchors as the surface knob does. To measure each
theme's cost in one run:

```
adb shell setprop debug.fosfora.theme classic   # then capture
adb shell setprop debug.fosfora.theme drift     # then capture
adb shell setprop debug.fosfora.theme tide      # then capture
adb shell setprop debug.fosfora.theme aurora    # then capture
adb shell setprop debug.fosfora.theme mirror    # then capture
adb shell setprop debug.fosfora.theme '""'
```

or one launch per theme with `scripts/xr/gpu-stages.sh --with
"music=1;gov=0" --cond "classic=theme=classic" --cond "drift=theme=drift"
--cond "tide=theme=tide" --cond "aurora=theme=aurora" --cond
"mirror=theme=mirror" --only "classic,drift,tide,aurora,mirror"`. Back up
`rooms/` first: the room ends on the last theme.

**Logs.** Loading a room: `room <id>: theme drift`, `theme drift, edited
since`, `no theme saved: classic (the defaults)` or `no theme saved:
custom (4 entries)`. Applying one: `room <id>: theme drift (the menu):
every kind default and every surface (18 in the room, 1 of them live
planes on their kind's default, 17 entries) -> table streamlines@1.00 teal
rms · floor rings@1.00 blue bass · …, saved rooms/<id>.json`, the source
being the menu, voice or the knob; a kind whose edges are not on the face
adds them, `wall none@1.00 warm white rms edges in`. The knob also logs a
launch value it kept and an unknown name. The edge bars log the mode per
kind whenever it changes: `edge bars: per kind table in · floor face ·
wall in · ceiling in · frame in · other in (theme mirror)`, or `(no theme
saved)`, or `(forced on every kind)`.

**Not yet.** The worn look of each theme seated in the scanned room, the
screencaps of the control, and each theme's cost in `MEASURED.md`.
Aurora should cost the most: the ported Aurora on every wall and the
ceiling (1.2 to 1.5 ms per full view of one face in D2b) and the
streamlines on the floor (1.7), the biggest faces all on costly shaders.
Every face on a ported effect cost about 2.9 ms in D2b, so expect about
that. Drift comes next (streamlines on every wall), then Tide (the cheap
pulse on the walls and the ceiling, the curls on the floor), then Classic
(about 1 ms, as in D2a). A theme does not pick the world effect. Since
board #5431 it does pick the edge bars' mode per kind; Mirror's worn look
and its cost seated are still to take.
