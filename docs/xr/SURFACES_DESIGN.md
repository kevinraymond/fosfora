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
"Loading your room" for 3 s when one does (until board #4204 it read
"Room: 17 surfaces"; the count is now in the log only), "No room found. Rescan from
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

## The sky (Nov 13, board #5436)

**Why.** The owner wore the five themes and picked Aurora as the look to
work on: to lie back and be immersed. Seated, the walls and the ceiling
fill the view, and Aurora was drawn once per face: every wall and the
ceiling ran its own copy of the picture, so the curtains stopped at each
corner and started again and the room read as panels. Now Aurora on the
walls and the ceiling is one sky that wraps the room, and the floor
reflects it. `sky.rs` holds the plain-number side and the shader
(`SKY_WGSL`), and `surface_port.rs` composes Aurora with it.

**The mapping.** On a wall or the ceiling Aurora no longer maps the
face's (u, v) to its frame. The fragment's world point is taken as a
direction from one fixed point in the room: its azimuth about the point
runs along the frame's x and its elevation up the frame's y. Every wall
and the ceiling then show one continuous picture: the curtains run
around the room and rise onto the ceiling, where the azimuth converges
above the point. Aurora alone is composed with the sky's stages
(`vs_sky`, `fs_sky`); the vertex stage also hands on the world point and
the eye, solved from `view_proj` as the tunnel does. Its per-face path
is `fs_main`'s, and the other seven ports are composed and drawn as
before.

**The point** is fixed in the room, not the head, so the sky does not
swim when the wearer moves: the center of the scanned walls in plan at a
seated eye height, 1.2 m over the floor (kept 0.3 m under the ceiling).
The walls' footprints (each one's center line, hidden walls included,
since they close the room's outline) are fit with a rectangle turned to
the longest wall, and its center is the point. The floor is the scanned
floor's top, else the walls' foot; the ceiling the scanned ceiling's
underside, else the walls' top. Without a scanned wall (roam, live
planes alone) the point is the stage floor's center, which is the
stage's origin seated and follows the head in roam (it re-centers 3 m
away), and the room is a 5 m square around it with a 2.5 m ceiling. Live
planes never move the point: they come and go as the wearer looks
around, which is why the space fit leaves them out too.

**The seam.** The effect's noise is not periodic in x and the core's
shader is unchanged, so the turn has one seam. It is behind the seated
wearer: the frame's middle column is the direction the wearer faced at
launch (the first frame) or at the last recenter (the hand menu's
Recenter, or "recenter"), leveled, and the frame's two ends meet
opposite it. It stays there until the next recenter. Within 0.2 rad
(11.5°) of the seam on either side the effect is evaluated a second time
at the other end's continuation and the two cross-fade, half and half on
the seam: 23° of the turn, behind the wearer, cost two evaluations.

**The frame's shape.** Elevation maps to y linearly, from the horizon at
the frame's bottom row to 1.1 rad (63°) at its top: what a 2.5 m wall
spans seen from 2 m, so the curtains keep about the proportions they had
on one wall. The frame is a whole turn wide at the same 1024 pixels per
radian (6434 by 1126), so at the horizon a degree across is a degree up.
At the `.pfx`'s band spread the bands sit at -2°, 9°, 20°, 31°, 43°, 54°
and 65° of elevation, brilliance lowest and sub-bass highest, as on a
wall, where the frame's bottom is the floor. With the walls 2 m from the
point and a 2.4 m ceiling, brilliance, presence and upper-mid run around
the walls from eye level up, mid along the ceiling's edge, and low-mid,
bass and sub-bass ring the ceiling.

**The zenith.** Overhead the azimuth converges: a frame row shrinks to a
point there and the noise would pinch and alias. The light fades out
between 74° and 84° of elevation, above the highest band's reach (65°
plus its 7.6° of wave), so the rings close in on a dark center: on a
ceiling 1.2 m over the point, dark within 0.13 m of the point's zenith
and back to full 0.34 m out. At 84° the finest detail (the shimmer, 10
to a frame height) is still 13 display pixels a period at 20 pixels a
degree, so nothing aliases where light is left. A fragment with no light
left (the dark center, and on the floor the reflection's dead zone)
returns without calling the effect.

**The rim.** The surfaces pass fades every face out over its last 5 cm.
Two walls in a corner both faded there, which left a dark line down the
corner, and a wall and the ceiling did the same along the ceiling's
edge. On the sky's faces an edge another of them continues (its middle
within 10 cm of another sky face) drops the fade. An edge with nothing
beyond it (a wall's foot, the side of a hidden wall) keeps its extent
and the fade.

**The joins** (board #5438). Each quad is lifted off its face and runs on
into its neighbors' slabs, so two quads would draw the same sky twice
along a corner, a bright line. At first each joined quad was trimmed to
the neighbor's drawn plane, its edge moved to where that plane crosses
the edge's middle. Worn, that showed as a thin dashed dark line along the
ceiling's edge and a dotted bright one further on: a scan's planes are
never quite level or plumb, so the two trimmed edges crossed at the
middle and parted toward their ends, a crack (dark) on one side and an
overlap (bright, the blend adds) on the other. Now a face joined across a
corner runs 5 cm past the neighbor's drawn plane at both ends of the
edge, however the two planes lean, so both quads cover every pixel near
the join, and the fragment decides: near a joined edge a face keeps its
fragment only if the pixel's view ray meets its own drawn plane before
the neighbor's, ahead of the eye. Both faces decide from the same
numbers, so their answers are exact complements: the ray through the
pixel's center, solved from the eye camera and the size of the target
the faces draw into (the eye camera the ports read carries it after
`view_proj`, 80 bytes, which `gfx` writes per eye: the faces layer's size
or the eye's); and the two planes, written into both faces' rows as the
same floats and taken in a fixed order. A first try had each face test
its own interpolated point against the other's plane; the two then
decided from slightly different numbers, and about one pixel in 2,000 on
the line went to both. The decision runs only from a little before
where the neighbor's plane crosses the face (`cut_from`), so no plane
cuts far from the edge it joins, and the pixel's ray is solved only
there: about 130 of the IR's expressions for a fragment near a join (7 %
of an evaluation), nothing elsewhere. A neighbor beside a face in the
same plane is joined without the fade and without the decision, as
before.

**Hidden walls** stay undrawn, so a room closed by one has a hole in the
sky there: the wall's span from the point, from under the horizon to the
ceiling's edge. A 4 m hidden wall 2 m from the point leaves 90° of the
turn's lower part open; the ceiling over it still carries the rings. The
log says how much of the turn the hidden walls leave undrawn.

**The reflection.** Aurora is now on the floor's list too (last, so the
floor's cycle is nine long and Aurora is on three lists), and on a floor
it draws the sky's reflection. For each floor fragment the view ray from
the eye is reflected about the floor and followed to where it leaves the
room's box (its walls in plan and its ceiling), and the sky is shown in
the direction of that point from the sky's point: the floor shows what
the walls and the ceiling show, as a mirror would, and moves with the
head. The tests check it against the mirrored eye's line through the
floor point. It reads as light on a dark, slightly rough floor: 0.4 of
the sky; full where the view ray meets the floor at 60° from straight
down or flatter (toward the walls) and gone from 25° in (toward the
feet, 0.56 m around a seated wearer); and its glow widened to 0.8 of its
range (0.04 of the frame's height against the sky's 0.02), which spreads
each curtain along the frame's y, which on the floor is the view
direction. One evaluation per fragment: across the seam the reflection
fades out over the same 23° instead of cross-fading. The stage floor is
drawn as a surface when the room has no scanned floor, so it reflects
too.

**The cost**, from the shader's IR (naga's expressions, a proxy for its
ALU work, not a device number). One evaluation of Aurora runs about
1,900: seven bands, each a gradient noise of about 210 and two sines,
and the shimmer's noise. Around it `fs_main` adds about 90, a sky
fragment about 210 (the angles, the rim per edge, the fades) and a
reflection fragment about 310 (the reflected ray and the box besides).
So a sky fragment costs about 6 % more than a per-face one, a reflection
fragment about 5 % more than a sky one and nothing past the wrapper
inside its dead zone, and the seam's 23° about twice. Seated, the Aurora
theme cost 0.37 ms of App over the saved room before (`MEASURED.md`,
Themes). The sky adds about 6 % to each wall and ceiling fragment; on
the floor an evaluation of Aurora replaces the streamlines, which cost
about the same per full view in D2b (1.5 and 1.7 ms on the stage floor),
and draws nothing near the feet; tables, frames and other surfaces trade
their behaviors for the tunnels. Expect about the same cost, within a
few tenths of a millisecond.

**What stays.** The tint, the strength, the peak alpha, the immersion's
lift, occlusion by what stands in front, and the edge fade wherever no
sky face continues it. Aurora on a table, a frame or any other surface
maps per face as before, and the other seven ports are untouched. The
port block grows from 8 rows to 12 for every port: the rim per edge, the
point and the ceiling, the room in plan, its axis and the facing; and to
18 for the joins (board #5438): the face's drawn plane, the neighbor's
per edge, and per edge from where the join decides.

**The knob.** `debug.fosfora.sky 0|1`, default 1, read at launch and
polled once a second: 0 puts Aurora back on per-face mapping everywhere,
so both can be captured and costed in one launch. The log gives `sky: on
(debug.fosfora.sky) · point (x, y, z), the center of the scanned room's
4 walls in plan (4.60 x 3.80 m, floor 0.02, ceiling 2.43), 1.20 m over
the floor · seam toward (x, z), behind the facing at launch · hidden
walls leave 0° of the turn undrawn` at launch and whenever the state,
the point, the seam or the hidden walls' share changes ("the last
recenter" after one; "no scanned walls: the stage floor's center"
without a room), and `sky: debug.fosfora.sky 0, Aurora per face` on a
change of the knob.

To capture the sky on and off at the same spot unworn (after the
headless setup; back up `rooms/` first, the theme writes the room): set
`music 1`, `gov 0`, `immersefix 1` and `theme aurora`, launch seated in
the scanned room, then `sky 1` and `sky 0` in turn with a screen capture
2 s after each, and `sky ""` last. For the cost, one launch per
condition, reading the eye surface's (or the faces surface's) render:
`scripts/xr/gpu-stages.sh --with "music=1;gov=0;immersefix=1" --cond
"classic=theme=classic" --cond "sky=theme=aurora;sky=1" --cond
"face=theme=aurora;sky=0" --cond "sky2=theme=aurora;sky=1" --only
"classic,sky,face,sky2"`.

**Tested on the desktop** (`sky.rs`): the room's point from a box room
turned 30° and from an L-shaped one, with and without a scanned floor
and ceiling, under a low ceiling, and the stage's stand-in; the facing
from the head, looking down and up included; the frame's turn and the
seam's place for two facings; two points 0.5 mm either side of every
corner and the wall-to-ceiling edge land under a pixel apart on the sky,
and two walls' most of a frame apart per face; the cross-fade across the
seam and the reflection's fade there; the bands' elevations against the
zenith fade and the shimmer's period at its end; the reflection against
the mirrored eye's line for three eyes and four floor points, and that
it moves with the head; the rim on a box room and with a wall hidden;
the joins (`joined_faces_run_past_each_other_and_carry_the_neighbors_plane`
in a box room and one as a scan reports it, the ceiling tilted 0.4°, a
wall leaning 0.3° and one turned 0.5°; and
`along_every_join_exactly_one_face_keeps_each_view_ray`: rays through
points 0.1 mm to 4 cm off every join, for four eyes in three rooms, kept
by exactly one face); the mapping per kind and with the knob off; the rows;
`SKY_WGSL` validated with naga, its constants and the port block's
layout against the Rust. Two ignored GPU tests (`tests/sky_gpu.rs`):
`fs_sky` over a probe against `sky::sample` pixel by pixel in five views
(894,919 pixels on an RTX 4090, worst 2.1e-4 of light, at a slant inside
a rim) and per face against `fs_main` (3e-8); and the real Aurora on the
whole room from a seated eye (`FOSFORA_SKY_DUMP=<dir>` writes the
views). A third, `the_sky_faces_meet_without_a_dark_or_bright_line`,
draws every sky face at a Quest 3 eye buffer's size (2064 by 2208) and
holds each pixel along the line under the ceiling and down a corner to
its neighbors 4 px across it: in a box room and in the tilted one, at
one, two and three layers, level (the corner line then runs down a
column of pixel centers) and with the head rolled 2.3°, over a flat
effect and over Aurora. The trimmed join failed it in the tilted room
(0.75 darker and 0.56 brighter than both neighbors); now no pixel is
darker or brighter than both (0.0000 over the flat effect, and over
Aurora no more than its curtains bend across 8 px, 0.017).

