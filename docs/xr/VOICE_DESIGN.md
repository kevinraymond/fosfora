# The voice path: design (board #3751)

*A design note, not a build. The numbers it needs (on-device transcription
time, the mic path's cost) are being measured; where a figure is still
open it says so. Kevin decides the shape; this note gives him the options
and a recommendation.*

## Why a voice path

Everything in the room today is driven by the hands: a pinch throws, a
drag moves the cloud's home, the palm menu switches effects, the editor
assigns a surface by pointing at it. That is right for the hands-first
rule (I5), and it leaves two things out that one more input would give:

1. **Access.** A wearer who cannot hold a pinch steady, or cannot raise a
   palm to the face, can still say "next effect", "the desk in amber",
   "edit the room". Every spoken command maps onto an action the hands
   already have, so the voice path adds no new behavior to learn, only a
   second way in.
2. **Intent.** "Make the wall calmer", "something warmer on the table",
   "like a campfire" are not commands; they are what a wearer means. A
   language model that knows the room (its surfaces, the catalogue, the
   current assignments) can turn that into the same actions.

One path serves both: the microphone, a push-to-talk gesture, speech to
text on the headset, and **one intent layer** that either matches a small
spoken grammar or hands the sentence to a model. The two consumers differ
only in who turns words into actions.

## The path

```
mic (AAudio, VOICE_RECOGNITION preset)
  → push-to-talk window (the left fist held, or a thumb tap on the index)
  → 16 kHz mono buffer, 1 to 6 s
  → speech to text on the headset (Whisper tiny.en or base.en)
  → the sentence
  → the intent layer
      ├─ the grammar: a fixed set of phrases → an Action (no network)
      └─ the agent: the sentence + the room's state → a model → Actions
  → the same Action enum the hand menu and the editor use
  → the in-world label shows what was understood and what happened
```

### The microphone

The Quest's own microphones are a poor music source (board #3248: beam
formed, high-passed, the speaker output cancelled) and a good speech
source for the same reasons. The voice path uses the AAudio input with
the `VOICE_RECOGNITION` preset the runtime defaults to, which is the
opposite of what the music path wants; the two are separate streams and
never share one. `RECORD_AUDIO` is asked for at launch when the voice path
is on (`permissions.rs`), with the same pickup as the mic source.

### Push to talk, not always listening

An open microphone that transcribes everything is wrong for a room with
other people in it, costs CPU all the time, and makes the wearer wonder
what was heard. Push to talk: the window opens on a gesture and closes
when it ends or at 6 s. Candidates, in the vocabulary of the gesture map
(`XR_DESIGN.md`, "Hands: the gesture vocabulary"):

