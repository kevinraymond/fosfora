# TouchDesigner quick start

Fosfora as the audio-analysis brain of a TouchDesigner network. Fosfora runs headless
(`--signal`, no window, no GPU) and broadcasts what it hears over OSC: beats, downbeats,
tempo, bar and phrase position, section, key, energy, and a prediction that rises before a
likely drop-like moment. Two files on the release page receive it:

- **`fosfora_examples.tox`** is four ready-made views, all reacting within seconds of
  dragging it in: a Dashboard of every channel, a 3D Grid of the music's structure, a
  Lights rig of eight moving heads with a DMX output, and Keeping time, which shows bars as
  they arrive next to Steady Bars and TouchDesigner's own clock.
- **`fosfora_signal.tox`** is the component the examples are built on, for your own network.
  It turns the stream into CHOP channels you wire like any other: one-frame pulses for
  events, a bar phase that moves every frame, section and key as both indices and text.

Fosfora's analysis is signal processing plus two small tree models for kick and beat
evidence, trained on Creative Commons music and built into the program. It runs on the
CPU, with no GPU, no neural network, and no cloud service.

The wire protocol is [SIGNAL.md](SIGNAL.md); this page is the TouchDesigner side of it.

## Requirements

- TouchDesigner 2025 on macOS or Windows. Both files were saved in 2025.33230 (their About
  page says which build under Built With); TouchDesigner files open in newer builds but not
  reliably in older ones. There is no Linux build of TouchDesigner. The free Non-Commercial
  license is enough; both use only stock operators.
