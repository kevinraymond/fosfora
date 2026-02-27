# Changelog

<!-- Release workflow extracts notes between ## vX.Y.Z headers via awk. -->
<!-- Keep the "## vX.Y.Z — date" format for automatic release notes. -->

## v0.3.3 — 2026-02-27

### Fixes
- Rebuild release binaries with clean CI cache (fixes audio callbacks not firing in CI-built binaries)
- Auto-retry audio stream on PipeWire when callbacks stall (race condition workaround)
- Suppress noisy ALSA/JACK/OSS stderr messages on Linux during device enumeration
- Overhaul blend modes: replace SoftLight with ColorDodge, HardLight, Exclusion, Subtract

## v0.3.2 — 2026-02-26

### Fixes
- Fix audio not working on Linux with PipeWire (upgrade cpal to 0.17.3 — fixes ALSA start threshold)
- Add audio health monitoring: detect and warn when device opens but callbacks never fire
- Add diagnostic logging for first audio callback (visible with `RUST_LOG=phosphor_app=info`)

## v0.3.1 — 2026-02-26

### Fixes
- Audio not working in release (CI-built) binaries on macOS and Linux
- macOS: add audio-input entitlement for hardened runtime codesigning
- Linux: filter device list to only usable devices (removes raw ALSA entries)
- Device switch race condition: join old audio thread before opening new device
- Show actual error message on audio capture failure

## v0.3.0 — 2026-02-26

### New Features
- In-app WGSL shader editor with live hot-reload
- Audio input device selector with runtime switching and persistence
- Built-in/user sections in effects panel with delete and copy-shader
- NDI luma-to-alpha toggle for downstream compositing
- Per-effect shader alpha for NDI compositing transparency

### Improvements
- macOS app icon and polished DMG installer with drag-to-Applications
- Shader editor UI refinements (transparent background, vector icons, minimize/expand)
- NDI feature enabled in release builds
- Hide default Phosphor effect from UI, reduce particle count

### Testing
- 236 new unit tests across 27+ modules (coverage 11% → 13%)

### Fixes
- Clippy approx_constant warnings
- Auto-release CI when Cargo.toml version changes

## v0.2.0

Initial public release with multi-layer composition, GPU particles, audio-reactive
shaders, MIDI/OSC input, web control surface, preset system, media layers, video
playback, and NDI output.
