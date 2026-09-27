# Test track for the XR build (S6)

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
