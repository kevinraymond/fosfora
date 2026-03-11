# Changelog

<!-- Release workflow extracts notes between ## vX.Y.Z headers via awk. -->
<!-- Keep the "## vX.Y.Z — date" format for automatic release notes. -->

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