**Where it will look wrong.**
- *A room that is not a box*: the point is the center of the walls'
  bounding rectangle, which in an L-shaped room can sit near the inside
  corner or outside the floor, and the reflection takes the rectangle
  for the walls, so where the L turns, the floor reflects the sky as if
  the walls stood on the rectangle.
- *A wall the scan missed*: no face, so a hole the wall's size, as a
  hidden wall leaves, and the rectangle shrinks to the walls it has,
  moving the point.
- *A sloped ceiling*: the sky follows it (the mapping is by direction),
  but the room's box has one ceiling height, the lowest underside, so the
  reflection of the high side is off.
- *The seam*: the 23° cross-fade shows as two sets of curtains braiding
  into each other behind the wearer, and the reflection is dark there.
  After a wearer turns their chair it is wherever they no longer face
  until a recenter.
- *The zenith*: a dark disc 0.25 m across over the point, back to full
  light 0.7 m across; lying back off the point, it is off to one side,
  not straight overhead.
- *A floor with a rug box on it*: the rug is its own box, not the floor,
  so its occluder hides the reflection under it and, as any other
  surface in the theme, its top is a tunnel going down into the floor:
  the pit under the feet Mirror kept the floor out of. The reflection
  takes the room as empty, so a table never blocks it either.
- *The music's weight*: the bass bands ring the ceiling and the walls at
  eye level carry the upper bands, so in a bass-heavy song a seated
  wearer looking ahead sees the quieter curtains, and the floor, which
  reflects mostly the low elevations, the quietest.
- *A gap in the scan*: two walls more than 10 cm apart at a corner keep
  their fades, and the dark line comes back there.

**Not yet.** The device: the cost rows in `MEASURED.md`, the captures of
the sky on and off, and the worn look seated and lying back.

### The layers (board #5438)

**Why.** The sky was painted on the surfaces. It sat exactly at the wall,
so when the wearer moved their head nothing in it moved against anything
else, and it read as a lit wall. The owner wants to lie back and be inside
it. The tunnel in the edge bars showed that parallax alone carries depth
for him, with one eye. So the curtains now hang in layers at different
depths behind the surfaces, with true parallax.

**The layers.** The sky is drawn in up to three. The near layer is the sky
as above, on the surfaces and mapped by the fragment's own position, so the
look stays anchored to the room. Each farther layer is the sky as it would
appear on a larger shell: the view ray from the eye through the fragment
is followed on past the surface to the shell, and the point it meets is
mapped by its direction from the sky's point, exactly as the near layer
maps a surface point. A far layer is then fixed on its shell in the room.
As the head moves it shifts against the surfaces by the right amount for
its depth, for each eye (the vertex stage hands each eye's position on, as
for the reflection).

**The shells** are the room's box pushed out: its rectangle in plan grown
by the depth on every side, and its ceiling raised by it. They have no
floor: a ray going down leaves through a wall under the floor, where the
sky has no bands. A box rather than a sphere around the point, for three
reasons. It is convex, so a shell runs on across the room's corners and
onto the ceiling without a break. It is the same depth behind every wall
and the ceiling, where a sphere is farther behind a wall's middle than
behind a corner, and one sized for a long room's near walls passes inside
its far corners. And its exit is the slab test the reflection already
runs (`sky::box_exit`). The shells are 12 m and 36 m behind the walls'
center lines and the ceiling, times `debug.fosfora.skydepth`. They were
1.5 m and 4 m; worn, the owner liked the depth and asked for two layers,
the surface and one about 12 m back. The third, under the knob, is three
times as deep, still past the second, so each layer composites under
the nearer ones. Seated at the point, a 10 cm head move shifts a far
layer along the wall by the move times the share of its distance that
lies past the wall (the tests' numbers; the drawn quad is 4 cm in front
of the wall's center line):

| Wall ahead (quad) | 12 m shell | 36 m shell | skydepth 2: 24 m | 72 m |
|---|---|---|---|---|
| 2 m (1.96 m) | 8.6 cm | 9.5 cm | 9.3 cm | 9.7 cm |
| 3 m (2.96 m) | 8.0 cm | 9.2 cm | 8.9 cm | 9.6 cm |
| 6.17 x 5.49 m room, 2.75 m (2.71 m) | 8.2 cm | 9.3 cm | 9.0 cm | 9.6 cm |
| 6.17 x 5.49 m room, 3.09 m (3.05 m) | 8.0 cm | 9.2 cm | 8.9 cm | 9.6 cm |

So in the room worn the 12 m layer moves with the head at about four
fifths of the move, not all of it: the near layer stays on the wall and
the far one slides 8 cm against it. The 36 m layer moves nearly with the
head, 1.2 cm more than the 12 m one.

**How they differ.** Stacked copies offset by parallax read as a blur at
rest, so the far layers are other curtains that belong with the near one:

- *Around the turn.* A far layer takes its curtains from further around
  the frame: 2.1 rad for the middle layer, 3.2 rad for the farthest. At
  those offsets none of the seven bands' two waves (2 + 0.5 i and 2.3
  times that per frame height across) comes within 0.4 rad of phase of
  another layer's, and the ripple's noise is another stretch. The turn
  moves no seam: the seam is where the azimuth wraps, behind the wearer,
  in every layer.
- *In height.* Their bands sit lower, by half and a quarter of the bands'
  11.3° spacing, so the middle layer's bands run between the near layer's
  and the farthest's between those.
- *In light.* 1, 0.6 and 0.35 of the light, and cooler: the far two's
  colors times (0.8, 0.9, 1) and (0.6, 0.78, 1), after the coverage, as
  the lane's tint. They stay at the new depths: drawn on the desktop GPU,
  the 12 m layer still reads as other, dimmer curtains between the near
  layer's.
- *Not in time.* Aurora reads its clock, its curtains' travel, its band
  spread and its glow from its uniform: one value per draw. A
  per-layer time offset or parameter would need a draw per layer (the
  vertex, raster, blend and the rest of the fragment three times) or the
  effect's text rewritten past its entry. The frame coordinate is what
  the wrapper can vary per call, and it does. Splitting the seven bands
  between the layers cannot be done from the wrapper either: the effect
  sums them inside its loop.

Each far layer is composited under what is drawn so far (`sky::under`),
the same as drawing it first with the pass's premultiplied blending. The
alpha stays within 1 at full immersion, and a bright near curtain hides
what is behind it.

**What a far layer keeps and drops.** The rim, the strength, the peak alpha
with the immersion's lift and the lane's tint apply to every layer. Each
fades at the zenith by its own elevation. Seated at the point the three
dark centers coincide. A shell 12 m up is seen in nearly the same
direction from the eye as from the point, so off the point a far layer's
dark center lies almost over the eye, not over the point, and a trace of
its fading rings crosses the near layer's (2e-3 of alpha 0.25 m off the
point). A far layer fades out across the seam's 23° instead of
cross-fading: a second evaluation per layer there would cost more for
light the wearer seldom faces. Under its lowest band's reach (-0.34 rad on
its own frame: the band's center, the waves' and the ripple's reach and
three widths of its widest glow) a layer adds only the shimmer, so it
fades out over the 0.1 rad below and is not evaluated under that: the foot
of the walls. Wherever none of a far layer's light is left (the dark
center, the seam's line, under its bands) the effect is not called for
it.

**The reflection** stays one layer, the near one. Reflecting the far
layers would cost an evaluation each on the floor.

**One layer is the sky before the layers**, bit for bit, away from the
joins (which changed, above). The near layer's code path is the one it
was, and with one layer nothing else runs. The sky's faces carry the
count and the depth multiplier in `mode.z` and `mode.w`. A desktop GPU
test draws the shader and a copy of it from before the layers over the
probe and over the real Aurora, on the joined quads without the joins'
decision (the copy has none), and compares every pixel's bits: identical
in all six views on an RTX 4090. The Quest's shader compiler is not
checked.

**The cost, by layer count**, from the shader's IR as above, counted this
way: naga's emitted expressions, calls inlined, Aurora's band loop counted
seven times (a proxy for ALU work, not a device number; it gives about
2,035 for an evaluation where the count above gave 1,900). A sky fragment
off the seam with light in every layer:

| Layers | Evaluations | Expressions | Against one layer |
|---|---|---|---|
| 1 | 1 | about 2,170 | 1 (the sky before the layers: 2,160) |
| 2 | 2 | about 4,360 | 2.0 |
| 3 | 3 | about 6,540 | 3.0 |

Around its evaluation each far layer does about 150 of its own (7 % of an
evaluation): the shell's exit 56, the angles 18, the light 18, the frame
12, its three fades, the tint and the compositing. In the seam's 23° the
near layer adds its second evaluation as before. The far layers skip
theirs only at the dark center, on the seam's line and at the walls'
foot, so seated nearly every wall and ceiling fragment pays the full
count. The floor's reflection is unchanged. On the device (`MEASURED.md`,
"The sky's layers") a second layer cost about 0.4 ms of App and a third
about 1.1 ms more. A shell's depth changes none of the work.

