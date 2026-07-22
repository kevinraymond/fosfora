# Changelog

<!-- Release workflow extracts notes between ## vX.Y.Z headers via awk. -->
<!-- Keep the "## vX.Y.Z — date" format for automatic release notes. -->

## v1.13.0 — 2026-07-22

### Changed
- **Documentation moved into `docs/`, README rewritten for players** — the four reference documents now live in `docs/`, joined by a new `docs/GALLERY.md` and `docs/CREDITS.md`. The README leads with a hero clip and a download link instead of an architecture table, and hands off depth to `docs/`.
- **Effect catalogue corrected: 24 → 38 browsable effects** — the docs had never listed Beam, Cleave, Frost, Polycephalum, Splat, Strata, Tide, Vessel or the eight Lattice rules. `docs/GALLERY.md` is now generated from the shipped `.pfx` files, with a check mode that fails if the docs and the effects disagree.
- **Stale docs fixed** — `SECURITY.md` listed 1.8.x as the supported release, `CONTRIBUTING.md` asked for Rust 1.85+ against a 1.97 toolchain, `TUTORIALS.md` pointed the **+ New** button at the wrong effects directory (it writes to `assets/effects/` beside the app), and `bridges/README.md` was still entirely pre-rename.
- **README and gallery media regenerated** — a new hero montage and a clip of every one of the 38 effects, replacing a five-month-old teaser and a UI screenshot that existed only as a remote URL, so it broke in the release tarball.
- **Release notes rewritten for v1.9.0–v1.12.0** — those four sections had grown into design documents, averaging 1,400 characters per entry against v1.8.0's 238, and the published GitHub release pages with them. They are now one to three sentences per entry covering what changed for you and what might break; implementation detail and internal issue numbers live in the commit history. `CLAUDE.md` records the rule.

- **README shows what the app does past the defaults** — a second row of clips under the effect grid: water parting around a silhouette, four blended layers, a photoscanned 3D capture shattering on the drop, custom bindings driving three parameters from the drums, the harmony and the drop, your own image scattered into particles and sprung back, and a scene cue dissolve. The presets behind them ship in `scripts/capture/demos/`, so you can load them and take them apart.
- **Obstacles, Volumetric and the binding matrix now have tutorials** — three features the README advertises had no documentation at all. Each has a section covering the controls, the OSC addresses, and the part that decides whether it looks right.

### Fixed
- **Both built-in presets had a dead layer** — Crucible and Spectral Eye each pointed a layer at "Swarm", an effect removed several releases ago, so that layer silently kept whatever effect the *previous* preset had left there and wore the new preset's opacity and blend. The same file rendered differently depending on what you loaded before it. Both now use Array, and a preset that names a missing effect disables that layer instead of showing the wrong one.
- **The "Audio Reactive" and "Beat Sync" templates mostly did nothing** — they targeted `warp_intensity`, `color_shift` and `rotation`, which exist on one effect each out of 38, so Audio Reactive created four bindings of which three were dead and Beat Sync created two that were both dead, on almost anything you applied them to. They now pick each effect's own equivalent parameter. Template bindings also survive changing the selected layer, which previously killed them.
- **Four of the seven transform curves were doing nothing** — `ease_in_quad`, `ease_out_quad`, `ease_in_cubic` and `ease_out_cubic` were offered in the curve picker but not implemented, so choosing one left the value unchanged. They now work, and `linear`, `log` and `exp` — implemented all along but never offered — are selectable.

### Added
- **28 more audio features are now bindable from the binding matrix** — build-up, drop, the harmonic/percussive split, pitch, key, downbeat and bar phase, stereo width and pan, the six spectral-contrast bands and more were being computed every frame but appeared in neither source picker, so reaching them meant hand-editing a JSON file. All 74 features are now in the matrix, grouped, with the long groups collapsed by default.
- **`scripts/capture/` — a reproducible media pipeline** so the gallery can be regenerated at each release instead of going stale: a synthesized rights-clear demo loop, a capture script that films the real app under isolated config and private audio routing (your default sink is never touched), and a build step that renders the WebP tiles, hero montage and contact sheet.

## v1.12.0 — 2026-07-21

### Added
- **New effect: Splat — audio-reactive 3D Gaussian-splat playback** — load a pre-trained 3DGS capture (`.ply` or `.splat`) and play it back as a living cloud: the scene breathes with the music, shatters radially on the `drop` trigger and re-coalesces through the next phrase, onsets scatter a subset, and the spectral centroid drives a depth-of-field focal plane. Scenes stream and decode on a background thread, so the effect loads instantly and a failed load can never leave a layer half-swapped. Ships at a 2M-splat default — on an RTX 4090 at 1080p, 125 FPS at 1M and 70 at 2M.
- **Splat scene management** — a **Splat Scene** panel section shows the loaded scene, decode progress and errors, with a **Load Scene…** picker and a one-click demo-scene download. The loaded scene path round-trips through presets, re-decoding in the background on restore and warning-and-skipping if the file has moved.
- **Splat renders with real occlusion, matching reference viewers** — the default sorted path depth-sorts splats and alpha-composites front to back the way SuperSplat and PlayCanvas do, rather than averaging every splat along a ray with no occlusion — which washed fine detail to mid-tone and collapsed the tonal range. The order-independent path survives as a cheaper fallback for weak GPUs (`"sort": false`).
- **New effect: Frost — spectral-flatness material dissolution** — the sound's texture becomes the image's texture: pure sustained tones freeze the screen into crisp faceted ice, while noisy, breathy or distorted material erodes it into a drifting sand field with wind-blown dunes. The morph is not a crossfade — an erosion mask crumbles facet patches edge-first, so the crystal visibly *becomes* sand. Hiss fizzes the grain, bass gusts the drift, onsets shatter and glint.
- **New effect: Cleave — percussive/harmonic duet** — react to the drums separately from the melody. Ice-blue shards stab radially outward from a draggable fracture point on every drum hit, threaded through slow warm ribbons that swell and stiffen with pads and melody. A breakbeat reads as shards and a pad bridge as ribbons, shifting exactly as the arrangement does; a `balance` param forces either voice by hand, even mid-breakbeat.
- **New effect: Vessel — body-as-container fill-and-release** — your silhouette (webcam, depth or image obstacle) becomes a vessel that fills with trapped light as a buildup rises, then bursts outward exactly on the drop, each particle carrying its stored velocity. With no obstacle armed, a centered "amphora" takes the body's place. A `liquidity` param morphs it from pooling liquid to weightless drifting fireflies, and a bindable `release` param fires the burst by hand for sets without detector-clean drops.
- **New effect: Tide — flow-around-body water** — a luminous waterfall pours from the top edge and visibly parts, pools and eddies around obstacle silhouettes. Drums break the sheet into whitewater and upward foam spray; pads stiffen the flow so it glides as glass. Water pools on heads and shoulders and parts at the crest. Verified live at ~214K particles above 100 FPS with trails on.
- **Particle sims can read the harmonic/percussive split and the buildup/drop detectors** — `percussive_energy`, `harmonic_energy`, `harmonic_ratio`, `buildup` and `drop` have been computed live since the audio DSP tier landed and were already available to fragment shaders, but particle uniforms had no room for them, so no particle effect could separate drums from melody or follow song structure. Now appended, 832 → 864 bytes with all existing offsets stable.
- **Obstacles can be automated** — the obstacle surface was the only major subsystem with no binding targets and no OSC addresses, so the marquee interactive feature couldn't be cued or driven by audio at all. It now has binding targets (`particle.obstacle_enabled` / `_threshold` / `_elasticity` / `_mode`) and per-layer OSC addresses (`/phosphor/layer/{n}/obstacle/{enabled|mode|threshold|elasticity}`), so threshold can breathe on `audio.rms` and mode can hop on a MIDI knob.