- **The left fist, held.** Already a pose (`pose.rs`, Murmur's hold). In
  Flux effects it is unassigned. Clear to see, easy to hold for a
  sentence, hard to make by accident.
- **A thumb tap on the index** (`XR_META_hand_tracking_microgestures`, the
  `tap_thumb` action is wired and reserved). Starts a window that closes
  on silence or at 6 s. Lighter than a held fist; needs the tap to be
  learned.

Recommendation: the fist to open, release to close, with the thumb tap as
a second opener once the fist has proven itself worn. While the window is
open the in-world label reads "Listening…" and the beat pulse pauses on
the label, so the wearer sees the microphone is live.

### Speech to text on the headset

On the headset, not in the cloud: the sentence never leaves the device
unless the agent is asked, the latency is bounded by the CPU and not by
the network, and the grammar works in a room with no Wi-Fi. Whisper
(`whisper.cpp` through the `whisper-rs` crate, Unlicense; the models are
MIT) is the candidate: `tiny.en` (75 MB) and `base.en` (142 MB), English
only, which the command grammar is anyway.

**Budget.** A 3 s utterance should come back in under a second, so the
label can answer while the gesture is still fresh. Measured on the Quest 3
(the spike in `MEASURED.md`, "On-device speech to text"): `tiny.en` with
whisper's default 30 s encoder window takes 0.5 s on 6 threads and 0.7 s
on 4; `base.en` with the default window 1.05 s on 6 and 1.46 s on 4, over
the budget; `base.en` with the encoder window cut to the clip's length
plus 1.28 s (`audio_ctx`) takes 0.16 to 0.19 s on 6 threads, 0.19 to 0.23
on 4, and still 0.37 to 0.43 on 2, with every transcript right. `tiny.en`
with the cut window is unsafe (a repetition loop of 4 s on one clip, a
dropped word on another). So the model is **`base.en` with the cut
window, on 2 to 3 threads** (the render loop keeps the rest), at about
0.3 to 0.4 s per utterance and 204 MB peak, loaded in 0.13 s. Two checks
remain before V1 commits: the same measurement inside the running app
with the renderer live, and real recorded voice for words like "bass",
which the synthetic clips heard as "base" (the grammar snaps such words
to its own list).

**Where it runs.** On its own thread, after the window closes, with the
threads the render loop does not need (the loop's CPU is 2 ms of a
14 ms frame; four cores are idle). A transcription in flight never blocks
a frame. The model loads once at launch, after the room, so the first
command does not pay the load.

**Memory.** `base.en` with the cut window peaks at 204 MB (331 MB with
the default window; `tiny.en` 120 and 216 MB). The app's budget on the
Quest is comfortable for that.

**Licensing.** `whisper-rs` is Unlicense, `whisper.cpp` MIT, the models
MIT; all pass `deny.toml`. The model file is downloaded by the build (as
the effects' assets are packed) and bundled in the APK, which grows by
the model's size; or fetched on first use into the app's files, which
keeps the APK small and needs a network once. Recommendation: bundle
`base.en` (148 MB) and ship `libc++_shared.so` with it (the whisper build
links it dynamically).

### The intent layer

One module, `intent.rs`, desktop-tested, with two halves and one output:
the `Action` enum the hand menu already produces (`hud.rs`), extended with
the editor's assignments (a surface, a behavior, color, band, strength)
so a sentence can do what a pointed hand does.

**The grammar.** A small set of phrases, matched after normalizing the
sentence (lower case, punctuation off, numbers as words). Each phrase is a
template with slots that take the catalogue's own names, so the grammar
is generated from the data the app already has and never drifts from it:

| Phrase | Action |
|---|---|
| "next effect" / "previous effect" | `NextEffect` / `PrevEffect` |
| "embers" / "flock" / any world effect's name | switch to it |
| "edit the room" / "stop editing" | `SetEditRoom(true/false)` |
| "the desk in amber" / "amber on the desk" | the pointed-at-or-named surface: color |
| "the wall on the bass" | the surface: band |
| "the table dimmer" / "brighter" / "half" | the surface: strength step or value |
| "streamlines on the table" / "every table streamlines" | the surface, or the kind, behavior |
| "nothing on the ceiling" / "clear the walls" | behavior none |
| "particles on" / "off", "music play" / "stop" | the menu toggles |
| "rescan the room" | `Rescan` |
| "what is this" (pointing) | the label reads the surface's assignment |

Surfaces are named by their kind's friendly name (`surfaces::friendly_name`:
"desk", "table 2", "side wall") or by pointing: with no name in the
sentence the surface under the beam is meant, as the editor's rows do.
A sentence the grammar does not match goes to the agent when it is on,
else the label says "Didn't catch that" with the sentence under it.

**The agent.** The sentence, the room (surfaces with kind, friendly name,
size and current assignment), the catalogue, the current effect, and the
grammar's own action list go to a language model with one instruction:
answer with actions from that list, or with one sentence to show. The
model runs off the headset over the network (a hosted model through its
HTTP API; the key lives in the app's config, never in the repo), so the
agent is the one part of the path that needs Wi-Fi, and it says so when
there is none. Latency: one round trip, 1 to 3 s; the label reads
"Thinking…" meanwhile, and a spoken command that the grammar matches
never waits for it. Every action the agent returns is applied through the
same path as the grammar's, logged, and shown on the label, so the wearer
always sees what the room did and why. Nothing the agent says can do what
the hands cannot: the action list is the boundary.

The agent is the second step, not the first. The grammar alone already makes the
room usable without steady hands, and it works with no network; the agent
makes "something like a campfire" work. Build the grammar first; the
agent is the grammar's consumer with a model in front of it.

### What the wearer sees

- Window open: "Listening…" on the label in front of the far hand, the
  mic's level as a thin bar under it (the wearer sees the headset hears).
- Transcribed: the sentence on the label for 1.5 s.
- Acted: the same text the hands' actions show ("desk: streamlines ·
  amber · mid"), after the sentence.
- Not understood: "Didn't catch that" with the sentence, so the wearer
  can rephrase or point.
- Agent: "Thinking…", then the actions' labels, or its one sentence.

Everything is also in the log (`voice: heard "…" → <actions>`), as every
gesture is.

## Steps

1. **V1, the window and the text.** The fist opens a push-to-talk window
   on the AAudio voice stream; the transcription runs on its own thread;
   the label shows "Listening…" and the sentence. Gate: a 3 s sentence
   on the label within the budget, measured unworn from a WAV fed through
   `debug.fosfora.voicefile` (the same device as the music file path), and
   the window's cost on the frame (expect none).
2. **V2, the grammar.** `intent.rs` with the table above, generated from
   the catalogue and the room, desktop-tested; the actions applied through
   the menu's and the editor's paths. Gate (worn): the ten phrases, each
   once, each doing what the hands would.
3. **V3, the agent.** The model call with the room's state, the action
   list as the boundary, the label's "Thinking…", a network-off message.
   Gate (worn): five sentences the grammar cannot match, each ending in a
   sensible room; the round trip measured.
4. **V4, access.** A settings toggle that makes voice the primary input
   (the menu reads its rows aloud is out of scope; the label is the
   feedback), and the thumb tap as the second opener.

## V1 as built

The first step, built; the reviewer's device numbers go to `MEASURED.md`.
The defaults of the open questions below were taken: the left fist opens
the window, `base.en` is bundled, the grammar comes later.

**The module.** `crates/fosfora-xr/src/voice.rs`. Its pure part builds and
tests on the desktop: the window (`PushToTalk`), the stream's 48 kHz
stereo to whisper's 16 kHz mono (`mono_16k`: the channels averaged, a box
low-pass as long as the rate ratio, then linear interpolation), the
encoder window (`audio_ctx`, the clip plus 1.28 s at 50 frames a second
within 64 to 1500: 214 for 3 s), short clips padded to 1.1 s (whisper.cpp
returns nothing under 1 s), the log and label text, and a small RIFF
reader for the test clip. Its device part (`Voice`) is Android only.

**The window's rules.** The left fist (hand 0, `Pose::Fist` from
`pose.rs`) opens it once it has lasted 0.15 s; a shorter fist is nothing.
Release closes it, and so does 6 s of holding, after which the hand has to
open before the next press. A closed window is *closing* until its
transcription returns: a fist meanwhile is ignored, so two windows never
overlap and one transcription runs at a time. The fist opens the window in
every mode: with Edit room on, with the hand menu up, and on Flock, where
the same fist is also the flock's predator. While `RECORD_AUDIO` is
missing the window never opens (logged once); a late grant is picked up
by the permission poll.