**The knobs.** `debug.fosfora.skylayers 1|2|3`, default 2, and
`debug.fosfora.skydepth`, a multiplier on the shells' depths (0 to 4,
default 1; 0 puts them on the surfaces), read at launch and polled once a
second with the sky's knob. The log gives `sky layers: 2
(debug.fosfora.skylayers), the sky on the surfaces and a layer 12.00 m
behind them at 0.6 of the light (debug.fosfora.skydepth 1.00)` at launch,
and the same with `(was: ...)` after it on each change.

To capture 1, 2 and 3 layers at the same spot unworn (after the headless
setup; back up `rooms/` first, the theme writes the room): set `music 1`,
`gov 0`, `immersefix 1`, `theme aurora` and `sky 1`, launch seated in the
scanned room, then `skylayers 1`, `skylayers 2` and `skylayers 3` in turn
with a screen capture 2 s after each, and `skylayers ""` last. A headset
on the desk does not move, so the captures show how the layers look
together, not their depth: the parallax needs the worn look. For the cost,
one launch per condition, reading the eye surface's (or the faces
surface's) render: `scripts/xr/gpu-stages.sh --with
"music=1;gov=0;immersefix=1;theme=aurora;sky=1" --cond "one=skylayers=1"
--cond "two=skylayers=2" --cond "three=skylayers=3" --cond
"one2=skylayers=1" --only "one,two,three,one2"` (the second `one` brackets
the drift between launches).

**Tested on the desktop** (`sky.rs`):
`the_view_ray_meets_each_shell_behind_the_surface_it_passes` (square on,
at a slant, near a corner, on the ceiling straight up and at a slant near
its edge, for both shells, against a marched ray);
`a_far_layer_moves_against_the_wall_by_its_depth_as_the_head_moves` (the
table above, the same curtain found again after the move, the near layer
unmoved, each shift more than 7 cm and less than the move); `a_far_layer_runs_on_across_a_corner_and_onto_the_ceiling`
(points 0.5 mm either side of every corner and wall-to-ceiling edge, an
eye off the point, two depths: under a pixel apart on the frame and the
same light); `one_layer_is_the_sky_as_it_was`;
`the_farther_a_layer_the_less_light_and_the_cooler`;
`a_far_layer_is_not_evaluated_where_it_adds_no_light` (the zenith, the
seam, under the bands, the band floor against `aurora.wgsl`'s lowest band,
the drops and the turns' phases); `the_layers_knobs_and_their_log_lines`;
and the layers' constants in `SKY_WGSL` against the Rust. The composed
Aurora validates with naga in `surface_port.rs`'s port test. On the GPU
(`tests/sky_gpu.rs`): `the_sky_shader_matches_its_rust_copy` now at one,
two and three layers and three at 2.5 times the depth (worst 1.7e-4 of
light on the sky's faces); `one_layer_draws_what_the_sky_drew_before_the_layers_bit_for_bit`;
and `aurora_renders_as_one_sky_with_a_dark_zenith_and_a_fainter_floor` per
face and at one, two and three layers (dumps `sky1_*`, `sky2_*`, `sky3_*`).

**Where it will look wrong.**
- *Through a doorway or past a wall's end*: the layers are drawn on the
  sky's faces alone, so where there is no face (an open door, a wall's
  end, a hidden or missed wall) every layer stops at the face's edge. The
  far curtains slide behind that edge as the head moves, as behind a
  window frame, but the opening itself shows the real room, not the sky
  going on.
- *Against furniture*: what stands in front cuts every layer at its
  outline, which is right. The far curtains slide along the outline as the
  head moves, so a coarse occluder (a box larger than the chair, the depth
  map's ragged edges) shows more against them than against the still near
  layer.
- *The seam*: the far layers fade out across its 23°, so behind the wearer
  only the near layer's braid is left, thinner and flat.
- *The zenith*: each layer is dark where its own direction from the point
  is straight up. Seated at the point the three centers coincide. Off the
  point, or lying back off it, the near layer's dark disc stays over the
  point and the far layers' sit almost over the head: two dark centers,
  and the far rings cross the near one's.
- *The far layer near the head's motion*: at 12 m the far layer moves 80
  to 86 % of a head move in a room of 2 to 3 m, so it reads less as
  curtains at a depth than as a sky that follows the head, with the near
  layer sliding over it. The depth is in the difference between the two.
  `skydepth` under 1 brings it nearer (0.125 puts it at 1.5 m, where it
  moved 4.4 cm); the third layer, 36 m back, adds little against it (1.2
  cm more).
- *Any room*: the shells are pushed out by fixed depths, 12 m and 36 m,
  larger than any room. That is only distance, and the parallax is what
  that distance gives. The far curtains are as large in angle as the near
  ones (each layer is mapped by direction from the point), so their depth
  comes from parallax and light alone, not from size.
- *A room that is not a box*: the shells are the bounding rectangle's, so
  behind a wall inside the rectangle (an L's inner walls, an alcove) a
  layer is deeper than its depth and shifts more there.

**Not yet.** The device: the worn look of the 12 m layer and of the
joins seated and lying back. The layers' cost is in `MEASURED.md`
(measured at 1.5 and 4 m; a shell's depth changes none of the work).

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

The message comes after the other labels. It shows where the scan label
does, 1.2 m ahead at eye height, only while no other label is up: the
voice's, then the editor's, a hand menu reply, then the scan's and the
permission's (board #5461: the permission's now holds 6 s, not the whole
wait; only the controllers label comes after the message, and never holds
it, `XR_DESIGN.md`). It waits while any of them shows, and its
4 s hold counts only while it is the one shown, so a scan that ends with
"Room: 17 surfaces" plays out first and a fist mid-message covers it
without eating it. `debug.fosfora.immerseopen 0`, a pinned level
(`immersefix`) and a launch without passthrough skip it; the music's
start then glides to 0.35 in about 2 s. At depth off there is no message
and no dimming; at low the opening settles at 0.175.

Board #5452: the show now waits for the wearer before any of this
(`XR_DESIGN.md`, "The first run"). While it waits the level sits at 0.25
(`WAIT_LEVEL`) and the opening does not start. After the begin the
message holds that level and the glide runs from it to 0.35. A launch
with `debug.fosfora.begin 1` never waits and opens over the plain room as
described above.

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
right cell, "Immersion: full", each press the next of off, low, full and black (the room held dark whatever the music does);
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
launch rule. The first six add no shader and no behavior. Contour came
with its own look, the contours, which is in the catalogue like any other
(below). Every pairing is on its kind's own list
(`SurfaceBehavior::catalogue`), so none of the pairings first proposed
needed changing.

**The five.** Each cell is behavior, color, band, strength. Classic,
Drift and Tide draw the edge bars on the face for every kind; Aurora's
and Mirror's edge modes are in their cells.

| kind | Classic | Drift | Tide | Aurora | Mirror |
| --- | --- | --- | --- | --- | --- |
| table | streamlines, own (blue), rms, 1 | streamlines, teal, rms, 1 | curls, rose, rms, 1 | none, teal, rms, 1, edges in | none, blue, rms, 1, edges in |
| floor | rings, own (violet), bass, 1 | rings, blue, bass, 1 | curls, violet, rms, 1 | aurora (the reflection), the key's tint, rms, 1, edges off | rings, blue, bass, 1, edges on the face |
| wall | spectrum, own (warm white), rms, 1 | streamlines, blue, rms, 1 | pulse, violet, bass, 1 | aurora (the sky), the key's tint, rms, 1, edges off | none, warm white, rms, 1, edges in |
| ceiling | none, own (amber), rms, 1 | rings, blue, bass, 1 | pulse, violet, bass, 1 | aurora (the sky), the key's tint, rms, 1, edges off | none, warm white, rms, 1, edges in |
| frame | pulse, own (amber), bass, 1 | pulse, teal, bass, 0.6 | rings, rose, bass, 1 | none, teal, rms, 1, edges in | none, blue, rms, 1, edges in |
| other | curls, own (green), rms, 1 | curls, teal, rms, 1 | pulse, rose, bass, 1 | none, teal, rms, 1, edges in | none, blue, rms, 1, edges in |

Classic is the built-in defaults (`default_for`), each kind's own color,
so applying it gives exactly a room with no file. It is the only theme with the
wall spectrum, since the edge bars carry the spectrum on every edge in
every theme. Drift is the calm one. The other three color by one rule:
the room's planes (walls, floor, ceiling) take one color and what stands
in it (tables, frames, other surfaces) the other, so a table reads
against the floor and a frame against its wall. Every band is the
behavior's own, the rms for the flows and the bass for the rings and the
pulse, which is Drift's rule as asked; picking a band per theme waits
for signal dumps of real songs. Every strength is 1 except Drift's
frames at 0.6, because the pulse jumps on the downbeat and is the one
restless look in the calm theme. The rings and the ported Aurora do not
read the band (D3's table), so it only matters there if a hand edit
later changes the behavior.

**Aurora, finished** (board #5436). The owner wore the five and picked
Aurora to work on. Walls and the ceiling run Aurora, which is now one
sky around the room, and the floor runs it too, which on a floor is the
sky's reflection ("The sky", above). All three take the key's tint: the
ceiling was green against the walls' key tint, which would cut the one
sky with a line of color at the ceiling's edge. Their edge bars are off,
so nothing cuts the sky at a corner. Tables, frames and other surfaces
run no behavior with their edges in, teal: tunnels of light under the
sky. The band column is the behavior's own as everywhere; Aurora reads
its own bands.

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

**Flint and Contour** (board #5533). Two more themes, made so that each
bundled track can bring a look of its own (below). The owner's words for
them were "hardness and angles" and "gentle rolling and topographic
scenery". They come after Mirror in the cycle.

| kind | Flint | Contour |
| --- | --- | --- |
| table | shards in one color, warm white, rms, 1, edges standing | none, warm white, rms, 1, edges on the face |
| floor | tessera, blue, rms, 1, edges on the face | contours, green, rms, 1, edges off |
| wall | shards in one color, own (untinted), rms, 1, edges on the face | contours, green, rms, 1, edges off |
| ceiling | shards in one color, warm white, rms, 0.6, edges on the face | contours, green, rms, 1, edges off |
| frame | reticle, amber, rms, 1, edges on the face | none, warm white, rms, 1, edges on the face |
| other | shards in one color, amber, rms, 1, edges standing | curls, warm white, rms, 0.4, edges on the face |

*Flint* is built from the catalogue's angular looks, each chosen for what
it does on a drum. The eight dumped songs (110 to 185 BPM; none is rock,
and the two nearest the rock track's 104 BPM are at 110 and 115) have a
kick every 0.2 to 0.65 s whose level halves in 70 to 150 ms, and a snare
that reads as a high onset with little kick. The rock track the app now
ships for Flint reads the same way: a kick every 0.9 s that halves in
75 ms and takes shards' edges from 2.5 to 4.7, and a snare onset of 0.7.

- **The walls and the ceiling: shards.** A seated or lying wearer sees
  mostly walls and ceiling, so the facets are there. The cell edges are
  lit by `1 + 2 bass + 3 kick`, so they sit near 3 between kicks and jump
  to 4.0 to 4.7 on one, then fall back with the kick. A snare's onset (0.6
  to 0.8) shifts the cell centers by a fifth of a cell and does not
  brighten them: the cells crack on the snare and flash on the kick.
  Sustained energy shows as the cells' fill, which follows the rms and
  never drops out. A wall 2.45 m high shows seven cells up its height,
  about 35 cm each.
- **A ceiling's shards are capped while the flash limit is on**
  (`SurfaceBehavior::drawn_strength`, `surfaces::CEILING_SHARDS_LIMITED`,
  0.6). Shards move their cell edges with the onsets, which the flash
  limit cannot hold to its direction: a position, not a light. The
  ceiling is the nearest face (1.25 m over a seated eye), so a cell there
  (54 cm) is wider than the 10 degrees the rule counts in and an edge
  crossing a region is a change of its light. Flint over eleven songs
  (the eight dumped ones and the three bundled tracks), the limiter on,
  by the ceiling's strength:

  | strength | worst region, flashes a second | songs at 3 | songs where the ceiling is the worst region |
  | --- | --- | --- | --- |
  | 1.0 | 3.5 on one song | three more | eight |
  | 0.8 | 3 | one | two |
  | 0.7 | 3 | one | two |
  | 0.6 | 3 | one | one |
  | 0.5 | 3 | one | one |

  The one song at 3 stays there down to 0.4, so that count is not the
  strength's. 0.8 is the highest value tried that keeps every song within
  the rule. The cap is 0.6: the highest whose counts are those of 0.5 on
  every song, a quarter under the highest that passed, with nothing
  measured to gain below it. Flint's own ceiling sits at the cap.
  - *Where.* The cap is applied where a box's strength is resolved for
    drawing (`app.rs`, the surfaces pass's loop), and nowhere else. The
    room file, the Strength row and the voice's replies keep the wearer's
    own number, so nothing has to be put back when the limit goes off.
  - *Live.* It reads the limiter's state each frame, so it follows the
    wearer's "Limit flashing" setting, the two spoken sentences and the
    knob. A strength does not glide anywhere in this path (a Strength
    step or a theme lands on the next frame), so the cap lands on the
    next frame too. With the limit switched on, a ceiling the wearer
    raised over 0.6 steps down to 0.6 once. Switched off, it steps back
    up to their number. On Flint as the theme sets it nothing changes at
    the switch.
  - *Only the ceiling.* Walls are not capped. Counted with the eye 1 m
    from a wall, Flint's walls at full strength read 3 flashes a second
    on four of the eleven songs, in regions of that wall, against one
    song from the middle of the room, and none over 3. A wall's cells are
    35 cm, the ceiling's 54.
- **Steel, not stained glass** (`Look::mono`, `Port::mono`). Shards' own
  fills are magenta and green. Flint draws every shards in one color: the
  effect's `saturation` input at 0, so the cells are gray with white
  edges, and the look's color tints them. The walls' are untinted, the
  ceiling's and the tables' warm white, other things' amber. It is asked
  per kind by the theme and follows the saved theme's name, as the edges
  do, so nothing new is in the room file. Classic's shards, a hand-picked
  shards in any other room and a Flint surface edited to another look
  draw as before.
- **Tables and other surfaces: shards** too, warm white on the tables and
  amber on the rest.
- **The floor: tessera.** A grid of tiles that opens from the center over
  four bars. Its lines carry a hit that decays across each beat
  (`exp(-6 x beat phase)`) and a harder one on the bar. It reads the bar's
  clock, not the kick, so it holds its pulse through a passage with no
  drums. Blue. Its palette turns with the bar's phase, so under the tint
  it dims and brightens once a bar. Nothing in the effect or the port
  holds it still, and none was added.
- **Frames: the reticle.** Crosshairs that take a new place every bar,
  with a bracket that closes on each beat. Amber, the one hot color.
- **Edges.** On the face for the floor, the walls, the ceiling and the
  frames, so the room's own corners are lit lines. Standing on tables and
  other surfaces: a fence of spikes on each rim.

Shards were not on a wall's or a ceiling's list. They are now, after
those kinds' other ports and before the contours: a list entry each and
their tests, no new shader. The on-device model's spec was written before,
so it does not offer shards there; the grammar does.

*What the walls cost.* Not measured on the headset. Shards are the
cheapest of the eight ports per fragment: 1.2 ms over a full view of one
face in D2b, against Aurora's 1.5 (`MEASURED.md`, D2b, drawn in the eye
pass then). The Aurora theme runs a port on every wall, the ceiling and
the floor, the same faces Flint lights, and costs 0.3 to 0.4 ms of App
over a room with none in the faces layer (`MEASURED.md`, Themes and
Aurora as one sky). Shards' cost does not fall with a larger cell: every
fragment looks at nine cells whatever their size. So expect Flint at
Aurora's cost or a little under.

A first version (Nov 16) had bezel on the walls, nothing on the ceiling
and shards in their own colors on the tables. Bezel is a border: a 4 m
wall was dark with chrome at its rim, the ceiling was empty, and the one
surface with facets was a table in magenta and green. Tried and left out:
prism, the other angular look on a table's and a floor's list. It turns
and swells with the bass, which is the opposite of a hit that cuts off,
and before the limiter it is the catalogue's hardest flasher (12 to 16
general flashes a second on the dumped songs). Tessera on the ceiling was
considered for a grid above and below. Shards went there instead: alone
on every surface they flash least of the ports with the limiter on, and
tessera reaches the limit on two of the songs. Astrolabe and fenestra are
held back and were not considered.

*Contour* is the contours on the room's three planes, one landscape in
green rising to warm white, with the edge bars off there as on Aurora's
sky, so no bar cuts the land at a corner. Tables and frames run nothing
and keep their outline on the face in warm white, which reads against the
green lines where a tunnel's rungs would not. Other surfaces run the
curls at 0.4: small closed rings beside the map's.

**The edges per kind.** The edge bars draw each shape in its kind's mode
under the room's saved theme (`themes::edge_mode`), edited since or not: a
hand edit changes behaviors and colors, not the theme's edges. A custom
room, with no theme, draws every kind on the face. The mode
comes from the saved name alone, so nothing new is written to the room
file. A build without Mirror reads a room saved on it as one naming an
unknown theme: no theme, Mirror's looks kept, every kind on the face. A
live plane takes its kind's mode, a hidden wall and the stage floor
are still not outlined, and an unlabeled anchor draws on the face. Faces on
the face and standing are both strips, so a frame of mixed modes is one
buffer write and at most two draws per eye, the strips and the tunnels.

**The Edges control over the theme.** The hand menu's Edges control (the
right half of the cloud row's right cell) now steps theme, off, face,
stand, in, and back to theme. "Edges: theme" (board #5461; "Edges" over
the state in half a cell), the default, follows the
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
params, so Classic leaves the room exactly as one with no file, and a behavior
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
before themes has no theme: it is fresh when it runs the built-in
defaults (no entries, or entries that equal them) and starts on Aurora
(below), and it shows custom otherwise, and nothing changes until the
wearer picks. One with no entries but changed kind defaults shows custom. Older files (version 1,
and version 2 from before themes) read as before. A file written now also
reads in the build before themes, whose reader ignores fields it does not
know: the kind defaults and the entries, with their params, read as
written, so every scanned surface keeps its look, and only the name and
the kind looks are lost until that build saves the room.

**A fresh room starts on Aurora** (boards #5448, #5447). A room with no
theme saved and nothing edited by hand runs Aurora from its first frame
with surfaces: a new install, a newly scanned room, a room saved before
themes that runs the built-in defaults, and a room just cleared with
`debug.fosfora.surface clear`. Aurora is applied as the Theme control
applies it, every kind default and every surface, but nothing is written.
The first change the wearer makes writes the room's file as if they had
picked Aurora and then made that change, so the file then names
`"theme": "aurora"`. A theme picked by the menu, the swipe, by voice or by
the knob is saved by name, Classic too (`"theme": "classic"`), so a picked
Classic stays and is never taken for a room nobody touched. A room whose
surfaces were edited by hand before themes stays custom. Without a room
(no anchors) and for a room file that does not read, the built-in
defaults run as before. Immersion and the rest of a first launch do not
change.

**The control.** "Theme" over the theme's name ("Drift", "Drift*",
"custom") in the right half of the music row's right cell, with Voice in
the left half, in both layouts (debug panel on and off). Each press steps
to the next theme (Classic, Drift, Tide, Aurora, Mirror, Flint, Contour,
Classic; from custom, to Classic) and applies it once the frame's boxes are in, as All: none does.
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
press does. "previous theme", "last theme", "theme back" step it back, as
the thumb's swipe left does (board #5541). The reply names the theme it came to ("theme: Aurora"). A name
no theme has replies with the list: No theme called "sunset": Classic,
Drift, Tide, Aurora, Mirror, Flint or Contour. "mirror theme", "theme
flint" and "set the theme to contour" work through the same grammar, with
no new phrase: the grammar asks `Theme::from_name`, so a theme added to
the table is named by voice at once. The on-device model has no part in
it. Its actions have no theme, so a theme is the grammar's alone. The word
"theme" has to start or end the sentence,
so "drift" or "aurora" alone keep meaning what they did. The agent's
actions have no theme, and an unknown theme is not forwarded to it.

**The knob.** `debug.fosfora.theme <name>` (classic, drift, tide, aurora,
mirror, flint, contour) is polled once a second after the lanes' update and applies that theme
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
adb shell setprop debug.fosfora.theme flint     # then capture
adb shell setprop debug.fosfora.theme contour   # then capture
adb shell setprop debug.fosfora.theme '""'
```