### Fixed
- **HPSS energies were silently zero on real music at normal listening volume** — `percussive_energy` and `harmonic_energy` reached the normalizer as raw power (~1e-7 through a loopback capture), below its span-collapse epsilon, so they read a hard 0 — while full-scale synthetic test signals cleared it and made the pipeline look healthy. Every HPSS-gated visual was therefore largely dormant in live use. The energies are now dB-mapped at the producer, which is volume-invariant at any playback level: an A/B on identical live drum-and-bass read a median of 0.000 before and 0.635 after.
- **Splat: close-range parity with SuperSplat** — the sorted render looked right only at maximum camera distance; filling the screen it degraded to an icy white crust of speckle, with a black square artifact and a waxy low-contrast cast. Five compounding causes, from a projected-radius cap that collapsed every surface splat to 8px through to the global ACES tone curve lifting darks. Effects can now opt into a linear tonemap via `postprocess.tonemap`; every other effect is untouched.
- **Splat: raw 3DGS / SuperSplat exports load right-side-up** — captures use a Y-down convention while the orbit camera is Y-up, so a straight export appeared upside-down and had to be hand-inverted before export. The default rotation is now `[180, 0, 0]`, applied consistently to positions and covariance orientation; still a live slider for per-scene tweaks.
- **Tide no longer white-washes on real music** — the dominant cause was trail coverage: total light scales with trail *length*, which scales with speed, so loud music drove the flow 1.6× faster and multiplied deposited light with colour and alpha held flat. Alpha is now normalized by speed, so faster water reads as longer, thinner, dimmer streaks, and splash gates were retuned above the live sustained-groove floor so spray reads as sparks. Silence is untouched.
- **Frame hitches no longer white-flash particle effects** — the per-frame `dt` was raw wall-clock delta, so any stall (a mouse click's input hitch, a window drag, an effect swap) integrated as one giant step: particles teleported past kill bounds and mass-respawned, and trail ribbons smeared across the whole screen as a white flash. `dt` is now clamped to 50 ms, so a stall renders as momentary slow motion. Normal frame times are unaffected.
- **Obstacle maps are no longer aspect-distorted, and horizontal surfaces bounce** — obstacle UVs ignored aspect entirely, so a 256×256 depth map on a 1920×1080 window was 78% too wide; and normals were returned y-down but consumed y-up, so a floor-shaped obstacle's "outward" normal pointed *into* the floor and particles were swallowed rather than bounced. New per-layer **Fit** mode (Fill default, Fit, Stretch); existing presets restore as Fill, since the old stretch was a defect rather than a choice.
- **Scene transport bindings no longer re-fire every frame** — a binding to `scene.transport.go` / `prev` / `stop` was level-triggered, so holding a MIDI pad, or wiring any source that stays high like a sustained beat feature, advanced the timeline ~60 times per second instead of once per press. Audio-driven cue control was unusable. Transport targets now trigger on the rising edge; every continuous target stays level-driven and unchanged.
- **`layer.{n}.blend` bindings reach all 10 blend modes** — the normalized bus output was fed straight into an integer conversion that only ever yields 0 or 1, so a knob bound to blend mode toggled between Normal and Add and could never reach Screen, Colour Dodge, Multiply, Overlay, Hard Light, Difference, Exclusion or Subtract. The raw-integer OSC path already reached all ten and is unchanged.
- **Preset-scoped binding edits are no longer silently lost** — the debounced auto-save only ever flushed *global* bindings, so a preset-scoped binding you added or edited reached its sidecar only via an explicit preset save; quit or switch presets first and the edit was gone, with nothing in the UI to warn you. Preset-scoped edits now light the "unsaved changes" bar like any param edit. Quit also force-flushes pending global binding edits made inside the 1 s debounce window.
- **Right-panel edits round-trip through presets** — hand-tuned Lattice, particle-sim (emit rate / burst / lifetime / speed / size / drag) and Volumetric parameters were applied to live state but never saved into a preset, so they snapped back to the effect's `.pfx` defaults on the next effect reload or restart. Only `.pfx` `inputs` params used to survive. Old presets load unchanged, and the panel Reset still restores `.pfx` values.

### Changed
- **Splat's order-independent fallback tuned for real captures** — real captures use very low per-splat opacity (median ≈0.03) and extreme thin surfels, which the sort-free path rendered washed-out and translucent. Its depth weight now only reshades the average rather than gating visibility — a near-bias was deleting the far half of the figure — and coverage accumulation was raised so faint splats build a solid surface. This is now the low-end path; the sorted renderer above is the default.
- **Frost `audio_reactivity` default 1.0 → 0.4** — full-scale reactivity slammed the crystal↔sand morph between extremes on typical live material. Presets that saved the old value are unaffected.
- **Binding matrix "+ Add" popup migrated to the egui 0.33 `Popup` builder** — the last user of the deprecated popup APIs. No behavior change.

## v1.11.0 — 2026-07-19

### Added
- **Lattice — 3D cellular automata as selectable volumetric effects (flagship)** — a voxel grid of cells evolving by 3D birth/survival rules into self-organising structures: growing crystals, pulsing organisms, expanding shells, collapsing caverns. Unlike the 2D fields, the CA state *is* the 3D volume. Ships as eight browsable effects — Clouds, Crystal, Pyroclastic, Architecture, Coral, Builder, 445 and Pulse — at a selectable 32³–256³ resolution, with a contextual **Lattice (3D CA)** panel for live rule, grid, seeding and look controls.
- **Lattice presets sculpted so they stop collapsing into a featureless ball** — density is blended into the volume over time so fast rules fade rather than strobe, growth runs at a rate per *second* with a per-preset silence floor, an optional spherical domain kills the cube silhouette, and cell age shifts hue. The ball itself is fixed by a **per-cell lifetime** (new Lifetime slider) that keeps the structure turning over; four presets whose rules a lifetime cannot sculpt were replaced with ones it can — Amoeba → **Builder**, Architecture → **Chunky**, Crystal → **Brain**, Coral → **Shells**.
- **Volumetric Mode — particle density 3D ray marching** — a global toggle that renders any particle effect as a continuous fog or nebula instead of discrete dots: the app's first true 3D-volumetric look, and the prerequisite for Lattice. Dense regions cast shadows, so the cloud shows real 3D form rather than the flat uniform glow self-emission alone produces. Controlled from a **Volumetric (R3)** panel section and over OSC; `beat_phase` pulses density, `kick` bursts emission, `rms` drives absorption.
- **Polycephalum — 12-species slime-mold warfare keyed to the chroma vector** — twelve competing physarum organisms, one per pitch class, flood the screen with veined pulsing networks and fight for territory. Loud pitch classes move faster and deposit more, semitone neighbours repel while fourths and fifths cooperate, so a key change makes one colour visibly conquer another's ground. The first effect to consume the full 12-element chroma vector. Verified at ~59 FPS with 500K agents.

### UI
- **One shared param-row system across every panel** — the side panels had grown five incompatible "label + control" idioms, with labels riding inside sliders at varying offsets and a label column so narrow that Settings resorted to two-line labels, so nothing aligned column to column and the Audio tuning sections read as a zig-zag. Every row now shares one `label | control | value` grammar. Also replaces three byte-indexed name truncations that would *panic* on a multibyte name at the cut boundary.
- **Effects browser — search, type filter, favorites, readable names** — with 38 effects in a fixed 3-column grid and a 10-character cut, the browser was a wall of "Lattice A… / Lattice C…" clones. Adds type-ahead search (never auto-focused, so live typing is never hijacked mid-set), a clickable type filter, and a pinnable ★ favorites row persisted across sessions. Long names drop the grid to 2 columns and get a 22-character cut, so "Lattice Architecture" fits whole.
- **Lattice panel grouped** — ~28 always-visible rows became 8 rows plus four collapsible groups, with Rule, Grid size, Reseed and Randomise — the things you actually reach for live — kept on top.
- **Volumetric and Post-Processing panels extracted and aligned** — the last two right-panel sections without their own file, now grouped and on the shared row system.
- **One Triggers table for MIDI and OSC** — the ten mappable actions were listed twice, once inside each protocol's subsection, in two scrolls of near-identical UI and two places to look when checking what a controller button does. They are now a single table (action, MIDI badge, OSC badge) in its own collapsed **Triggers** section.
- **Binding matrix — cards get names and a no-signal warning** — a collapsed card showed only the enable dot, transform pills and meters, so an unnamed binding read as just "~ Smooth". Cards now lead with the binding's name and flag an enabled binding whose source isn't in the live snapshot — an unplugged controller or an un-substituted template previously just silently did nothing. Also fixes a per-frame memory leak, an idle expanded card rewriting the bindings file every second, and a duplicate-id collision where deleting one card removed two.
- **Binding matrix — cables stay in their bay, popups stop closing the overlay** — cables to rows scrolled out of view drew straight across the header, footer and neighbouring columns; they are now clipped, with off-screen endpoints drawn as faded stubs. Selecting an item from a picker that overhangs the panel edge no longer dismisses the entire overlay, and Escape now closes an open popup before it closes the matrix.
- **Binding matrix — live-workflow polish** — applying a template now switches to the tab where its bindings actually land (they were created in the other tab, reading as "nothing happened"); the MIDI Faders template substitutes a live device name instead of producing eight permanently-dead wildcard bindings; cards group by target instead of creation order; a Duplicate button clones a binding under a fresh id, the fastest way to build "same source, three targets" stacks; and hovering a card brightens its cables and dims the rest.

### Documentation
- **`AUDIO-FEATURES.md` — plain-English reference for all 74 audio features** — what each feature *means* musically rather than how it is computed, written for a VJ or musician deciding what to hook a slider to and anchored to named sounds ("a held synth pad sits near 0, a snare or white-noise sweep sits near 1"). Every entry carries a source: a paper or standard, a librosa descriptor, an explicit "no published algorithm" label so a heuristic is never dressed up as research, or a port note. All 41 links were verified to resolve, with archived snapshots where a canonical URL has rotted.

### Fixed
- **Stale audio documentation corrected** — `TUTORIALS.md` still described the pitch and spectral-contrast features as reserved and reading `0.0`; both have been live since v1.10.0. Both `TUTORIALS.md` and `TECHNICAL.md` also described adaptive normalization as "running min/max", which is the pre-A2 normalizer rather than the shipped one.

## v1.10.0 — 2026-07-18

### Added
- **Spectral contrast and timbre dynamics — `contrast_0…5` / `contrast_mean` / `timbre_flux`** — two gaps closed: nothing scored the per-band peak-versus-valley "grit" that separates a clean sine from a saw in the same octave, and nothing measured how fast a timbre was *moving*. Contrast reports that grit across six octave bands; `timbre_flux` measures timbre-shape change orthogonal to loudness, so a constant-volume filter sweep fires it while a plain volume change does not. A sawtooth reads contrast ≈ 0.92 against a sine's ≈ 0.52.
- **`audio.dmfcc.0..12` binding sources** — per-coefficient MFCC rates of change, available to any parameter binding. Kept out of the shader uniform to save 52 bytes.
- **Monophonic pitch tracking — `pitch` / `pitch_confidence`** — the engine could name the key and the pitch classes present but never the actual melodic f0, so no effect could follow a lead line or a bassline's note. A YIN tracker now reports f0 as a 0..1 log-frequency (55 Hz → 0.0, 1760 Hz → 1.0, so an octave is always 0.2) plus a confidence, holding the last pitch through unvoiced gaps so a pitch-keyed visual doesn't snap to the lowest note on every rest. Also broadcast in real Hz as `/phosphor/audio/pitch_hz`.
- **Harmonic/percussive split — `percussive_energy` / `harmonic_energy` / `harmonic_ratio`** — until now every feature saw the full mix: pads polluted kick flux, hi-hats polluted chroma, and routing drums and melody to separate visual channels meant ML stem separation. A causal median-filter split now partitions the spectrum into a sustained-tonal and a broadband-transient channel with zero added latency, giving two musically meaningful buses and a level-invariant ratio between them. A click train reads a ratio of 0.00, a sustained two-tone chord 0.999.
- **Stereo field — `pan` / `stereo_width` / `stereo_corr`** — every capture backend used to collapse to mono *at the callback*, so stereo was destroyed before anything downstream could see it and these three fields sat at `0.0`. The capture path now carries native stereo and derives the mono mix from it, so recording and every existing feature see a byte-identical signal for a 2-channel source. Unlocks pan-to-position and width-to-spread mappings. On input devices with more than two channels this now takes the front L/R pair rather than averaging every channel.
- **Shader ABI v3** — one batched bump reserving 13 feature slots so the DSP above could land without further churn: `NUM_FEATURES` 61 → 74, `AudioFeatures` 244 → 296 B, `ShaderUniforms` 352 → 400 B. Existing offsets are untouched and particle uniforms are unchanged, but compiled user shaders need recompiling once for this bump.

## v1.9.0 — 2026-07-17

### Added
- **Audio auto-reconnect** — the stall watchdog was detection-only: it raised a toast and left you to re-pick the device by hand. A confirmed device death now reopens the capture automatically, backing off 1/2/4/8 s across five attempts, with a new **AUD** status-bar dot reporting live, quiet, reconnecting or failed. Teardown runs off the render thread, so a backend stuck in a blocking read can't freeze the visuals. On by default; a destroyed PipeWire capture node is back with live data in under a second.
- **Follow the default output device when it changes (Linux)** — the common case on Linux isn't a device death, it's switching your system output mid-set, and PipeWire silently *migrates* the capture stream to the new default sink's monitor with no error and no stall — so the watchdog is structurally blind to it and the app goes on capturing the wrong device while looking perfectly healthy. The default sink is now polled every 5 s and the capture reopens onto whatever is current, within ~4 s end to end. Reuses the auto-reconnect setting; the Windows equivalent is a follow-up.
- **Smooth beat and bar phase at render rate** — the render thread took the newest 86.1 Hz analysis frame as-is, so on a 120–144 Hz display ~28% of frames reused an identical feature vector and phase-locked motion visibly stair-stepped, with a dropped frame reading as a phase pop. Features are now interpolated between frames, and both `beat_phase` and `bar_phase` run on a phase-locked loop advanced every render frame. Duplicate `bar_phase` frames went 10.4% → 0%, with tempo holding to 1.998 wraps/sec against 2.000 expected. Pulse timing is unaffected.
- **Tempo prior control — genre presets, auto-adapt, tap tempo, half/double** — the prior that decides which octave a track reads at was hardcoded at 150 BPM, so a 172 BPM drum-and-bass track could fold to ~86 with no way to say otherwise. Adds six genre presets (Neutral, Wide, House, Drum & Bass, Hip-hop, Ambient), an auto mode that walks the centre toward the tempo actually being locked, ×2 / ÷2 overrides and tap tempo — all MIDI/OSC-mappable and applied live. The default reproduces the exact previous behavior, so upgrading changes nothing until you pick a preset.
- **Musical key detection + true constant-Q chroma** — the chroma wheel hard-rounded every FFT bin to the nearest of 12 pitch classes (its "constant-Q" tooltip was simply false). It is now a real constant-Q chromagram with a slow tuning estimator, so a 432 Hz-tuned track no longer smears across pitch classes. A rolling correlation against the 24 Krumhansl-Kessler profiles reports the key with hysteresis, so it doesn't flicker between relatives. The audio panel gains a key readout (`C maj · 82%`) and the key is broadcast over OSC.
- **Build-up / drop / section-boundary detection** — the engine can finally see structure beyond a single beat, so the drop no longer has to be hand-triggered. `buildup` is a 0..1 tension ramp blending loudness rise, spectral brightening, onset density and the classic pre-drop sub-bass withdrawal; `drop` is a one-frame pulse fired when a sustained build-up breaks into a loudness jump with the sub-bass returning, with a 16 s refractory; `section_novelty` peaks where the arrangement's block structure changes. Heuristics tuned for electronic music.
- **Build-up / drop tuning panel** — those thresholds were hardcoded with no user access. Ten knobs are now live-tunable from a **TUNING · A18 build/drop** section in the audio panel, applied with no pipeline rebuild, no capture gap and no counter reset, persisted to `settings.json` and surviving a device switch.
- **EBU R128 / BS.1770 loudness — `loudness_m` / `loudness_s` / `loudness_trend`** — proper K-weighted momentary (400 ms) and short-term (3 s) LUFS, with filter coefficients re-derived for the actual device sample rate. `loudness_trend` is the rising component, a ready-made build hint. Also establishes a perceptual **silence gate** (momentary < −55 LUFS) that the other detectors now share, so silence is judged the same way across devices and content.
- **Downbeat / bar-phase / meter tracking** — a tracker sitting after beat detection scores 3/4 against 4/4 and each candidate bar phase by how much more "downbeat-like" the beats at that phase are, locking the winner with ~8-beat hysteresis and falling back to 4/4 aligned to the strongest recent beat when confidence is low. `downbeat` fires on the bar's one, `bar_phase` is a 0→1 sawtooth, `beat_in_bar` the normalized index. Roughly 70–80% accurate on 4/4 electronic music.
- **Audio textures — waveform / spectrum / scrolling mel-spectrogram** — three shader textures that shipped as 1×1 placeholders now carry real data: `waveform(x)` is a zero-crossing-triggered min/max PCM window (so the trace holds still), `spectrum(x)` a log-frequency magnitude spectrum, and `spectrogram(uv)` a scrolling 64-band mel history. Unlocks oscilloscopes, spectrum bars and waterfalls.
- **Beam — vector-CRT oscilloscope effect** — the first effect to draw the audio signal itself. Gaussian beam energy is integrated along the waveform polyline and dwell-weighted by inverse screen length for the signature bright-slow, dim-fast CRT look, rendered into a slow-decay feedback pass that acts as the phosphor persistence. Two modes: scope (horizontal sweep with a min/max envelope) and radial (waveform wrapped on a circle whose radius grows with `rms`). `centroid` sets colour temperature, `zcr` beam focus, `beat` persistence kick.
- **Strata — spectral-canyon effect** — a heightfield flown over the last ~8 s of mel history: the audio *is* the terrain, loud making ridges and quiet chasms, with lateral position as frequency and the newest audio erupting nearest the camera before ageing into the distance. Soft raymarched shadows, sun-tinted height fog, and height and slope materials with snow caps that follow loudness. Six params including a zoom for the overview. `rolloff` drives draw distance, `flatness` gloss, `beat` and `kick` ridge glow.
- **Mel bands as binding sources** — the 64-band mel column is now exposed as `audio.mel.0..63` binding sources, so any effect parameter can be driven by an individual band. Binding-only, not broadcast over OSC (64 args per frame would be heavy).
- **Audio panel STRUCTURE readout and an OSC Broadcast-TX toggle** — the new detectors are visible in-app without an external OSC monitor: short-term loudness, loudness trend, build-up, section novelty and a drop indicator that flashes on the pulse. Also fixes an OSC-panel gap — TX broadcast had host, port and rate fields but no enable checkbox, so it could only be turned on by hand-editing `osc.json`.
- **Shader ABI v2** — one batched bump reserving every slot the detectors above needed, so each could land without further churn: `AudioFeatures` 46 → 61 features (184 → 244 B), `ShaderUniforms` 288 → 352 B, plus the audio-texture bindings. Existing offsets are untouched, but compiled user shaders need recompiling once for this bump. Effects can bind the reserved fields immediately and light up automatically as each detector ships.
- **Particle shaders gain five more audio features** — `flatness`, `rolloff`, `bandwidth`, `bpm` and `beat_strength`, filling reserved padding with no layout change, so compute shaders can react to spectral shape and tempo strength the way fragment shaders already could.
- **Community health files** — `CODE_OF_CONDUCT.md`, `SECURITY.md` and a pull-request template.

### Changed
- **Spectral feature correctness pass — centroid / flux / flatness / rolloff** — all four were subtly wrong. Centroid lived in the top octave and was unstable near silence; it is now a power-weighted mean of log-frequency across a musical 40 Hz–18 kHz range, usable as a brightness fader. Flux doubled when the volume doubled (it was effectively a second RMS) and is now level-invariant, measuring change rather than level. Flatness now cleanly separates tonal pads from noise sweeps. All six spectral-shape features are also broadcast over OSC — previously only `kick` was.
- **Kick detection no longer double-normalized** — `kick` was auto-levelled twice, so it was nearly always saturated on quiet material and its scale depended on two interacting auto-levelers. It is computed once now, as a level-invariant log-magnitude flux against its own long-term percentile, gated on the perceptual silence flag. Kick-bound strobes stop firing on hi-hat bleed, bassless leads and quiet passages, and stop saturating on loud sustained bass.
- **Gated percentile normalization with per-feature policies** — one symmetric running min/max was applied to every feature, so quiet-room noise stretched to full scale and visuals danced to silence, and a single transient spike squashed everything for ~2 s. Each feature now picks a policy: energy features use gated percentile ranging, frozen under perceptual silence so a quiet room can't be ranged up; known-range spectral features hold their last value through silence instead of dancing; MFCCs standardize symmetrically about 0.5.
- **Unified band scaling** — the seven frequency bands were computed in two incompatible families, half linear RMS and half dB, so a visual bound to `band.0` and one bound to `band.5` behaved like different species and the normalizer saw wildly different input dynamics. All seven now share one dB domain with an equal-loudness tilt above 2 kHz. A **Settings → Band Scale** dropdown keeps `Legacy` available, reproducing the exact previous behavior for presets tuned to the old feel.
- **SuperFlux onset detection closes the 250–500 Hz snare gap** — the onset stage used four fixed bands with a hole at 250–500 Hz (snare bodies, toms, male vocals), and false-triggered on vibrato and pitch slides. It is now SuperFlux: a 64-band contiguous filterbank whose reference frame passes through a frequency maximum filter, so a partial can drift slightly between frames without registering an onset. The kick/snare/hat balance is preserved and tempo tracking downstream is undisturbed.
- **Deterministic 512-sample analysis hop** — the audio thread slept 10 ms and analyzed whatever had arrived, so the effective hop varied with scheduler jitter and grew under load: flux amplitudes moved with hop size and the tempo estimator had to guess its own frame rate from wall-clock timestamps. It now runs exactly one frame per 512 samples on a sample clock, so timing is exact even when a burst of hops is processed in one wakeup, and the estimator's frame-rate guessing is gone.
- **Audio feature schema as a single source of truth** — normalization, smoothing and stale-decay policies lived in a hand-ordered 46-entry table and positional index literals, so adding a feature risked silently shifting indices out from under those stages. All three now read one ordered table, pinned to the struct layout by a compile-time assertion. No behavior change.

### Fixed
- **`beat_phase` dropped to 0 several times a beat on loud audio** — the phase-freeze gate tested normalized `rms`, which floors at exactly 0.0 whenever the signal touches the bottom of its own recent range — on rhythmic material, the trough between every hit. So it fired on perfectly loud audio, manufacturing false wraps: a 120 BPM signal read 2.12 wraps/sec instead of 2.00. It now gates on the perceptual silence flag, which a loud trough can't trip. Spurious zeros 164 → 0.
- **`bar_phase` swept through silence while `beat_phase` sat frozen** — the beat detector took the perceptual silence gate and pinned its phase at 0; the downbeat tracker never took it, so through a quiet passage the bar clock kept ramping against a dead beat, and any bar-synced visual inherited the mismatch. Verified live on a silent monitor: across 361 samples `beat_phase` held a single value while `bar_phase` swept the full 0→1.
- **Quitting during an audio stall could hang the process** — shutting down dropped the capture inline, which joins a thread that may be blocked in a timeout-less read, so the join never returned and the app hung on exit rather than closing. It now hands the backend to the detached reaper and exits.
- **PulseAudio read errors retried forever at 10 Hz** — once the server kills a `pa_simple` stream every subsequent read fails, and the capture loop had no exit path, so a dead stream pinned a thread logging an error ten times a second for the rest of the session. It now gives up after ~1 s and publishes a failure the watchdog can act on. A successful read resets the counter, so transient errors are unaffected.
- **A device switch stranded an in-progress recording's audio** — switching installed a fresh recording ring, but a recording started earlier holds the old one, so its writer was left draining a ring nobody writes to and the rest of the take recorded silence. The ring is now threaded through the reopen. Note that a recording still captures the sample rate at start, so reopening onto a device with a different rate pitch-shifts the remainder.
- **A stale or foreign pipeline cache no longer aborts startup** — a `pipeline_cache.bin` written by a different GPU or driver (a GPU swap, hybrid graphics, an eGPU, or a driver update) made the first pipeline creation fail, so the app died with "Failed to initialize app" and a black launch. The cache is now validated on load, and discarded and rebuilt if the driver rejects it.
- **BPM test harness fed frames at the wrong clock** — the synthetic convergence tests spaced kicks at 100 Hz while the detector had moved to an 86.1 Hz hop, so every target tempo reached the estimator 13.9% low (a nominal 172 arrived as 148). The ±15% tolerance bands were wide enough to hide it, so the tests passed while asserting something other than what they claimed.

### Documentation
- **README and TUTORIALS rewritten for newcomers** — a plain "What is this?", a 3-step quick start with first-10-seconds expectations, a "Make it yours" section, and an FAQ covering audio-input selection, black screens and macOS notarization. Also corrected the effects lists (the README listed 8 of the old set; TUTORIALS listed four effects that don't exist), the keyboard table, the build prerequisite and the audio band ranges.

## v1.8.0 — 2026-07-15

### Changed
- **Rebranded to Fosfora** — the project, app, window title, NDI source name, macOS bundle, release binaries (`fosfora` / `Fosfora.app`), and documentation are now "Fosfora" (Northern Sami for phosphorus). The signature **Phosphor** effect keeps its name as a heritage nod. Two runtime surfaces are intentionally unchanged this release to avoid breaking existing setups and will migrate later: the config directory (`~/.config/phosphor/`) and the OSC namespace (`/phosphor/*`), along with the Python `bridges/` tooling that targets it. The macOS bundle identifier (`com.kevinraymond.phosphor`) is also unchanged so existing microphone permissions are preserved.

### Fixed (docs)
- **Doc accuracy** — corrected the built-in effect count (23 → 24) and removed the non-existent "Swarm" effect from the README table; corrected the audio-feature count (20 → 46) in the README and tutorials; de-duplicated the AI-assist disclaimer in the README "Note from Dev"; fixed the `.pfx` `passes[].feedback` default in TECHNICAL.md (documented as `false`, actually `true`).

### Security
- **Dependency advisories** — bumped `anyhow` 1.0.102 → 1.0.103 (RUSTSEC-2026-0190, unsound `Error::downcast_mut`) and updated `wayland-scanner`/`uds_windows` patch versions; added documented deny.toml ignores for quick-xml RUSTSEC-2026-0194/0195 (DoS via untrusted XML — only reachable through `wayland-scanner`, a build-time proc-macro parsing trusted bundled protocol XML; remove once the Wayland stack allows quick-xml ≥ 0.41)
- **Dependency advisories** — bumped `rustls-webpki` 0.103.9 → 0.103.13 (RUSTSEC-2026-0049, RUSTSEC-2026-0098) and `tar` 0.4.44 → 0.4.46 (RUSTSEC-2026-0067, RUSTSEC-2026-0068) to clear the cargo-deny audit

### Fixed
- **Multi-pass feedback default** — `passes[].feedback` now defaults to `true`, matching legacy single-shader behavior, so converting an effect from `shader:` to `passes:` no longer silently disables feedback (set `"feedback": false` explicitly to opt out; all built-in effects set it explicitly and are unaffected)
- **Beat pulses no longer dropped** — beats are latched via an atomic counter so 1-frame beat triggers survive channel overflow and multi-frame drains; burst-on-beat effects fire on every beat even under load
- **Visuals settle on audio stall** — held audio features now decay to silence in ~1s instead of freezing at the last loud frame when the device stops delivering data (BPM readout preserved); a watchdog surfaces a status toast once per stall episode (detection only — no auto-reconnect, which could hang on a capture thread blocked in a timeout-less read; on Windows a >10s playback pause may trigger the toast since WASAPI loopback delivers no packets during silence)
- **Normalizer field pass-through** — all beat-detector-owned features (onset, beat, beat_phase, bpm, beat_strength) now bypass adaptive normalization; an off-by-one index range previously misclassified onset and beat
- **Recording A/V sync** — audio recording now starts from the moment recording begins instead of draining minutes of stale ring-buffer history into the encoder; ring-buffer reads are also clamped to capacity so a lapped consumer recovers with the newest window
- **Save errors surfaced** — effect saves (particle definition and debounced parameter saves) and recording-start failures now show a status-bar error toast instead of failing silently; a failed parameter save no longer marks the shader editor's paired `.pfx` as clean
- **Clippy warnings** — cleared 13 default-feature clippy lints (collapsible match guards in `app.rs`/`main.rs`/`web/state.rs`, derivable `Default` for `ParticleQuality`, redundant `String` clones in UI panels)
- **Depth feature** — `depth` feature now depends on `webcam` (depth estimation requires webcam input), eliminating dead-code warnings when building with `--features depth` alone
- **Windows CI warnings** — removed unused import, allowed dead code on `wasapi_available()`, fixed unreachable expression in `create_audio_fifo()`, fixed function pointer cast in midir patch
- **Cross-target clippy debt** — fixed a Windows-only `ptr_as_ptr`/`ptr_cast_constness` violation in `wasapi_capture.rs` and a `webcam`/`depth`-feature `implicit_clone` in `webcam.rs`, both previously invisible to the host-only clippy runs (board #1500)

### Added
- **Pre-commit hook** — `.githooks/pre-commit` runs `cargo fmt --check` and `cargo clippy -D warnings`
- **Reduced-motion detection (macOS/Windows)** — implemented the platform detection that was previously stubbed to always return `false`: macOS via `NSWorkspace.accessibilityDisplayShouldReduceMotion` (objc2-app-kit), Windows via `SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION)`. Linux (gsettings) already worked. This is the detection backend only — `ReducedMotion` is not yet consumed by any effect/animation, so there is no user-visible behavior change until it is wired in

### Changed
- **CI lints cfg-gated platform code** — the cross-OS `build` matrix now runs `cargo clippy -D warnings` per native target (Linux/macOS/Windows), so `#[cfg(target_os = …)]` code that host-only clippy never compiled is finally linted; the Linux `lint` job now denies warnings too (matching the pre-commit hook). The pre-commit hook stays host-only for fast commits — see CONTRIBUTING.md for the manual cross-target command. (board #1500)
- **README** — expanded build-from-source section with per-feature prerequisites; added Binding Matrix section and `B` keyboard shortcut
- **NDI docs** — clarified that NDI output is built into official release downloads (only the NDI runtime needs installing); the `--features ndi` flag now framed as a from-source-only step in README and TUTORIALS

### Documentation
- **Stale-docs sweep** — corrected the `AudioFeatures` doc comment (45 → 46 features) and renamed the byte-size test to match its 184-byte assertion; fixed the audio-panel footer to report the real multi-resolution FFT sizes (`4096/1024/512` rather than a bare `512`)

## v1.7.1 — 2026-03-12

### Fixed
- **Binding matrix button** — clicking the "Matrix" button in the left panel now correctly opens the binding matrix (previously opened for one frame then immediately closed due to same-frame click-outside detection)
- **Status bar hint** — added `B binding matrix` keyboard hint to the status bar

### Added
- **Cargo.toml metadata** — added `description`, `repository`, `readme`, `publish` fields to member crate
- **CI format check** — added `cargo fmt --all -- --check` job to CI workflow
- **Dependency auditing** — added `deny.toml` and `cargo-deny` CI job for license compliance and vulnerability scanning
- **Direct video recording** — record Phosphor output directly to MP4/MKV via FFmpeg subprocess with NVENC hardware encoding (auto-fallback to CPU encoders). Supports H.264, HEVC, and AV1 codecs up to 8K resolution at 30/60 FPS with configurable CQ quality. Includes audio capture from the active audio input (muxed as AAC via named FIFO). Recording runs independently alongside NDI output. New "Outputs" subsection in Settings panel with record button, codec/resolution/FPS/quality/audio controls, and live status display (duration, file size, encoder info). No new crate dependencies — uses the same subprocess pattern as the FFmpeg webcam backend.
- **Shared FrameCapture** — extracted double-buffered GPU readback from `NdiCapture` into `gpu::frame_capture::FrameCapture`, reused by both NDI and recording systems
- **OutputResolution::Res8K** — 7680x4320 output resolution option for both NDI and recording
- **OutputResolution moved to `gpu::types`** — shared by NDI and recording modules (NDI re-exports for backwards compatibility)

### Changed
- **Background shader compilation** — shader hot-reload (fragment + compute) now compiles on a dedicated background thread instead of blocking the main render loop. Eliminates 50-500ms frame hitches when saving `.wgsl` files during development. Old pipeline continues rendering while the new one compiles; swap is atomic on completion.
- **Error handling sweep** — replaced all runtime `.unwrap()` calls with `.expect("reason")` documenting invariants (mutex locks, container access, thread spawns). Added `// SAFETY:` comments to all 22 unsafe blocks across FFI bindings (WASAPI, PulseAudio, JACK, NDI, ALSA) and internal unsafe code (ring buffer, pipeline cache). Enabled `clippy::undocumented_unsafe_blocks` lint to prevent future regressions.
- **Idiomatic Rust pass** — eliminated per-frame allocations and unnecessary clones across the hot path:
  - `ParamStore::split_borrow()` for disjoint field access (removes 3× `defs.clone()` per frame in MIDI/OSC/WS update paths)
  - `PlaybackState` now derives `Copy` (removes `state.clone()` per frame in timeline tick)
  - Binding bus: snapshot moved instead of cloned; `HashMap::get_mut` fast-path for runtime lookup (avoids `binding.id.clone()` per enabled binding per frame)
  - Match on `&ParamDef` / `&TransformDef` references instead of cloning owned values
  - PFX hot-reload: `new_effect` moved instead of cloned; `old_effect` borrow replaces full clone
  - Shader hot-reload: effect borrowed from loader instead of cloned
  - Added `#[derive(Debug)]` to `BindingRuntime`, `LearnState`, `BindingTemplate`, `TemplateEntry`, `NdiFrame`, `PresetDecodeRequest`, `DepthFrame`; `Default`+`Clone` on `BindingRuntime`
  - `builtin_raster_images()` returns `&'static [String]` instead of `&'static Vec<String>`
  - MIDI panel: port selection deferred to after iteration (avoids `available_ports.clone()`)

### Fixed
- **Accretion drift** -- N-body simulation no longer drifts off-screen over time. Nonlinear centering force (gentle at origin, strong near edges), center-biased seed spawning, and tighter boundary kill prevent compounding center-of-mass shift

## v1.7.0 — 2026-03-11

### Added
- **Particle binding targets** — particle system settings (emit rate, burst on beat, lifetime, speed, size, drag, turbulence, gravity X/Y, vortex strength) exposed as `particle.*` targets in the binding bus. Applies to all layers' particle systems. New "Particles" group in Binding Matrix target picker.
- **Xbox Controller bridge** — `xbox_controller.py` streams gamepad inputs (analog sticks, triggers, d-pad, 11 buttons → 23 fields) into Phosphor's binding bus via `evdev`. Radial deadzone (configurable `--deadzone`), Y-axis inversion, auto-detect by name matching, hot-plug reconnection, 60 FPS default. Docker support with `privileged` + `/dev/input` mount.
- **Bridge source preview thumbnails** — vision bridges (MediaPipe hands/pose/face, YOLO) send annotated camera frame thumbnails (160x120 JPEG, ~8fps) over binary WebSocket. Phosphor decodes and renders inline previews in the binding matrix above each WS source group's fields, giving immediate visual feedback about camera position, detection quality, and model output.
  - `PhosphorBridge.push_preview(frame)` — rate-limited JPEG thumbnail sender with `--no-preview` and `--preview-fps` CLI flags
  - Binary WS wire format: `[source_name_utf8] [0x00] [jpeg_bytes]` — zero impact on numeric data path
  - Thumbnails auto-clean when source fields expire; hidden when group is collapsed (no decode overhead)

### Docs
- **Bridges README quick start** — added "What's a Bridge?" explainer and beginner-friendly quick start with three paths (no hardware, webcam, gamepad)

### Fixed
- **Multi-layer binding targets** — bindings now support explicit layer indices (`param.0.Turing.drag`) and auto-migrate legacy 3-part targets (`param.Accretion.trail_decay` → `param.1.Accretion.trail_decay`) on preset load, fixing bindings only affecting the active layer
- **Binding Matrix light theme readability** — replaced all hardcoded dark-mode colors (`from_white_alpha`, `from_black_alpha`, `from_rgb(0x22,...)`) with `ThemeColors` semantic equivalents so the UI is readable across all 6 themes (Dark, Light, Midnight, Ember, Neon, High Contrast)
- Added `text_dim`, `hover_fill`, `hover_border`, and `backdrop` fields to `ThemeColors` for fine-grained UI element theming
- **Binding Matrix collapse/expand all** — single toggle button in Sources and Targets column headers to collapse or expand all groups at once

### Changed
- **Configurable video device** — bridge docker-compose accepts `VIDEO_DEVICE` env var (e.g. `VIDEO_DEVICE=/dev/video4 docker compose up pose`) for multi-camera setups; host device is always mapped to `/dev/video0` inside the container so OpenCV finds it at index 0
- **YOLO bridge dynamic class detection** — no longer hardcodes 4 COCO classes; discovers all 80 classes at runtime and only sends fields for classes currently detected. Schema is re-sent when a new class first appears.
- **Per-field WS expiry** — binding bus now expires individual WS fields after 5s of no updates (was per-source). Unbound fields are removed from the picker; bound fields stay alive at 0.0 so bindings survive when a dynamic source temporarily disappears.
- **PhosphorBridge `send_schema()`** — new method to re-send schema mid-session for dynamic field discovery

### Added
- **Bridge Scripts** — Python companion scripts in `bridges/` for streaming external data into the binding bus via WebSocket
  - Base class `PhosphorBridge`: WS connect, schema declaration, data push, reconnect, graceful shutdown, common CLI args
  - 9 bridge scripts: MediaPipe hands/pose/face, YOLO object detection, RealSense depth, Smart LFO generator, iPhone ARKit (UDP), Leap Motion (placeholder), Azure Kinect (placeholder)
  - Test echo server for validating bridge output without Phosphor running
  - Split requirements files: base, vision, depth, lfo
  - Docker packaging: layered images (base → vision/lfo/realsense/leap), GPU variant, docker-compose with profiles
  - All bridges use the existing WS `/bind` protocol — no Rust code changes needed
- **Binding Bus** — universal source→transform→target system replacing per-parameter MIDI/OSC mappings
  - Any source (audio features, MIDI CC, OSC, WebSocket) can drive any target (effect params, layer opacity/blend/enabled, global opacity)
  - 10 composable transforms: remap, smooth, invert, quantize, deadzone, curve, gate, scale, offset, clamp
  - Preset-scoped and global-scoped bindings with JSON persistence (sidecar files for presets)
  - WebSocket `/bind` protocol: external apps send `{"type":"data","source":"...","fields":{...}}` for real-time control (e.g. MediaPipe hand tracking)
  - New Bindings panel in left sidebar with source legend, live value bars, inline transform editor, learn mode
  - Bindings panel UI overhaul: collapsible Preset/Global sections, two-line collapsed rows with accent bars and source badges, custom-painted source picker with live bars and WGSL uniform references, inline transform parameter editing with DragValues, Raw/Norm/Out preview area
  - Source picker shows friendly names for all 46 audio sources (e.g. "Sub Bass" instead of "band.0"), sub-grouped by type (Bands, Features, Beat, MFCC, Chroma)
  - Binding templates: 4 built-in presets (Audio Reactive, Beat Sync, Spectral Bands, MIDI Faders) with one-click apply
  - PostFX binding targets: bloom threshold/intensity, vignette, chromatic aberration, film grain
  - Scene transport binding targets: next cue, previous cue, stop
  - Raw shader uniform binding targets: override any of 23 uniform fields (audio bands, features, feedback_decay, time) directly from any source
  - One-time migration of legacy MIDI/OSC param mappings to bus bindings on first launch
  - MIDI and OSC systems now accumulate last-seen values for bus source collection (zero overhead on existing paths)
  - **Binding Matrix modal** — fullscreen three-column flow editor replacing sidebar bindings panel
    - Sources (left), binding cards (center), targets (right) with collapsible groups
    - Bezier connection lines between sources → cards → targets with animated flow dots
    - Expanded card editing: source/target pickers, transform pills, Raw/Norm/Out preview, learn mode
    - Effect/Global scope tabs, templates dropdown, keyboard shortcut `B` to toggle
    - Sidebar bindings section now shows compact stub with active count + "Matrix" button

### Fixed
- Bridge containers on Linux (native Docker Engine): added `extra_hosts: host.docker.internal:host-gateway` to all services so `host.docker.internal` resolves correctly (no-op on Docker Desktop)
- WebSocket binding sources now expire after 5 s of silence — `web.bind_values` was never cleared after ingestion, so stale keys kept refreshing `last_seen` and sources persisted forever
- Source picker dropdown overlap: moved `max_height` from inner `set_max_height()` (which capped content area) to `ComboBox::height()` (which caps scroll viewport), fixing row pile-up when content exceeded 350 px

## v1.6.0 — 2026-03-07

### Added
- Acknowledgments section in README — credits for third-party libraries, algorithms, research, and fonts
- Updated README feature table — compute rasterizer, 23 effects, MFCC/chroma audio, webcam/depth layers, scene cues
- Webcam device selector — enumerate and switch between multiple cameras in webcam layer panel, obstacle panel, and layer add button
- Persist selected webcam device in settings (remembered across restarts)
- FFmpeg webcam capture backend — opt-in setting for virtual cameras (Irium, DroidCam, etc.) that only expose DirectShow on Windows; uses `ffmpeg` subprocess instead of nokhwa, no new crate dependencies
- Preset save/restore resolves webcam by device name, with fallback to default
- Inline "Camera not available" indicator in webcam panel when capture is dead

### Changed
- Clean build: eliminated all 62 compiler warnings in phosphor-app (unused vars, dead code suppressions, borrow scope fix, removed dead `ParticleSourceRequest` enum and `recreate_draw_bind_groups` method)
- Suppressed 2 `unexpected_cfgs` warnings in vendored midir patch

### Fixed
- Webcam mirror checkbox now actually flips the image (was setting flag but shader never read it)
- Webcam device switch failure no longer kills the active capture — restores previous device on error
- Deduplicate webcam device list — Linux V4L2 exposes multiple nodes per physical camera (main, metadata, IR); now keeps only the lowest index per name
- Input device enumeration now works when native loopback (PulseAudio/WASAPI) is active — microphones and other input devices appear in the device selector dropdown
- Selecting a specific input device now correctly switches to cpal capture instead of re-entering the loopback path
- Deduplicate input device names (ALSA exposes multiple nodes per card with identical descriptions)
- Handle non-f32 sample formats (i16, i32) when opening input devices — fixes "Sample format 'f32' is not supported by hardware" error
- Suppress JACK client library stderr spam ("Cannot connect to server") via null error/info handlers on Linux
- Move device enumeration to background thread — eliminates ~200ms UI freeze from ALSA/JACK probing every scan cycle

## v1.5.0 — 2026-03-06

### Docs
- Add QUICK-REFERENCE.md with keyboard shortcuts, OSC/MIDI commands, and CLI flags

### Fixed
- Scene cue delete button disappearing when the same preset is added as a cue multiple times (egui widget ID collision)
- Crash (index out of bounds) when deleting a scene cue while the timeline is playing

### UI
- Right panel restyle: single-row param layout (name/slider/value/M/O), compact 16×14 MIDI/OSC badges, smaller Reset All button, large particle count header with 3px health bar, standalone bordered blend mode badge, compact particle param rows replacing grid, tab-strip obstacle source selector with active state styling, compact obstacle slider rows with separate value labels, audio mapping hover background lift with fixed-width 52px feature names
- Remove redundant activity dots from Settings subsection enable rows; status bar always shows MIDI/OSC/WEB/NDI indicators with three-state dots (active/enabled-idle/disabled)
- Consolidate MIDI, OSC, Web, NDI/Outputs, and Global into single collapsible "Settings" section with subsections — header shows labeled status dots, subsection headers use smaller font/arrow for visual hierarchy, protocol-specific badge colors
- Trigger grids (MIDI + OSC) use fixed-width label columns (52px) for proper badge alignment across rows
- OSC TX settings wrapped in subtle bordered frame; Global settings use fixed-width labels aligned to dropdowns
- Layers panel visual refresh: type color legend (FX/MD/WC), left color strip per card, layer index numbers, type badges, type-colored add buttons (all three on one row), subtle "Clear All" text link with red hover/confirm, footer type breakdown counts, hover-only delete buttons, stronger dimming for disabled layers
- Reorder sidebar panels: Effects → Layers → Presets → Scenes
- Layer badge shows "N/8" capacity format
- Scenes panel visual refresh: 2-column scene grid tiles, two-line cue rows with bottom progress bars (hold elapsed + transition progress with stripes), accent active-cue highlighting, pulsing red LIVE dot, GO button green fill, STOP red outline, hover-reveal delete buttons, hold time always visible, scrollable cue list (up to 6 visible), add-cue bar with separator
- Timeline bar polish: accent-colored active cue, striped from-cue during transitions, two-line target cue label (name + "Morph 58%"), transition-typed playhead colors
- Presets panel: add "Save current state as preset:" label above name input, and make "+ New" button clear all layers to a fresh Phosphor state with two-click confirmation when unsaved changes exist
- Amber dirty-state styling for presets panel: custom header with pulsing dot and inline preset name, amber-tinted dirty bar with Update/Reset buttons, amber selected tile, pulsing card border, and bumped tile height to 26px
- Add effect type indicators to effects panel: left color strip (purple SH / orange PS / teal FB), two-char badge, type legend, and footer breakdown count
- Add hover tooltips to effect type color legend explaining Shader, Particle+Shader, and Feedback categories
- Show Copy and Edit buttons side by side — Copy enabled for visible built-ins only, Edit enabled for user effects only; fixes Copy being clickable on hidden Phosphor branding effect
- Add "msc: N" badge to PARTICLES panel showing `max_scaled_count` with tooltip; merge blend mode and feature badges into single wrapping horizontal row; normalize morph section to 9pt font
- Add `EffectType` enum (Shader, Particle, Feedback) with optional `effect_type` field in .pfx format — auto-detects from data when absent
- Tag 5 feedback effects (Accretion, Array, Chaos, Mycelium, Turing) with explicit `"effect_type": "feedback"` in their .pfx files

### Audio
- Add `dominant_chroma` feature: CPU-side argmax of 12 chroma bins, normalized 0–1, forwarded to both fragment and particle GPU uniforms
- Cymatics audio remapping: chroma-driven Chladni mode selection (curated visually-distinct (n,m) pairs per pitch class), mfcc[0] vibrational amplitude, chroma peakedness pattern clarity
- Murmur audio remapping: mfcc[1] flock cohesion (bright=tight, dark=dispersed), mfcc[3] predator aggressiveness, flux speed agitation, dominant_chroma rim/sky hue; fix emitter waves by probing whole screen + inheriting donor heading
- Symbiosis audio remapping: mfcc[4-6] per-species force modulation (timbre shifts ecology), dominant_chroma species color rotation, flux interaction radius expansion, zcr Brownian noise jitter
- Morph audio remapping: dominant_chroma target shape selection (pitch class picks morph target instead of sequential), mfcc[1] transition speed modulation, mfcc[2] mid-transition scatter/explosion strength, chroma-tinted background
- Flux audio remapping: mfcc[1] curl strength (bright=tight spirals, dark=loose), flux-driven turbulence agitation, dominant_chroma emission/background hue anchor
- Add `zcr` (zero-crossing rate) to ParticleUniforms for compute shader access

### Effects
- Add obstacle collision to all 11 remaining particle sim shaders (phosphor, flux, murmur, raster, tesla, genesis, cymatics, symbiosis, turing, morph, as) — obstacles now work across all 16 effects

### Fixes
- Fix preset showing as dirty during ParamMorph scene transitions — morph interpolation now resets param changed flags since it's timeline playback, not user edits
- Clamp particle count to GPU dispatch dimension limit (`max_compute_workgroups_per_dimension × 256`) to prevent validation errors on high-count effects
- Add per-effect `max_scaled_count` field — quality multiplier won't push past this ceiling; applied to Accretion (60K), Genesis (24K), Murmur (2.4M), and Symbiosis (4M)
- Hide Stress test effect from effects panel
- Fix morph slider animating when auto-cycle mode is "Off" — transition progress now only advances when auto-cycling is active
- Remove redundant image source UI (IMG badge, image selector dropdown, Load Image/Video/Webcam buttons) from particle panel — morph targets handle image loading
- Fix morph snapshot/image/text/video filling wrong slot when `target_count` diverges from def count — all handlers now use def vector length for slot selection and pad gaps

### Morph Effect (New)
- **Shape target morphing**: particles store home positions for up to 4 target shapes and morph between them on beat drops with spring physics and turbulence
- **Multiple target types**: image files, geometry shapes (circle, ring, grid, spiral, heart, star), and random scatter — mixed freely across 4 slots
- **5 transition styles**: Spring (default), Explode-reform, Flow (curl noise), Cascade (left-to-right wave), and Direct (pure lerp) — selectable via UI dropdown
- **Per-particle stagger**: each particle transitions at a slightly different time for organic cascading motion
- **Audio reactivity**: onset triggers morph to next target, bass drives spring vibration, mid adds turbulence during transition, brilliance creates scatter
- **Strided aux buffer**: 4x normal aux size interleaves target data (32MB at 500K particles), no bind group changes
- **ParticleUniforms extended**: 4 new fields (morph_progress, morph_source, morph_dest, morph_flags) — 784→800 bytes, existing effects unaffected
- **Morph UI controls**: target slot display with click-to-morph buttons, transition progress bar, auto-cycle mode (Off/On Beat/Timed), hold duration slider, transition style dropdown, runtime image and geometry loading into morph slots
- **Text morph targets**: type any string in the morph panel and particles form the letters — uses fontdue to rasterize Inter-Bold glyphs into a bitmap, then samples through the standard image pipeline. Supports `"text:HELLO"` in .pfx files
- **Video frame morph targets**: click "+ Video" to load a video file — evenly-spaced frames fill available morph slots (up to 4), cycling between different video frames on beat
- **Snapshot morph targets**: click "Snapshot" to freeze current particle positions into a new morph target — GPU readback captures alive particles as a reusable shape

### WBOIT (Weighted Blended Order-Independent Transparency)
- **Order-independent transparency**: new `"blend": "wboit"` mode approximates correct alpha compositing in a single unsorted pass — no back-to-front sort needed
- **Two-pass rendering**: particles render into accumulation (Rgba16Float) + revealage (R8Unorm) targets with WBOIT weight function, then a fullscreen composite blends onto the scene
- **Automatic compute raster fallback**: WBOIT is incompatible with compute rasterization; `"render_mode": "compute"` + `"wboit"` falls back to billboard with a warning, and `"auto"` mode excludes WBOIT effects
- **Resize-safe**: WBOIT textures recreated on window resize alongside compute raster framebuffer

### Symbiosis Effect (New)
- **Multi-species particle life simulation**: 2–8 species interact via an asymmetric 8×8 force matrix, producing emergent ecosystems, crystals, and predator-prey dynamics
- **6 named presets**: Ecosystem, Crystals, Hunters, Membrane, Chaos, Symmetric — smoothly interpolated on switch
- **Audio-reactive matrix**: bass boosts force scale, mid modulates friction, presence/brilliance expand interaction radius, onset shuffles random matrix entries for emergent disruption
- **Toroidal topology**: particles wrap around screen edges with correct wrapped-distance neighbor queries
- **Force matrix in ParticleUniforms**: 8×8 float matrix (256 bytes) appended to uniforms — no new bind groups needed, all existing effects unaffected

### Fix: CR scatter bind group crash on particle quality change
- **Request adapter-supported buffer limits**: device now requests the GPU adapter's actual `max_storage_buffer_binding_size` and `max_buffer_size` instead of wgpu defaults (128MB/256MB), allowing high-VRAM GPUs to use their full capacity
- **Cap particle count to device limits**: after applying quality multiplier, particle count is clamped so the largest buffer (`sorted_particles_buffer` = N×9×4 bytes) stays within the device's storage buffer binding limit — prevents invalid bind groups and crash on quality change

### Gaussian Area Splat for Compute Rasterizer (Fix)
- **Soft Gaussian splat for particles >1.5px**: compute raster now matches billboard renderer output for larger particles (e.g., Array at ~2.7px radius). Previously all energy concentrated into a 2×2 bilinear kernel, making dim-colored effects nearly invisible
- **Three-tier kernel**: single-pixel (≤1px), bilinear 2×2 (1–1.5px), Gaussian area splat (>1.5px, capped at 8px). Weight `col.a × glow²` matches billboard's `SrcAlpha` blend
- **Multi-tile binning**: particles overlapping multiple 16×16 tiles are binned/scattered to all covered tiles (up to 3×3). Sorted buffer sized 9× max_particles accordingly
- **Tiled path support**: Gaussian loop in shared-memory tiled shader with per-pixel tile boundary clipping

### Preset: Particle Image Save/Restore (Fix)
- **Particle image path now saved in presets**: selecting a different particle image (e.g., skull → phoenix) and updating the preset persists the choice across preset loads
- **Mark dirty on particle/obstacle changes**: changing particle image, video source, webcam source, video transport settings, or obstacle settings now marks the preset as dirty so the Update button appears
- **Backward compatible**: old presets without `particle_image_path` load normally (default image from .pfx)

### Particle Quality Setting (New)
- **Global quality multiplier**: Low (0.25x), Medium (0.5x), High (1x), Ultra (2x), Max (4x) — scales both `max_count` and `emit_rate` proportionally so fill time stays consistent
- **Default: High (1x)** — no change for existing users; backward-compatible with old settings.json files
- **Combo box in Global panel**: same pattern as theme selector; changing quality mid-session reloads the active effect to rebuild GPU buffers
- **Built-in effects upgraded**: all effects now default to 1–2M particles with compute rasterization as the baseline

### Window Size
- **Default window size changed to 1920×1080** (was 1280×720)

### Compute Rasterization (New)
- **Atomic framebuffer rendering**: 3-pass compute raster pipeline bypasses vertex→rasterizer→fragment for sub-pixel particles. Eliminates 2×2 fragment quad waste and rasterizer bottleneck for high particle counts
- **Fixed-point encoding**: 12-bit fractional precision (4096 scale) via `atomicAdd` on 4 separate i32 storage buffers (R/G/B/A), supporting up to ~1000 overlapping particles per pixel within i32 range
- **Size-adaptive kernel**: single-pixel fast path for particles ≤1.5px, Gaussian kernel footprint for 1.5–5px, billboard fallback for larger particles
- **Render mode field**: `"render_mode"` in .pfx files — `"billboard"` (default), `"compute"`, or `"auto"` (auto-selects compute for ≥100K particles with no sprites/trails and size ≤0.005)
- **Fullscreen resolve**: render pass with hardware blend state (additive or alpha) reads decoded framebuffer with Reinhard tonemapping for additive mode
- **COMPUTE badge**: cyan badge in particle panel when compute rasterization is active
- **Tiled shared-memory accumulation**: 4-pass bin→prefix-sum→scatter→tile-raster pipeline for ≥50K particles. Accumulates in 4 KB workgroup shared memory per 16×16 tile (~100× faster atomics), then flushes to global framebuffer with plain stores. Direct draw path retained as fallback for low counts. Automatic path selection based on 1-frame-latent alive count

### Performance & Build Optimization
- **Build profiles**: dev builds use opt-level 1 (deps at 2 for faster shader compilation), release uses thin LTO + codegen-units=1
- **GPU buffer clears**: spatial hash dispatch uses `encoder.clear_buffer()` instead of allocating a zeroed Vec per frame (up to 262KB saved per frame at max grid)
- **Sort params cached**: bitonic sort parameters written once at buffer creation via `mapped_at_creation` instead of ~210 `write_buffer` calls per frame
- **Parallel prefix sum**: spatial hash prefix sum upgraded from single-thread sequential to 256-thread Blelloch scan with shared memory (supports up to 65K cells)
- **Pipeline cache**: compiled shader data persisted to `~/.config/phosphor/pipeline_cache.bin` for faster startup on subsequent launches
- **Device loss handling**: GPU device lost callback + uncaptured error handler + graceful render-time detection (no more hard crash on driver reset)
- **GPU profiler** (`--features profiling`): wgpu-profiler integration with egui overlay panel, timestamp query support
- **Clippy pedantic**: workspace-level clippy lints enabled, auto-fixed 500+ warnings, rustfmt config added
- **FxHashMap**: `rustc-hash` for runtime-only hash maps (MIDI trigger tracking)

### Audio Panel Redesign (Improved)
- **BPM ring**: circular beat-phase arc with orbiting dot replaces static BPM number; flashes white on downbeat
- **Chroma wheel**: radial 12-segment pie chart with energy-proportional radius replaces linear bars; dominant note displayed in center
- **MFCC heatmap**: single-row dark blue→cyan→white timbral fingerprint replaces bar graph; selective labels (DC, Slope, Shape, Formant, Det6)
- **Spectrum bars**: vertical gradient mesh bars (bright top→faded bottom) with peak-hold markers (0.95 decay)
- **Dynamics section**: 7 labeled rows with thin track bars + percentage values; kick uses boolean dot indicator
- **Section headers**: self-documenting sections (SPECTRUM 7 bands, DYNAMICS 7 features, CHROMA 12 pitch classes, TIMBRE·MFCC 13 coefficients)
- **Tooltips**: per-bar spectrum tooltips (name, Hz range, character), per-row dynamics tooltips (feature description), per-cell MFCC tooltips (coefficient meaning), chroma wheel and BPM ring tooltips
- **Footer**: feature count summary (7 bands · 7 dynamics · 12 chroma · 13 mfcc · 512 fft)

### MFCC + Chroma Audio Feature Extraction (New)
- **13 Mel-Frequency Cepstral Coefficients**: captures timbral content — different instruments/voices produce distinct MFCC patterns. Computed from 26-band mel filterbank + DCT-II on the existing 4096-pt FFT
- **12 Chroma pitch-class energies**: maps spectral energy to musical pitch classes (C through B). Individual notes/chords light up specific bars. Frequency range 20–5000 Hz
- **GPU-accessible**: new `mfcc(i)` and `chroma_val(i)` WGSL helper functions in both fragment and compute shaders. ShaderUniforms 256→288 bytes, ParticleUniforms 416→528 bytes
- **Audio panel visualization**: chromatic-colored chroma bars (12 pitch classes) and MFCC bar graph in the audio panel overlay
- **Smoothed + normalized**: 25 new features go through the existing adaptive normalizer and asymmetric EMA smoother (attack 0.03s, release 0.15s)
- **Non-breaking**: existing effects unchanged — they simply don't read the new fields. NUM_FEATURES 20→45

### Spatial Hash Grid (Improved)
- **Dynamic grid sizing**: `grid_dims(N)` computes `clamp(sqrt(N/16), 40, 256)` — grid scales automatically with particle count instead of fixed 40×40
- **Shader constant patching**: SH_GRID_W/H constants in particle_lib, count, prefix_sum, and scatter shaders patched at pipeline creation time
- **Particle panel slider fix**: emit_rate slider max now scales to 10% of max_count for high-count effects (e.g. 100K emit_rate for 1M particles)

### Murmur Effect (Upgraded)
- **Topological K=7 nearest neighbors**: replaced fixed-radius Boids with scale-free K-nearest neighbor queries — flock correlations work from 40K to 1M particles without parameter tuning
- **Vicsek noise phase transitions**: angular noise (eta parameter) modulated by bass drives order→chaos transitions; proper research-accurate model instead of ad-hoc disorder
- **Predator avoidance**: Lissajous-drifting predator with onset-triggered strike jumps; exponential falloff repulsion causes realistic flock splits
- **Heading-level roost centering**: quadratic ramp centering force feeds into heading computation (not just position) — prevents alignment-consensus drift that plagued the old position-only correction
- **KNN-based rim lighting**: edge detection from neighbor anisotropy (COM displacement / k-radius); color_mode slider (0=silhouette, 1=full rim); audio-reactive brightness (RMS) and color temperature (centroid: cool blue ↔ warm amber)
- **Non-accumulating color**: base color re-derived from gradient each frame instead of reading previous frame's output — prevents wash-out over time
- **Adaptive separation**: K-th neighbor distance as interaction scale; density-invariant at any particle count
- **Donor-based emission**: new particles spawn near existing flock members for organic growth
- 1M particles, 8 params (noise_eta, cohesion, color_mode, predator, separation, speed, smoothing, audio_drive), alpha blending, bloom postprocess

### Mycelium Effect (New)
- **Chain-based growth system**: 2,500 pre-allocated chains (80 segments max each, 200K particles total) — leader particles at tips follow curl noise flow fields while depositing follower particles that form spring-connected tendrils
- **Self-activation architecture**: dead particles detect when they should activate by reading chain state from the input buffer — no cross-thread writes, no race conditions. Growth propagates via time-based segment intervals
- **Branching on audio onset**: reserve chains (beyond initial 100 leaders) self-activate by branching from random active chain tips; onset boosts branch probability 15×, brilliance widens branch angles
- **Spring physics**: followers connected by asymmetric springs (strong toward tip, weak toward root) with critical damping; creates organic elastic motion through the network
- **Death cascade**: roots age faster with spectral flux; when a root dies, death propagates tip-ward with 50ms/segment delay — visual dissolve from root to tip, then chain slot recycles for new branches
- **4 color modes**: depth (teal-blue root → forest green → phosphorescent tip), generation (teal/gold/magenta by branch depth), velocity (dim blue → bright green), age (bright green → dark brown)
- **Beat-phase traveling wave**: periodic brightness pulse travels root→tip synced to detected beat, modulated by RMS
- **Bioluminescent feedback trails**: background shader with differential RGB decay (green persists longest), organic noise warp for living feel, HDR clamp
- **Audio integration**: bass→growth speed, onset→branching, mid→curl intensity, brilliance→branch angle, flux→death rate, beat_phase→traveling glow, rms→brightness, centroid→hue shift
- 200K particles, 8 params, additive blending, bloom postprocess, obstacle collision

### Chaos Effect (New)
- **Strange attractor dynamics**: 5 classic attractors (Lorenz, Rössler, Halvorsen, Thomas, Chen) with RK4 integration — particles trace chaotic trajectories that reveal butterfly, torus, and knot-like shapes through accumulated feedback trails
- **Attractor morphing**: spectral centroid smoothly blends between adjacent attractor types; slider + audio drive crossfade the derivative functions (all attractors normalized to [-1,1] space)
- **Audio-reactive bifurcation**: bass shifts the bifurcation parameter across chaos boundaries (e.g. Lorenz ρ through ~24.74), onset provides impulse push past critical points for dramatic order→chaos transitions
- **RK4 integrator**: 4th-order Runge-Kutta for accurate trajectory integration; brilliance modulates time step for faster/more chaotic motion at high frequencies
- **4 color modes**: velocity (blue→gold speed ramp), z-depth (cool→warm gradient), wing (cyan vs magenta by sign of attractor x), age (bright warm→dim cool fade)
- **Feedback trail rendering**: background shader with centripetal UV warp, differential RGB decay (red fades fastest → cool aging trails), HDR clamp
- **3D projection**: slow constant-speed Y-axis rotation, perspective depth scaling via projection param, depth-based size and brightness modulation
- **Divergence guard**: NaN/out-of-bounds particles reset to random position near origin; soft clamp to normalized space prevents runaway
- 50K particles, 8 params, additive blending, bloom postprocess, obstacle collision

### Turing Effect (New)
- **Gray-Scott reaction-diffusion**: full 2D R-D simulation on ping-pong compute textures, running N steps/frame (default 8, audio-modulated up to 32)
- **Hybrid R-D + particles**: 200K particles sample the R-D field for gradient-based chemotactic forces, flowing toward high-concentration pattern regions
- **Particle coloring from chemistry**: color, size, and alpha derived from local B concentration — particles are visible in pattern regions, transparent in substrate
- **Audio-reactive chemistry**: bass→feed rate (2.5× boost), mid→kill rate (2.5× boost), brilliance→diffuse_b, onset injects 4 randomized B drops per beat (PCG hash, 0.35 strength), beat doubles sim steps
- **Particle audio reactivity**: onset pulses gradient force (3× surge), bass pumps particle size, beat kicks velocity, RMS brightness (0.8 coeff), centroid shifts palette (0.5 coeff)
- **Gray-Scott parameter space**: feed/kill rate sliders traverse spots → worms → mitosis → coral → chaos regimes
- **R-D infrastructure**: new `ReactionDiffusionDef` in particle types, `RDUniforms` struct, group 4 bind group for particle compute, `create_rd_resources()` factory
- 512×512 R-D grid, 8 params (gradient_strength, drag, feed_rate, kill_rate, diffuse_b, sim_speed, drop_size, brightness)

### Accretion Effect (New)
- **Tiled N-body gravity**: O(N²) gravitational simulation using workgroup shared-memory tiling (GPU Gems 3 Ch. 31) — every particle attracts every other, forming orbital systems, accretion discs, and slingshot ejections
- **Angular-momentum-preserving damping**: only radial velocity is damped, preserving tangential (orbital) velocity for stable long-lived disc structure
- **Audio seed injection**: beat onsets inject "seed" particles (local attractors within the disc); seed mass scales with `mid`, gravity strength modulated by `bass`
- **Spiral feedback trails**: background shader warps trails with inward pull + rotation, creating galaxy-like spiral arms; differential RGB decay shifts aging trails from warm gold to cool blue
- **4 init patterns**: disc (tangential orbits), ring (near-circular), two-body (binary system), collapse (zero-velocity infall)
- **3 color modes**: velocity (blue→gold), proximity to seeds (blue→orange), orbital energy (blue=bound, red=escaping)
- **Audio reactivity**: onset brightness flash (warm-tinted), bass particle size breathing, RMS glow, bass/mid drive feedback warp strength
- 30K particles, 8 params, additive blending, central pressure support, obstacle collision

### Array Effect (New)
- **Audio-band speaker emitters**: 5 toroidal ring emitters, one per frequency band (sub-bass through brilliance), each firing disc-shaped particle rings outward like speaker cones pushing air
- **Two layout modes**: vertical speaker stack (default) and concentric rings — crossfade with `layout` param
- **Per-band audio reactivity**: each emitter responds to its own frequency band for emission density, speed, and glow
- **Beat-phase breathing**: ring radii modulate with beat phase for rhythmic pulsing
- **ParticleUniforms expanded** 400→416 bytes: added `low_mid`, `upper_mid`, `presence`, `brilliance` audio fields to compute shaders (all 7 bands now available)
- 300K particles, 8 params (trail_decay, ring_radius, spread, layout, color_mode, speed_mult, beat_pulse, emitter_glow)

### Obstacle Refinements
- **Contain mode**: new collision mode that traps particles inside the obstacle shape — inverted collision test, binary search, and normals
- **Flow Around improvement**: redirects approach energy into tangential direction instead of stripping it, preventing pile-ups
- **Video obstacles**: luminance-to-alpha conversion so video frames work as obstacles (bright = solid, dark = passable); UI now shows "Video: filename"
- **Webcam cleanup**: Clear and source-switching now properly stop webcam/depth when no longer needed
- **Tooltips**: all obstacle panel controls have descriptive hover text

### Monocular Depth Estimation (New)
- Feature-gated `depth`: `cargo run --features depth,webcam` for MiDaS v2.1 small monocular depth estimation
- Webcam frames → background `phosphor-depth` thread → ONNX Runtime (ort crate) → 256×256 depth map → obstacle texture
- Particles collide with person's depth silhouette — nearby objects block, far objects pass through
- Model download from HuggingFace (~66 MB), user-initiated via UI button with progress indicator
- Model cached at `~/.config/phosphor/models/midas_v21_small_256.onnx`
- 1-2 frame latency depth pipeline: bounded(1) channels, try_send drops stale frames
- UI: "Depth" button in Obstacle panel (auto-shows "Download" if model missing, progress % during download)
- Preset save/load: `obstacle_depth` field with backward-compatible serde defaults
- 7 new tests (downscale, model path, download progress states, preset serde) — 352 total

### Obstacle Collision (New)
- Image-based 2D obstacle textures for particle collision — load PNG/JPEG/WebP with alpha channel
- 3 collision modes: **Bounce** (reflect velocity along surface normal, scale by elasticity), **Stick** (zero velocity, hold at surface), **Flow Around** (tangent projection, remove normal component)
- Obstacle texture extends compute group 1 bind group (bindings 2+3) alongside existing flow field
- WGSL collision helpers in `particle_lib.wgsl`: `obstacle_alpha()`, `obstacle_normal()` (central-difference gradient), `apply_obstacle_collision()` — any `*_sim.wgsl` can call them with 3 lines
- `ParticleUniforms` extended 384→400 bytes: obstacle_enabled, obstacle_threshold, obstacle_mode, obstacle_elasticity
- Collapsible "Obstacle" UI panel in right sidebar: enable toggle, Load Image (rfd), Clear, mode dropdown, threshold/elasticity sliders
- Integrated into Cascade effect; same 3-line pattern (`prev_pos`, integrate, `apply_obstacle_collision`) works in any particle sim
- Preset save/load with backward-compatible serde defaults
- Opaque image support: images without alpha (JPEG, opaque PNG) auto-detect and use luminance as collision mask — dark areas are passthrough, bright areas are solid
- Anti-strobe: binary search finds exact surface contact point along integration step (4 iterations), places particle just outside surface; velocity reflection only when moving into obstacle
- Texture format `Rgba8Unorm` (not sRGB) for raw data sampling; UV Y-flip for correct clip→texture mapping
- 6 new tests (ObstacleMode conversions, preset serde, backward compat, alpha preprocessing) — 351 total

### Cascade Effect (Fix)
- Fixed beat flash strobe: replaced binary `u.beat` full-screen flash with smooth `beat_phase` envelope (fast 25%-of-beat quadratic fade), reduced amplitude from 0.05 to 0.03

### Cascade Effect (New)
- **Cascade**: solid walls of particles emit inward from all 4 screen edges, audio-segmented by frequency band
- Pixel-perfect wall emission: particle index maps deterministically to perimeter pixels (1920 top/bottom + 1080 left/right at 1080p), guaranteeing gap-free solid walls that scale with resolution
- Band-to-edge mapping: bottom=bass+sub_bass (red-orange), left=mid (teal), right=mid (blue-violet), top=centroid (white-cyan)
- Audio directly drives wall penetration depth: band energy controls particle speed, so walls extend/retract with the music like Aurora's ribbons
- Continuous audio push force during simulation keeps accelerating particles when their band is active
- Curl noise turbulence for organic lateral drift, beat-triggered velocity pulse, onset jitter
- 3 color modes: band colors (4 distinct per edge), monochrome white-blue, speed/energy gradient
- Background shader: feedback trails with directional inward UV warp, per-band audio-reactive edge glow strips (width scales with audio), beat flash
- Functions as both standalone effect and layerable edge frame via convergence param + brightness alpha compositing
- 8 params: trail_decay, inward_speed, spread, curl_strength, color_mode, edge_glow, convergence, beat_sync

### Genesis Effect (Retuned)
- **Genesis** (Particle Lenia): retuned from reference implementation (znah.net/lenia) for proper self-organizing behavior
- Ring kernel calibrated to reference ratios: peak at 30% of R, width 23% of R (proper ring with falloff)
- Kernel weight W_K scaled from 0.003 → 0.02 for correct field density at particle count
- Interaction radius narrowed to [0.05, 0.20] from [0.10, 0.50] — manageable search radius
- Removed curl rotation (was compensating for over-smooth density field at 20K)
- Stronger growth forces (step×0.10) and higher speed cap (MAX_SPEED=0.25) for VJ-paced dynamics
- Soft radial containment spring past screen edge + hard clamp safety net (replaces square boundary)
- Beat-triggered seed drops: ~3% of particles teleport to a random screen location on each beat, creating fresh organisms that self-organize from the music
- 9 initial seed clusters in 3×3 grid (fills screen), zero-mean paired-hash noise (no drift bias)
- Stronger bloom (threshold 0.15, intensity 0.55) for visual presence

### Tesla Effect (New)
- **Tesla** (magnetic field): 200K charged particles follow magnetic field lines as a flow field, creating interweaving helical trajectories
- Magnetic monopole flow field: particles follow superposition of dipole field directions; charge sign determines direction along field lines (+charge follows B, -charge follows -B)
- 4 dipole arrangements via `dipole_mode` param: parallel (same polarity), antiparallel (bar magnet, default), ring (4 alternating), quadrupole (4 + oscillating center)
- Orbital correction near poles: smoothly blends field-following to tangential orbit, preventing convergence at sink poles
- Helical oscillation: per-particle perpendicular sine wave with random phase, controlled by `helix_tightness`
- Proximity dimming: particles near dipoles render dim, brightening as they arc away
- Charge-based coloring: cyan for positive, magenta for negative; `charge_ratio` controls mix
- Audio-reactive: bass drives flow speed, onset triggers velocity jitter, beat triggers per-particle polarity flips (cyan↔magenta)
- 3 color modes: charge-based (cyan/magenta), speed-based (cool→hot), lifetime gradient (cool→warm)
- Feedback trails with electric UV shimmer, dim background field line visualization
- 8 params: trail_decay, field_strength, charge_ratio, dipole_mode, field_rotation, color_mode, helix_tightness, flip_sensitivity

### AoS → SoA Particle Buffer Refactor
- **Structure of Arrays layout**: split single 64-byte `Particle` storage buffer into 4 separate 16-byte SoA buffers (`pos_life`, `vel_size`, `color`, `flags`) — position-only reads (spatial hash, sort keygen) now load 16 instead of 64 bytes per particle (4× bandwidth savings)
- **13-entry compute bind group**: bindings 1-4 read, 5-8 write, 9 counters, 10 aux, 11 dead, 12 alive indices
- **6-entry render bind group**: 4 SoA read buffers + uniforms + alive indices
- **`read_particle()` / `write_particle()` helpers** in particle_lib.wgsl — effect compute shaders use convenience functions, hot-path neighbor reads (murmur boids) access individual arrays directly
- **Spatial hash and sort keygen optimized**: count/scatter shaders bind only `pos_life`, sort keygen binds only `vel_size`
- Request `max_storage_buffers_per_shader_stage: 16` (all desktop GPUs support this)
- Total memory identical (64 bytes/particle), just reorganized for better GPU cache line utilization

### Raster Shard Flex
- **Flexible shard motion**: Voronoi shards deform organically instead of moving as rigid blocks — distance-dependent rotation twist, per-particle noise within shards, and wider burst spread with outer particles flying further

### Raster Grid Artifact Fix
- **Jittered grid sampling**: ±0.4×step deterministic offset per particle breaks up visible row/column banding in smooth gradient areas
- **Bilinear color interpolation**: fractional pixel positions sample from 4 surrounding pixels, eliminating color stairstepping in gradients
- **Gradient-based size modulation**: luminance gradient magnitude stored in `home.w`; smooth areas get 30% larger particles (fill gaps), edges stay neutral
- **Sparkle boost**: bright pixels at high-gradient locations (stars, glints) get audio-reactive size pulsing — per-particle phase offset creates independent twinkling driven by onset and mids
- New unit tests for jitter, bilinear sampling, and gradient helpers

### Raster Image Loading Fixes
- **Fix 2048x2048 image loading**: `upload_aux_data()` no longer shrinks aux buffer below `max_particles` size — subsequent `update_aux_in_place()` calls for larger images were silently dropped
- **Fix image selector not updating**: switching built-in images now updates `ps.def.emitter.image` so the ComboBox reflects the active selection
- **Fix particle slider clamping**: slider ranges now dynamically extend to include the current value — prevents Raster's `emit_rate=100K` / `lifetime=999` from being silently clamped to slider defaults, which corrupted the effect every frame the panel was drawn
- **Built-in effects are runtime-only**: particle slider changes no longer write back to `effect_loader` for built-in effects — use presets to persist tweaks
- **Warning on aux buffer overflow**: `update_aux_in_place()` now logs a warning when a write is skipped due to buffer size
- Raster particle cap raised from 500K to 2M (covers 2048² images at grid step=1)
- Raster default particle size reduced from 0.004 to 0.002

### Particle Video/Webcam Source + Transitions
- **Video as particle source**: Raster (and any image-emitter effect) can now use video files as particle source — particles update home positions per-frame tracking video content
- **Webcam as particle source**: live webcam feed drives particle positions in real-time (feature-gated `webcam`)
- **Source transitions**: smooth 0.5s interpolation of particle home positions when switching between image/video/webcam sources (no hard snap)
- **Per-frame aux updates**: pre-allocated aux buffer at `max_particles` size enables `write_buffer` updates without recreating GPU bind groups (8MB DMA transfer for 500K particles)
- **`sample_rgba_buffer()`**: new function samples raw RGBA byte data directly, skipping file I/O — used for video frame and webcam frame particle sampling
- **UI source controls**: particle panel shows source type badge (IMG/VIDEO/CAM), source name, Load Video / Webcam buttons (feature-gated), video transport (play/pause, speed, loop, seek)
- **Preset save/load**: particle video path, speed, looping, and webcam source persisted in `LayerPreset` with serde defaults for backward compat
- **Load Image button**: users can load custom PNG/JPEG/WebP images as particle source (replaces default image)
- **Animated GIF as particle source**: GIFs with multiple frames auto-detect as animated source with per-frame particle position updates (same as video, no feature gate required)
- **Background loading**: all particle source loading (image, GIF, video) happens on a background thread via `ParticleSourceLoader` — no UI freeze during decode
- **EmitterDef `video` field**: `.pfx` files can specify `"video": "clip.mp4"` or `"video": "webcam"` for built-in video/webcam source at effect load time
- Video decode reuses existing `media/video.rs` infrastructure (ffmpeg pre-decode to RAM, 60s max)
- **Built-in image selector**: combo box in particle panel for quick-switching between bundled raster images (skull, phoenix, jellyfish, hand, samurai_mask) without file dialog
- **Morph-safe preset loading**: when a preset layer has the same effect already loaded, skip the full effect reload — keeps particle systems alive so morph transitions interpolate params smoothly instead of destroying and rebuilding all particles
- 12 new tests: `sample_rgba_buffer` (5), `ParticleImageSource` (2), `SourceTransition` (3), `LayerPreset` serde (3), `EmitterDef` serde with video (2)

### Raster Effect (New)
- **Raster** (video wall): 500K particles map to image pixel positions, colored by source image
- Voronoi shard fragmentation: 32 seed points create irregular fragments with per-shard rotation, translation, and depth variation
- Frayed edges: particles near Voronoi cell boundaries break free with individual jitter
- Three bass displacement modes via `bass_mode` param: shards (default), tangential swirl, radial push
- 2D sinusoidal wave displacement from mids, onset-driven per-particle scatter from highs
- Spring-return physics pull particles back to displaced home positions (sustained audio holds displacement)
- Beat onset impulse with shard-coherent burst direction
- Luminance-based particle sizing: brighter pixels render larger for visual depth
- 8 exposed params: trail_decay, spring_k, bass_strength, mid_wave, high_scatter, burst_force, depth_scale, bass_mode
- Optional feedback trails via trail_decay for motion blur during scatter
- Image source `MAX_DIM` raised from 512 to 2048 for higher-resolution sampling

### Particle Effects Overhaul
- **Delete 4 effects**: removed Coral, Helix, Nova, Vortex (low quality, never worked well)
- **Murmuration → Murmur**: renamed, removed feedback trails (root cause of "sperm" look), brighter sky for contrast, larger/darker/more opaque particles (0.014 size, 0.9 alpha), reduced count 70K→40K
- **Ribbons fix**: lower feedback clamp 0.12→0.06, stronger vignette, halved particle brightness and alpha to prevent additive washout
- **Veil fix**: lower feedback cap 0.4→0.15, stronger decay dampening, halved particle brightness and alpha, minimum dampening floor even without audio
- **Cymatics enhancement**: 25K→50K particles, emit rate 1700→3500, burst 100→500, 3 new params (rotation, symmetry, glow), more vivid color gradient, stronger nodal line contrast
- **Swarm → Spirograph**: complete reimagine — hypotrochoid parametric curves with 5 arms of different petal-count patterns, drifting centers, audio-reactive ratio morphing. 6 params: trail_decay, draw_speed, pattern_scale, complexity, drift, color_spread
- **Spirograph** (new): multi-pattern hypotrochoid curves with 5 arms of distinct petal-count patterns, drifting centers, audio-reactive ratio morphing. Replaces Swarm.
- **Compute shader `param()` access**: particle compute shaders can now access effect params 0-7 via `param(i)` helper, forwarded from ParamStore through `effect_params` fields in ParticleUniforms (repurposed padding bytes, no size change)
- Updated Crucible and Spectral Eye presets (Swarm→Spirograph)
- Effect count: 15 curated effects (was 12 before particle additions, peaked at 19, now curated down to 15)

### Particle System Hardening for 1M Support
- **GPU-side buffer zero-init**: replaced CPU `vec![0u8; 128MB]` + `write_buffer` with `encoder.clear_buffer()` — eliminates 128MB+ CPU allocation at 1M particles
- **Device limits validation**: `max_count` clamped to device `max_storage_buffer_binding_size` with log warning
- **Bitonic sort auto-cap**: depth sort auto-disabled above 65K particles (would require 210 dispatches/frame at 1M)
- **Trail buffer safety**: trails disabled above 500K particles; trail length capped to fit device binding limit

### Parameter Persistence
- **Slider values persist to .pfx**: adjusting parameters in the Parameters panel now updates the `default` values in the .pfx file for user effects, so values survive effect reload and app restart
- Debounced 500ms save — writes only after slider stops moving, not on every frame
- Editor's Effect tab stays in sync when params are saved from UI

### Shader Editor
- **Fix: Save now persists both tabs**: Save (Ctrl+S) writes both the active and paired file when either has unsaved changes — previously only saved the currently active tab, losing edits to the other file

### Effect Browser
- **Particle badge**: small accent-colored dot in top-right corner of effect buttons that use particles
- **Particle count in hover**: hover text shows particle count (e.g. "70K particles", "1M particles")

### Veil Effect
- Fix feedback blowout on loud audio: particle brightness now decreases with volume, stronger loudness dampening on alpha, audio-reactive decay, lowered feedback clamp

### Helix Effect Overhaul
- **Dipole field-guided particles**: replaced uniform B_z Lorentz force with magnetic dipole field — particles follow curved field lines from emitter, creating visible force field pattern through trails
- **Helical oscillation**: perpendicular oscillation around field lines (charge-dependent CW/CCW), bass tightens helix frequency
- **Feedback warps along field**: particle trails flow along magnetic field direction with chromatic aberration, creating persistent field line visualization
- **Dark background**: replaced overpowering painted field lines with near-black background + very subtle dipole hints; particles are the main visual
- **Bright particles**: brightness 0.22→0.70, alpha 0.30→0.55, particle size 0.010→0.018; emit rate 100→200 for dense field coverage
- **Beat burst**: speed pulse along field lines on beat detection; onset scatters particles laterally off field lines
- **Tuned post-processing**: bloom threshold 0.50→0.35, bloom intensity 0.45→0.60 for force-field glow; brighter saturated color gradient

### Chaos Effect Overhaul
- **Visible Lorenz butterfly**: fixed base rho from 22 (below chaos threshold) to 28 (canonical chaotic regime); particles now trace the iconic butterfly attractor pattern
- **XZ butterfly projection**: replaced Y-axis rotation with XZ plane view (the classic Lorenz butterfly orientation) plus gentle ±8° tilt oscillation for 3D depth feel
- **Stable attractor dynamics**: reduced audio modulation of sigma/rho/beta to subtle range (was dramatically reshaping the attractor every frame); fixed `time * audio-varying` rotation bug that caused violent camera jumps
- **Trail-based visualization**: dim particles (alpha × 0.2) with high trail persistence (decay 0.92) accumulate into visible trajectory lines; clean feedback without twist distortion preserves tight attractor lines
- **Density increase**: 20K particles at 500/s emit (was 6K at 80/s) for dense attractor coverage
- **Removed dead params**: attractor_mix, zoom, speed inputs were not wired to compute shader (compute shaders cannot access `param()`)
- **Brightness fixes**: removed × 0.18 fallback color dim, raised initial alpha 0.3→1.0, unclamped color from 0.5→1.0 max

### Particle Effect Review & Enhancement
- **Murmuration** (hero effect): full Boids flocking model with separation, cohesion, and alignment; angular heading smoothing eliminates jitter; alpha blend for opaque bird silhouettes; twilight sky gradient background; depth-based sizing; 15K particles; audio: beat→cohesion spike, bass→disorder, onset→flock split
- **Coral** (fix): replaced numerical central-difference gradient with analytical derivative of Turing cosine waves; added soft exponential boundary repulsion (no more hard clamp collapse); FBM curl noise for organic diffusion; reduced attraction_strength 2.5→1.2; aspect ratio correction; color gradient and size curve
- **Chaos** (fix): adaptive sub-stepping (4 steps) prevents divergence; NaN/divergence detection auto-kills bad particles; proper Rossler scaling (native scale then rescale); centered perspective projection around attractor center; color gradient for temperature-based trajectory coloring; size/opacity curves
- **Helix** (fix): increased E-field upward from 0.05→0.20; reduced B_z from 3.0→1.5 base (wider spirals that actually rise); beat adds upward levitation pulse; moved emitter from y=-0.6 to y=-0.4; RMS-reactive upward drift; two-strand DNA color gradient; size/opacity curves
- **Nova** (overhaul): color gradient lifecycle (white-hot→vivid→orange→dim red); size/opacity lifetime curves; ground bounce for trailing sparks (restitution 0.3); speed/size/life variance for natural variation; 15→85% shell/spark ratio; staggered burst emission; stronger ground glow with beat pulse
- **Phosphor** (polish): reduced sparkle overbright (mix 0.55→0.3, mult 1.6→1.2); tightened feedback decay 0.88→0.84; stronger beat inward pull 0.02→0.06; size/opacity curves for crisp particle falloff; reduced bloom intensity 1.2→1.1
- **Veil** (bloom fix): raised background cap 0.2→0.6 for richer fabric; lowered bloom_threshold 0.9→0.7
- **Ribbons** (bloom fix): raised bloom_threshold 0.4→0.60; reduced bloom_intensity 0.6→0.40; reduced trail_width 0.008→0.005; capped feedback at 0.8
- **Cymatics** (polish): analytical Chladni gradient (no numerical eps); smooth bilinear mode crossfade between integer modes; raised background cap 0.5→0.85; sharper nodal line glow (exp factor 12→18); soft boundary repulsion; color gradient and size curve
- **Vortex**: chromatic aberration in gravitational lensing (R/G/B channel separation); accretion glow flash near event horizon on beat; temperature-based color gradient for disk coloring
- **Swarm**: wider orbit radius 0.3→0.5; reduced radial damping 8.0→4.5; stronger beat orbit expansion
- **Flux**: sparkle system (8% of particles get 2.5x brightness with flicker); opacity curve for gentle fade-out; raised background cap 0.5→0.8

### Advanced Particle Forces
- ParticleUniforms extended from 192 to 384 bytes with new force, emitter, curve, and sort fields
- FBM noise forces: configurable octaves, lacunarity, persistence with turbulence and curl noise modes
- Wind force: constant directional force via `wind: [x, y]`
- Vortex field: rotational force with center, strength, and falloff radius
- Ground bounce: configurable y-level and restitution coefficient
- New emitter shapes: disc (uniform area fill), cone (directional spread with angle control)
- Emitter velocity inheritance: particles inherit emitter motion for trailing effects
- Speed, lifetime, and size variance: per-particle randomization (0-1 range)
- Lifetime curves: 8-point LUT for size and opacity over particle lifetime
- Color gradient: up to 8 hex colors interpolated over lifetime
- Particle spin: per-particle rotation with configurable speed, rendered via vertex rotation
- `apply_builtin_forces()` helper in `particle_lib.wgsl`: single call applies all forces (gravity, wind, drag, noise, attraction, vortex, flow field)
- Curve/gradient evaluation helpers: `eval_size_curve()`, `eval_opacity_curve()`, `eval_color_gradient()`
- Bitonic depth sort: optional GPU merge-sort on alive indices for correct alpha-blended rendering
- `parse_hex_color()` utility for `#RRGGBB` / `#RRGGBBAA` to packed u32
- All new fields default to zero/disabled — existing .pfx files and custom shaders work unchanged

### PFX Hot-Reload + Editor Integration
- `.pfx` file hot-reload: editing effect metadata (params, postprocess, passes, particles) live-updates without restarting
- Differential updates: param-only or postprocess-only changes skip GPU pipeline rebuilds
- `merge_from_defs` on ParamStore preserves slider positions when param definitions change
- Editor tab switching: Shader and Effect tabs in code editor for `.wgsl` and `.pfx` side by side
- JSON syntax highlighting for `.pfx` editing
- "Edit Shader" → "Edit Effect", "Copy Shader" → "Copy Effect" button labels
- 12 new unit tests for PfxDiff and merge_from_defs

### Bug Fixes
- Fix BPM detection reporting double tempo (~290 instead of ~145 BPM): remove cascading octave-up correction that fought multi-ratio disambiguation, tighten tempo prior σ from 1.5 to 1.0, widen frame time clamp to tolerate real audio thread timing, reduce Kalman snap escape from 50 to 30 frames

## v1.2.1 — 2026-03-05

### Audio
- Add WASAPI loopback capture for Windows — auto-captures desktop audio (what's playing through speakers) without requiring Stereo Mix
- WASAPI backend uses `windows` crate COM APIs: `IMMDeviceEnumerator` → `IAudioClient` (loopback) → `IAudioCaptureClient`
- Same fallback pattern as Linux: try WASAPI loopback first, fall back to cpal input devices
- Supports float32, int16, and int24 formats with stereo→mono downmix
- Device friendly name shown in status bar (e.g. "Speakers (Realtek Audio)")

### Bug Fixes
- Fix webcam "Failed to fulfill requested format" on Windows: cameras that only support raw formats (YUYV/NV12) now work via automatic fallback from MJPEG to any supported format

### CI
- Upload debug build artifacts on PRs for all platforms

## v1.2.0 — 2026-02-28

### GPU Particle System
- GPU-driven pipeline: counter buffer, indirect draw, alive/dead index lists — zero dead particle processing
- `particle_lib.wgsl` auto-prepended to all compute shaders: shared structs, bindings, hash/rand, emit/alive helpers
- Alive/dead protocol: atomic emission claims, compact alive index rendering
- 3D curl noise flow field: 64x64x64 baked texture at `@group(1)`, `sample_flow_field()` helper
- Trail rendering: per-particle ring buffer, ribbon triangle strips with tapering width/alpha, separate indirect draw
- Spatial hash grid: 40x40 GPU 3-pass pipeline (count → prefix sum → scatter), neighbor query at `@group(3)`
- ParticleUniforms extended from 128 to 192 bytes (flow field + trail params)
- ParticleRenderUniforms extended from 32 to 48 bytes (frame_index + trail params)
- Particle UI panel: alive/max count with utilization bar, emit rate, burst, lifetime, speed, size, drag sliders, feature badges

### New Effects
- **Flux**: 25K particles following curl noise flow field, audio-reactive flow strength and speed
- **Ribbons**: 8K particles with flow field + 16-point trail ribbons, audio-reactive width/opacity
- **Chaos**: 40K particles tracing Lorenz/Rossler strange attractors with RK4 integration, 3D perspective projection, audio-reactive bifurcation parameters
- **Helix**: 15K charged particles spiraling via Lorentz force F=q(E+v×B), positive/negative charges, audio-reactive B_z and E fields
- **Murmuration**: 60K flocking particles with Vicsek model, spatial hash neighbor query, audio-reactive order↔disorder phase transition
- **Cymatics**: 30K particles forming Chladni nodal line patterns via gradient descent, audio frequency bands select mode numbers
- **Coral**: 40K particles tracing Turing-like organic growth patterns, hexagonal spots morphing to labyrinthine stripes

### Bug Fixes
- Fix Murmuration crash: create spatial hash before compute pipeline so shader bindings validate at pipeline creation
- Fix particle size exponential blowout in all 6 new effects: size calculation read back previous frame's computed size (`p.vel_size.w`), compounding scale factors >1.0 each frame causing particles to grow until they fill the screen. Fix stores initial size in `pos_life.z` and uses that as base instead of the accumulated value
- Tune bloom thresholds (0.70–0.85), reduce particle counts (4K–10K), lower feedback decay, add hard caps to bg shaders
- Fix all 6 new effects washing out to uniform brightness: reduce particle counts 3-8x (into 4K-10K working range), raise bloom thresholds to 0.70-0.85, lower bloom intensity to 0.30-0.35, reduce HDR clamp from 1.5 to 1.0 in bg shaders, lower alpha multiplier from 2.0 to 1.5, restore per-particle brightness/alpha to visible levels, reduce feedback decay for faster trail fade
- Fix Chaos visibility: increase projection zoom 50% so attractor shape fills screen instead of concentrating in a small region
- Fix Helix vertical bands: widen emission spread and add oscillating horizontal E-field to create interweaving spirals
- Tune all new effects: lower trail_decay defaults to 0.78-0.80, reduce particle counts and emit rates

## v1.1.0 — 2026-02-28

### Scene System
- Scene = ordered cue list referencing presets with transitions (Cut, Dissolve, ParamMorph)
- SceneStore: save/load/delete scenes to `~/.config/phosphor/scenes/*.json`
- Timeline state machine: Idle → Holding → Transitioning with auto-advance and beat sync
- ParamMorph transitions: smooth interpolation of all params and layer opacities
- Dissolve transitions: GPU crossfade via fullscreen shader with snapshot capture
- Scene panel in left sidebar: cue list management, transport controls, save/load/delete
- Per-cue transition type editing: click to cycle Cut → Dissolve → Morph
- Per-cue transition duration editing via DragValue
- Advance mode selector: Manual / Timer / Beat Sync with per-cue hold times and beats-per-cue
- Auto-save: cue and timeline changes persist to disk immediately
- Timeline bar above status bar: cue blocks, playhead during hold and transitions, click-to-jump
- Scene status indicator (SCN) in status bar with cue counter
- Keyboard: Space (next cue), T (toggle timeline)
- MIDI triggers: SceneGoNext, SceneGoPrev, ToggleTimeline
- OSC: `/phosphor/trigger/scene_go_next`, `scene_go_prev`, `toggle_timeline`
- Web control: Prev Cue / Next Cue / Timeline trigger buttons
- MIDI Clock sync: parse 0xF8/FA/FB/FC system realtime, derive external BPM, beat-synced advance
- MIDI Clock → Timeline: auto-follow transport (play/stop), MIDI clock beats drive BeatSync mode with audio fallback
- OSC scene control: `/phosphor/scene/goto_cue`, `/scene/load` (int or string), `/scene/loop_mode`, `/scene/advance_mode`
- OSC outbound timeline state: `/phosphor/state/timeline/active`, `cue_index`, `cue_count`, `transition_progress` at TX rate

### UI
- Redesign scene panel: card-framed scene list, section headers (TRANSPORT / CUE LIST), sized transport buttons (PLAY ghost-border, STOP|PREV|GO), transition type badges with color per type, ghost-border "+ Cue" button
- Widen side panels from 270px to 315px

## v1.0.1 — 2026-02-27

### UI
- Shader compilation errors in status bar now have a dismiss (×) button

### Documentation
- Add comprehensive TUTORIALS.md covering all features
- Quick Start now leads with binary downloads, build-from-source in collapsible details

## v1.0.0 — 2026-02-27

### Rendering
- HDR multi-pass pipeline with Rgba16Float render targets and ping-pong feedback
- Post-processing chain: bloom extract, separable Gaussian blur, chromatic aberration, ACES tonemapping, vignette, film grain
- Audio-reactive post-processing (RMS → bloom, onset → chromatic aberration, flatness → film grain)
- Shader hot-reload via file watcher with 100ms debounce and error recovery

### Effects
- 12 curated audio-reactive effects: Aurora, Drift, Tunnel, Prism, Shards, Pulse, Iris, Swarm, Storm, Veil, Nova, Vortex
- .pfx JSON effect format with multi-pass pipeline support
- WGSL shader library auto-prepended (noise, palette, SDF, tonemap)
- In-app WGSL shader editor with live hot-reload, built-in/user sections, copy-shader

### GPU Particles
- Compute shader particle simulation with ping-pong storage buffers and atomic emission
- Vertex-pulling instanced rendering with additive and alpha-blend pipelines
- Sprite atlas textures, image decomposition (grid/threshold/random sampling)
- Per-particle aux buffer for home positions and packed RGBA
- Configurable emitters: point, ring, line, screen

### Audio
- Multi-resolution FFT (4096/1024/512-pt) with 20 audio features across 7 frequency bands
- Adaptive normalization with per-feature running min/max
- 3-stage beat detection: onset detector → FFT autocorrelation tempo estimator → predictive beat scheduler
- Kalman filter BPM tracking with octave disambiguation
- Audio input device selector with runtime switching and persistence
- PulseAudio capture on Linux with dlopen (no libpulse build dependency), cpal/ALSA fallback
- `--audio-test` CLI flag for standalone audio diagnostics (no GPU required)

### Layer Composition
- Up to 8 compositing layers, each with independent effect/params/particles
- 10 blend modes: Normal, Add, Screen, ColorDodge, Multiply, Overlay, HardLight, Difference, Exclusion, Subtract
- Per-layer opacity, enable/disable, lock (prevent changes), pin (prevent reorder)
- Drag-and-drop layer reordering
- GPU compositor with single-layer fast path (zero overhead for one layer)

### MIDI + OSC
- MIDI input via midir: MIDI learn, auto-connect, hot-plug detection, CC mapping for params and triggers
- OSC input/output via rosc: RX on port 9000, TX on port 9001, OSC learn, layer-targeted messages
- Config persistence for both MIDI and OSC bindings

### Web Control Surface
- Same-port HTTP + WebSocket server with embedded single-file touch UI
- Bidirectional JSON state sync with full snapshot on connect
- Multi-client support, audio broadcast at 10Hz, mobile-first dark theme
- Auto-reconnect with exponential backoff

### Media Layers
- PNG/JPEG/GIF/WebP as compositing layers with aspect-ratio-correct letterbox
- Animated GIF/WebP playback with forward/reverse/ping-pong, speed control, loop toggle
- Video playback (feature-gated `video`): MP4/MOV/AVI/MKV/WebM via ffmpeg pre-decode, seek slider, 60s max

### Webcam Input
- Live camera feed as compositing layer (feature-gated `webcam`) via nokhwa
- Cross-platform: v4l2, AVFoundation, MediaFoundation
- Capture thread with frame drain, device controls, mirror, preset save/load with device reconnection
- Corrupted frame recovery, dead thread detection, panic hardening

### NDI Output
- Runtime-loaded NDI SDK via libloading (feature-gated `ndi`), no build-time dependency
- GPU capture with double-buffered staging and 1-frame latency readback
- Per-effect alpha preserved end-to-end through compositor and post-processing
- Luma-to-alpha toggle, configurable source name and resolution
- Sender thread with bounded channel (drops frames to maintain VJ performance)

### Preset System
- Save/load multi-layer compositions with effect params, blend modes, opacity, lock/pin state
- Async preset loading for media/video layers with background decoding thread
- Generation-based cancellation for rapid MIDI preset cycling
- MIDI triggers for next/prev preset navigation
- Bundled "Crucible" preset showcasing all 8 layers

### UI & Accessibility
- egui overlay (D key toggle) with WCAG 2.2 AA dark/light themes plus Midnight, Ember, Neon VJ themes
- Auto-generated parameter controls, 7-band audio spectrum, BPM display with beat flash
- Status bar with keyboard hints, particle count, audio health, NDI/MIDI/OSC/Web status dots
- macOS app icon and polished DMG installer

### Testing
- ~251 unit tests across 17 modules covering audio, params, effects, layers, presets, MIDI, OSC, web, NDI, themes

## v0.2.0

Initial development release.

---
NDI® is a registered trademark of Vizrt NDI AB.