**The stream.** Each window opens its own AAudio input stream (the voice
recognition preset, 48 kHz stereo, 16-bit, no low-latency mode) when it
opens and closes it when it closes; the frame drains its ring each frame.
It never shares a stream with the music path. The open and close times
are logged; if opening per window costs more than about 50 ms on the
headset, the stream moves to launch and the window only reads it.

**The thread.** One worker thread, `fosfora-voice`, loads the model at
launch (one whisper context and one state, kept warm) and then
transcribes each closed window: greedy, `language en`, one segment, no
context, the encoder window cut to the clip, at most 64 tokens (a
repetition loop ends), the knob's threads. The frame thread only polls a
channel; neither the load nor a transcription runs on it. A window that
closes before the load has finished is dropped ("Voice is still loading").

**The knobs** (read at launch): `debug.fosfora.voice 0|1` (default 1 when
the model was installed), `debug.fosfora.voicethreads <n>` (1 to 6,
default 3), `debug.fosfora.voicefile <path>` (a 16 kHz mono 16-bit WAV
transcribed 3 s after the model loads, as if a window had closed, so the
gate runs unworn). With voice on, `RECORD_AUDIO` is asked for at launch on
any audio source.

**The label.** The voice path has its own label, drawn before the room
editor's and the scan label. "Listening…" at the left palm, facing the
head, while the window is open; "…" when it closes, until the text
returns; then the sentence for 1.5 s, or "Didn't catch that" when nothing
was heard (whisper's non-speech annotations such as `[BLANK_AUDIO]` count
as nothing). The `voicefile` clip's label floats 1 m ahead of the head.
The log has the window's length, the transcription's time and encoder
window, the time from the window's close to the text, and
`voice: heard "…" (N ms)`.