With the bundled tracks playing, set `debug.fosfora.tracktheme 0` first,
or the next track brings its own theme over the one under test.

or one launch per theme with `scripts/xr/gpu-stages.sh --with
"music=1;gov=0" --cond "classic=theme=classic" --cond "drift=theme=drift"
--cond "tide=theme=tide" --cond "aurora=theme=aurora" --cond
"mirror=theme=mirror" --only "classic,drift,tide,aurora,mirror"`. Back up
`rooms/` first: the room ends on the last theme.

**A bundled track brings its theme** (board #5533, `music::BUNDLED`,
`themes::Bringing`, `RoomLanes::bring_theme`). Each track the app ships
with names a theme in the one list that names the track. Today the first
brings Aurora, the second Flint and the third Contour. Replacing a track
is an edit of that list, and nothing else names a track.

- **Only the bundled tracks.** The wearer's own music brings no theme.
  The wearer's tracks follow the bundled ones in the same list
  (`XR_DESIGN.md`, "Your music"), and when one of theirs starts after a
  bundled track, the room's own look comes back.
- **Never saved.** When a track's theme is laid, the room's own file is
  set aside in memory and nothing is written while the track's theme
  shows. An edit of a surface made on it shows at once, is not saved, and
  goes when the next theme comes. The file on disk holds only what the
  wearer chose, so a relaunch shows the room's own theme until the first
  bundled track starts.
- **The wearer's pick is theirs.** A theme picked by the menu, the swipe,
  the voice or the knob is laid over the room's own file and saved, as
  always. It holds until the next bundled track starts. It does not hold
  for the rest of the session: one stray swipe would then turn the pairing
  off with no way back short of a relaunch.
- **Through dark, not a cut.** From the new track's first frame the
  room's light falls to nothing (every lit face, the edge bars and the
  depth light). The theme is laid while it is dark, and after 0.25 s the
  new look rises. The song that ends keeps its look to its last note,
  whatever the immersion level is doing by then, and the wearer sees the
  room go dark as the new song begins and come back changed. A room that
  already shows the theme is laid at once with no fade.
- **The fade keeps to the flash limit's direction** (`themes::Turn`). It
  falls only in frames where the limiter lets the lights fall, 1 s of
  them in all, and waits through a rise. It rises only where they may
  rise: on a hit, so the new look comes up with the first beats, 0.3 s of
  rising frames in all. With the direction still for a second (quiet or
  steady music, or the limiter off) it runs on its own clock, 1 s down and
  1.5 s up. A fade on its own clock was built first and measured: it put
  one more change of light into a second the music had already filled,
  and Aurora's red curtains on the 4 Hz pulse train went from 3 flashes to
  3.5 while they rose. Keeping to the direction, the three changes between
  the bundled tracks' themes stay at 2.5 general and 3 red at worst on the
  four pulse trains, which is Aurora's own figure there
  (`a_tracks_theme_coming_stays_within_three_flashes`).
- **`debug.fosfora.tracktheme 0`** turns the pairing off, live: the room's
  own look comes back through the same fade, and a theme under test stays
  through a track change. Set to 1 again it brings the playing track's.

Logs: `room <id>: theme flint (the track): … , not saved: the room keeps
its own (Mirror)`; an edit under it ends `, not saved: on a track's theme,
the room keeps its own`; `room <id>: the track's theme leaves: the room's
own again (Mirror)`; `themes: the wearer picked a theme since the track
started: it stands`.

