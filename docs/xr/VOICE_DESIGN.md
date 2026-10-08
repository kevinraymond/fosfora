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
label can answer while the gesture is still fresh. The spike measures
`tiny.en` and `base.en` on the Quest 3 with 4 and 6 threads; the numbers
go in `MEASURED.md` and here. If `base.en` fits the budget it is the one
(fewer misheard commands); if only `tiny.en` does, the grammar grows
synonyms instead.

**Where it runs.** On its own thread, after the window closes, with the
threads the render loop does not need (the loop's CPU is 2 ms of a
14 ms frame; four cores are idle). A transcription in flight never blocks
a frame. The model loads once at launch, after the room, so the first
command does not pay the load.

**Memory.** `base.en` holds about 200 MB resident. The app's budget on
the Quest is comfortable for that; the spike reports the real figure.

**Licensing.** `whisper-rs` is Unlicense, `whisper.cpp` MIT, the models
MIT; all pass `deny.toml`. The model file is downloaded by the build (as
the effects' assets are packed) and bundled in the APK, which grows by
the model's size; or fetched on first use into the app's files, which
keeps the APK small and needs a network once. Recommendation: bundle
`tiny.en`, fetch `base.en` on first use if the spike says it is worth it.

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

## Open questions for Kevin

1. The opener: the left fist held, or the thumb tap, or both from the
   start? Default taken if silent: the fist, the tap later.
2. Bundle the model in the APK or fetch on first use? Default: bundle
   `tiny.en` (75 MB), decide on `base.en` after the spike.
3. Is the agent in scope for the first release of the voice path, or does
   the grammar ship first and the agent follow? Default: grammar first.
4. Which hosted model, and where does its key live on the headset?
   Default: the app's config file, entered through the companion web
   remote later; the agent is off until a key exists.
