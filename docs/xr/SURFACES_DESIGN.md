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
room turns on and the rows under the pointer stay put.

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