- Fosfora on the same machine as TouchDesigner, or on any machine on the same LAN. The
  analysis runs wherever the music is: Fosfora listens to an input device, so run it on the
  machine that can hear the mix (a loopback or monitor device for system audio, see the
  [README](../README.md#the-visuals-arent-reacting-to-my-music)).
- UDP port 9010 open on the TouchDesigner machine if Fosfora runs elsewhere.

## Five-minute quick start

**1. Run Fosfora in signal mode**, pointed at the TouchDesigner machine:

```
fosfora --signal --host <TD machine IP> --port 9010
```

On the same machine, plain `fosfora --signal` is enough (it sends to 127.0.0.1:9010). Pick
the input device once in the windowed app, or pass `--device "<name>"`; signal mode reads
the same saved settings.

**2. Drag in the examples.** Download
[`fosfora_examples.tox`](https://github.com/kevinraymond/fosfora/releases/latest/download/fosfora_examples.tox)
from the latest release and drag it into a network. The node shows the picture, the `out`
TOP inside it (1280x720). Its View parameter (Examples page) shows all four views tiled, or
one at full size, which is where their text is readable:

- **Dashboard**: a ring that fills once per bar, markers for the beat in the bar, a tunnel
  that zooms with energy, and the section, key, tempo and phrase position.
- **Grid**: 640 boxes. An eighth-note march over four bars, a ripple for every kick, sparks
  for onsets, the phrase filling the rows, and a sweep at every new phrase and section.
- **Lights**: eight moving heads and a haze machine whose look follows the section, with a
  chase that changes direction every phrase. Its Art-Net DMX output is there but off.
- **Keeping time**: Fosfora's bar as it arrives, the same with Steady Bars on, and
  TouchDesigner's own Beat CHOP driven by Fosfora, as dials and scrolling traces.

Open Window on the same page shows the picture in its own window at 1280x720, or filling
a display with Fill Display on (Display picks which); Esc closes it. If you passed a
`--port` other than 9010, set Port on the same page to match.

**What you should see.** Within a second or two of Fosfora starting, the views come alive:
the Dashboard's `NO SIGNAL` line goes away and its kick rings and beat markers move. The
tempo reads `finding the tempo` until it locks, which takes anywhere from a few seconds to
half a minute on a fresh track (mean lock time 12 to 30 s on the
[benchmark sets](BENCHMARKS.md)); the Grid's march and the Lights' chase wait for the lock.
The section and key fill in over the first bars. When a build commits, a small
`drop-like moment likely (experimental)` line appears in a corner of the Dashboard; see
[Drop-like moments](#drop-like-moments-experimental) for how far to trust it.

Every view is a COMP with a README and a comment on each operator. The Dashboard, Grid and
Lights read a copy of the component with Steady Bars on, so their bars always finish.

## Use it in your own network

**1. Add the component.** Download
[`fosfora_signal.tox`](https://github.com/kevinraymond/fosfora/releases/latest/download/fosfora_signal.tox)
from the latest release, or copy `fosfora_signal` out of the examples, and drag it into your
network. Its Status parameter reads `Online` with the input device's name within a second
or two of Fosfora starting.

**2. Set the Port parameter** on the component to match `--port` (default 9010). Leave
Active on.

**3. Wire a pulse to something visible.** Connect the component's first output (the CHOP)
to a Null CHOP and name it `fosfora`. Create a Circle TOP and put this in its Radius X
parameter (Radius Y too, or set it to `me.par.radiusx`):

```python
0.3 + 0.1 * op('fosfora')['beat']
```

The circle jumps on every beat and falls back the next frame. For a softer hit, take the
pulse with a Select CHOP (Channel Names `beat`) and put a Lag CHOP after it (Lag 0 on the
way up, 0.12 s on the way down), which is what the examples' Dashboard does. Lag only the
pulses: a Lag on `beat_phase` or `bar_phase` would smear their jump back to 0.

**4. Wire a phase to a ramp.** Create a Ramp TOP and put this in its Phase parameter:

```python
op('fosfora')['beat_phase']
```

The ramp now scrolls once per beat, locked to the music. Swap `beat_phase` for `bar_phase`
and it scrolls once per bar.

**What you should see.** Within a second Status reads `Online`, the `online` channel goes
to 1 (the heartbeat is 1 Hz) and `energy` moves with the music. `beat` pulses start as soon
as a tempo hypothesis exists; `locked` goes to 1 once the tempo holds. `bpm` reads 0 until
the first lock while Hold Tempo is on. `section` and `key` fill in over the first bars;
`in_outro` never goes to 1 on live audio (see the [honesty notes](SIGNAL.md#honesty-notes)).
The example's operators are a worked version of all of this, with a comment on each one.

## Reference

### Outputs

The component has two outputs. Output 1, `out_signal`, is a CHOP with one sample per
frame. Output 2, `out_states`, is a DAT with the string-valued state (`name`, `value`,
`confidence` columns). Wire from the outputs, or reach inside with
`op('fosfora_signal/out_signal')`; both names are part of the component's interface.

**Pulses** (1 for Pulse Frames frames when the event arrives, 0 otherwise)

| Channel | Fires on | From |
|---|---|---|
| `beat` | a beat | `/beat` |
| `downbeat` | the first beat of a bar | `/downbeat` |
| `onset` | a note or hit onset | `/onset` |
| `kick` | a kick-band onset | `/stem/drums/onset` |
| `boundary` | a confirmed section boundary with confidence at or above Boundary Gate | `/section/boundary` |
| `drop` | a drop-like moment (experimental, see below) | `/drop` |

**Musical time**

| Channel | Value |
|---|---|
| `bpm` | tempo in BPM; with Hold Tempo on, the last locked tempo, otherwise the raw estimate |
| `bpm_conf` | 0..1, how much recent evidence agrees with that tempo |
| `locked` | 1 while the tempo is locked |
| `bar_phase` | 0..1 sawtooth over the bar; advanced every frame at the current tempo when Smooth Phase is on |
| `beat_phase` | 0..1 sawtooth over the beat (bar phase times four, 4/4 assumed) |
| `beat_in_bar` | 1..4 |
| `phrase_bar` | bar within the phrase, 1-based |
| `phrase_beats_left` | whole beats until the next phrase boundary |
| `phrase_len` | inferred phrase length, 8, 16 or 32 bars (0 until inferred) |
| `phrase_conf` | 0..1 |

**Levels** (0..1): `energy` (short-term loudness, device-independent), `build` (riser
tension), `drums`, `bass`, `melody`. The last three are spectral proxies, not separated
stems; bleed exists.

**Drop-like prediction** (experimental, see [below](#drop-like-moments-experimental)):
`predict_drop` 0..1, `drop_warn` (1 at 0.5 and above), `drop_imminent` (1 at 0.8 and above).

**States**

| Channel | Value |
|---|---|
| `section` | 0 intro, 1 build, 2 drop (drop-like, experimental), 3 break, 4 outro, 5 steady, -1 none yet |
| `section_conf` | 0..1 |
| `in_intro` `in_build` `in_drop` `in_break` `in_outro` `in_steady` | 1 in that section (`in_drop` is the drop-like one; live audio never reaches `in_outro`) |
| `key` | 0..11 major C..B, 12..23 minor Cm..Bm, -1 none yet |
| `key_conf` | 0..1 (keys are only announced above 0.3) |

The same section and key arrive as text in `out_states`, with the input device name
(`status_device`) and estimator tier (`status_tier`).

**Event details**: `onset_strength`, `kick_strength`, `boundary_conf`, `boundary_age`
(seconds ago; the boundary detector always reports late, by a fixed amount it states).

**Health**: `beat_count`, `bar_count`, `drop_count` (Fosfora's running totals),
`onset_count`, `kick_count`, `boundary_count` (counted by the component as they arrive),
`lost` (events missed on the network, counted from gaps in Fosfora's totals), `online` (1
while a heartbeat arrived within Offline Timeout), `uptime` (seconds the sender has been
running).

**Anything else** Fosfora sends appears after these, named after its address with `/` and
`.` replaced by `_`: the raw feature bus (`fosfora --signal --feat-bus` gives `feat_sub_bass`
... `feat_chroma_11`) and Ableton Link state in builds that have it (`link_tempo`,
`link_peers`). Additive addresses in future `/v1/` releases arrive the same way without a
component update (see the [version policy](SIGNAL.md#version-policy)).

### Drop-like moments (experimental)

`drop` pulses when a sustained build ends in a loudness jump with the sub-bass back.
`predict_drop` (0..1) is designed to rise before one: `drop_warn` is 1 at 0.5 and above (a
build has committed), `drop_imminent` is 1 at 0.8 and above (the tier to act on). The `drop`
section (2, `in_drop`) carries the same caveat. All of them miss some real drops and fire on
moments that feel like one without being one, and on a continuous stream (a radio station, a
DJ mix) a committed build can carry across a song change and fire in the next track. They
are calibrated against EDM-style arrangements; on other material the 0.5 tier also fires on
chorus-scale dynamics. Treat them as cues, not ground truth.

### Parameters

| Parameter | Default | What it does |
|---|---|---|
| Status | | Read-only: what is arriving (see [Troubleshooting](#troubleshooting)) |
| Active | on | Listen for Fosfora |
| Port | 9010 | UDP port `fosfora --signal` sends to |
| Pulse Frames | 1 | How many frames each pulse stays at 1 |
| Smooth Phase | on | Advance `bar_phase` every frame at the current tempo instead of stepping at the send rate |
| Steady Bars | off | Every bar finishes: a moved downbeat is followed only once it holds for a bar, at the next bar line (see [How events arrive](#how-events-arrive)) |
| Hold Tempo | on | `bpm` keeps the last locked tempo while the lock is lost |
| Boundary Gate | 0.5 | Minimum confidence for a `boundary` pulse; 0.5 keeps under a third of boundaries at higher precision |
| Section Gate | 0 | Minimum confidence for `section` to change |
| Key Gate | 0 | Minimum confidence for `key` to change |
| Offline Timeout | 3 s | Seconds without a heartbeat before `online` falls to 0 |
| Reset | pulse | Forget everything received so far |

The About page shows the component version, the schema it reads (`/fosfora/v1`), the
TouchDesigner build it was saved in, and an Open Docs button for this page.

### How events arrive

Fosfora sends on three cadences ([emission semantics](SIGNAL.md#emission-semantics)):
events the moment they are detected, continuous values at the TX rate (30 Hz by default,
`--rate` changes it), and on-change values (section, key, phrase length) when they change
plus a 1 Hz re-broadcast. The component maps them to two kinds of channel:

- **Pulses** are one frame wide (or Pulse Frames wide). `beat`, `downbeat` and `drop` are derived
  from the running count on the wire, so a lost datagram still produces one pulse and adds
  to `lost`; a count that goes backwards means Fosfora restarted and is not counted as loss.
  A pulse is shown on the first frame the CHOP cooks after the message lands. The CHOP
  only cooks while something reads it (TouchDesigner pulls, it does not push), so an event
  that arrives while nothing is reading is history, not a pulse. For logic that must never
  miss an event, watch the matching count channel (`beat_count`, `onset_count`, ...) with a
  CHOP Execute DAT's `onValueChange`, or a Logic CHOP set to On When Value Changed.
- **Held values** keep the last received value until the next message. Between 30 Hz
  messages, `bar_phase` and `beat_phase` are extrapolated at the current tempo while locked,
  so they move smoothly at any frame rate; a small backward step when the next message lands
  is held rather than shown, so `beat_in_bar` never flickers. Everything else steps at the
  send rate; put a Lag or Filter CHOP after it if you need it smooth.
- **Steady Bars.** Fosfora restarts its bar wherever it currently hears the downbeat, and
  on some music that moves by a beat or two, and sometimes back, within seconds. Shown as
  it arrives, the bar is cut short: 1 2, then 1 2 3 4. With Steady Bars on, every bar
  finishes. A moved downbeat is followed only once Fosfora has reported it for a whole bar,
  and only at the end of a bar, where the last beat is held until the new downbeat arrives,
  so a bar can run long but never short. `bar_phase`, `beat_phase`, `beat_in_bar`,
  `downbeat`, `bar_count`, `phrase_bar` and `phrase_beats_left` then follow the steady bars;
  `bar_phase` holds just under 1 while the last beat waits, and `beat_phase` keeps cycling.
  Leave it off to see exactly what Fosfora sends.

Pulses with Pulse Frames at 1 are easy to miss in a TOP. Turn them into envelopes with a Lag
CHOP (0 up, 0.1 to 1.5 s down) before driving brightness or scale with them.

### Latency

- Events are emitted within one analysis hop of the audio, about 11.6 ms at 44.1 kHz, plus
  whatever the audio input buffer adds on the Fosfora side.
- TouchDesigner adds up to one frame (16.7 ms at 60 fps) before a pulse shows, since the
  message lands between cooks.
- Wired LAN adds well under a millisecond; Wi-Fi adds jitter, from a few milliseconds up to
  tens. Use a cable for anything that has to feel tight.
- In the head-to-head described below, Fosfora's beats landed about 10 ms behind the
  annotations on average (lastBeatStanding 4 to 6 ms), with the analysis clocked from the
  audio and no network in the path. A locked beat is already sent 10 ms ahead of its grid
  instant to make up for the analysis delay; if you need more lookahead, drive the visual
  from `beat_phase` instead, which you can offset freely.
- Continuous values are at most one TX period old (33 ms at 30 Hz). Raise `--rate` to match
  your frame rate if that matters; the bus is small enough.

## What to expect

In a head-to-head on identical audio (194 tracks of the Harmonix holdout set) against
lastBeatStanding, a beat tracker that runs inside TouchDesigner, Fosfora picked the tempo
better (Acc1 0.59 vs 0.48, Acc2 0.96 vs 0.68) and placed beats worse (beat F 0.70 vs 0.79).
Most of Fosfora's misses are a grid at the right tempo but half a beat off, or at double the
tempo on slow pop. Downbeats, sections, key, phrase
position and the drop-like prediction have no counterpart in that tool. The full per-dataset
numbers, against published offline and online systems, are in [BENCHMARKS.md](BENCHMARKS.md);
what the stream does and does not claim is in the [honesty notes](SIGNAL.md#honesty-notes).

## Troubleshooting

**Status says `Waiting for fosfora --signal on port 9010`.** Nothing has arrived.
- Check `--host`: it is the TouchDesigner machine's LAN address, not the Fosfora machine's.
  On the same machine, omit it.
- Check the port: the component's Port parameter and `--port` must match (both default to
  9010).
- Check the firewall on the TouchDesigner machine. macOS asks once whether TouchDesigner may
  accept incoming connections; if that was declined, allow it under System Settings, Network,
  Firewall. On Windows, allow TouchDesigner on private networks, or open UDP 9010.
- From any machine, `oscdump 9010` (liblo) shows whether the stream is reaching the port at
  all.

**Status says `Port 9010 in use`.** Another program on this machine holds the port (an
OSC tool, or another TouchDesigner process), and the component shows an error. Pick a free
port in the Port parameter and pass the same one to `--port`. Other OSC In operators in the
same TouchDesigner project can share the port with the component.

**Status says `No heartbeat for N s` or `Fosfora stopped`.** Fosfora quit (`stopped` is a
clean shutdown), crashed, or the network went down. The channels hold their last values.

**Status says `Online` but nothing moves.** Fosfora hears the wrong input. `energy` near 0 while
music plays means the device it is listening to is silent; pick the monitor or loopback
device in the windowed app or with `--device`, as in the
[README FAQ](../README.md#the-visuals-arent-reacting-to-my-music).

**Section and key never appear.** You are reading the stream with an OSC In CHOP instead of
the component. The OSC In CHOP ignores string arguments, so `/section` and `/key` never become
channels. The component reads an OSC In DAT and parses every message, which is why it exists;
if you bypass it, use an OSC In DAT for the string-typed addresses.

**Pulses never show.** Nothing reads `out_signal` every frame (see How events arrive).
Connect it to the operators you drive, or to a Null CHOP that something displays.

**The network freezes on a Mac after a few minutes idle.** TouchDesigner stops producing
frames while the Mac's display sleeps, even with Realtime off, and resumes where it stopped
when it wakes. For an unattended run, keep the display awake (`caffeinate -d` in a terminal,
or set display sleep to Never under Energy settings).

**Bars restart early or run short.** Fosfora moved its downbeat (see Steady Bars under
[How events arrive](#how-events-arrive)). Turn on Steady Bars so every bar finishes.

**Beats land off the beat.** Fosfora's grid can lock to the right tempo at the wrong phase,
or at a 3:4 or 2:3 ratio of the true tempo (see What to expect). `locked` and `bpm_conf`
tell you when the grid is trustworthy; gate grid-synced cues on `locked` and watch
`beat_in_bar` against the music before relying on downbeats.

## Updating

Dragging a `.tox` into a network embeds a copy in your project, so it never changes
underneath you. To update the examples, delete them and drag in the new file. To update the
component, do the same with `fosfora_signal.tox`; its channel names and both outputs are kept
across `1.x` versions, so whatever you wired to it keeps working. If you would rather pick up
a new file automatically, set the component's External .tox parameter (Common page) to the
saved file and turn Enable External .tox on.

## Rebuilding

Both `.tox` files are generated, never edited by hand. `integrations/touchdesigner/src/`
holds the component's extension and callbacks; `build_fosfora_signal.py` assembles the
component and `build_examples.py` builds the four views around it from the callbacks and
shaders in `examples/`, both using `tox_build.py`. To rebuild, open a TouchDesigner textport
(Alt+T) and run the component's script, then the examples', each with `__file__` set to its
own path:

```python
__file__ = '/path/to/integrations/touchdesigner/build_fosfora_signal.py'; exec(open(__file__).read())
__file__ = '/path/to/integrations/touchdesigner/build_examples.py'; exec(open(__file__).read())
```

They create `/project1/fosfora_signal` and `/project1/fosfora_examples` and write each `.tox`
with a `.json` beside it: the version, the build it was saved in and hashes of its inputs,
and for the examples the component they embed. Commit all four. `test_fosfora_signal_ext.py`
exercises the extension outside TouchDesigner and fails when the sources changed since the
last rebuild, or when the examples embed an older component.

## Addresses

The component reads every address in the
[`/fosfora/v1/` reference](SIGNAL.md#address-reference--fosforav1). Addresses added later
within `/v1/` reach `out_signal` as extra channels; a breaking change would arrive as
`/fosfora/v2/` alongside and would not be read until the component is updated.
