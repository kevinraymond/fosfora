## Downloads

| Platform | File | Notes |
|----------|------|-------|
| **macOS** | `.dmg` | Universal binary (Apple Silicon + Intel) |
| **Linux x86_64** | `.tar.gz` | glibc 2.35+ (Ubuntu 22.04+, Debian 12+) |
| **Windows x64** | `.zip` | Windows 10+ |
| **TouchDesigner** | `fosfora_examples.tox`, `fosfora_signal.tox` | Four ready-made views driven by `fosfora --signal`, and the component they are built on for your own network: [setup](https://github.com/kevinraymond/fosfora/blob/main/docs/TOUCHDESIGNER.md) |

## Getting started

**macOS**: Open the DMG, drag Fosfora.app to Applications (or run directly). Signed and notarized.

**Linux / Windows**: Extract, run `fosfora` from the extracted directory. The `assets/` folder must be next to the binary.

## Requirements

- **GPU**: Vulkan (Linux/Windows) or Metal (macOS)
- **Audio**: Built-in mic or line-in for audio-reactive visuals
- **NDI®** (optional): [NDI® SDK runtime](https://ndi.video) for network video output
- **Video playback** (optional): `ffmpeg` on PATH for media layers

---
NDI® is a registered trademark of Vizrt NDI AB.