**The build.** `whisper-rs` 0.16 is an Android-only dependency of
`fosfora-xr`; the desktop workspace never builds whisper.cpp. The cmake
build inside `whisper-rs-sys` sees only `CMAKE_*`, `GGML_*` and
`WHISPER_*` variables, so `android/cmake/android.toolchain.cmake` wraps the
NDK's toolchain file to set the ABI and API level, and `scripts/xr/run.sh`
exports it with `GGML_NATIVE=OFF` and
`GGML_CPU_ARM_ARCH=armv8.2-a+dotprod+fp16` for build and lint. whisper.cpp
links the NDK's `libc++_shared.so` (1.8 MB), which `run.sh` copies next to
the cdylib. `scripts/xr/fetch-model.sh` fetches `ggml-base.en.bin` with its
SHA-256 into the git-ignored `assets/xr/models/` (source and license in
`LICENSE.md` there); the APK stores it uncompressed, and the asset install
streams it to internal storage in chunks. `whisper-rs` and `whisper-rs-sys`
are Unlicense, which `deny.toml` admits for those two crates only. The
debug APK is 171.5 MB: the model 148.0 MB, `libfosfora_xr.so` 18.9 MB
(17.6 MB without whisper), `libc++_shared.so` 1.8 MB.

**Not yet.** The grammar and the agent (V2, V3); the microphone's level
bar under "Listening…"; the thumb tap as a second opener; the measurements
inside the running app (the transcription time with the renderer live, the
window's cost on the frame, the stream's open time, the first launch's
unpack of the model), which are the reviewer's.

## V2 as built

The second step, built; the reviewer runs the unworn gate through the
`say` knob and the worn gate with the ten phrases.

**The module.** `crates/fosfora-xr/src/intent.rs`, pure and
desktop-tested: data plus one matcher. The vocabulary (`Vocabulary`) is
built from the frame at the moment of the sentence: the world effects'
names ("Flux Cloud", "Embers", "Flock"), `SurfaceBehavior::ALL` by
`name()`, the lane boxes by `surfaces::friendly_name` ("desk", "table
14", "wall 5", "window"; the box index appended where two share a name)
and the surface under the editor's beam while Edit room is on. `parse`
returns one `Intent` or a `Miss`; every intent is something a hand
already does:

| Intent | The hands' way |
|---|---|
| `NextEffect`, `PrevEffect`, `Effect(i)` | the hand menu's effect row |
| `EditRoom`, `Cloud`, `Pitcher`, `Music` | the menu's toggles (the Cloud toggle's row reads "Particles") |
| `Rescan`, `Recenter`, `AllNone` | the menu's buttons |
| `Behavior { target, behavior }` | the editor's tap (a surface) or hold (a kind), set instead of stepped |
| `Color`, `Band`, `Strength` | the menu's surface rows with Edit room on |
| `Describe(target)` | the status cell and the rows' label |

