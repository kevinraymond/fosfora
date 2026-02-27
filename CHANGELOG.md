# Changelog

<!-- Release workflow extracts notes between ## vX.Y.Z headers via awk. -->
<!-- Keep the "## vX.Y.Z — date" format for automatic release notes. -->

## v0.3.7 — 2026-02-26

### Added
- **Webcam input layers** — live camera feed as a compositing layer (`--features webcam`), cross-platform via nokhwa (v4l2/AVFoundation/MediaFoundation)
  - Capture thread with bounded channel, automatic frame drain (latest-only)
  - "+ Webcam" button in layer panel, webcam controls panel (device name, resolution, mirror, disconnect)
  - Preset save/load with webcam device reconnection
- `ParticleSystem::set_compute_shader()` and `clear_customization()` for runtime particle shader management (infrastructure for future particle design system)
- **Combined "All Media" file filter** — media file picker defaults to showing all supported types (images + video) when video feature is enabled

### Fixes
- Fix NDI runtime discovery on Windows and macOS — check `NDI_RUNTIME_DIR_V6`/`V5` env vars and well-known install paths
- **Webcam robustness** — validate camera access before spawning capture thread (user-friendly EBUSY error), catch libjpeg panics on corrupted MJPEG frames (skip frame instead of dying), detect dead capture threads with status bar notification, clean up capture when deleting webcam layers

## v0.3.6 — 2026-02-27

### Fixes
- Fix PulseAudio capture delivering audio only ~2x/sec (set explicit fragsize=4096, was using PA default ~88KB)
- Runtime-load PulseAudio via dlopen — release binaries no longer crash on systems without libpulse installed
- Truncate long audio device names in UI to prevent panel width blowout

### Added
- `--audio-test` CLI flag: standalone audio diagnostic (no GPU, works over SSH)
- Periodic audio health logging (reads/s, latency stats, throughput) every 5s
- `PHOSPHOR_AUDIO_DEBUG=1` env var for per-read verbose logging
- `AudioSystem::Drop` for clean thread shutdown on exit

### Changed
- Skip cpal device enumeration when PulseAudio is active (eliminates JACK "cannot connect" noise)
- CI no longer requires `libpulse-dev` build dependency

## v0.3.5 — 2026-02-27

### Fixes
- Use PulseAudio capture on Linux (bypasses ALSA, fixes audio not working in release binaries on PipeWire systems)
- Fall back to cpal/ALSA if PulseAudio unavailable

## v0.3.4 — 2026-02-27

### Fixes
- Auto-retry audio stream on PipeWire when callbacks stall (synchronous retry — insufficient)

## v0.3.3 — 2026-02-27

### Fixes
- Rebuild release binaries with clean CI cache (fixes audio callbacks not firing in CI-built binaries)
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
- NDI® luma-to-alpha toggle for downstream compositing
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
playback, and NDI® output.

---
NDI® is a registered trademark of Vizrt NDI AB.
