# Fosfora Documentation

Start with the [main README](../README.md) if you haven't yet — it covers install and the first
five minutes. Everything below goes deeper.

| Doc | What's in it | Read it if you… |
|---|---|---|
| **[GALLERY.md](GALLERY.md)** | Every built-in effect, in motion, at default settings | …want to see what Fosfora looks like before installing |
| **[TUTORIALS.md](TUTORIALS.md)** | The full user manual — 13 chapters, effects through OSC | …are learning the app and want to be walked through it |
| **[QUICK-REFERENCE.md](QUICK-REFERENCE.md)** | Panel map, shortcuts, blend modes, MIDI/OSC address tables, config files | …already know the app and just need the number |
| **[AUDIO-FEATURES.md](AUDIO-FEATURES.md)** | All 83 audio features in plain English, with the research behind each | …are building bindings and want to pick the *right* feature |
| **[BENCHMARKS.md](BENCHMARKS.md)** | Measured accuracy of the causal analysis engine vs published systems, per dataset | …want to know how far to trust the beat/tempo/key/section telemetry |
| **[TECHNICAL.md](TECHNICAL.md)** | Architecture, render pipeline, shader authoring guide, `.pfx` format | …are writing an effect or hacking on the engine |
| **[trama/INTEGRATION.md](trama/INTEGRATION.md)** | Phase-0 survey for trama, the node-graph effect system: what the engine already provides and where the graph hooks in | …are following or reviewing the trama workstream |
| **[trama/DECISIONS.md](trama/DECISIONS.md)** | trama's running decision log — one line per decision, with reasoning | …want the *why* behind trama's design without reading diffs |
| **[EXPERIMENTAL.md](EXPERIMENTAL.md)** | The screenplay pipeline: song → editable markdown screenplay → realized scene, rendered headless | …build from source and want AI-drafted scenes you steer by editing a text file |
| **[CREDITS.md](CREDITS.md)** | Libraries, papers and reference implementations Fosfora is built on | …want to know whose shoulders this stands on |

Also at the repo root: [CHANGELOG.md](../CHANGELOG.md) · [CONTRIBUTING.md](../CONTRIBUTING.md) ·
[SECURITY.md](../SECURITY.md) · [CODE_OF_CONDUCT.md](../CODE_OF_CONDUCT.md)

And in [`bridges/`](../bridges/README.md): the Python scripts that stream hand tracking, pose,
face, gamepads and depth cameras into Fosfora's binding matrix.