**The normalizer.** Lower case; apostrophes dropped ("what's" is
"whats"); every other mark a space ("re-center" is "re center");
whitespace collapsed; "the", "a", "an", "please" and "to" dropped
wherever they stand, names included, so both sides compare alike. Number
words stay words.

**The templates**, tried in order; each slot takes a whole run of words,
so a name is never cut in two:

| Template (slots in braces) | Intent |
|---|---|
| `next effect`, `next` / `previous effect`, `last effect`, `previous` | NextEffect / PrevEffect |
| `edit (the) room`, `start editing` / `stop editing`, `done editing`, `edit room off` | EditRoom |
| `rescan (the) room`, `scan (the) room` / `recenter (the) cloud`, `re-center` | Rescan / Recenter |
| `clear everything`, `clear all`, `nothing anywhere`, `all none`, `everything off` | AllNone |
| `cloud on/off`, `particles on/off`, `pitcher on/off`, `music play/stop/on/off`, `play (the) music`, `turn (the) cloud off`, `turn off (the) music`, `show/hide (the) particles` | the toggles |
| `{effect}` alone, or `switch to {effect}`, `show {effect}` | Effect |
| `what is {target}`, `what's {target}`, `describe {target}`, `what is this` | Describe |
| `clear {target}` | Behavior none |
| `{behavior} on {target}`, `put {behavior} on {target}`, `{target} {behavior}` | Behavior |
| `{target} in {color}`, `{color} on {target}`, `make {target} {color}`, `{target} {color}` | Color |
| `{target} on (the) {band}`, `{target} follow(s) (the) {band}`, `{target} {band}` | Band |
| `{target} {strength}`, `{brighter/dimmer} on {target}` | Strength |

