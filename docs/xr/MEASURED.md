# Measured on device

Numbers only, newest step at the bottom of each table. Each row names its
commit. A claim in a report that isn't here doesn't count (invariant I4).

## Environment (S0)

| Item | Value |
|---|---|
| Dev host OS / arch | Debian 13 (trixie), Linux 6.12, x86_64, 32 threads |
| Rust toolchain (in repo) | 1.97.0 + `aarch64-linux-android` target |
| Android NDK | 27.0.12077973 (`~/Android/Sdk/ndk`); SDK platforms to android-36.1, build-tools to 36.1.0 |
| JDK | OpenJDK 21.0.11 |
| cargo-ndk | 4.1.2 |
| Quest 3 Horizon OS version | v207 (`207.0.0.297.1234`), Android 14 / SDK 34, `ro.ovr.os.api.version` 160 |
| OpenXR loader (source, version) | (S1) |
| Desktop baseline: clippy / test wall time | ci.yml lint+test matrix all green on `3f7cce0` (warm cache, 32 threads): fmt 1s, clippy ×8 sets 110s, test ×7 sets 368s (default: 1022 passed, 0 failed, 111 ignored), `cargo deny` ok; total 479s |

## Adapter limits (S2)

| Limit | Value | Required |
|---|---|---|
| max_storage_buffers_per_shader_stage | | ≥ 16 |
| max_bind_groups | | ≥ 5 |
| max_storage_buffer_binding_size | | |
| max_buffer_size | | |
| Swapchain format / per-eye size | | |

## Frame timing

| Step | Commit | Content | Display Hz | Held 60 s? | CPU ms | GPU ms | Tool |
|---|---|---|---|---|---|---|---|
| S1 | | clear color | | | | | |
| S2 | | triangle | | | | | |
| S4 | | 2D effect on quad | | | | | |

## Particle sweep (S5)

| Particles | Raster scale | Hz target | GPU ms | Held? | Notes |
|---|---|---|---|---|---|
| 100K | 1.0 | 72 | | | |
| 250K | 1.0 | 72 | | | |
| 500K | 1.0 | 72 | | | |
| 1M | 1.0 | 72 | | | |
| 2M | 1.0 | 72 | | | |

Ceiling at 72 Hz: — · at 90 Hz: — · at 0.75× raster: —

## Audio (S6)

| Item | Value |
|---|---|
| Test track, known BPM | |
| Detected BPM after lock | |
| Lock-in time (s) | |
| Audio-to-photon latency (ms, method) | |
| Mic source reacts? | |

## Mixed reality (S7)

| Configuration | Particles | GPU ms | Held 72 Hz? |
|---|---|---|---|
| Passthrough off (S5 baseline) | | | |
| Passthrough on | | | |
| + hands as obstacles | | | |
| + scene planes / mesh | | | |
