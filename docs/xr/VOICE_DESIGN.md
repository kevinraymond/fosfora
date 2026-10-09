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
5. **V5, the on-device provider.** Our own 17M decision model answers
   what the grammar misses on the headset, no network and no key; the
   agent by default when it is installed.

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
NoMatch). So are several words none of which is a kind word, a number or
a word of a surface's name ("shut that song up" is NoMatch, and reaches
the agent); a single unknown word, or one next to such a word, is a name
the room lacks ("the shelf in blue", "table 9 in blue", "lamp 2 up"). An ambiguity lists five
names, then "or N more".

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

## V3 as built

The third step, built; the reviewer runs the unworn gate with sentences
through the `say` knob on a headset with a `voice.json`, and the worn gate
with five sentences the grammar cannot match.

**The module.** `crates/fosfora-xr/src/agent.rs`: the config, the two
providers' requests and replies, the room's JSON and the label are pure
and desktop-tested; the call is a thin `ureq` wrapper on its own thread
(`fosfora-agent`). `intent.rs` gains `AgentAction` (one item of the
reply: `kind`, `target`, `value`) and `Intent::from_json`, which resolves
an action through the grammar's own slots and target lookup.

**The provider is the wearer's choice** (Kevin, Oct 8), named in
`voice.json` under the config dir (`files/config/voice.json` in the app's
data), read once at launch. Every field but the key for Anthropic is
optional:

| Field | Meaning | Default |
|---|---|---|
| `provider` | `anthropic` (the Messages API) or `openai` (any OpenAI-compatible chat completions endpoint) | `anthropic` |
| `base_url` | where the API lives | `https://api.anthropic.com`, `https://api.openai.com/v1` |
| `model` | the model asked | `claude-opus-5-5` for Anthropic; **required** for `openai` |
| `api_key` | the key | **required** for Anthropic; none for a local server (no `Authorization` header is sent) |
| `extra` | an object merged into the top level of an `openai` request body (the Anthropic request ignores it) | empty |