A leading `put`, `set`, `make`, `turn`, `give`, `paint` or `use` is
dropped before the surface templates ("set the desk to amber", "turn the
desk off").

**The slots.** `{target}`: a surface's friendly name ("desk", "table 14");
a name without its number ("table") for the surfaces sharing it; a kind
for all of it ("every table", "all the walls", or a plural alone,
"walls"); "this", "that", "here", "it" for the pointed surface; or no
words, which also means the pointed surface. A shared name is the pointed
surface when that is one of them, every floor when they are all floors
(the room's floor and the stage floor are one floor to the eye, and both
are drawn), else a miss that lists them. `{behavior}`: a behavior's name,
the eight ports included ("aurora on the wall"), `ripple` for the rings,
a dropped trailing "s" forgiven ("streamline"), and `none`, `nothing`,
`clear`, `off` for none; one the target's kind does not cycle through
(`SurfaceBehavior::catalogue`) is refused. `{color}`: the hand menu's
names (blue, violet, warm white, amber, green, teal, rose), `key` for the
key's tint, the kind's own as "normal color", "its own color" or "default
color", "color" after a name allowed. `{band}` and `{strength}`: below.

**The synonyms**, one table:

| Said | Means |
|---|---|
| desk, desks | the table kind (when no surface is called "desk") |
| couch, sofa, chair (and plurals) | the other kind |
| floor, ground | the floor kind |
| ceiling, roof | the ceiling kind |
| frame, window, door (and plurals) | the frame kind (a lamp is the other kind, as its anchor's label makes it) |
| all, every, each `{kind}`; a plural | the kind: its default and every surface of it |
| bass, base, low, lows | band 1 (whisper heard "bass" as "base" in the spike) |
| mid, mids, middle | band 2 |
| high, highs, treble | band 3 |
| rms, level, volume | band 0 |
| brighter, up, more, stronger / dimmer, down, less, weaker, darker | a strength step up / down (the Strength row's tenth) |
| half, full (max) | strength 0.5, 1 |
| off, zero with a level word ("strength off", "brightness zero") | strength 0 |
| off, nothing, none, clear after or before a surface | behavior none |
| purple, white, orange, pink | violet, warm white, amber, rose |

**The misses** and what the label says:

| Reason | Example | Label |
|---|---|---|
| NoMatch | "Flibber jabber." | Didn't catch that: "Flibber jabber." |
| NoSurface | "in amber" with nothing pointed | Point at a surface, or name one |
| UnknownSurface | "the shelf in blue" | No surface called "shelf" |
| UnknownBehavior | "fire on the desk" | No behavior called "fire" |
| NotOnThisKind | "embers on wall 2" | embers doesn't run on a wall |
| Ambiguous | "table in amber", two tables, neither pointed | Which one: table 1 or table 2? |

A target longer than three words is no name, so a long sentence that
ends in a slot word misses as NoMatch instead of naming an unknown
surface ("switch to the next effect and fade the colors to purple" is
NoMatch). An ambiguity lists five names, then "or N more".

**The wiring** (`app.rs`). Where V1's transcription arrives, the sentence
goes through `parse` with the frame's vocabulary. The menu's intents
become the `Action`s the hand menu's presses produce, consumed by the
next frame's action loop together with the menu's (the loop no longer
sits inside the panel-shown branch), so the saves and the logs stay one
path; a toggle sets its `controls` field first, as the menu does, and
naming a world effect sets `switch_to` to it. The surface intents write
in the same frame (its `lane_boxes`) through `intent::apply` and the
lanes' typed writers: a behavior on one surface through `assign` with its
strength kept (the tap's way), on a kind through the new
`RoomLanes::set_kind` (the hold's writer with a set instead of a step,
at the kind's first surface's strength), a color, band or strength
through `set_params`, on each surface of a kind in turn. `Describe` reads
`label::param_text` ("table 14: streamlines · amber · rms · 1.0"); a kind,
its behaviors ("all tables: streamlines, curls").

**The label and the log.** The voice label, where V1 showed the sentence:
the reply for 1.5 s on success, in the editor's words ("desk:
streamlines", "all walls: spectrum", "desk: amber", "wall 5: bass",
"desk: dimmer", "next effect", "particles off"), the miss for 2.5 s.
Nothing heard still reads "Didn't catch that". The log, one line per
sentence: `voice: heard "<sentence>" → <intent or miss> · label "<text>"`,
after V1's `voice: heard "…" (N ms)`, and the lanes' own lines for a
write.

**The `say` knob.** `adb shell "setprop debug.fosfora.say 'the desk in
amber'"` feeds a sentence to the grammar as if heard, voice on or off,
its label ahead of the head: read at launch and polled once a second, fed
once per value, waiting for the room's anchors as
`debug.fosfora.surface` does. An app cannot clear a `debug.` property,
so the same sentence again needs the knob set to "" (or to another
sentence) first.

**Where it differs from the design above.** "particles on/off" is the
Cloud toggle, the hand menu's row for it reading "Particles"; the
pitcher is reached as "pitcher on/off". The debug panel's toggle is not
reachable by voice. A kind's reply reads "all walls", the hold's words.
The rings do run on a wall (the wall's catalogue has them), so the
refused example is embers on a wall.

**Not yet.** The agent (V3): a sentence the grammar misses says so and
goes nowhere else. Numbers as digits: "table fourteen" does not name
table 14 (whisper writes digits for a spoken number, which do). Anything
but English. Two actions in one sentence.

## Open questions for Kevin

1. The opener: the left fist held, or the thumb tap, or both from the
   start? Default taken if silent: the fist, the tap later.
2. Bundle the model in the APK or fetch on first use? Default: bundle
   `base.en` (148 MB; the spike made it the model).
3. Is the agent in scope for the first release of the voice path, or does
   the grammar ship first and the agent follow? Default: grammar first.
4. Which hosted model, and where does its key live on the headset?
   Default: the app's config file, entered through the companion web
   remote later; the agent is off until a key exists.