**Logs.** Loading a room: `room <id>: theme drift`, `theme drift, edited
since`, `no theme saved: starts on aurora, saved once something changes`
or `no theme saved: custom (4 entries)`; without a room, `no theme saved:
classic (the defaults)`. The fresh room's Aurora logs as an applied theme
with the source `a fresh room` and ends `not saved until something
changes`. Applying one: `room <id>: theme drift (the menu):
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
Aurora should cost the most: the ported Aurora on every wall, the
ceiling and, since board #5436, the floor as its reflection (1.2 to 1.5
ms per full view of one face in D2b), the biggest faces all on the
costly shader. Every face on a ported effect cost about 2.9 ms in D2b, so
expect about that; its tables, frames and other surfaces now run the
edge bars' tunnels instead. Drift comes next (streamlines on every wall), then Tide (the cheap
pulse on the walls and the ceiling, the curls on the floor), then Classic
(about 1 ms, as in D2a). A theme does not pick the world effect. Since
board #5431 it does pick the edge bars' mode per kind; Mirror's worn look
and its cost seated are still to take.

## Themes as scenes (Nov 18, board #5543)

**Why.** The owner wore the seven and found the parts of a theme
unrelated: the surfaces had a look each, but the depth light borrowed the
colors of "other" surfaces and the fireflies were the same in every
theme. He wrote each theme out as one scene, a row per part
(`themes_new.csv`): a night city under the lights, a forge in a cave, a
meadow drawn as a map, a hi-fi, deep water, a shore at dusk, a club of
infinity mirrors.

**What a theme is now** (`themes.rs`). The six surface rows as before,
and three more that follow the saved theme's name like the edges, so the
room file is unchanged:

- **The depth light** (`themes::Depth`): a look, the colors of its sides
  and its tops, a band, and whether it occludes. Before, it took all of
  these from the row for other surfaces.
- **The air** (`themes::Air`): the fireflies' pool and sprites as
  something else, by a color, a band, a count and a motion (wander,
  twinkle, rise, drift, orbit).
- **The floor's reflection** (`Theme::reflection`): the share of the
  sky's reflection a floor shows. Aurora's is 0.3, so the floor reads as
  dark water or ice. It scales the reflection's strength in `app.rs`; no
  shader changed. Only Aurora has a sky, so the file's values for Tide
  and Mirror wait for a decision on what their floors mirror.

**Occlude.** Every theme's depth light occludes: what the depth map sees
hides the surfaces' light behind it at every immersion level, a black
base under its own light. That is the hard cutout `depthlightveil 0`
always gave (`depth_light.rs`, the veil), now the default under a theme.
The knob at 0 or 1 still decides over the theme, and a room with no theme
keeps the veil. What comes back with it is the map's coarse silhouette
at the edge of a real thing, which the veil was made to hide.

**The rows.** Each cell is behavior, color, band, strength, edges; "one
color" is the ported effect without its own hues. A row in italics waits
for an effect that is not built and is as it was.

| kind | Drift | Tide | Aurora | Mirror | Flint | Contour |
| --- | --- | --- | --- | --- | --- | --- |
| table | curls, teal, mid, 1, off | rings, rose, bass, 1, off | windows, warm white, mid, 1, off | none, blue, mid, 1, in | sparks, amber, bass, 1, face | terraces, green, mid, 1, off |
| floor | contours, teal, bass, 0.3, off | *curls, violet, rms, 1, face (wash)* | aurora (the reflection at 0.3), key, rms, 1, off | tessera, blue, bass, 1, face, one color | tessera, blue, bass, 0.5, off, one color | contours, green, rms, 1, off |
| wall | streamlines, blue, rms, 1, off | pulse, violet, bass, 1, off | aurora (the sky), key, rms, 1, off | *none, warm white, rms, 1, in (bezel)* | shards, warm white, rms, 1, face, one color | contours, green, rms, 1, off |
| ceiling | rings, blue, bass, 1, off | pulse, violet, bass, 0.6, off | aurora (the sky), key, rms, 1, off | *none, warm white, rms, 1, in (astrolabe)* | shards, warm white, rms, 0.4, off, one color | streamlines, warm white, rms, 0.4, off |
| frame | pulse, teal, rms, 0.6, face | rings, rose, bass, 1, face | windows, amber, mid, 0.6, off | *none, blue, rms, 1, in (fenestra)* | pulse, amber, bass, 0.8, face | pulse, warm white, rms, 0.6, face |
| other | curls, teal, mid, 1, off | curls, rose, mid, 1, off | windows, amber, mid, 1, off | none, blue, mid, 1, in | embers, amber, mid, 1, off | terraces, green, mid, 1, off |
| depth light | bubbles, teal and warm white, bass | sparkle, rose, high | beams, teal and the key, bass | glints, warm white, high | coals, the heat ramp, bass | grass, green and warm white, mid |
| air | teal, high, 1500, drift | rose, bass, 1000, rise | warm white, high, 2000, twinkle | warm white, high, 1500, orbit | amber, bass, 1500, rise | green, high, 2000, wander |

Classic keeps the built-in defaults, with the tables' bars standing as
level meters, meters on the depth light and the air colored by band. The
file also moves its tables and other surfaces to the mids. That waits: a
band that is not the behavior's own would end the rule that a room with no
file runs Classic.

**What is built since, and what is not.** The video's three themes have
every row: windows, terraces, and sparks and embers drawn on the face
("Windows, terraces, sparks and embers" below); beams, coals and grass on
the depth light ("Beams, coals and grass"); and the air's color, band,
count and five motions ("The air per theme"). Still waiting: the wash
(Tide's floor), four depth looks (bubbles, sparkle, glints, meters, each
drawing as the points in the theme's colors and on its band), and the
depth light's color by band (Classic). Astrolabe and fenestra are held
back for flashing (board #5462). Mirror's walls
were tried with bezel and the tunnel together (ef5d5ad, unworn captures
`recordings/themes3/groundwork-ef5d5ad/mirror*.png`): no chrome showed and
the wall's tunnel was fainter than the ceiling's, so they keep the tunnel
alone until bezel draws with one. Tide's tables run
the rings, which a table's list in the room editor does not carry: the
one pairing off the catalogue.

**Every depth look stays on the surface** (the owner, Nov 18). The depth
light shades the depth map's mesh and nothing else, so a beam is a streak
running up a chair and a bubble a point sliding up it; nothing leaves the
thing for the air.

## Beams, coals and grass (Nov 18, board #5543)

**Why.** Each theme's depth light names a look, and only the points were
built. The video's three themes need theirs: beams under Aurora's sky,
coals in Flint's forge, grass in Contour's meadow. Every look stays on the
surface of the thing (the owner, Nov 18): the depth light shades the depth
map's mesh and nothing else.

**How a look is drawn** (`depth_light.rs`, `fs_mesh_light` and
`fs_mesh_light_sonar` in `env_depth.rs`). A look is the points' machinery
read another way. It uses the same tile of soft points, one read per
plane and octave, the same plane weights, octave blend, fades and vertex
stage. The look's id rides in two new rows of the light's uniform
(`light_look`, `light_move`), and the fragment branches on it once. With
the id at 0 the points draw what they drew, bit for bit
(`with_the_sweep_off_the_light_draws_what_it_drew_bit_for_bit` still
holds).

- **Beams** (Aurora: teal, the key's tint, bass). On an upright surface
  the tile is read twice as close across and drawn out six times along
  up, so a point is a lane. A streak runs up each lane, its head four
  cells from the next, teal along its length and the key's tint at the
  head. The heads' travel is a clock the bass drives: 0.1 periods a
  second in silence, 2 at a full band, so a kick launches them. The
  streak is 0.3 of a period long in silence and all of it at a full
  band. A top shows the points in the key's tint.
- **Coals** (Flint: bass). The points as lumps: heat falls from each
  center, each lump smolders on its own slow clock, and the bass squared
  lifts all of them, so a kick flares. The color is a ramp by the heat:
  a dark red, then amber, then warm white. Flint's row gives amber for
  the sides and warm white for the tops; the dark red is made from the
  amber, so the row needs no third color.
- **Grass** (Contour: green, warm white, mid). An upright surface is cut
  into rows two cells high (8 cm at 1 m). Each row reads one line of the
  tile, three times as close across, so nearly every lane grows a blade
  from the row's foot: green at the root, warm white at the tip. The
  tips sway with a wave that crosses the room, wider with the mids, and
  lean one way with the gust, the overall loudness smoothed over about
  half a second up and a second down. A top shows the points, moved a
  little by the same wind.

**The flash rule.** A kick reaches a look through the music value the
points' crests take, the one the limiter follows
(`flash::follower::DEPTH_MUSIC`), so nothing new goes round the limiter.
The beams' travel and the wind are motion. Counted on the depth light's
Rust copy, which the GPU test holds to the shader
(`flash_room::the_depth_lights_looks_stay_within_three_flashes`): on the
pulse trains at 2, 4, 8 and 16 Hz no look flashes at all without the
sweep, and with the sweep the worst is 1.5 general and, for the coals at
four times the gain, 1.0 red. Over the eleven songs with the sweep as it
ships the worst is 2.5 general (the far wall, the same under all three
looks, so the sweep's) and 0.5 red (the coals). The limit is 3.

**Stable.** A look is a function of the world point and its clocks. 4 mm
of depth noise moves a streak 0.06 of its packing, a coal 0.03 and a
blade 0.09, and a head move inside an octave changes nothing
(`a_look_stays_put_under_depth_jitter_and_a_head_move`).

**Cost.** Not measured on the headset yet. By the source: the same reads
as the points (one per plane and octave, one typical, six at most), about
thirty-five operations around each for the coals against the points'
forty, about fifty for the beams and the grass, and one more wave per
fragment for the grass's wind. No hash, no transcendental, no discard.

**Not yet.** The grass's rows show on a large flat surface, as shelves of
tufts; a chair or a pile breaks them up, and walls are the room's boxes
in a scanned room. Bubbles, sparkle, glints and meters have ids and draw
as the points. Renders of a recorded room in each look come from
`the_recorded_room_in_each_look` (`FOSFORA_LOOK_REC`, `FOSFORA_LOOK_DUMP`).

## The air per theme (Nov 18, board #5543)

**Why.** The fireflies were the same in every theme. The owner's rule for
the air: "it should be basically replacing the fireflies with <other
thing>, the same way the fireflies work now." So a theme gets no new
effect. It gets the fireflies' pool as its own thing: the same slots under
the same count and cap, the same sprites, the same draw.

**What a theme gives** (`themes::Air`, read through the room's saved
theme like the edges; `fireflies::Look` resolves it for the sim):

| theme | what it reads as | color | band | count | motion |
| --- | --- | --- | --- | --- | --- |
| Aurora | stars | warm white | high | 2000 | twinkle |
| Flint | ash and sparks | amber | bass | 1500 | rise |
| Contour | fireflies | green | high | 2000 | wander |
| Classic | fireflies | by band | each its own | 2000 | wander |
| Drift | plankton | teal | high | 1500 | drift |
| Tide | spray | rose | bass | 1000 | rise |
| Mirror | a mirror ball's specks | warm white | high | 1500 | orbit |

A room with no theme has the fireflies exactly as they were: the air's two
rows are all zero, and zero is the fireflies' own tints, the loudness and
the wander.

- **The color** is a surface palette color, each particle up to a fifth of
  the way toward white by its slot, so 2,000 of them are not one flat
  color. By band (Classic), a particle follows the bass, the mids or the
  highs by its slot and wears that band's color: violet, blue and warm
  white. Those are the colors of the parts of Classic that follow those
  bands (the floor's rings, the tables, the wall's spectrum). The app had
  no colors per band to borrow: the edge bars and the spectrum draw every
  band in the surface's one color.
- **The band** is the level the light and the size follow, where the
  fireflies follow the loudness. The sim reads it from the same uniforms
  it read the loudness from (`u.rms`, `u.bass`, `u.mid`, the higher of
  `u.presence` and `u.brilliance`), which the flash limit has already
  held. The size follows the same band gliding (`Look::level`).
- **The count** is the theme's, under `FIREFLY_CAP`, unless
  `debug.fosfora.fireflies` is set to a number, which wins
  (`Knobs::under`). The density and the governor still take their share
  of it.
- **The motion** is one of five (`fireflies_xr_sim.wgsl`):
  - *Wander*: the fireflies' own loops, untouched.
  - *Twinkle*: born in the upper part of the slab (from 0.3 m over the
    anchor to the top), each on one slow circle at 8 % of the drift speed,
    under a centimeter a second. Each flickers on a period of its own
    between 0.9 and 2.2 s. With the band silent a star dims to 0.55 of
    its peak; with it full, to 0.12. So the highs make the stars
    scintillate, and the light of all of them together stays about level.
    A star is 0.6 of a firefly's size.
  - *Rise*: born in the lowest 15 cm over the floor and let go with the
    band: a dead slot fills at 1.5 a second times the band's level, and at
    4 % of that with it silent. It goes up at five times the drift speed
    (0.3 m/s at the default, each 0.6 to 1.4 of it) with a sway a fifth of
    that, lives 5 to 10 s, fades and shrinks over the last half, and dies.
    It fades in over 0.4 s.
  - *Drift*: every particle carried one way at 1.5 times the drift speed,
    with a wobble of its own. One that leaves the volume comes back in on
    the far side, past the bound where the fade has it unseen. The way is
    fixed in the room (`XR_DRIFT_DIR`). It does not follow the walls'
    streamlines: those run along a curl field with no one direction to
    share.
  - *Orbit*: each particle has a radius, a height and an angle of its own
    and goes round the anchor's upright axis by one phase. The app steps
    the phase (`fireflies::orbit_step`, 0.15 rad/s at the default drift, a
    turn in about 42 s) and hands it to the sim, so the depth light's
    glints can read the same number and turn in step when they are built.

**A change of theme.** A particle carries the stamp of the air it was
born into, its motion and its color. When the theme changes, the
particles of the old air fade out where they are over 0.75 s, still in
their own motion and color, and die. Their slots then fill with the new
air, which fades in. No particle jumps from one motion to another, and
none changes color while it is seen. A track that brings its theme goes
through dark (`themes::Bringing`), and the air now goes through dark with
it: its brightness follows the same veil as the surfaces' light, and the
stamp changes when the theme is laid, in the dark. A theme the wearer
picks is laid at once, with no veil, and there the 0.75 s fade is the
whole change.

**Cost.** No texture read, no flow field and no noise, as before. Per
particle, the transcendentals: wander 6 (as it was), twinkle 3, rise 2,
drift 4, orbit 3 and a square root. Every motion adds three reads of the
aux rows and a few compares to what a firefly cost. So no motion costs
more than the fireflies did, and the count is under the same cap. Not
measured on the headset yet.

**The flash rule.** The air draws nothing on the desktop, so the flash
count does not see it (above). What holds it:

- The light and the size read only levels the limiter holds, as the
  fireflies' did. Rise's release reads the same level. The sim still
  reads no beat, onset or beat phase (`fireflies.rs` pins the list).
- A star's own flicker turns about once a second at most (a period of
  0.9 s or more), a third of the three the rule allows, and every star is
  on a period and a phase of its own.
- The sprites are small. The rule exempts a flash over less than a
  quarter of a 10 degree field, 25 square degrees. A firefly 2 m away is
  about 1 square degree and a star about 0.4, and a 10 degree region
  holds about five of them at 2,000 in the volume. That is an estimate
  from the sizes and the count, not a measurement.

**Tested** on the desktop GPU (RTX 4090, `tests/fireflies_gpu.rs`, the
sim itself behind the loader's preamble, 300 particles):

- Stars: the fastest 0.006 m/s; dimmest against brightest 0.55 with the
  highs silent and 0.12 with them full; the mean opacity 0.62 silent and
  0.57 full; the largest step of one star's opacity in a frame 0.017
  silent and 0.044 full.
- Rise: 57 of 300 alive after 3 s with the bass silent and 295 with it
  full. In 14 s more, 475 died of age and 454 were let go; every one was
  born in the layer over the floor and had faded under 0.01 before it
  died.
- Drift (at 0.15 m/s): 408 wraps in 30 s, each unseen on both sides, no
  particle born again, no velocity more than 20 degrees off the way.
- Orbit: every particle keeps its radius and its height, and all sweep
  the same angle the phase turned.
- By band: a particle's light answers its own band by the full 60 % and
  the two others by nothing.
- A change of theme: the fireflies gone in 0.76 s, 300 stars after it,
  the light of all of them never moving more than 1.9 % of the start in
  one frame.

**Not yet.** The headset: the cost of each motion, the worn look of each
air, and whether a count or a size wants tuning per theme. The depth
light's glints reading the orbit's phase. The key's tint as an air's
color: a color index is the palette's, and the key's reads as white.

## Windows, terraces, sparks and embers (Nov 18, board #5543)

**Why.** Three of the scenes ask for looks the surfaces pass did not
have: windows for Aurora's city, terraces for Contour's hills, and sparks
and embers for Flint's forge. Sparks and embers existed by name but drew
nothing since the particle cloud went (board #5470).

**One face per box.** The surfaces pass draws one quad per box: a
volume's top face, or a plane's face toward the room
(`surfaces::acting_face`). It draws no sides. So a table's windows are on
its top, a city block seen from above, and a terraced table shows the
rings on its top and no strata down its sides. A storage unit taller than
the eye shows nothing of either. Sides need more faces in the pass, which
is a change to its slots and its cost, and was left for a decision.

**Windows** (`SurfaceBehavior::Windows`, id 17). A grid of cells 5 cm
wide (4 cm on a frame) and 1.4 times as tall, a lit pane in the middle of
each. Each window has its own level, a hash of its cell, and is lit when
the lit share passes it. The share runs from 0.2 with the band silent to
0.85 with it full, so the mids set how many are lit. Each third of a
second another quarter of the windows is picked and the highs add up to
0.3 to the share for those, so single windows come on and go off. The
smallest cell is 6.6 pixels of the half-size faces layer at 3 m; under 4
pixels the panes fade into their mean light and by 2 only the mean is
left.

**Terraces** (`SurfaceBehavior::Terraces`, id 18). The contours' land at
half its relief plus a hill, 0.625 high at the face's middle (five
contour intervals) and none from the ellipse the face's edges touch
outward. The contours' own lines close into rings round the top, bent by
the room's land and moving with it. The lines between the index lines
stay at 0.45 at least, since a small top shows one index line at most.
The slopes are the surface's green and the top the high land's warm white.

**Sparks** (id 2) on the beat's envelope as the flash limit shows it. On
the beat the face's middle flashes, the strike. As the envelope falls a
front runs out to the corners and short streaks behind it point away from
the strike and fade. Between strikes the middle glows with the band. The
beat of the bar picks the set of sparks, so four strikes in a bar differ.

**Embers** (id 1). A bed of coals, two octaves of value noise drifting
slowly, glowing at 0.35 with the band silent and brighter with it; over
it motes, one to an 8 cm cell, drifting one way, up to half the cells
shedding one at a full band.

**The heat ramp.** Sparks and embers color by their own light: the
surface's color times (1, 0.45, 0.3) at no light, the color itself at
0.65, and 0.7 of the way to white at full. For amber the deep end is an
orange with a red share of 0.75, under the 0.8 the flash rule calls a
saturated red, and the same holds for every palette color.

**Who can give them.** Windows and terraces are a theme's alone
(`SurfaceBehavior::theme_only`): they are on no kind's list, so the room
editor never steps onto one and the voice and the agent do not know their
names. "Windows" is also what a wearer calls a frame, so the voice must
not hear it as a look. Embers and sparks stay off the wearer's lists as
before, and the voice still gives their stand-ins (the curls, the pulse);
a theme gives them and a saved room that names one now shows it. Only the
held-back looks (astrolabe, fenestra) still show nothing.

**The flash rule.** Windows and embers only gain light as their levels
rise, so the limiter's rule covers them. Sparks move with the envelope
(the front and the cells), which the rule does not cover, so they are
counted like the themes (`flash_room.rs`), as is every part alone.

**Cost per fragment**, counted from the shader, not measured on the
device:

| look | lattice hashes | sines and cosines | square roots | smoothsteps | texture reads |
| --- | --- | --- | --- | --- | --- |
| windows | 3 | 0 | 0 | 6 | 0 |
| terraces | 0 | 10 (the contours') | 3 | 4 | 0 |
| sparks | 3 | 0 | 3 | 6 | 0 |
| embers | 11 | 0 | 0 | 7 | 0 |
| streamlines, for scale | 8 off a line, 16 on one | 0 | 1 off a line, 2 on one | 2 off a line, 6 on one | 0 |

No look loops over cells or reads a texture. A hash is five multiplies
and nine shifts and xors on integers. The sparks' and the embers' counts
include the heat ramp's two smoothsteps.

### The wash (Nov 18, board #5543)

**Why.** Tide is a shore at dusk and its floor is the water: "violet
water, rose foam line, one advance and retreat per bar". The floor had
the curls.

**What it draws** (`surface_fx.rs`, id 19, a theme's alone like the
windows). Once a bar a wave runs up the floor and drains back. It runs up
over 0.4 of the bar and drains over the rest, half a cosine each way, so
it sets out, turns and arrives at no speed. Behind its edge the water is
the floor's color, a little brighter in a sheet that thins over half a
meter, with faint sets lying in it that the wave passes over. A line of
foam rides the edge, and the
wave leaves another at its furthest reach, which fades until the phase
wraps. The edge is bent along the shore by two slow sines, 6 cm and 3 cm.

**Two colors.** The water is the row's color. The foam is the color the
room's theme gives its tables (`surface_fx::wash_foam`): rose over Tide's
violet, and warm white with no theme. A slot has one color, so the foam's
rides in the wave's frame (`Audio::wash`) and lands in the first rings
row, where the contours keep the high land's color. Nothing new is saved.

**Where it comes from** (`surface_fx::wash_front`). The water comes from
where the wearer faces toward where they sit, from the face's far side,
and its run ends under the head, so a wave stops short of the wearer. The
side is taken from the first facing that has a level part (a wearer lying
back faces the ceiling and names none). It changes only when the wearer
has turned past 60 degrees, and then on the frame the wave's phase wraps,
when the water is out and no foam is lit. A side picked from the head's
place on the floor would flip for a wearer in the middle of the room.
With no facing or no head it runs along the face's longer side over 0.8
of it.

**Nothing jumps** (`surface_fx::Shore`, one for the room, stepped every
frame). The wave keeps a phase of its own that runs after the bar's at
one bar a second at most and never backward. When the analysis moves a
bar on, the wave catches up at that pace; when it moves one back, the
wave waits for it. When the bar's phase stands still for half a second
(no music) the wave runs on at a wave every five seconds. The row's band
(the bass) reaches it only as the land's gliding level (1.5 s): how far
up its run a wave comes, 0.45 to 0.85, and how bright the water and the
foam are. The beat, the downbeat and the frame's own bass do not touch
the water.

**The sky's mirror** (the owner, Nov 18: "it could just reflect the
violet dusk sky slightly"). Tide's sky is the pulse on its walls and
ceiling. Under the water and the foam, never over them, the floor shows
that pulse's level at the theme's share (`Theme::reflection`, 0.2 for
Tide, the file's value) of the walls' own peak, in the floor's color,
which is the sky's violet. It is full where the view grazes the floor and
gone toward the feet, by the cosines the Aurora floor's reflection fades
by (`sky::REFLECT_COS`). At a full pulse it is under a tenth in alpha. It
is the one part of the floor that moves with the downbeat, as the sky
does.

**The edge for the depth light.** Tide's sparkle "flares as the wash
front passes under it". `wash_front(face, &audio.wash, level)` gives the
world point the water comes from (`origin`), the way it runs (`toward`),
and how far along it the edge (`front`) and the foam line (`peak`) are. A
world point is `(p - origin).dot(toward)` meters along the run.

**The flash rule.** A floor is the largest face in the room, and the
wave is the look's own motion, which the limiter does not hold back, so
it is counted like the themes (`flash_room.rs`). Eleven songs on Tide,
the limiter on, each part alone too: 66 rows, no failing region; the
whole theme 2.5 a second at worst, as before the wash, and the wash alone
1.5. A first version drew a much brighter sheet behind the edge over dark
water (0.3 of the sheet) with the sets riding the wave: a region counted
2.5 on a fast song, the wave's coming and its going each a rise and a
fall. Now the deep water is 0.8 of the sheet and the sets lie on the
floor and creep, so a wave passing a place is one rise and one fall. The
mirror moves with the sky's pulse and counts with it.

**Cost per fragment**, counted from the shader: three sines, one square
root and eight smoothsteps, no hash, no loop and no texture read. The
wave's place, its foam's light and the band are worked out once a frame
on the CPU.

**Not measured:** the device. The foam line is 6 cm wide; whether it
holds in the half-size faces layer at the far wall is for the headset.

## The contours (Nov 16, board #5533)

**Why.** The owner asked for "gentle rolling and topographic scenery"
with the classical track. Nothing in the catalogue draws land, and every
look so far starts its own picture on each face, Aurora's sky aside.

**The look** (`surface_fx.rs`, behavior 16, `contours`). Contour lines,
as on a topographic map, over one landscape that fills the room.

- **One landscape.** The land is a height field in world space: three
  sines of the fragment's position, two of them with their phase bent by
  another sine, so the hills are not a grid. Every face samples it where
  it stands. A line that reaches a wall's edge goes on across the next
  wall, the floor or the ceiling, and no face fades toward its edges. Each
  face's quad is lifted 2 cm off its surface, so at a corner the two quads
  are 2.8 cm apart, and across that gap the height changes by under half
  the step between two lines.
- **Lines.** A line runs where the height crosses a multiple of 0.125
  (the land runs from -1 to 1). The distance to it in meters is the
  height's distance over the slope along the face, from the field's own
  gradient, so a line is 6 mm wide on a steep slope and on a gentle one,
  softened over a pixel. Every fifth line is an index line.
- **Scenery.** The color runs from the surface's own on low land to warm
  white on high land.
- **Gentle.** The land drifts on its own clock, a few centimeters a
  second, and the beat does not speed it up. The music reaches it only
  through values that glide (`Terrain`): the band's level over 1.5 s and
  the bass over 0.8 s. The level raises the land by up to two line steps,
  so the lines spread outward from the peaks like a tide over a map. It
  also lights the lines between the index lines (none under a level of
  0.15, all by 0.6, at 0.6 of an index line's light) and widens every
  line from 0.6 of its width to all of it. The bass swells the relief by
  up to 12 %. On a downbeat, at most once in 12 s and only with the level
  over 0.25, one faint line sets out from the low land and climbs to the
  peaks over 7 s, fading in and out. From one frame to the next the level
  moves a line by under 2 % of the step between two lines.

**Cost.** A fragment costs ten sines and cosines, five dot products of
three, one normalize and three smoothsteps. No texture read, no hash, no
loop, and the gradient costs no sine the height did not need. The
streamlines cost eight lattice hashes off a line and sixteen on one. So
a contour fragment should cost what a streamlines fragment costs or
less. Not measured on the headset yet. Drift, with the streamlines on
every wall, cost 0.1 ms of App over the saved room, and Aurora, with a
ported effect on the walls, the ceiling and the floor, 0.4 ms
(`MEASURED.md`, Themes). Contour lights the same three kinds of plane as
Aurora with a cheaper fragment, so expect between the two.

**In the catalogue.** Last on the lists of a floor, a wall and a ceiling,
where the land runs from one face onto the next. A table, a frame and
other things show too small a piece of it to read as land; the knob still
puts it there. The room file and the knob call it `contours`, and so does
the voice ("every wall contours"): the grammar's vocabulary is built from
the offered list. The hosted agent's instruction describes it and its
room lists carry it. The on-device model does not offer it: its
candidates come from its spec, which was written before the look, and the
grammar's direct command is enough. Its band is the rms by default and
the Band row changes it; the color row sets the low land's color.

**Flashes.** The look maps the level to a position as well as to light,
which is outside the limiter's proof, so it is measured as the themes
are. Without the limiter it stays at half a flash a second on the pulse
trains: nothing in it can jump.

## Photosensitive safety (Nov 14, board #5451)

**Why.** Worn on Aurora, the flashing was too much for someone with
photosensitive epilepsy. In a headset the picture fills the view, so this is
a safety requirement, not a taste setting: while the limit is on it holds
for every theme and every behavior, today's and any added later, whatever
the song does.

**The wearer's choice** (board #5534). The limit costs the show ("Keeping
the show" below), so it is the wearer's setting, and it is on unless they
turn it off. A new wearer is warned that the show flashes and asked, before
anything flashes, whether to limit it; a wearer who does not answer has the
limit. The hand menu's "Limit flashing" row and the spoken "limit flashing"
and "full show" change it later. It is saved in `hand_menu.json` as
`"limit_flashing"`. `XR_DESIGN.md`, "The flash question", has the card and
its rules. Everything below describes the app with the limit on. With it
off the music drives the light as the analysis has it, and nothing here is
promised.

**The rule** (WCAG 2.3.1, three flashes or below threshold). No more than
three general flashes and no more than three red flashes in any one-second
period. A general flash is a pair of opposing changes in relative luminance
of 0.1 or more, where the darker state is under 0.8. A red flash is a pair of
opposing transitions involving a saturated red (`R / (R + G + B)` at least
0.8) where `(R - G - B) x 320` changes by more than 20. The rule exempts
flashes over less than a quarter of a 10 degree field; the sky and the walls
fill the view, so nothing here takes the exemption. `flash.rs` counts it per
region: a zigzag on each region's luminance and red value with the rule's
thresholds, two transitions to a flash, over the frames of each second.

**What flashed.** Every way the audio reaches the light, and how much it
flashed on its own before the limit: the worst second in any region on the
pulse trains below, general / red. Real songs, measured outside the repo,
flashed on the same paths.

- *The sky and its reflection* (Aurora, the port): seven curtains, each as
  bright as its band times `1.5 + 0.5 rms` and wider with it, all times
  `1 + 0.8 onset` (the onset jumps in one hop) and a 10 % beat breathing on
  the beat's phase, in the key's tint: 16.5 / 16 on the sky, 16.5 / 0 on its
  reflection, in two thirds of the regions. With the key on C or A the tint
  is red and the low curtains a saturated red.
- *The wall spectrum* (Classic) and *the edge bars*: the mel bands as bars,
  each up at once and down at 2.5 wall heights a second: 8 / 0 on the wall
  spectrum, 0.5 / 0 on the edge strips and the tunnels.
- *The rings*: born on each beat, 0.8 s of decay, spreading at 2.5 m/s, over
  a glow with the bass. They flashed wherever a theme has them (the floor,
  Drift's ceiling, Tide's frames): up to 16 / 0.
- *The pulse*: the downbeat's envelope over 0.15 of its band. At most once a
  bar, so it flashed only on the fastest pulses: 4 / 0 at 16 Hz.
- *The streamlines and the curls*: their band never takes them under 0.6 of
  their light, and the lines are thin: 0.5 / 0 at most.
- *The other ports* (hand edits only): prism zooms its kaleidoscope with the
  bass, astrolabe, bezel, fenestra, reticle and tessera build on the bar's
  clock, shards breaks on the kick: up to 21 / 0 (prism).
- *The depth light's points* and *the particles* (the band's level, the beat
  envelope, the beat and onset at a birth, the beat's phase): not drawn on
  the desktop.
- *Immersion* follows the song's loudness over 2 s up and 4 s down (a drop:
  0.25 s, then 8 s held): too slow to flash.
- *The debug panel's meters* and *the beat strobe* (`debug.fosfora.flash`).

**Where the limit sits.** `flash::Limiter`, one place between the analysis
and everything the audio lights. `app.rs` hands it the frame's hop as the
analysis made it, and every look reads what comes out: the features, the mel
bands, the triggers, and the values the frame works out from them before
they light anything (the beat and downbeat envelopes, the spectrum's bars and
glow, the key's tint, the depth light's music). The world effect's sim and
the rings take the same hop.

**How it holds the count.** Every value the limiter lets through moves in one
shared direction at a time: in a rise any value may go up or hold and none
may go down; in a fall the other way round. The direction turns at most four
times in any second, each direction lasting 0.1 s at least. A look's light
in a region goes up with the levels it reads (more bass, a brighter curtain;
more onset, a brighter sky), and so do sums, products and curves of them. So
within a direction each region only brightens or only dims, and it can turn
only where the limiter turns: four turns, five runs, at most five
transitions, two and a half flashes, general or red. The sixth transition
the rule allows is left for what the looks do on their own, Aurora's curtains
drifting across a region; while three of the second's four turns are spent,
the looks' clocks run at half speed so that drift stays within it.

**Keeping the show.** A rise takes a value up at once, so a hit lands on its
own frame. A beat turns the direction to a rise as it fires. A level falls
fast after a sparse hit and more slowly the more often it is hit (0.05 s to
0.4 s from 2.5 to 5 hits a second), so a run of hi-hats rides on a swell
instead of strobing. Every beat up to 120 BPM goes through, each one rise
and one fall; from 125 BPM every other beat does (half time), and a beat
held back leaves the light as it is for a quarter second so the next one has
its turns.

**What the wearer loses.** In busy music the light moves in steps a few times
a second: a value that wants the other way waits for the next turn. Fast
songs land at half time. Nothing pulses on the beat's phase any more
(Aurora's 10 % beat breathing, the embers' shedding after a beat, the
sprites' beat size), since the phase runs on its own clock and no direction
could hold it; the beat, the downbeat and the drop now stay up until the
light falls instead of one frame. The spectral shape, the key and the pans
glide over 0.3 s, so hues drift instead of jumping. A port that moves its
shapes with the music (every port but Aurora) reads its levels gliding over
0.5 s, so prism's zoom swells with the bass instead of snapping. And in busy
passages the curtains, the streaks and the curls drift at half speed. The
edge bars follow the turns too, so they step with them: 0.5 flashes a second
before on the pulse trains, up to 2.5 now, within the rule. Worn, that
stepping read as bars out of time with the music, and
`debug.fosfora.barsfree` is for it (the edge bars' heights, below).

**What it does not cover.**
- *The real room* behind the passthrough: the limit dims nothing it does not
  draw. Immersion moves the passthrough too slowly to flash, and the
  measurement takes the room black, the worst contrast.
- *Light that runs on by itself*: a ring far from where it started, a burst
  of embers, the tunnel's rungs (the bars of the last 0.6 s). A beat goes
  through only when the direction may rise now and fall within a quarter
  second, so these start on the limiter's turns, and they are measured, not
  proven.
- *Light that is not monotone in the music*: a bar's tip and cap line moving
  through a region, a look mapping a level to a hue or a place. Measured.
- *A look built on the bar's clock* (astrolabe, bezel, fenestra, reticle,
  tessera, all hand edits): the clock passes as the analysis has it, a bar a
  second or longer, and their steps on it are measured, not proven.
- *The looks' own motion*: measured with the music held still, at most one
  flash a second.
- *The depth light and the particles*: not drawn on the desktop, so
  measured on neither; they read only what the limiter lets through.
- *The beat strobe* (`debug.fosfora.flash`) is a strobe by design and runs
  only with the limit off.

**Measured** (`flash_room.rs`, desktop GPU). The room scanned for the sky
tests (4.6 by 3.8 m, 2.45 m high) with a table ahead, a frame on the right
wall and a chair on the left, seen from a seated eye as a cube of six
160-pixel faces, every way the wearer can look, at 72 frames a second with
the room black and every look at immersion 1. Regions 10 degrees square,
every 5 degrees (1,734). The worst flashes a second in any region, general /
red, the limiter off then on.

Pulse trains, every level at full depth together, a beat on each pulse, the
key on C so Aurora's tint is red (6 s each):

| theme | 4 Hz | 8 Hz | 12 Hz | 16 Hz |
| --- | --- | --- | --- | --- |
| Classic | 4.5 / 0 -> 2.5 / 0 | 8.5 / 0 -> 2.5 / 0 | 12 / 0 -> 2.5 / 0 | 16 / 0 -> 3 / 0 |
| Drift | 4.5 / 0 -> 2.5 / 0 | 8 / 0 -> 2.5 / 0 | 3.5 / 0 -> 2.5 / 0 | 1 / 0 -> 3 / 0 |
| Tide | 3 / 0 -> 2.5 / 0 | 2.5 / 0 -> 2 / 0 | 3.5 / 0 -> 2.5 / 0 | 4 / 0 -> 2.5 / 0 |
| Aurora | 4.5 / 4.5 -> 2.5 / 3 | 8.5 / 8.5 -> 2.5 / 2.5 | 12.5 / 12.5 -> 2.5 / 2.5 | 16.5 / 16 -> 2.5 / 2.5 |
| Mirror | 4.5 / 0 -> 2.5 / 0 | 8 / 0 -> 2.5 / 0 | 0.5 / 0 -> 2.5 / 0 | 0.5 / 0 -> 3 / 0 |
| Flint | 5.5 / 0 -> 2.5 / 0 | 9 / 0 -> 2.5 / 0 | 12.5 / 0 -> 2.5 / 0 | 16.5 / 0 -> 2.5 / 0 |
| Contour | 0.5 / 0 -> 2 / 0 | 0.5 / 0 -> 1.5 / 0 | 0.5 / 0 -> 1.5 / 0 | 0.5 / 0 -> 0.5 / 0 |

Each behavior alone on every surface that offers it, no edge bars, the worst
of the four pulse trains, off -> on: the rings 16 -> 3, the wall spectrum 8
-> 2, the pulse 4 -> 1, the streamlines 0.5 -> 0, the curls 0 both ways,
Aurora 16.5 -> 2.5, prism 21 -> 2.5, shards 16.5 -> 1, astrolabe 16 -> 1.5,
bezel 11 -> 1.5, fenestra 10.5 -> 2, reticle 6 -> 1.5, tessera 15 -> 1.5.
With the music held still (every level at 0.3, then 0.8, no beat) every
theme stays at 1 or under, limited or not.

Flint and Contour (board #5533) were measured on Nov 16, after the five.
Contour's own light hardly moves; what the limiter leaves it is the edge
bars on its table. Flint lights the walls and the ceiling with shards, so
without the limiter over a thousand of the 1,734 regions fail on every
train. Its own motion is also the largest of the themes: with the music
held still it reads 1.5 flashes a second at a level of 0.3 and 2.5 at
0.8, where the others stay at 1 or under, because shards' cell edges move
on their own clock and with the onsets. Over eleven songs (the eight
dumped ones and the three bundled tracks) Flint's worst is 3 flashes a
second, on one song, in a region of the ceiling overhead, and 2.5 or
under on the other ten; on its own rock track it reads 2. Contour's worst
is 2.5, on one song, and 1 on its own track.

A ceiling's shards draw at 0.6 at most while the limit is on (Themes,
Flint). With that, a Flint ceiling raised to full strength by hand stays
at 3 on the same eleven songs, where it reached 3.5 on one before
(`flints_ceiling_raised_by_hand_stays_within_three_flashes`). Shards
picked by hand at full strength, in their own colors, on every surface
that offers them read 2.5 at worst on the pulse trains and 3 on two of
the eleven songs, one of them at 3 red flashes as well. They read 1
before they were on a wall's and a ceiling's list. Walls are not capped.
With the eye 1 m from a wall, Flint's walls at full strength read 3 on
four of the eleven songs, in regions of that wall, and none over.

The same runs over real songs (`the_dumped_songs_stay_within_three_flashes`,
eight tracks, fast ones and dense hi-hats among them) read dumps kept
outside the repo, and their numbers stay there too. Every theme stayed
within the rule on all of them. Two hand edits went just over it, each in
one region on one song: astrolabe on the ceiling and fenestra on a wall.
Both build their picture on the bar's clock and the buildup, which cross
the look's own thresholds where the limiter does not turn.

**Held back** (board #5462). Until those two are bounded they are not
offered: `SurfaceBehavior::held_back`, and `OFFERED` without them. The
room editor's cycle steps past them as it steps past embers and sparks,
the voice answers "Astrolabe is not available" (`VOICE_DESIGN.md`,
"Looks that are not offered"), and the agent's lists leave them out. They
stay in the catalogue, the knob and the flash checks above. A saved
surface on one shows nothing and the lanes log it once, as a surface left
on embers does. The floor's cycle is now the rings, the streamlines, the
curls, tessera, prism, aurora; the wall's ends at aurora and bezel; the
ceiling's at aurora; the frame's at bezel and reticle.

**The edge bars' heights (Nov 16, board #5532).** Worn, the bars along the
room's edges did not seem to be in time with the music. They were in time
with the limiter. Replayed on eleven songs, a bar behind the limiter never
shows 41 to 76 % of the rises the analysis makes on its band and shows the
rest up to 264 to 417 ms late (the 95th percentile). On the frame of a hit it
is where the analysis has it 30 to 62 % of the time. Different bars also
move more alike than the music has them, and at 124 BPM a third of the beats
go through. The analysis is not late: on a click track the mel bands rise
within a hop of the click and of the beat it fires. The numbers are in
`MEASURED.md`, "The edge bars against the music".

`debug.fosfora.barsfree`, read at launch, says what the edge bars' heights
read. At 0, the default, nothing changes: they are the spectrum's bars as the
limiter shows them. At 1 the edge bars keep bars of their own on the mel
bands as the analysis made them (`edge_bars::Heights`), so each bar is at the
analysis's height on the frame of its own hits. Only the heights are free.
The wall spectrum's bars and glow, the mel bands every look reads, the
features and the triggers stay behind the limiter, and the edge bars read
nothing else of the music. The default is one constant,
`flash::BARS_FREE_DEFAULT`.

Free heights are outside the proof above, so they are bounded and measured
instead. A bar rises at once and only its fall takes time, so the fall speed
bounds how fast the strips can dim a region of the view. Free, the strips'
bars fall at 1.25 of the tallest bar's height a second
(`edge_bars::FREE_RELEASE_PER_S`), half the wall spectrum's 2.5. At 2.5,
short hits three or more times a second on every band took a region beside a
strip to 3.5 and 4 flashes a second. At 1.5 the strips alone reached exactly
three. At 1.25 they stay at two in the themes' colors. The tunnels' rungs
keep bars that fall at 2.5, since a rung is a small share of its face and
the tunnels alone count half a flash either way.

What the wearer gets at 1:

- *On the face and standing.* Every bar rises on its own hits, on their
  frame, and falls at half the speed it did: a full bar takes 0.8 s to come
  down, not 0.4. On a band that is hit fast the bar rides higher and swings
  less. On the eleven songs 18 to 40 % of the rises of a tenth or more show
  as less than a tenth for that reason, against the 41 to 76 % the limiter
  never shows.
- *In.* The rim and every rung follow the analysis. A hit starts at the rim
  on its frame and travels in a rung every 60 ms, as the tunnel was designed,
  where at 0 it starts on the limiter's turns.
- *Both.* The bars stop moving together, and stop landing on every other
  beat above 120 BPM.

Counted on the GPU as above, each mode forced on every surface as the Edges
control forces it, on the four pulse trains and on twelve trains of short
hits (two frames long, 1.5 to 6 a second, what lets a bar fall furthest
between two rises). The worst flashes a second, general / red, over the five
themes and the sixteen trains, the heights limited then free:

| edges on every surface | the bars alone | each theme whole |
| --- | --- | --- |
| on the face | 2.5 / 0 -> 2 / 0 | 3 / 3 -> 3 / 3 |
| standing | 2.5 / 0 -> 2 / 0 | 3 / 3 -> 3 / 3 |
| in | 0.5 / 0 -> 0.5 / 0 | 3 / 2.5 -> 3 / 2.5 |

Of the 240 runs of a whole theme, four count half a flash more with the
heights free than limited (2 to 2.5) and eleven count half a flash less. The
threes are the ones the themes already count with the bars limited: 16 Hz
pulses on Classic, Drift and Mirror, and Aurora's red on the 4 Hz pulses and
on hits at 2 and 4 a second.

The same on real songs
(`the_dumped_songs_with_free_edge_bars_stay_within_three_flashes`): the
three bundled tracks and the eight kept outside the repo, each theme whole,
in the theme's own edge modes and with each mode forced on every surface,
220 runs each way. None is over three, limited or free. In the themes' own
modes, and with the tunnels on every surface, the free heights never count
more than the limited ones. With strips forced on every surface they count
half a flash more in six runs. Five of those are Aurora going from 2.5 to 3:
its sky surfaces then carry strips in the key's tint over the sky. That is
the worst case at 1, and it sits on the three.

In `cargo test`, with no GPU, the bars' own light is counted on the shaders'
Rust copies in white, brighter than any color a bar takes, with nothing in
front of the bars: the strips reach three flashes on hits 2.25 to 2.75 times
a second and count nothing faster, and the tunnels count none.

Not covered at 1: a room that puts a tall strip nearer the eye than the
measured one does, so its bars fill more of a region, and `edgebarsgain` or
`edgebarsheight` turned up. In white the strips' count sits on the three
with nothing to spare. So the default stays 0 until both settings have been
worn, and the fall speed is the number to lower if a room needs more room.

**Tests.** In `cargo test` (`flash.rs`, no GPU), the count:
`a_square_wave_flashes_at_its_rate_and_three_a_second_pass`,
`small_and_bright_changes_are_not_flashes`,
`a_slow_wave_counts_by_its_swings_not_its_speed`,
`red_flashes_need_a_saturated_red_and_a_swing_of_twenty`,
`the_window_is_a_rolling_second`,
`a_steady_or_unreadable_region_never_flashes`. The construction, on pulse
trains from 2 to 30 Hz and on noise in every value:
`the_direction_turns_at_most_four_times_in_any_second`,
`every_value_moves_with_the_direction_or_holds`,
`any_light_that_rises_with_the_values_stays_within_the_rule`. The show:
`a_kick_at_120_bpm_lands_on_its_beat_and_clears_before_the_next`,
`a_fast_run_of_hits_rides_on_a_swell`,
`every_beat_goes_through_to_120_bpm_and_every_other_one_faster`,
`beats_go_through_at_most_every_beat_gap_and_the_phase_holds`,
`the_looks_slow_while_the_turns_are_spent_and_shapes_glide`; and
`off_it_passes_everything_as_it_is`, `the_knob_and_its_lines`,
`the_slots_are_the_core_features`. On a GPU (`flash_room.rs`, `#[ignore]`d,
`cargo test -p fosfora-xr --lib flash_room -- --ignored --nocapture`), each
failing on any region over three:
`every_theme_stays_within_three_flashes_on_the_pulse_trains`,
`every_part_alone_stays_within_three_flashes_on_the_pulse_trains`,
`every_behavior_on_every_surface_stays_within_three_flashes`,
`the_looks_own_motion_never_flashes`, and
`the_dumped_songs_stay_within_three_flashes` (`FOSFORA_FLASH_DUMPS`). For
the edge bars' heights (board #5532), in `cargo test`:
`edge_bars::the_heights_are_the_limits_or_free_the_analysis_own`, and in
`flash_room.rs` `free_bars_rise_on_the_frame_of_their_own_hits`,
`free_edge_bars_alone_stay_within_three_flashes` and
`free_strips_falling_as_fast_as_the_wall_spectrum_pass_three_flashes`. On a
GPU: `the_edge_bars_with_free_heights_stay_within_three_flashes` and, with
the dumps, `the_dumped_songs_with_free_edge_bars_stay_within_three_flashes`.
With the dumps and no GPU,
`the_bars_of_the_dumped_songs_against_the_analysis` prints the bars' timing
tables.

**The knob.** `debug.fosfora.flashlimit 0|1` overrides the wearer's setting
while it is set: 0 turns the limit off for an A/B, 1 turns it on whatever is
saved. Unset, the setting rules, and with nothing saved the limit is on
(`flash::decide`). Read at launch and polled once a second; the log says
`flash limit: on · every audio-driven light turns at most 4 times a second
(each direction at least 100 ms), beats 380 ms apart at least`, or `flash
limit: OFF, the music drives the light unlimited`, and then which decided:
`· the wearer's setting`, `· the wearer has not chosen`, or `·
debug.fosfora.flashlimit 0 overrides the wearer's setting`. At launch and
on each change. The line ends with the edge bars' heights: `the edge bars'
heights limited (debug.fosfora.barsfree 0)`, or `the edge bars' heights
follow the analysis, falling 1.25 heights a second (debug.fosfora.barsfree
1)`. That knob is read at launch only.

**Not yet.** The device: the worn look of each theme with the limit on, its
cost (a few hundred values a frame on the CPU, nothing on the GPU), and a
screen capture with the limit off and on. The depth light and the particles
measured on the headset's own frames.