V5 changed the default: a file naming no `provider`, or no file at all,
picks the on-device provider whenever its files are installed ("V5 as
built"); Anthropic stays the default for a file naming none when they
are not.

Anthropic, with a key (the key is a placeholder here; the real one never
enters the repo):

```json
{ "provider": "anthropic", "api_key": "<the key>" }
```

A local OpenAI-compatible server on the LAN (llama.cpp's server, Ollama,
LM Studio), no key, the sentence staying in the room. Ollama's thinking
models spend the whole output budget on hidden reasoning unless the body
carries `"think": false` (2.5 s and a schema-valid reply with it, on
qwen3.5); OpenAI's own API refuses unknown fields, so it lives in
`extra`, never in the code:

```json
{ "provider": "openai", "base_url": "http://192.168.1.20:11434/v1",
  "model": "qwen3.5", "extra": { "think": false } }
```

Put it on the headset with adb (the file is git-ignored wherever it sits):
`adb shell "run-as dev.fosfora.xr sh -c 'cat > files/config/voice.json'" < voice.json`,
then relaunch.

**The private-address rule.** `https://` anywhere; plain `http://` only to
a private address (`localhost`, loopback, 10/8, 172.16/12, 192.168/16),
since a key over plain HTTP on the internet is a leak; anything else
leaves the agent off with the reason in the log. A `.local` name does not
count: use the server's address.

**Off, and why.** A missing file, a broken one (the log names its line and
column, never its contents), `openai` without a model, `anthropic`
without a key, an unknown provider, an `extra` that is not an object, or
plain `http://` to a public address: the agent is off, logged once, and
the grammar's misses keep V2's text.

**One task, two wrappers.** Both providers send the same three things,
so the model sees the same task whoever serves it:

- *The instruction* (`agent::INSTRUCTION`, one fixed paragraph, so it
  caches): the model is the room's assistant; it gets the room and one
  sentence; it answers only with the JSON object (a list of actions in the
  order to apply them, and `say`, at most 60 characters, for the label);
  what each action kind does and what each behavior looks like (embers
  shed off a top face on the beat, aurora is flowing curtains, and so on),
  so "a campfire" can find embers; surfaces by their names, kinds by their
  words, behaviors only from the target's own kind; the pointed surface
  when the sentence says "this" or names none; no actions and a sentence
  saying so when the actions cannot express the request; never an
  invented name.
- *The schema* (`agent::schema`), every object closed
  (`additionalProperties: false`) and every field required: `actions`, a
  list of `{kind, target, value}`, and `say`. The kinds: `next_effect`,
  `prev_effect`, `effect`, `edit_room`, `cloud`, `particles`, `pitcher`,
  `music`, `rescan`, `recenter`, `all_none`, `behavior`, `color`, `band`,
  `strength`, `describe`. The target: a surface's name, a kind word
  (`table`, `floor`, `wall`, `ceiling`, `frame`, `other`), `pointed`, or
  empty. The value: an effect's name, `on`/`off`, a behavior, a color, a
  band, `up`/`down`/`half`/`full`/`off`, or empty.
- *The room* as JSON, then the sentence, in the user message: the world
  effect showing and every effect; every surface as `{name, kind, size_m:
  [w, h], behavior, color, band, strength}` (the acting face's sides,
  larger first; the color `own` for the kind's own); the pointed surface's
  name or null; each kind's behaviors (`SurfaceBehavior::catalogue`); the
  color names and the band names.

**The Anthropic request**: `POST {base_url}/v1/messages` with
`content-type`, `x-api-key`, `anthropic-version: 2023-06-01` and
`anthropic-beta: server-side-fallback-2026-07-01`; the body has the
model, `max_tokens` 4096 (shared with the model's thinking, which runs
at every effort; the reply itself is short), `output_config` with
`effort: low` and the
schema as a `json_schema` format, `fallbacks: "default"` (a refusal is
retried server-side on a fallback model), the instruction as one system
block with `cache_control`, and one user message. It sends no `thinking`
(the model thinks by default and refuses an explicit setting), no
`temperature`, no `tool_choice`, no prefill. The reply: a status other
than 200 is an error with its body logged in full (never a header); a
`refusal` stop is refused, with `stop_details.category`; a `max_tokens`
stop is cut off; else the first `text` block, after any thinking blocks.

**The OpenAI-compatible request**: `POST {base_url}/chat/completions`
with `content-type` and, when a key exists, `authorization: Bearer`; the
body has the model, `max_tokens` 1024, the schema as a strict
`response_format` named `room_actions`, the instruction as the system
message and the room and sentence as the user message, then `extra`'s
members. No `temperature`, no tools. The reply: `finish_reason` `length`
is cut off, `content_filter` refused, else the first choice's content.

**Either way**, the reply is read from the first balanced `{…}` in the
text, so a server that ignores the schema, or wraps the JSON in prose or a
code fence, still works when the model obeys the instruction. An endpoint
that refuses the format (Anthropic's 400 naming `output_config` or
`format`; a 400 or 422 naming `response_format`) gets the request once
more without it, in the time left, and the log says which path a call
took (`schema` or `no schema (retried)`). The fixtures in the tests take
the schema path; which one a live endpoint takes is the reviewer's to
read from the log.

**The wiring** (`app.rs`). Two misses go to the agent
(`agent::forwards`): NoMatch, a sentence no template fits, and
UnknownBehavior, one that fits a template's shape but names a behavior
the catalogue lacks ("a campfire on the desk", "fire on the desk"),
which a model can read for its meaning (Kevin, Oct 8). The grammar's
other misses (no such surface, which the agent cannot know either; not
on this kind; which one; nothing pointed at) are already answers and
keep their labels. The frame builds the room's state
from V2's vocabulary, the lanes' assignments and each box's acting face,
builds the request, and hands it to the call's thread; the label reads
"Thinking…" (13 s, past the call's 12 s limit) and the frame loop polls
the channel, never waits. One call is in flight: a newer sentence's call
replaces it and the older answer is dropped. On an answer, each action
becomes an intent through `Intent::from_json` and applies through
`apply_intent`, the function V2's grammar intents go through (refactored
out of the frame, unchanged), in order. A dropped action (a name the room
lacks, a color like "orchid") is the grammar's own miss, named on one log
line; the other actions still apply.

**What the label says:**

| Answer | Label (2.5 s) |
|---|---|
| every action applied, or none and a sentence | `say` (the actions' own labels would flicker past; the log has them) |
| some actions dropped | the applied ones' labels and "skipped N" |
| every action dropped | the first miss's own text ("No surface called …") |
| nothing at all | The agent didn't answer |
| no network (no route, no name, refused) | No network for that |
| a refusal | I can't help with that one |
| the time limit passed | Took too long |
| an HTTP error, an unreadable or cut-off answer | The agent didn't answer |

**The log**, per sentence: V2's `voice: heard "<sentence>" → miss NoMatch
· label "Thinking…"` (or `miss UnknownBehavior("campfire") · label
"Thinking…"`); the call, `voice agent: <provider> <model> · <N> ms
· in <input tokens> [cached <cache reads>] out <output tokens> · <K>
actions · schema` (a count a server leaves out is `-`; on an error, the
error in place of the counts); then `voice agent: heard "<sentence>" →
[<intent or miss>, …] · say "<say>"`, `voice agent: dropped …` when an
action was dropped, and the lanes' own lines for each write. An error's
line: `voice agent: heard "<sentence>" → <error> · label "<label>"`. A
non-200 body is logged in full before it (`voice agent: <provider>
answered <status>: <body>`).

**The knob.** `debug.fosfora.agent 0|1`, read at launch: default on
whenever `voice.json` configures a provider, 0 keeps the call off for
sweeps. The `say` knob from V2 exercises the whole path, the call
included.

**The cost.** One call is one request with the room's JSON: the
instruction (about 0.7 K tokens, fixed) plus the room (a surface is
about 40 tokens; the 18-box replayed room comes to about 1 K), so about
1 to 2 K input tokens and a reply under 150 output tokens (on Anthropic
the output count includes the model's thinking, so it runs higher; the
4096 ceiling leaves it room, and only the tokens used are billed). The Anthropic
system text is marked for caching, so repeated sentences read it from the
cache (`cached` in the log; the model's minimum cacheable prefix is 512
tokens, which the instruction passes); `effort: low` keeps the model's
thinking short. A local server costs nothing per call. The reviewer
reports the per-call token counts from the log.

**Permissions.** The manifest declares `android.permission.INTERNET` (a
normal permission, granted at install, no runtime ask); `permissions.rs`
is unchanged.

**Five sentences for the `say` knob** on the replayed room (nothing
pointed), each a NoMatch for the grammar, with a reply a model may
sensibly give; the test `the_five_sentences_reach_the_agent_and_their_replies_map`
checks that each reaches the agent and that these replies map onto
intents:

| Sentence | A sensible reply's actions |
|---|---|
| Something like a campfire on table 14. | behavior table 14 embers; color table 14 amber |
| Make the walls calmer. | behavior wall aurora; strength wall down |
| Less going on. | cloud off; behavior wall none |
| Make the room feel like the ocean. | behavior floor rings; color floor teal; behavior wall aurora; color wall blue |
| Give the ceiling a starry night. | behavior ceiling astrolabe; color ceiling violet |

**Where it differs from the brief.** The schema's kinds add `pitcher`, so
every grammar intent is reachable (`cloud` and `particles` are both the
Particles toggle, as in the grammar). `provider` is optional, defaulting
to Anthropic. `AgentError::NoNetwork` carries the transport's message for
the log. Loopback addresses (127/8, `[::1]`) count as private, as
`localhost` does. An OpenAI-compatible 422 naming `response_format` is
retried like a 400. UnknownBehavior misses go to the agent too, not
only NoMatch (Kevin, Oct 8), and the Anthropic `max_tokens` is 4096, not
1024, since the model's thinking shares it (Kevin, Oct 8). A bare kind word as the target means every surface of
that kind even where a spoken "table" would ask which one, unless a
surface carries that exact name.

**Not yet.** A conversation across turns (each sentence is one request,
with no memory of the last); more providers; the agent proposing new
behaviors, parameters or surfaces (it only reaches what the hands reach);
spoken replies; streaming.

## V4 as built

The fourth step, built: voice as an input the wearer can rely on, not a
feature behind a knob. Nothing new to act on: the grammar's actions are
V2's. The reviewer runs the unworn gate through the `say` knob and the
menu toggle, and the worn gate for the tap, the silence close, the meter,
the help and Flock.

**The toggle, saved.** The hand menu's Music row had an empty right cell;
it is `Voice: on` / `Voice: off` now, in both layouts, so no row count
changes. The state is in `hand_menu.json` next to the debug panel's,
`{"debug":false,"voice":true}` (`palm_panel::MenuFile`); a file without
the field reads voice as on, so a headset updated from V3 keeps
listening. `debug.fosfora.voice 0|1` forces the toggle at launch and
saves it, as `debug.fosfora.hud` does the debug panel's. Off: neither
opener does anything for voice, an open window is dropped unheard, a
transcription that returns is dropped, and the label shows nothing.
`voice: on (menu)` / `voice: off (menu)` in the log on a change.

The model loads at launch when the toggle is on at launch; a launch with
it off loads nothing, so a sweep with `debug.fosfora.voice 0` stays free
of the model as before. The first turn on from the menu loads it then
(and asks for `RECORD_AUDIO` if it is missing; a grant is picked up
within a second); turning it off keeps the model loaded, so the toggle is
instant from then on.

**Two openers.** The left fist held, as V1; and a thumb tap on the index
(`XR_META_hand_tracking_microgestures`, either hand), logged `voice:
window open (thumb tap, <hand>)`. Both are on whenever voice is.
`PushToTalk` knows which opened the window (`Window::Open { opener: Fist
| Tap }`):

| Opener | Opens | Closes |
|---|---|---|
| Fist | after 0.15 s held | on release, or at 6 s; ignores taps and the level |
| Tap | at once | once speech has been heard and 0.6 s of audio has passed, after 0.8 s under the quiet level; at 6 s; on a second tap; ignores the fist |

Requiring speech first keeps a wearer who taps and then gathers a
sentence from being cut off before they begin; a pause between words
shorter than 0.8 s does not close. The level is the RMS of the stream's
last 50 ms (`voice::level`), taken where the ring is drained. The quiet
level starts at 0.01 RMS on the ±1 scale (-40 dBFS); the knob
`debug.fosfora.voicequiet <level>` tunes it against the room. The launch
logs the threshold, and each close its cause: `voice: window closed after
2.40 s (quiet 0.8 s at 0.004)`, `(release)`, `(6 s)`, `(second tap)`.
A fist held through a tap window has to open before it presses again.

**The meter.** While a window is open, a thin bar under "Listening…"
whose length is the level, log-scaled over 40 dB (`voice::meter_width`:
-40 dBFS and below empty, -20 dBFS half, full scale the whole bar), so
the quiet level is exactly the empty bar; redrawn each frame. When the
window closes the bar goes; "…" and the sentence are as in V1.

**Two things in one sentence.** A sentence the grammar misses whole is
cut into clauses (`intent::split_clauses`) at " and ", " then ", ", ",
";" and a sentence's end ("Next effect. Embers." is two: whisper writes
two commands said with a pause as two sentences), never inside a name
the room has (an effect, a surface, a behavior or a color whose name
carries a splitter); a leading "and" or "then" is dropped, and so is a
clause of fillers alone ("please"). Each clause is parsed on its own
and applied in order; a clause that misses does not stop the others; at
most three, more is one miss (`TooMany`, "Three things at most in one
sentence"). Matching the whole sentence first keeps every V2 sentence one
clause, commas and all ("The desk, in amber, please."), so splitting only
rescues a sentence V2 missed. A clause sharing a slot with the next one
is not understood ("the desk and the table in amber" is "desk", a miss,
and "table in amber").

| Said | Label |
|---|---|
| Amber on table 14 and the walls on the bass. | table 14: amber · all walls: bass |
| Embers on the desk and flibber jabber. | desk: embers · Didn't catch that: "flibber jabber." / Try "what can I say" |

The log lists every intent: `voice: heard "…" → [Color { … }, Band { … }]
· label "…"`; a one-clause sentence keeps V2's line exactly. A label of
several lines is logged with its lines joined by " | ".

**With the agent.** A clause that misses the way the agent takes (NoMatch
or UnknownBehavior, `agent::forwards`) goes to it instead of showing its
miss, "Thinking…" in its place in the label, after the matched clauses
were applied, so the model sees the room after them. When every clause
goes, or the sentence had too many, the whole sentence goes, exactly as
V3 sent it ("Make the room feel like the ocean." is unchanged); else only
the clauses that go, joined with " and " ("Embers on the desk and make it
cozy." applies the embers and asks the agent "make it cozy."). The
grammar's other misses stay its answers.

**Number words.** The normalizer turns `one` to `twenty`, `thirty`,
`forty`, `fifty` and the compounds ("twenty one") into digits, so "table
fourteen" names table 14. "One" after "this", "that", "next", "previous"
or "last" stays a word ("this one", "next one"); "zero" stays a word
("strength zero").

**"What can I say".** "What can I say", "what can I do", "help", "voice
help" show five example phrases for 4 s, a line each (the voice label
grows for them), and the next five on each ask, wrapping
(`intent::examples`). They are built from the room: a round robin over
the menu's phrases ("next effect", "music play and next effect", "edit
the room", …), a template per surface of this room turning through a
behavior of its kind, a color, a band, a strength and "what is"
("streamlines on table 0", "storage 1 in blue", "wall 2 on the bass"),
the world effects ("switch to Embers") and the kinds ("every wall
rings"); each under 30 characters, and only those whose every clause
parses on the same vocabulary, so the help never shows a phrase the
grammar would miss.

**The hint.** A NoMatch the agent does not take gets one line under
"Didn't catch that": the nearest example, by the words they share (glue
words such as "on" aside), then by the common prefix of their words, else
`Try "what can I say"`. "Switch over to the flock now." hints `Try
"switch to Flock"`.

**The fist and Flock.** With voice on, the left fist opens the voice
window everywhere, so Murmur's predator fist no longer acts on the left
hand: the flock sees an open left hand (`voice::flock_poses`, masking
the pose `pose::decide` reads), the right fist keeps its predator, and
the log says so once per change. With voice off, Flock is as it was.

**The label's place.** At the left palm when it is located, as V1; with
the palm out of view (a tap from the right hand, the left one down), 1 m
ahead of the head along its view, as the `say` knob's and the
`voicefile` clip's label (`voice::label_anchor`). V1 put it at the head
itself then.

**The knobs.** `debug.fosfora.voice 0|1` (forces the toggle at launch,
saved), `debug.fosfora.voicequiet <level>` (the tap window's quiet level,
default 0.01), and V1 to V3's unchanged: `voicethreads`, `voicefile`,
`say`, `agent`.

**The `say` knob on the replayed room** (nothing pointed, the agent off;
`the_v4_sentences_on_the_replayed_room` checks these lines):

| Sentence | Label |
|---|---|
| Amber on table 14 and the walls on the bass. | table 14: amber · all walls: bass |
| Table fourteen in teal, then wall five on the highs. | table 14: teal · wall 5: high |
| Wall sixteen pulse. | wall 16: pulse |
| What can I say? | next effect / streamlines on table 0 / switch to Flux Cloud / every table curls / music play and next effect (4 s) |
| Help. | storage 1 in blue / switch to Embers / every wall rings / edit the room / wall 2 on the bass (4 s) |
| Flibber jabber. | Didn't catch that: "Flibber jabber." / Try "what can I say" |
| Make table fourteen sort of glowy. | Didn't catch that: "Make table fourteen sort of glowy." / Try "what is table 14" |
| Next effect, particles off, music play and edit the room. | Three things at most in one sentence |

**Where it differs from the brief.** `split_clauses` takes the
vocabulary (it guards the room's names) and also splits at a sentence's
end. A sentence is matched whole before it is split. A tap window closes
on silence only once speech has been heard, so quiet before any speech
waits for the speech or `MAX_S`. A launch with the toggle off loads no
model, and the first turn on loads it. The `say` knob still feeds the
grammar with the toggle off, as V2's did with voice off. TooMany goes to
the agent when it is on (V3 sent such a sentence as a NoMatch).

**Not yet.** Spoken replies; a voice-only menu (the menu read aloud);
anything but English; a slot shared across clauses ("the desk and the
table in amber").

## V5 as built

The fifth step, built: a third provider, `local`, that answers what the
grammar misses on the headset with our own 17M decision model (run 3,
`MEASURED.md`, "Our own System One 17M, trained"): no network, no key,
0.15 to 0.26 s per sentence on the bench. The reviewer runs both
40-sentence test sets through the `say` knob on the replayed room,
measures the cascade inside the running app, and takes the worn gate.

**The module.** `crates/fosfora-xr/src/local.rs`. The spec, the
rendering, the cascade's control flow, the reply and the log lines are
pure and desktop-tested; the tokenizer, the ONNX Runtime session and the
worker build on Android and in the desktop tests. `agent.rs`'s `Provider`
trait is now what every provider shares (`name`, `model`, and `answer`:
the sentence and the room in, a channel the frame loop polls out); the two
network providers' request and parse moved to a sub-trait, `Http`,
unchanged. `Local` implements `Provider`: a `Reply` with one action and no
`say` (the model writes none), or a `miss`, a field `Reply` gained.

**The spec as data.** `assets/xr/models/s1-17m-spec.json` is the training
pipeline's `provider-spec.json` for run 3, copied verbatim and committed
(9.8 KB, ours); the app reads it at launch next to the model. From it
come the token budgets (prefix 512, candidate 64) and the special ids,
the layout (`instruction_state`), the request step's instruction, its yes
and no texts, the candidates' order and its threshold (`p(true) >= 0.5`,
read from the rule), the fifteen kinds and their texts, the target step's
instruction, the kinds that take a target, each kind's word and group
text and the pointed text (read out of the prose the spec carries them
in), every value question, each kind's behaviors and their texts, the
color, band, strength and toggle lists, and the effects' text. A
retrained model with new wording ships with a new spec and no code
change. The code fixes what the training toolkit fixes: how a prompt is
assembled, the state's JSON, the model's inputs and its three output
columns. A spec it cannot render for (other columns, another truncation
or layout, a step missing, a behavior the app lacks) leaves the provider
off with the reason in the log.

**The rendering**, exactly the toolkit's:

- *The state*, the room as V3 builds it (`agent::room_state`) reduced to
  what the training saw: `{"sentence", "pointed" (a name or "nothing"),
  "effect", "effects", "surfaces": {name: behavior, or "behavior, color"
  when its color is not its own}}`, with Python's default separators.
  Band, strength and size stay out (no decision needs them, and every
  token is paid on the headset).
- *The prefix*: `[CLS]`, `enc("Instruction: ")`, the instruction,
  `enc("\n")`, `enc("State: ")`, the state, `[SEP]`; each part tokenized
  on its own with no special tokens; the instruction and the state cut to
  share what the markers leave of 512 (half each, the odd token to the
  instruction, what one leaves unused to the other).
- *The request step* reads the state in the noul envelope,
  `{"noul":{"yes":…,"no":…},"state":{…}}`, compact.
- *A candidate*: `enc("Candidate: <id>: <text>")` cut to 63 tokens, then
  `[SEP]`.

The tests assert the exact token ids of the toolkit's seven fixtures
(both layouts, a system text, truncation at 512 and at 64, Japanese, an
emoji, an empty candidate) and of 28 decisions the training's cascade
evaluation ran on the replayed room, nine sentences through all their
steps (`crates/fosfora-xr/tests/data/s1-17m-parity.json`, derived from
the run 3 export). Those need the real tokenizer and run wherever it is
installed (`assets/xr/models/`); the desktop CI has none, and checks the
assembly with a one-id-per-character stand-in.

**The runtime and the library.** ONNX Runtime 1.28.0 from the official
onnxruntime-android AAR; the Gradle build puts only its
`libonnxruntime.so` into the APK (`XR_DESIGN.md`, "ONNX Runtime"). The
`ort` crate 2.0.0-rc.11 with `load-dynamic` dlopens it by name, which the
app's linker namespace resolves in its native library directory (as the
OpenXR loader is found); should that fail, the Activity's
`nativeLibraryDir` is tried. The tokenizer is the `tokenizers` crate with
the pure-Rust `fancy-regex` backend (the byte-level pre-tokenizer's
pattern needs look-ahead). One session (graph fully optimized, one
inter-op thread, `debug.fosfora.localthreads` intra-op threads, default
3, intra-op spinning off so the threads sleep between sentences) lives on
the `fosfora-local` worker. It loads once the speech model has (at once
with voice off; at most 15 s after launch whatever whisper does), so the
two loads never share the CPU. A sentence asked before the load has
finished waits for it.

**The cascade** (`local::cascade`), one run of the model per step with one
prefix and all its candidates; the choice column softmaxed over a
decision's candidates, the noul column for the request step:

1. *Request*: under the spec's threshold, a miss.
2. *Kind* over the fifteen: under the floor, a miss.
3. *Target*, for `band`, `behavior`, `color`, `describe` and `strength`:
   every surface by its friendly name (reading as its kind: "table",
   "window or door", "object", …), then `all <kind>s` for each kind with
   two or more surfaces, then `pointed` when something is pointed at.
4. *Value*, for every kind but the six that take none: a behavior from
   the target's kind's list, an effect of the room's, or the kind's fixed
   list (colors, bands, strengths, on/off).

**The floor.** `debug.fosfora.localmin`, default 0.35, on the kind and the
value: a weaker pick is a miss, so a weak answer never changes the room.
The target has none (up to twenty surfaces share its probability, and its
answer is checked against the room by the grammar's resolution anyway).

**The mapping.** The winning ids become one `AgentAction` and go through
`Intent::from_json`, the grammar's own resolution, as a network provider's
actions do; the spec's ids need nothing else: a surface's name, `all
tables` (a kind), `pointed`, `own color` (color 0), `level` (`rms`),
`brighter`/`dimmer` (a strength step), `off` (strength 0). `intent.rs`
gained one alias: `previous_effect`, the model's id for `prev_effect`.

**Threads.** Whisper's three threads and the cascade never run at once. A
sentence is transcribed, then decided: the cascade starts only with no
voice window open or closing (a `say` sentence that arrives while one is
waits, logged `voice local: waiting for the voice window to close`), and
while it runs no window opens and the `voicefile` clip waits, so no
transcription can start. The cascade takes about a fifth of a second; a
fist held through it opens the window when it ends.

**The default rule** (`agent::choose`):

| `voice.json` | Decision model installed | Agent |
|---|---|---|
| none | yes | local |
| none | no | off, as before |
| no `provider` | yes | local |
| no `provider` | no | V3's: Anthropic, or off without a key |
| `"provider": "local"` | yes | local |
| `"provider": "local"` | no | off, with the reason |
| `anthropic` or `openai` | either | V3's, unchanged |
| broken | either | off, V3's reason |

Installed means the model, the spec and both tokenizer files are in the
app's `assets/xr/models/`. `debug.fosfora.agent 0` still turns any agent
off.

**What the wearer sees.** "Thinking…" for the fifth of a second the
cascade takes, then the intent's own reply text, as the grammar shows it
("table 14: amber", "all walls: dimmer", "next effect"). A miss (not a
request, or under the floor) is the grammar's own: `Didn't catch that:
"<sentence>"` and the nearest example as a hint, for 2.5 s, and nothing
in the room changes. An action the grammar's resolution refuses shows its
own miss text, as V3's do.

**The log**, at launch:

- installed: `voice agent: local · s1-17m-int8 loads after the speech
  model (3 threads, floor 0.35)`, then once loaded `voice agent: on ·
  local · s1-17m-int8 · <N> ms` and `voice local: ONNX Runtime from
  libonnxruntime.so`;
- not installed, no `voice.json`: `voice agent: off (no voice.json, and
  no s1-17m-int8.onnx, s1-17m-tokenizer.json, s1-17m-tokenizer_config.json
  at <dir>)` (the spec ships with the APK);
- a failed load: `voice agent: off (local: <reason>)`.

Per sentence, after V2's `voice: heard "…" → miss NoMatch · label
"Thinking…"`: `voice local: <N> ms (request <p> · kind <id> <p> · target
<id> <p> · value <id> <p>) → <intent>` (the steps that ran; a miss reads
`→ miss NoMatch (not a request, under 0.5)` or `(kind under 0.35)`), then
`voice local: steps request <ms> · kind <ms> · target <ms> · value <ms>
ms · prefix <n> tokens`, then the frame's `voice agent: heard "…" →
[<intent>] · say ""` (or `→ miss NoMatch · label "Didn't catch that: …"`)
and the lanes' own lines for a write.

**The knobs.** `debug.fosfora.localthreads <n>` (1 to 6, default 3) and
`debug.fosfora.localmin <p>` (0 to 1, default 0.35), read at launch;
`debug.fosfora.agent 0|1` as before.

**The fetch.** `assets/xr/models/MODELS.txt` lists every model with its
URL and SHA-256 (the speech model's line moved there), and
`scripts/xr/fetch-model.sh` reads it. The decision model and its tokenizer
files are hosted at `huggingface.co/kjraym/fosfora-voice-s1-17m`
(Apache-2.0; the card, the spec and the files); the script downloads what
is absent, accepts a file already present whose SHA-256 matches (`present,
sha256 ok`), and stops on a present file that does not match. A URL of
`placeholder` (a model not hosted yet) makes the script warn and go on
when the file is absent: the APK builds and the provider is off with the
log line above. The APK workflow caches the files on their SHA-256 and
fetches them with the same script.

**The exit order.** The device bench aborted at its exit: `ort`
2.0.0-rc.13 releases its environment from the executable's `.fini_array`,
which on Android runs after the dlopened runtime's static destructors.
The app needs no patch for it. It runs rc.11 (the core pins
`=2.0.0-rc.11` for its depth feature, and one lock holds one `ort`),
which keeps no exit-time release at all, and the app never reaches libc
`exit` anyway: `android_main` returning makes `android-activity`'s glue
call `ANativeActivity_finish` and end the thread, the process stays
cached until the system kills it with SIGKILL (force-stop, the
low-memory killer), and nothing dlcloses `libfosfora_xr.so` (whose
`.fini_array` Android would run only on a dlclose). What rc.11 does
carry is that ONNX Runtime cannot create a second environment in one
process, which a relaunch in the cached process would ask for once the
first session was dropped; so the session is never dropped: the worker
parks it in a process-wide slot when the app goes away, and the next
launch takes it back (logged `· the session parked by the last launch`).

**The cost.** The APK grows by ONNX Runtime (`libonnxruntime.so`, 28.6 MB,
stored), the model (29.0 MB, stored), the tokenizer (3.6 MB, 0.8 MB
deflated) and the tokenizer and ONNX Runtime bindings in
`libfosfora_xr.so`: the debug APK with every model in is 236.3 MB
(`libfosfora_xr.so` 25.3 MB). The bench measured 0.15 to
0.26 s per sentence at 6 to 2 threads and 125 to 134 MB peak; the time
inside the running app with the renderer live, at 3 threads, and the
session's idle cost are the reviewer's to measure.

**Where it differs from the brief.** `ort` is 2.0.0-rc.11, not rc.13: the
core pins rc.11 and Cargo cannot hold both (prerelease versions of one
major unify, and `ort-sys` declares `links`); rc.11 asks for the 1.23 API,
which 1.28 serves; the host test passes on 1.30, and the free cascade on
the host gives set 1's 36 of 40 (24 of 25, 7 of 10, 5 of 5), as the
training report has it for run 3. `ort` and
`tokenizers` are Android dependencies and desktop dev-dependencies (for
the parity tests), so the desktop library builds neither. The Gradle
build takes only `libonnxruntime.so` out of the AAR instead of depending
on the whole AAR, so no Java code or JNI binding enters an app without
code. The `Provider` trait was split rather than given a stub request for
the local provider, and `Reply` gained `miss`. The floor is on the kind
and the value only. The provider counts as installed only with its
tokenizer files too. The per-sentence line is followed by one with each
step's time, for the in-app measurement.

**Not yet.** A retrain cadence (the generator regenerates the data; a new
run ships as a new model and spec with no code change); wider phrasing
(the misses in `MEASURED.md` are unseen words, not unseen structure); a
confidence-aware label (saying "I think you meant …" between the floor and
a sure answer); the model hosted, so `MODELS.txt` gets its URL.

## Open questions for Kevin

1. The opener: the left fist held, or the thumb tap, or both from the
   start? Default taken if silent: the fist, the tap later. V4 added the
   tap as the second opener ("V4 as built").
2. Bundle the model in the APK or fetch on first use? Default: bundle
   `base.en` (148 MB; the spike made it the model).
3. Is the agent in scope for the first release of the voice path, or does
   the grammar ship first and the agent follow? Default: grammar first.
4. Which hosted model, and where does its key live on the headset?
   Answered (Kevin, Oct 8): the wearer picks the provider in `voice.json`
   under the config dir ("V3 as built"); the agent is off until it exists.
