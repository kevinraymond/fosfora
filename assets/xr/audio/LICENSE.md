# Audio in the XR build

## Bundled tracks

`Let_the_Night_Come_In.ogg`, `Slow_Blue.ogg` and `Second_Wind.ogg` were made for this
app with YuE2 (https://github.com/multimodal-art-projection/YuE, model `m-a-p/YuE2-3B`
with the `m-a-p/YuE2-Vae` decoder), run locally. The style prompts and the lyrics of
"Let the Night Come In" were written for this app. "Slow Blue" and "Second Wind" are
instrumental. Each file is the model's output with the level lowered, 1 dB for "Let the
Night Come In" and "Second Wind" and 2 dB for "Slow Blue", encoded with
`ffmpeg -af volume=-1dB -ar 48000 -ac 2 -c:a libvorbis -q:a 5` (`volume=-2dB` for
"Slow Blue").

The YuE2 weights are licensed CC BY-NC 4.0 with an additional permission (2026-09-16)
that lets individual creators publish and distribute what they generate; the outputs are
not under the weights' NonCommercial term.

License of the three tracks: CC0 1.0 (public domain dedication,
https://creativecommons.org/publicdomain/zero/1.0/).

## Test track (S6)

`ember_glow_excerpt.ogg`: seconds 30–75 of "Ember Glow" by oglsdl, from
OpenGameArt (https://opengameart.org/content/ember-glow), released under
CC0 1.0 (public domain dedication,
https://creativecommons.org/publicdomain/zero/1.0/). Original file
`ember_glow_0.ogg` (sha256 `1ad3ffc8c378539be95a8ebe4234406d33c02624662b6cd187cf099ddb48f5f3`,
3:01, 48 kHz stereo Vorbis 320 kb/s), stated tempo 140 BPM, measured 140.0
±0.25 BPM over the whole track by onset autocorrelation (docs/xr/MEASURED.md,
"Audio (S6)"). Excerpt made with
`ffmpeg -ss 30 -t 45 -i ember_glow_0.ogg -ar 48000 -ac 2 -c:a libvorbis -q:a 4`.
Used only to drive the audio analysis on device; not part of any desktop build.
