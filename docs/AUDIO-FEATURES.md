# Fosfora Audio Features

What Fosfora hears in your music, in plain English — and the research behind each measurement.

Fosfora listens to your audio and turns it into 83 numbers, updated every 512 samples — 86 times a second at 44.1 kHz, 94 at 48 kHz. Every one of those numbers is available to every shader, every parameter slider, and every OSC client. This page explains what each one actually means musically, and links the paper or standard it comes from.

---

## Table of Contents

**Just want something that works? Jump to [Pick a Feature by What You Want](#pick-a-feature-by-what-you-want).**

1. [How to Read This Page](#how-to-read-this-page)
2. [Pick a Feature by What You Want](#pick-a-feature-by-what-you-want)
3. [Loudness and Level](#loudness-and-level)
4. [Spectral Shape](#spectral-shape)
5. [Rhythm, Beats and Tempo](#rhythm-beats-and-tempo)
6. [Pitch and Harmony](#pitch-and-harmony)
7. [Timbre](#timbre)
8. [Stereo Field](#stereo-field)
9. [Drums vs. Instruments](#drums-vs-instruments)
10. [Song Structure](#song-structure)
11. [Audio Textures](#audio-textures)
12. [How the Numbers Are Tamed](#how-the-numbers-are-tamed)
13. [Further Reading](#further-reading)

---

## How to Read This Page

**Every value runs 0 to 1** unless the entry says otherwise — `bar_index` and `beat_index` are the two exceptions, raw counts that only ever go up. That means you can wire any feature to any slider without doing math first.

**The name in `code font` is the real name.** It is exactly what you type in a shader, exactly what you see in the binding matrix, and exactly what goes out over OSC. There is no translation layer.

**Every entry ends with a `Source:` line.** Most point at the paper or standard the implementation follows. Some say *Fosfora-specific* — that means we made it up, there is no paper, and we would rather tell you than invent a citation. A few say *Ported from EASEy-GLYPH*, an earlier project of ours.

**The numbers level themselves automatically.** Quiet music still fills the 0–1 range, and loud music does not peg everything at 1. If a feature is behaving in a way that surprises you, [How the Numbers Are Tamed](#how-the-numbers-are-tamed) explains why.

---

## Pick a Feature by What You Want

| I want the visual to… | Use | Why this one |
|---|---|---|
| Flash on every drum hit | `onset` | Fires on any attack, from any instrument |
| Flash only on the kick | `kick` | Ignores bass notes, snares and hats |
| Stay locked to the groove | `beat_phase` | Smooth 0→1 ramp, one per beat |
| Know where the bar starts | `downbeat`, `bar_phase` | Bar-level clock, not just beats |
| Change color with the mood | `centroid` | How bright the music sounds |
| Get dirty when the music does | `flatness` | Noisy versus musical |
| React to melody but not drums | `harmonic_energy` | Drums have been filtered out |
| React to drums but not melody | `percussive_energy` | The other half of the same split |
| Swell as the track builds | `buildup` | Rises through a riser, peaks before the drop |
| Hard-cut at the drop | `drop` | One-frame trigger, 16-second lockout |
| Switch looks between sections | `section_novelty` | Peaks at verse/chorus boundaries |
| Follow a vocal or lead line | `pitch` + `pitch_confidence` | Gate on confidence or it chases noise |
| Move with the stereo image | `pan`, `stereo_width` | Left/right position and how wide |
| Fill the screen on loud parts | `loudness_m` | Perceptual, consistent across tracks |
| Tint by musical key | `key_class`, `key_is_minor` | Stable enough to hold a whole track |
| Draw the actual waveform | `waveform(x)` | An oscilloscope trace, not a number |

---

## Loudness and Level

### Frequency bands

**`sub_bass` `bass` `low_mid` `mid` `upper_mid` `presence` `brilliance`** — how much energy sits in each slice of the frequency range, from the lowest rumble to the highest sparkle. These are the workhorses: seven sliders that between them describe the whole shape of the sound. Wire `sub_bass` to scale, `presence` to sparkle, and you have a reactive visual in two minutes.

| Band | Range | Typical content |
|---|---|---|
| `sub_bass` | 20–60 Hz | Sub-bass rumble, kick drum fundamental |
| `bass` | 60–250 Hz | Bass guitar, kick body |
| `low_mid` | 250–500 Hz | Low vocals, warmth |
| `mid` | 500–2000 Hz | Vocals, guitars, snare |
| `upper_mid` | 2000–4000 Hz | Vocal clarity, guitar bite |
| `presence` | 4000–6000 Hz | Hi-hats, cymbal shimmer |
| `brilliance` | 6000+ Hz | Air, sparkle |

Source: Fosfora-specific — the band edges are a conventional seven-way split, but the measurement is ours: each band is read from whichever of three FFT sizes best resolves it, and with the default dB band scaling the top three get a gentle tilt (+3 dB per octave above 2 kHz) so cymbals are not permanently dwarfed by bass. The Legacy band scale has no tilt. See [`audio/analyzer.rs`](../crates/fosfora-app/src/audio/analyzer.rs).

**`rms`** — the plain, unweighted loudness of the signal. It is the simplest possible "how much sound is there" number, and it responds instantly. Reach for `loudness_m` instead when you want something that matches how loud the music *feels*; use `rms` when you want raw and fast.

Source: Standard descriptor — [librosa's `rms`](https://librosa.org/doc/latest/generated/librosa.feature.rms.html) documents the same root-mean-square measurement.

**`loudness_m`** — how loud the music actually sounds to a person, measured over the last 0.4 seconds. This is the same meter broadcasters and streaming services use, so it weighs frequencies the way your ears do instead of just counting signal. Prefer it over `rms` whenever you want an "intensity" that behaves the same across different tracks and different sound cards.

Source: [ITU-R BS.1770](https://www.itu.int/rec/R-REC-BS.1770/en) and [EBU R 128](https://tech.ebu.ch/docs/r/r128.pdf) — K-weighting plus a 400 ms sliding window, mapped from −60..0 LUFS onto 0..1. Fosfora measures a mono downmix and skips the BS.1770 gating, which applies only to whole-programme measurement.

**`loudness_s`** — the same perceptual loudness, but averaged over 3 seconds instead of 0.4. It ignores individual hits and tracks the overall level of the section you are in. Use it for slow background changes that should not twitch on every snare.

Source: [ITU-R BS.1770](https://www.itu.int/rec/R-REC-BS.1770/en) — the short-term window from the same standard.

**`loudness_trend`** — is the track getting louder? It rises when the recent level pulls ahead of the longer average, which is exactly what happens through a build-up, and sits at 0 the rest of the time. It is the cleanest single "something is coming" signal Fosfora produces.

Source: Fosfora-specific — the rising difference between the momentary and short-term loudness, in [`audio/loudness.rs`](../crates/fosfora-app/src/audio/loudness.rs). It also feeds `buildup`.

---

## Spectral Shape

These six describe the *character* of the sound rather than its loudness. They are what you want when the visual should change with the texture of the music, not its volume.

Each links to librosa's documentation, which defines the same formula in readable, runnable form. For the formal definitions, all six are specified in [Peeters (2004)](http://recherche.ircam.fr/anasyn/peeters/ARTICLES/Peeters_2003_cuidadoaudiofeatures.pdf) — see [Further Reading](#further-reading).

**`centroid`** — how bright the music sounds. It tracks where the weight of the sound sits: muffled, bass-heavy material reads near 0, while hi-hats, strings and cymbals pull it toward 1. This is the best default for driving hue, color temperature, or anything that should look brighter when the music gets brighter.

Source: Standard descriptor — [librosa's `spectral_centroid`](https://librosa.org/doc/latest/generated/librosa.feature.spectral_centroid.html) documents the same measurement. Fosfora averages on a musical (octave-based) axis rather than a raw Hz one, so moving up an octave always shifts it by the same amount.

**`flux`** — how fast the sound is changing. A steady held chord reads near 0; a busy passage with notes and hits constantly arriving reads high. It is deliberately built to ignore volume, so turning the music up does not make it rise.

Source: [Dixon, *Onset Detection Revisited*, DAFx-06](https://www.dafx.de/paper-archive/2006/papers/p_133.pdf) §2.1 defines the spectral flux measure. See also [Bello et al., *A Tutorial on Onset Detection in Music Signals* (2005)](https://hajim.rochester.edu/ece/sites/zduan/teaching/ece472/reading/Bello_2005.pdf).

**`flatness`** — how noisy versus musical the sound is. A held synth pad, a bass note, or a sung vowel reads near 0; a snare, a cymbal wash, or vinyl crackle reads near 1. Good for driving grain, static, or anything that should get dirty when the music does.

Source: Standard descriptor — [spectral flatness](https://en.wikipedia.org/wiki/Spectral_flatness), also known as Wiener entropy. Fosfora measures it across [mel bands](https://en.wikipedia.org/wiki/Mel_scale) rather than raw frequency bins, which tracks perception more closely.

**`rolloff`** — where the top of the sound ends. It marks the point below which almost all the energy lives, so it rises when a filter opens up or hi-hats come in, and drops when a track gets muffled. The classic pairing is a filter-sweep visual that opens as this rises.

Source: Standard descriptor — [librosa's `spectral_rolloff`](https://librosa.org/doc/latest/generated/librosa.feature.spectral_rolloff.html) documents the same formula. Fosfora uses the conventional 85% threshold, and reports it on the same octave-based axis as `centroid`.

**`bandwidth`** — how spread out the sound is. A single clean note or a lone sine tone reads low; a full mix, a distorted guitar, or a crash cymbal spreads energy everywhere and reads high. Pair it with `centroid`: that one says *where* the sound sits, this one says *how wide* it is around that point. It is the spread in octaves around the centroid: 0 is a pure tone, 1 is a standard deviation of four octaves, and pink noise reads about 0.8.

Source: Standard descriptor — [librosa's `spectral_bandwidth`](https://librosa.org/doc/latest/generated/librosa.feature.spectral_bandwidth.html) documents the formula on a Hz axis; Fosfora takes it on the octave axis `centroid` uses.

**`zcr`** — a rough, very cheap noisiness meter. It counts how often the waveform crosses zero, which is high for hissy and percussive sounds and low for smooth bass. It largely agrees with `flatness`, but reacts faster and costs nothing. It is reported as the pitch of a tone that would cross zero as often, on the same axis as `centroid`, so a clean 440 Hz note reads about 0.4 and white noise about 0.9.

Source: Standard descriptor — [librosa's `zero_crossing_rate`](https://librosa.org/doc/latest/generated/librosa.feature.zero_crossing_rate.html) documents the same measurement.

---

## Rhythm, Beats and Tempo

**`kick`** — the kick drum, and nothing else. It reads 0.5 or more on a kick and falls back toward 0 between hits. It judges each moment by how the whole spectrum changes, not just the low end, so a bass note, a tom, a snare or a hi-hat does not count, and a passage with no kick drum stays quiet. When you want a visual that thumps with the kick specifically rather than with any drum, this is the one.

Source: Fosfora-specific — a small gradient-boosted tree model over per-band change and the shape of the low-end spectrum, trained on Creative Commons electronic music from the Free Music Archive with kicks labeled from separated drum tracks. See [`audio/kick_model.rs`](../crates/fosfora-app/src/audio/kick_model.rs) and [`scripts/kick_model/`](../scripts/kick_model/).

**`onset`** — something just got hit. It spikes the instant a new sound starts — a kick, a snare, a plucked string, a vocal entry — and falls back toward 0 between hits. Unlike `beat`, it does not care about tempo, so it fires on every attack including the off-beat ones.

Source: [Böck & Widmer, *Maximum Filter Vibrato Suppression for Onset Detection*, DAFx-13](https://www.dafx.de/paper-archive/2013/papers/09.dafx2013_submission_12.pdf) — the SuperFlux algorithm. Its frequency maximum filter is why a vibrato or a wobble bass no longer registers as a stream of false hits.

**`beat`** — the metronome. It is 1 for a single frame on each beat and 0 the rest of the time, following the tempo Fosfora has locked onto rather than every individual hit. It fires on the predicted grid, so a missed or late hit does not shift it, but when a breakdown stops backing the grid with hits it goes quiet after a few beats rather than strobing through silence, and resumes on the grid when the drums return. `beat_phase` keeps running through the gap.

Source: Ported from [EASEy-GLYPH](https://github.com/kevinraymond/easey-glyph) — a predict-and-confirm scheduler that fires on a confirmed hit inside a short window around the expected beat, and fires anyway when a predicted beat is missed.

**`beat_phase`** — where you are between beats, as a smooth ramp from 0 to 1. This is usually more useful than `beat` itself: instead of a single-frame flash you get a continuous value to drive a swing, a bounce, or a rotation that stays locked to the groove. Feed it through something like `1.0 - phase * phase` for a decaying bounce.

Source: Ported from [EASEy-GLYPH](https://github.com/kevinraymond/easey-glyph) — Fosfora additionally re-runs this ramp on the render thread so it stays smooth at your frame rate rather than the analysis rate.

**`bpm`** — the tempo, divided by 300. Multiply by 300 to get real beats per minute (OSC clients receive the real value already). It settles within a few seconds of a steady groove starting and then holds very still, so it is safe to drive something slow and structural.

Source: Ported from [EASEy-GLYPH](https://github.com/kevinraymond/easey-glyph) — tempo is found by autocorrelating the onset stream ([Wiener–Khinchin](https://en.wikipedia.org/wiki/Wiener%E2%80%93Khinchin_theorem)), corrected for half/double-time errors against a tempo prior, then tracked with a [Kalman filter](https://en.wikipedia.org/wiki/Kalman_filter). Background reading: [Ellis, *Beat Tracking by Dynamic Programming* (2007)](https://www.ee.columbia.edu/~dpwe/pubs/Ellis07-beattrack.pdf).

**`beat_strength`** — how hard the hit behind this beat was. It is set only on the frame `beat` fires (0 the rest of the time): the strength of the onset that landed on the beat, or 0.5 when the beat fired on the grid with no hit to back it. Use it to scale a beat-driven flash so accented beats hit harder. Available in shaders and bindings, but not sent over OSC.

Source: Ported from [EASEy-GLYPH](https://github.com/kevinraymond/easey-glyph) — the onset strength captured at the moment the beat fired.

**`downbeat`** — the "one" of the bar. Like `beat`, it is 1 for a single frame, but only on the first beat of each bar, so you can save your biggest visual moves for it. Fosfora works out whether the music is in 3 or 4 and where the bar starts by noticing which beats are consistently more emphatic.

Source: Fosfora-specific — no published algorithm; it scores candidate meters and bar positions by accent contrast over the last 16 beats, in [`audio/downbeat.rs`](../crates/fosfora-app/src/audio/downbeat.rs). Expect roughly 70–80% accuracy on straightforward 4/4 electronic music and less elsewhere.

**`bar_phase`** — where you are in the bar, as a smooth 0 to 1 ramp. It is `beat_phase` one level up: a slow sweep that completes once per bar rather than once per beat, ideal for camera moves or color cycles that should breathe with the music's larger pulse.

Source: Fosfora-specific — the bar-level clock in [`audio/downbeat.rs`](../crates/fosfora-app/src/audio/downbeat.rs).

**`beat_in_bar`** — which beat of the bar you are on, as a number from 0 to 1. In 4/4 it steps through 0, 0.25, 0.5, 0.75. Use it to do something different on beat 3 than on beat 1. It steps rather than slides, so do not expect smooth motion from it.

Source: Fosfora-specific — the beat counter in [`audio/downbeat.rs`](../crates/fosfora-app/src/audio/downbeat.rs).

**`bar_index`** and **`beat_index`** — running counts of bars and beats since audio started, as plain whole numbers (0, 1, 2, …), **not** 0 to 1. Each steps up by one exactly when `bar_phase` or `beat_phase` wraps, so `bar_index + bar_phase` is a clock that never jumps backwards — use it for anything that should advance steadily over a whole set, like a slow rotation or a palette that walks forward every four bars (`floor(bar_index / 4)`). Shaders and bindings only.

Source: Fosfora-specific — the bar and beat counters in [`audio/downbeat.rs`](../crates/fosfora-app/src/audio/downbeat.rs) and [`audio/beat.rs`](../crates/fosfora-app/src/audio/beat.rs).

---

## Pitch and Harmony

**`chroma`** (12 values, read with `chroma_val(i)`) — how much of each of the twelve notes is sounding, ignoring which octave. Index 0 is C, 1 is C sharp, and so on up to 11 for B. A held C major chord lights up indices 0, 4 and 7; a bass-heavy drum loop lights up almost nothing.

Source: [Brown, *Calculation of a Constant Q Spectral Transform*, JASA 1991](https://www.ee.columbia.edu/~dpwe/papers/Brown91-cqt.pdf) — the [constant-Q](https://en.wikipedia.org/wiki/Constant-Q_transform) approach Fosfora's version is built on. See also [librosa's `chroma_cqt`](https://librosa.org/doc/latest/generated/librosa.feature.chroma_cqt.html). Fosfora tracks the track's actual tuning reference, so a recording tuned to 432 Hz still lands cleanly on the right notes.

**`dominant_chroma`** — which single note is loudest right now, as an index from 0 to 1. Multiply by 11 to get a pitch class. It jumps rather than slides, which makes it perfect for picking a palette entry and useless for driving smooth motion.

Source: Fosfora-specific — the strongest entry of `chroma`, in [`audio/analyzer.rs`](../crates/fosfora-app/src/audio/analyzer.rs).

**`key_class`** — what key the track is in, as a note index from 0 to 1. Multiply by 11 to get the root (0 is C). Unlike `dominant_chroma`, this looks at roughly the last twenty seconds, weighted toward the louder moments, so it stays put through individual chord changes and typically holds steady for a whole track.

Source: [Faraldo et al., *Key Estimation in Electronic Dance Music*, ECIR 2016](https://doi.org/10.1007/978-3-319-30671-1_25) — the "braw" key profiles, median chroma profiles from a corpus of 1,160 expert-annotated Beatport tracks, as shipped in [Essentia's `Key`](https://essentia.upf.edu/reference/std_Key.html). They beat the classic [Krumhansl & Kessler (1982)](https://doi.org/10.1037/0033-295X.89.4.334) profiles on the GiantSteps key benchmark, which is why Fosfora uses them.

**`key_is_minor`** — 0 for a major key, 1 for a minor one. It is the single cheapest way to make a visual feel bright or melancholy without any other analysis. It changes rarely, so treat it as a switch rather than a signal.

Source: [Faraldo et al. (2016)](https://doi.org/10.1007/978-3-319-30671-1_25) — the same profile match, reporting which of the 24 major and minor candidates won.

**`key_confidence`** — how sure Fosfora is about the key. Drum-only or atonal passages drive it down. Gate any key-driven color change on this, or the visual will lurch during the breakdown.

Source: [Faraldo et al. (2016)](https://doi.org/10.1007/978-3-319-30671-1_25) — the strength of the winning profile match.

**`pitch`** — the note a solo instrument or voice is singing, as a smooth 0 to 1 sweep across five octaves. 0 is a low A at 55 Hz, 1 is the A five octaves up, and every octave is exactly 0.2 apart, so it is easy to map onto anything musical. It tracks one line at a time, so it works on vocals and leads, not full chords. Notes below 55 Hz are not tracked: `pitch` holds its last value and `pitch_confidence` reads 0.

Source: [de Cheveigné & Kawahara, *YIN, a fundamental frequency estimator for speech and music*, JASA 2002](https://doi.org/10.1121/1.1458024) — the classic pitch tracker ([free full text](https://web.archive.org/web/20260426151005/http://audition.ens.fr/adc/pdf/2002_JASA_YIN.pdf); implementation notes at [librosa's `yin`](https://librosa.org/doc/latest/generated/librosa.yin.html)). Fosfora holds the last note through gaps so the visual does not snap to the bottom on every rest.

**`pitch_confidence`** — whether there is actually a note to track. Drums, noise and silence push it to 0. Always gate `pitch` on this; without it, a pitch-driven visual will wander during percussion-only passages.

Source: [de Cheveigné & Kawahara (2002)](https://doi.org/10.1121/1.1458024) ([free full text](https://web.archive.org/web/20260426151005/http://audition.ens.fr/adc/pdf/2002_JASA_YIN.pdf)) — the periodicity measure YIN produces alongside the pitch estimate.

**`pitch_hz`** *(OSC only)* — the same pitch as a real frequency in hertz, for clients that would rather have 440 than 0.6. It is not a shader uniform; it is derived on the way out.

Source: Fosfora-specific — derived from `pitch` in [`osc/sender.rs`](../crates/fosfora-app/src/osc/sender.rs).

---

## Timbre

**`mfcc`** (13 values, read with `mfcc(i)`) — a compact fingerprint of *tone color*. Together these describe what makes a trumpet sound different from a violin playing the same note at the same volume. Individual coefficients do not have tidy names; treat them as thirteen dials that move distinctly for distinct sounds, which makes them excellent for driving anything that should look different for different instruments.

Source: [Davis & Mermelstein (1980)](https://doi.org/10.1109/TASSP.1980.1163420), *IEEE Trans. ASSP* 28(4) — the original mel-frequency cepstral coefficients (paywalled; free walkthrough at [librosa's `mfcc`](https://librosa.org/doc/latest/generated/librosa.feature.mfcc.html)).

**`contrast_0` … `contrast_5`** — how peaky the sound is in each of six frequency ranges. High values mean clear notes standing out against a quiet background; low values mean everything in that range is mush. It is a good discriminator between a clean tonal mix and a dense distorted one.

Source: [Jiang et al., *Music type classification by spectral contrast feature*, ICME 2002](https://doi.org/10.1109/ICME.2002.1035731) (paywalled; the same feature is documented at [librosa's `spectral_contrast`](https://librosa.org/doc/latest/generated/librosa.feature.spectral_contrast.html)).

**`contrast_mean`** — the average of the six contrast values, as a single "how defined is this mix" dial. Use it when you want the idea without wiring six separate inputs.

Source: [Jiang et al. (2002)](https://doi.org/10.1109/ICME.2002.1035731) — averaged across bands by Fosfora.

**`timbre_flux`** — how fast the tone color is changing. A filter sweep, a vocal entering, or a pad morphing all push it up, while a steady sound sits near 0. Crucially it ignores volume changes, so unlike `flux` it will not fire just because someone rode the fader.

Source: Fosfora-specific — the rate of change of the MFCCs, following the standard delta approach documented at [librosa's `delta`](https://librosa.org/doc/latest/generated/librosa.feature.delta.html). See [`audio/timbre.rs`](../crates/fosfora-app/src/audio/timbre.rs).

---

## Stereo Field

**`pan`** — where the sound sits between the speakers. 0 is hard left, 0.5 is centered, 1 is hard right. OSC clients receive this as −1 to 1 instead. Wire it to horizontal position and the visual follows the mix.

Source: Fosfora-specific — a left/right energy balance over roughly the last 46 milliseconds, in [`audio/stereo.rs`](../crates/fosfora-app/src/audio/stereo.rs).

**`stereo_width`** — how wide the mix is. A mono recording reads 0, a normal stereo mix sits in the middle, and heavily spread or phase-tricked material approaches 1. Good for driving how far apart elements sit on screen.

Source: Fosfora-specific — a mid/side energy ratio in [`audio/stereo.rs`](../crates/fosfora-app/src/audio/stereo.rs).

**`stereo_corr`** — how similar the left and right channels are. 1 means identical, 0.5 means unrelated, and 0 means they actively cancel each other. OSC clients receive this as −1 to 1. It is the more technical cousin of `stereo_width`, and it is the one that catches deliberately out-of-phase production tricks.

Source: Fosfora-specific — a Pearson correlation between the channels, in [`audio/stereo.rs`](../crates/fosfora-app/src/audio/stereo.rs).

**`band_pan_sub_bass` `band_pan_bass` `band_pan_low_mid` `band_pan_mid` `band_pan_upper_mid` `band_pan_presence` `band_pan_brilliance`** — `pan`, split by frequency band: where each of the seven bands sits between the speakers, on the same 0 (left) to 1 (right) scale and with the same band edges as `sub_bass` … `brilliance`. A mix with the hi-hats panned left and the bass centred reads exactly that way here, where `pan` would only report the overall balance. A band with no real energy in it reads 0.5. OSC clients receive these as −1 to 1, like `pan`.

Source: Fosfora-specific — a left/right energy balance per band over the analyzer's 4096-point spectrum, in [`audio/stereo.rs`](../crates/fosfora-app/src/audio/stereo.rs).

---

## Drums vs. Instruments

Fosfora separates the sound into hits and held tones, and reports both. This is the group to reach for when you want drums and melody driving *different* parts of the same visual.

**`percussive_energy`** — how much drum there is. Kicks, snares and hi-hats push it up; a sustained pad or held chord leaves it near 0. Route it to anything sharp: strobes, shakes, hard cuts.

Source: [FitzGerald, *Harmonic/Percussive Separation Using Median Filtering*, DAFx-10](https://dafx10.iem.at/papers/DerryFitzGerald_DAFx10_P15.pdf) — Fosfora runs it moment by moment. The percussive side adds no delay; the harmonic side is a median over the last 0.2 seconds, so it trails a new sustained sound by about 100 milliseconds.

**`harmonic_energy`** — how much sustained, pitched material there is. Pads, vocals, bass notes and chords push it up; a drum-only passage leaves it near 0. Route it to anything smooth: flowing color, slow drift, glow.

Source: [FitzGerald (2010)](https://dafx10.iem.at/papers/DerryFitzGerald_DAFx10_P15.pdf) — the other half of the same separation.

**`harmonic_ratio`** — is this moment drums or music? 0 means pure percussion, 1 means pure sustained tone, and 0.5 means an even mix. Because it is a balance rather than a level, it stays meaningful whether the track is loud or quiet — ideal for crossfading between a strobe-driven look and a flowing one.

Source: [FitzGerald (2010)](https://dafx10.iem.at/papers/DerryFitzGerald_DAFx10_P15.pdf) — the balance between the two separated energies.

---

## Song Structure

These three look at the track over tens of seconds rather than milliseconds. They are the closest Fosfora gets to understanding the arrangement.

**`section_novelty`** — the track just changed character. It peaks when a song moves into a new part, verse to chorus or breakdown to main section, by noticing that the sound has stopped resembling what came just before. It peaks about three seconds after the change, and the confirmed section-boundary event sent over OSC (`/section/boundary`) arrives about six seconds after it, carrying how long ago the boundary actually was. Treat both as a cue to switch looks rather than as a hit.

Source: [Foote, *Automatic Audio Segmentation Using a Measure of Audio Novelty*, ICME 2000](https://ccrma.stanford.edu/workshops/mir2009/references/Foote_00.pdf) — self-similarity novelty with a checkerboard kernel.

**`buildup`** — tension is rising. It climbs through a riser by combining four things producers actually do before a drop: get louder, get brighter, add more hits, and pull the bass out. It is the single best driver for a slow global intensity ramp — a camera push-in, a widening glow, a rising blur.

Source: Fosfora-specific — no published algorithm; a weighted combination of four cues in [`audio/structure.rs`](../crates/fosfora-app/src/audio/structure.rs), tuned for electronic music. The weights are adjustable live in the audio panel.

**`drop`** — the moment the track lands. Fires once, for a single frame, when a long build-up is suddenly answered by a jump in loudness and the bass coming back in. It then refuses to fire again for 16 seconds, so a busy chorus cannot machine-gun it. Treat it as a bonus, not a cue to build a show around: on hand-labelled electronic tracks it catches about a quarter of real drops, and about a quarter of the times it fires are not drops. For a dependable pre-drop ramp, use `buildup`.

Source: Fosfora-specific — no published algorithm; a hand-tuned state machine in [`audio/structure.rs`](../crates/fosfora-app/src/audio/structure.rs), with thresholds exposed in the audio panel.

---

## Audio Textures

The features above are single numbers. These are whole pictures of the sound, available to shaders as functions rather than uniforms — for oscilloscopes, spectrum bars and waterfalls.

**`waveform(x)`** — the actual shape of the sound wave, for an oscilloscope trace. Give it a horizontal position from 0 to 1 and it returns the lowest and highest the wave reached there. Fosfora aligns the trace to the wave itself, so it sits still on screen instead of skating sideways.

Source: Fosfora-specific — zero-crossing-triggered min/max decimation in [`gpu/audio_textures.rs`](../crates/fosfora-app/src/gpu/audio_textures.rs).

**`spectrum(x)`** — how loud each frequency is right now, for drawing spectrum bars. Position 0 is the lowest frequency and 1 the highest, spaced musically so each octave gets equal width rather than the bass being crushed into the far left.

Source: Fosfora-specific — a log-frequency magnitude texture in [`gpu/audio_textures.rs`](../crates/fosfora-app/src/gpu/audio_textures.rs).

**`spectrogram(uv)`** — the last several seconds of sound as a scrolling image, for waterfall displays. Across is time, with 0 the oldest and 1 the newest; up is frequency. Sample it anywhere to get how loud that frequency was at that moment.

Source: Fosfora-specific — a scrolling [mel-band](https://en.wikipedia.org/wiki/Mel_scale) history in [`gpu/audio_textures.rs`](../crates/fosfora-app/src/gpu/audio_textures.rs).

### Binding-only sources

Two extra sets are available in the binding matrix but not as shader uniforms, to keep the uniform budget in check:

- **`audio.mel.0` … `audio.mel.63`** — the 64 individual frequency slices behind `spectrogram`. Use them when seven bands are not enough resolution.
- **`audio.dmfcc.0` … `audio.dmfcc.12`** — the individual rates of change behind `timbre_flux`, as raw plus-or-minus values the binding graph maps for you.

Source: Fosfora-specific — exposed in [`bindings/sources.rs`](../crates/fosfora-app/src/bindings/sources.rs).

---

## How the Numbers Are Tamed

Raw audio measurements are unusable for visuals. A quiet track would barely move anything and a loud one would peg everything at maximum. So every feature passes through three stages before you see it.

### Auto-leveling

Each feature is scaled by one of four policies, chosen per feature:

- **Adaptive** — auto-gain. Watches the last four seconds and stretches what it saw to fill 0–1, so quiet music still drives your visuals. It uses the 5th and 95th percentile rather than the minimum and maximum, so one loud spike cannot flatten everything for the next few seconds. Above the 95th percentile it eases toward 1 instead of clipping, so an unusually big hit still reads as bigger. Used for the seven bands, `rms`, `flux`, `timbre_flux`, and the two separation energies.
- **Fixed range** — already meaningful on its own scale, so it is left alone and simply held steady during silence. Used for `centroid`, `flatness`, `rolloff`, `bandwidth` and `zcr`.
- **Centered** — expressed relative to its own recent average, so 0.5 means "typical" and swings read symmetrically in both directions. Used for the 13 MFCCs.
- **Untouched** — the detector already produced a meaningful 0–1 number and rescaling would only distort it. Used for everything else, including the beat group, key, pitch, stereo and structure features.

The policy for every feature lives in one table in [`audio/schema.rs`](../crates/fosfora-app/src/audio/schema.rs).

Source: Fosfora-specific — percentile ranging in [`audio/ranging.rs`](../crates/fosfora-app/src/audio/ranging.rs) and [`audio/normalizer.rs`](../crates/fosfora-app/src/audio/normalizer.rs).

### Smoothing

Every feature then gets a fast rise and a slow fall, tuned individually. That is why a kick snaps up instantly but eases back down: instant response in both directions would look like flickering. Triggers such as `beat`, `downbeat` and `drop` skip this entirely, and so do the ramps like `beat_phase`, because smoothing a value that wraps from 1 back to 0 would sweep it backwards through the middle.

Source: Fosfora-specific — per-feature attack and release constants in [`audio/smoother.rs`](../crates/fosfora-app/src/audio/smoother.rs).

### The silence gate

One shared test decides whether there is any music at all: perceptual loudness below −55 LUFS counts as silence. When it trips, energy features fall to 0, the auto-leveling windows freeze so silence cannot rescale them, and values that should persist — tempo, key, pitch — hold their last reading instead of collapsing.

Source: Fosfora-specific — the gate lives in [`audio/loudness.rs`](../crates/fosfora-app/src/audio/loudness.rs) and is shared by every detector.

---

## Further Reading

The papers and standards Fosfora's audio analysis is built on.

**Onset and rhythm**
- Sebastian Böck and Gerhard Widmer, "Maximum Filter Vibrato Suppression for Onset Detection", *Proc. 16th Int. Conf. on Digital Audio Effects (DAFx-13)*, Maynooth, 2013, pp. 55–61. [PDF](https://www.dafx.de/paper-archive/2013/papers/09.dafx2013_submission_12.pdf)
- Simon Dixon, "Onset Detection Revisited", *Proc. 9th Int. Conf. on Digital Audio Effects (DAFx-06)*, Montreal, 2006, pp. 133–137. [PDF](https://www.dafx.de/paper-archive/2006/papers/p_133.pdf)
- Juan Pablo Bello, Laurent Daudet, Samer Abdallah, Chris Duxbury, Mike Davies and Mark B. Sandler, "A Tutorial on Onset Detection in Music Signals", *IEEE Trans. Speech and Audio Processing* 13(5), 2005, pp. 1035–1047. [PDF](https://hajim.rochester.edu/ece/sites/zduan/teaching/ece472/reading/Bello_2005.pdf) · [DOI](https://doi.org/10.1109/TSA.2005.851998)
- Daniel P. W. Ellis, "Beat Tracking by Dynamic Programming", *Journal of New Music Research* 36(1), 2007, pp. 51–60. [PDF](https://www.ee.columbia.edu/~dpwe/pubs/Ellis07-beattrack.pdf)

**Pitch, harmony and key**
- Alain de Cheveigné and Hideki Kawahara, "YIN, a fundamental frequency estimator for speech and music", *J. Acoust. Soc. Am.* 111(4), 2002, pp. 1917–1930. [PDF](https://web.archive.org/web/20260426151005/http://audition.ens.fr/adc/pdf/2002_JASA_YIN.pdf) · [DOI](https://doi.org/10.1121/1.1458024) · [record](https://pubmed.ncbi.nlm.nih.gov/12002874/)
- Judith C. Brown, "Calculation of a constant Q spectral transform", *J. Acoust. Soc. Am.* 89(1), 1991, pp. 425–434. [PDF](https://www.ee.columbia.edu/~dpwe/papers/Brown91-cqt.pdf)
- Ángel Faraldo, Emilia Gómez, Sergi Jordà and Perfecto Herrera, "Key Estimation in Electronic Dance Music", *Advances in Information Retrieval (ECIR 2016)*, LNCS 9626, pp. 335–347. [DOI](https://doi.org/10.1007/978-3-319-30671-1_25)
- Carol L. Krumhansl and Edward J. Kessler, "Tracing the dynamic changes in perceived tonal organization in a spatial representation of musical keys", *Psychological Review* 89(4), 1982, pp. 334–368. [DOI](https://doi.org/10.1037/0033-295X.89.4.334) · [record](https://pubmed.ncbi.nlm.nih.gov/7134332/)

**Timbre**
- Steven B. Davis and Paul Mermelstein, "Comparison of parametric representations for monosyllabic word recognition in continuously spoken sentences", *IEEE Trans. Acoustics, Speech, and Signal Processing* 28(4), 1980, pp. 357–366. [DOI](https://doi.org/10.1109/TASSP.1980.1163420)
- Dan-Ning Jiang, Lie Lu, Hong-Jiang Zhang, Jian-Hua Tao and Lian-Hong Cai, "Music type classification by spectral contrast feature", *Proc. IEEE ICME 2002*, vol. 1, pp. 113–116. [DOI](https://doi.org/10.1109/ICME.2002.1035731)

**Separation and structure**
- Derry FitzGerald, "Harmonic/Percussive Separation Using Median Filtering", *Proc. 13th Int. Conf. on Digital Audio Effects (DAFx-10)*, Graz, 2010. [PDF](https://dafx10.iem.at/papers/DerryFitzGerald_DAFx10_P15.pdf) · [repository](https://arrow.tudublin.ie/argcon/67/)
- Jonathan Foote, "Automatic Audio Segmentation Using a Measure of Audio Novelty", *Proc. IEEE ICME 2000*, vol. 1, pp. 452–455. [PDF](https://ccrma.stanford.edu/workshops/mir2009/references/Foote_00.pdf) · [DOI](https://doi.org/10.1109/ICME.2000.869637)

**Loudness**
- ITU-R BS.1770, "Algorithms to measure audio programme loudness and true-peak audio level". [Recommendation](https://www.itu.int/rec/R-REC-BS.1770/en)
- EBU R 128, "Loudness normalisation and permitted maximum level of audio signals". [PDF](https://tech.ebu.ch/docs/r/r128.pdf) · [version history](https://tech.ebu.ch/publications/r128)
- [libebur128](https://github.com/jiixyj/libebur128) — the de-facto reference implementation, whose filter formulation Fosfora follows.

**Spectral descriptors and signal processing**
- Geoffroy Peeters, "A Large Set of Audio Features for Sound Description (Similarity and Classification) in the CUIDADO Project", IRCAM technical report, 2004. [PDF](http://recherche.ircam.fr/anasyn/peeters/ARTICLES/Peeters_2003_cuidadoaudiofeatures.pdf) · [archived](https://web.archive.org/web/20251009112944/http://recherche.ircam.fr/anasyn/peeters/ARTICLES/Peeters_2003_cuidadoaudiofeatures.pdf) — the canonical definitions for `centroid`, `rolloff`, `bandwidth`, `flatness` and their relatives. The IRCAM copy is served over plain HTTP only; use the archived link if your browser enforces HTTPS.
- Greg Welch and Gary Bishop, "An Introduction to the Kalman Filter", UNC-Chapel Hill TR 95-041. [PDF](https://web.archive.org/web/20260522234924/https://www.cs.unc.edu/%7Ewelch/media/pdf/kalman_intro.pdf) — the tracking filter that stabilizes `bpm`. The original UNC URLs no longer resolve; this is the Internet Archive copy.

**General reference**
- [librosa](https://librosa.org/doc/latest/) — the audio analysis library whose documentation defines many of the standard descriptors above in runnable form.
- Fosfora's beat detection pipeline is ported from [EASEy-GLYPH](https://github.com/kevinraymond/easey-glyph).

For how the pipeline is built rather than what it means, see [TECHNICAL.md](TECHNICAL.md#audio-pipeline). For using audio in effects, see [TUTORIALS.md](TUTORIALS.md#audio-reactivity).
