use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::Result;

use super::format::PfxEffect;

/// Resolve the assets directory once (CWD-relative → exe-relative → macOS bundle).
/// The shipped `.pfx` effects, for tests that need the real effect table.
///
/// `CARGO_MANIFEST_DIR`, not [`assets_dir`]: that resolves CWD-relative and
/// `cargo test` runs with CWD = `crates/fosfora-app`, which has no `assets/`.
/// `preset/store.rs`, `bindings/templates.rs` and `gpu/pass_executor.rs` each
/// grew their own copy of this walk before it lived here.
#[cfg(test)]
pub fn shipped_effects_for_test() -> Vec<PfxEffect> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/effects");
    let effects: Vec<PfxEffect> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "pfx"))
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .filter_map(|j| serde_json::from_str::<PfxEffect>(&j).ok())
        .collect();
    assert!(
        !effects.is_empty(),
        "no .pfx files parsed from {}",
        dir.display()
    );
    effects
}

/// The effect the app opens on, and a fresh stack starts from.
pub const LAUNCH_EFFECT: &str = "Fosfora";

/// Effects that were renamed, as (old name, new name). The launch effect was
/// "Phosphor" until the app was; presets, bindings and loop specs saved
/// before still say so.
const RENAMED_EFFECTS: &[(&str, &str)] = &[("Phosphor", "Fosfora")];

/// An effect's current name for `name`, which may be one it had before a
/// rename. Anything else comes back unchanged.
pub fn current_effect_name(name: &str) -> &str {
    RENAMED_EFFECTS
        .iter()
        .find(|(old, _)| *old == name)
        .map_or(name, |(_, new)| new)
}

static ASSETS_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Pin the assets directory, for a frontend that knows where its assets are (the XR
/// app unpacks them out of its APK). Wins over the search in [`assets_dir`], but only
/// before its first call: once resolved the directory is fixed for the process, and a
/// late call hands the path back.
pub fn set_assets_dir(dir: PathBuf) -> Result<(), PathBuf> {
    ASSETS_DIR.set(dir)
}

pub fn assets_dir() -> &'static Path {
    ASSETS_DIR.get_or_init(|| {
        // 1. CWD-relative (dev workflow)
        let cwd = PathBuf::from("assets");
        if cwd.join("effects").is_dir() {
            log::info!("Assets: CWD-relative ({})", cwd.display());
            return cwd;
        }

        // 2. Exe-relative (installed binary)
        if let Ok(exe) = std::env::current_exe() {
            if let Some(exe_dir) = exe.parent() {
                let beside = exe_dir.join("assets");
                if beside.join("effects").is_dir() {
                    log::info!("Assets: exe-relative ({})", beside.display());
                    return beside;
                }

                // 3. macOS .app bundle: exe is in Foo.app/Contents/MacOS/
                let bundle = exe_dir.join("../Resources/assets");
                if bundle.join("effects").is_dir() {
                    let canonical = bundle.canonicalize().unwrap_or(bundle);
                    log::info!("Assets: macOS bundle ({})", canonical.display());
                    return canonical;
                }
            }
        }

        // Fallback — will surface as "Effects directory not found" later
        log::warn!("Assets directory not found; using CWD-relative fallback");
        cwd
    })
}

const LIB_FILENAMES: &[&str] = &[
    "shaders/lib/noise.wgsl",
    "shaders/lib/palette.wgsl",
    "shaders/lib/sdf.wgsl",
    "shaders/lib/tonemap.wgsl",
    "shaders/lib/chronoflow.wgsl",
    "shaders/lib/overlay_lib.wgsl",
];

/// The same libraries, embedded, for the offscreen compile/render probes — those
/// run without an assets directory and used to hand-list `noise + palette` at each
/// site. Adding a sixth library once broke seven of them at once, because every
/// probe carried its own copy of the list; `probe_libs_match_production` now pins
/// this pair to [`LIB_FILENAMES`] so the next lib cannot drift the same way.
#[cfg(test)]
const LIB_SOURCES: &[(&str, &str)] = &[
    (
        "shaders/lib/noise.wgsl",
        include_str!("../../../../assets/shaders/lib/noise.wgsl"),
    ),
    (
        "shaders/lib/palette.wgsl",
        include_str!("../../../../assets/shaders/lib/palette.wgsl"),
    ),
    (
        "shaders/lib/sdf.wgsl",
        include_str!("../../../../assets/shaders/lib/sdf.wgsl"),
    ),
    (
        "shaders/lib/tonemap.wgsl",
        include_str!("../../../../assets/shaders/lib/tonemap.wgsl"),
    ),
    (
        "shaders/lib/chronoflow.wgsl",
        include_str!("../../../../assets/shaders/lib/chronoflow.wgsl"),
    ),
    (
        "shaders/lib/overlay_lib.wgsl",
        include_str!("../../../../assets/shaders/lib/overlay_lib.wgsl"),
    ),
];

/// Production library preamble for probes: every `LIB_FILENAMES` entry, in order.
#[cfg(test)]
pub(crate) fn probe_libs() -> String {
    LIB_SOURCES
        .iter()
        .map(|(_, src)| *src)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Wrap a shader for a compile probe through the *production* path, so a probe
/// makes the same uniform-injection decision the app makes.
///
/// Probes used to concatenate `UNIFORM_BLOCK` unconditionally, which made them
/// structurally blind to the injection-suppression trap (#1855): the probe
/// passed while the app failed to load the same shader.
#[cfg(test)]
pub(crate) fn probe_preamble(source: &str) -> String {
    EffectLoader::for_test(&probe_libs()).prepend_library(source)
}

/// Standard uniform block prepended to all effect shaders.
///
/// Shader ABI v3 (400-byte `PhosphorUniforms`): the v2 batched bump #1505 reserved
/// the loudness / key / downbeat / stereo / structure tail and the A17 audio textures
/// (bindings 3-6); the v3 batched bump #1629 reserves the hpss / pitch / spectral-contrast
/// tail (13 scalars, absorbing v2's trailing pad). Reserved scalars read 0.0 and the audio
/// textures are 1x1 placeholders until their detectors land. Keep this byte-for-byte in
/// sync with `ShaderUniforms` (gpu/uniforms.rs) and `assets/shaders/default.wgsl`.
const UNIFORM_BLOCK: &str = r#"
struct PhosphorUniforms {
    time: f32,
    delta_time: f32,
    resolution: vec2f,

    sub_bass: f32,
    bass: f32,
    low_mid: f32,
    mid: f32,
    upper_mid: f32,
    presence: f32,
    brilliance: f32,
    rms: f32,

    kick: f32,
    centroid: f32,
    flux: f32,
    flatness: f32,
    rolloff: f32,
    bandwidth: f32,
    zcr: f32,
    onset: f32,
    beat: f32,
    beat_phase: f32,
    bpm: f32,
    beat_strength: f32,

    params: array<vec4f, 4>,
    feedback_decay: f32,
    frame_index: f32,

    dominant_chroma: f32,
    scroll_phase: f32,
    mfcc: array<vec4f, 4>,     // 13 MFCCs (indices 0-12 used, 13-15 padding)
    chroma: array<vec4f, 3>,   // 12 pitch class energies (C=0, C#=1, ..., B=11)

    // Reserved audio features (batched ABI bump #1505) — 0.0 until each detector lands.
    loudness_m: f32,       // A10 momentary loudness (#1461)
    loudness_s: f32,       // A10 short-term loudness
    loudness_trend: f32,   // A10 loudness slope/direction
    key_class: f32,        // A11 key root pitch class / 11 (#1462)
    key_is_minor: f32,     // A11 0.0 major, 1.0 minor
    key_confidence: f32,   // A11 key estimate confidence
    downbeat: f32,         // A12 1.0 on bar-start frame (#1463)
    bar_phase: f32,        // A12 0-1 sawtooth over the current bar
    beat_in_bar: f32,      // A12 beat index within the bar, 0-1
    pan: f32,              // A13 stereo balance, 0..1 (#1464)
    stereo_width: f32,     // A13 mid/side width
    stereo_corr: f32,      // A13 L/R correlation, 0..1
    section_novelty: f32,  // A18 self-similarity novelty (#1469)
    buildup: f32,          // A18 riser/tension estimate
    drop: f32,             // A18 drop/impact detection

    // Reserved audio features (batched ABI bump #1629, "v3") — 0.0 until each detector lands.
    percussive_energy: f32, // A14 transient energy, dB-mapped 0-1 (#1465)
    harmonic_energy: f32,   // A14 sustained energy, dB-mapped 0-1
    harmonic_ratio: f32,    // A14 harmonic vs percussive balance, 0-1
    pitch: f32,             // A15 log-frequency f0, 0-1 (#1466)
    pitch_confidence: f32,  // A15 YIN dip confidence, 0-1
    contrast_0: f32,        // A16 spectral contrast band ~200 Hz (#1467)
    contrast_1: f32,        // A16 ~400 Hz
    contrast_2: f32,        // A16 ~800 Hz
    contrast_3: f32,        // A16 ~1600 Hz
    contrast_4: f32,        // A16 ~3200 Hz
    contrast_5: f32,        // A16 ~6400 Hz+
    contrast_mean: f32,     // A16 mean contrast across bands
    timbre_flux: f32,       // A16 L2 norm of the delta-MFCC vector
    // A13b (#1801) per-band pan: where each of the 7 bands sits in the stereo image.
    // 0.5 = centred, 0 = hard left, 1 = hard right; a band with no energy holds 0.5.
    // Same band order as sub_bass..brilliance above. Read it with band_pan(i).
    band_pan: array<vec4f, 2>,

    // Overlay clock (v4): monotonic 0-based counters, stepping by 1 exactly when the
    // matching phase sawtooth wraps — `bar_index + bar_phase` is a continuous multi-bar
    // clock (raw counts, exact to 2^24).
    bar_index: f32,
    beat_index: f32,
    // Tempo trust (#81): how far recent tempo evidence agrees with the grid (0-1), and
    // 1.0 while the tempo is locked. Gate strobes on beat_locked to keep them off an
    // unconfirmed grid.
    tempo_confidence: f32,
    beat_locked: f32,
}

@group(0) @binding(0) var<uniform> u: PhosphorUniforms;
@group(0) @binding(1) var prev_frame: texture_2d<f32>;
@group(0) @binding(2) var prev_sampler: sampler;
// A17 audio textures (#1505) — 1x1 placeholders until the A17 DSP uploads real data.
@group(0) @binding(3) var audio_waveform: texture_2d<f32>;    // Rg16Float 1024x1: r=min, g=max
@group(0) @binding(4) var audio_spectrum: texture_2d<f32>;    // R16Float 512x1: log-magnitude
@group(0) @binding(5) var audio_spectrogram: texture_2d<f32>; // R8Unorm mel x frames history
@group(0) @binding(6) var audio_sampler: sampler;

fn param(i: u32) -> f32 {
    return u.params[i / 4u][i % 4u];
}

fn mfcc(i: u32) -> f32 {
    return u.mfcc[i / 4u][i % 4u];
}

fn chroma_val(i: u32) -> f32 {
    return u.chroma[i / 4u][i % 4u];
}

// A13b per-band pan, i in 0..6 (sub_bass, bass, low_mid, mid, upper_mid, presence, brilliance).
fn band_pan(i: u32) -> f32 {
    return u.band_pan[i / 4u][i % 4u];
}

fn feedback(uv: vec2f) -> vec4f {
    return textureSample(prev_frame, prev_sampler, uv);
}

// A17 audio-texture accessors (x in 0..1). Placeholder textures return 0.0
// until the A17 DSP lands.
fn waveform(x: f32) -> vec2f {
    return textureSampleLevel(audio_waveform, audio_sampler, vec2f(x, 0.5), 0.0).rg;
}

fn spectrum(x: f32) -> f32 {
    return textureSampleLevel(audio_spectrum, audio_sampler, vec2f(x, 0.5), 0.0).r;
}

fn spectrogram(uv: vec2f) -> f32 {
    return textureSampleLevel(audio_spectrogram, audio_sampler, uv, 0.0).r;
}
"#;

/// Build the WGSL declarations for `input_count` multi-pass graph inputs (#1481).
/// Each input `i` gets a raw texture `inputI_tex` at binding `7+2i`, a sampler
/// `inputI_sampler` at `8+2i`, and a convenience accessor `inputI(uv)`.
fn build_input_bindings(input_count: usize) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    for i in 0..input_count {
        let tex = 7 + 2 * i;
        let smp = 8 + 2 * i;
        let _ = write!(
            s,
            "@group(0) @binding({tex}) var input{i}_tex: texture_2d<f32>;\n\
             @group(0) @binding({smp}) var input{i}_sampler: sampler;\n\
             fn input{i}(uv: vec2f) -> vec4f {{ return textureSampleLevel(input{i}_tex, input{i}_sampler, uv, 0.0); }}\n"
        );
    }
    s
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Strip WGSL comments so a declaration check cannot be fooled by prose.
/// Block comments nest in WGSL, so depth is counted rather than closing on the
/// first `*/`. Newlines inside comments are kept so line numbers still line up.
///
/// Byte-oriented on purpose: every delimiter is ASCII and UTF-8 continuation
/// bytes are all >= 0x80, so a multi-byte character in a comment can never be
/// split by a removal boundary.
fn strip_wgsl_comments(source: &str) -> String {
    let bytes = source.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    let mut depth = 0usize;
    while i < bytes.len() {
        let rest = &bytes[i..];
        if rest.starts_with(b"/*") {
            depth += 1;
            i += 2;
        } else if depth > 0 && rest.starts_with(b"*/") {
            depth -= 1;
            i += 2;
        } else if depth > 0 {
            if bytes[i] == b'\n' {
                out.push(b'\n');
            }
            i += 1;
        } else if rest.starts_with(b"//") {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// True if `source` itself declares `struct PhosphorUniforms`, in code rather
/// than in a comment.
///
/// A bare `source.contains("PhosphorUniforms")` used to stand in for this, so
/// any mention of the name — including inside a comment — silently suppressed
/// uniform-block injection and the effect failed in production only, with
/// "no definition in scope for identifier: u" (#1855).
fn declares_uniform_struct(source: &str) -> bool {
    let code = strip_wgsl_comments(source);
    code.match_indices("struct").any(|(i, kw)| {
        // `struct` must be a whole token, not the tail of an identifier...
        let before_ok = code[..i]
            .chars()
            .next_back()
            .is_none_or(|c| !is_ident_char(c));
        let after = &code[i + kw.len()..];
        // ...separated from the name it declares...
        let sep_ok = after.chars().next().is_some_and(|c| !is_ident_char(c));
        // ...and that name must be exactly `PhosphorUniforms`.
        before_ok
            && sep_ok
            && after
                .trim_start()
                .strip_prefix("PhosphorUniforms")
                .is_some_and(|tail| tail.chars().next().is_none_or(|c| !is_ident_char(c)))
    })
}

pub struct EffectLoader {
    pub effects: Vec<PfxEffect>,
    pub current_effect: Option<usize>,
    lib_source: String,
    /// Particle library source (structs, bindings, helpers) prepended to compute shaders.
    particle_lib_source: String,
    /// Spatial hash grid dimensions, patched into particle_lib SH_GRID_W/H constants.
    /// Updated when a particle system with interaction is created.
    pub grid_dims: (u32, u32),
    /// 3D spatial hash grid edge, patched into particle_lib's SH_GRID_D constant.
    /// Updated alongside `grid_dims`; 1 unless the effect sets `interaction_3d`.
    pub grid_d: u32,
}

impl EffectLoader {
    pub fn new() -> Self {
        let base = assets_dir();
        // Load library sources
        let mut lib_source = String::new();
        for filename in LIB_FILENAMES {
            let path = base.join(filename);
            match std::fs::read_to_string(&path) {
                Ok(src) => {
                    lib_source.push_str(&src);
                    lib_source.push('\n');
                }
                Err(e) => {
                    log::warn!("Failed to load shader library {}: {e}", path.display());
                }
            }
        }

        // Load particle library source
        let particle_lib_path = base.join("shaders/lib/particle_lib.wgsl");
        let particle_lib_source = match std::fs::read_to_string(&particle_lib_path) {
            Ok(src) => src,
            Err(e) => {
                log::warn!(
                    "Failed to load particle library {}: {e}",
                    particle_lib_path.display()
                );
                String::new()
            }
        };

        Self {
            effects: Vec::new(),
            current_effect: None,
            lib_source,
            particle_lib_source,
            grid_dims: (40, 40),
            grid_d: 1,
        }
    }

    /// Reload shader library sources from disk (called when lib/*.wgsl changes).
    pub fn reload_library(&mut self) {
        let base = assets_dir();
        let mut new_source = String::new();
        for filename in LIB_FILENAMES {
            let path = base.join(filename);
            match std::fs::read_to_string(&path) {
                Ok(src) => {
                    new_source.push_str(&src);
                    new_source.push('\n');
                }
                Err(e) => {
                    log::warn!("Failed to reload shader library {}: {e}", path.display());
                }
            }
        }
        if new_source != self.lib_source {
            self.lib_source = new_source;
            log::info!("Reloaded shader library sources");
        }

        // Reload particle library
        let particle_lib_path = base.join("shaders/lib/particle_lib.wgsl");
        if let Ok(new_plib) = std::fs::read_to_string(&particle_lib_path) {
            if new_plib != self.particle_lib_source {
                self.particle_lib_source = new_plib;
                log::info!("Reloaded particle library source");
            }
        }
    }

    pub fn scan_effects_directory(&mut self) {
        self.effects.clear();
        let dir = assets_dir().join("effects");
        if !dir.exists() {
            log::warn!("Effects directory not found: {}", dir.display());
            return;
        }

        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "pfx"))
            .collect();
        entries.sort_by_key(|e| e.file_name());

        for entry in entries {
            match std::fs::read_to_string(entry.path()) {
                Ok(json) => match serde_json::from_str::<PfxEffect>(&json) {
                    Ok(mut effect) => {
                        // `interaction_3d` implies `interaction`: set it so every
                        // reader of the flag builds and reports the hash.
                        if let Some(pd) = effect.particles.as_mut() {
                            pd.interaction |= pd.interaction_3d;
                        }
                        let path = entry.path().canonicalize().unwrap_or_else(|_| entry.path());
                        log::info!("Found effect: {} ({})", effect.name, path.display());
                        effect.source_path = Some(path);
                        self.effects.push(effect);
                    }
                    Err(e) => {
                        log::warn!("Failed to parse {}: {e}", entry.path().display());
                    }
                },
                Err(e) => {
                    log::warn!("Failed to read {}: {e}", entry.path().display());
                }
            }
        }

        log::info!("Found {} effects", self.effects.len());
    }

    pub fn resolve_shader_path(&self, shader_rel: &str) -> PathBuf {
        assets_dir().join("shaders").join(shader_rel)
    }

    /// Load a fragment-pass shader with the uniform block + library preamble, and
    /// declare `input_count` multi-pass graph inputs (#1481) so the shader can sample
    /// prior passes' outputs as `input0(uv)..inputN-1(uv)` (or the raw `inputI_tex` /
    /// `inputI_sampler`). Single-shader passes pass `input_count = 0`.
    pub fn load_effect_source_with_inputs(
        &self,
        shader_rel: &str,
        input_count: usize,
    ) -> Result<String> {
        let path = self.resolve_shader_path(shader_rel);
        let source = std::fs::read_to_string(&path)?;
        Ok(self.prepend_library_with_inputs(&source, input_count))
    }

    /// Prepend the uniform block + library, then declare `input_count` pass-graph
    /// input bindings. Module-scope declarations are order-independent in WGSL, so
    /// the input block simply prepends ahead of the library preamble.
    pub fn prepend_library_with_inputs(&self, source: &str, input_count: usize) -> String {
        let base = self.prepend_library(source);
        if input_count == 0 {
            return base;
        }
        format!("{}\n{}", build_input_bindings(input_count), base)
    }

    /// Load a compute shader source. Prepends the noise library and particle library
    /// (structs, bindings, helpers) but NOT the fragment uniform block.
    pub fn load_compute_source(&self, shader_rel: &str) -> Result<String> {
        let path = self.resolve_shader_path(shader_rel);
        let source = std::fs::read_to_string(&path)?;
        Ok(self.prepend_compute_libraries(&source))
    }

    /// Prepend noise library + particle library to a compute shader source.
    /// Patches spatial hash grid constants (SH_GRID_W/H/D) to match current
    /// grid_dims and grid_d.
    pub fn prepend_compute_libraries(&self, source: &str) -> String {
        let (w, h) = self.grid_dims;
        let d = self.grid_d;
        let patched_plib = self
            .particle_lib_source
            .replace(
                "const SH_GRID_W: u32 = 40u;",
                &format!("const SH_GRID_W: u32 = {w}u;"),
            )
            .replace(
                "const SH_GRID_H: u32 = 40u;",
                &format!("const SH_GRID_H: u32 = {h}u;"),
            )
            .replace(
                "const SH_GRID_D: u32 = 1u;",
                &format!("const SH_GRID_D: u32 = {d}u;"),
            );
        format!("{}\n{}\n{}", self.lib_source, patched_plib, source)
    }

    /// Prepend the uniform block and library functions to a shader source.
    pub fn prepend_library(&self, source: &str) -> String {
        // Only skip injection when the shader declares the struct itself — a bare
        // mention (a comment, a doc reference) must not suppress it (#1855).
        if declares_uniform_struct(source) {
            format!("{}\n{}", self.lib_source, source)
        } else {
            format!("{}\n{}\n{}", UNIFORM_BLOCK, self.lib_source, source)
        }
    }

    /// Returns true if the effect is a built-in (shipped) effect.
    pub fn is_builtin(effect: &PfxEffect) -> bool {
        effect.author == "Fosfora"
    }

    /// Create an EffectLoader with pre-supplied library source (for tests).
    #[cfg(test)]
    pub fn for_test(lib_source: &str) -> Self {
        Self {
            effects: Vec::new(),
            current_effect: None,
            lib_source: lib_source.to_string(),
            particle_lib_source: String::new(),
            grid_dims: (40, 40),
            grid_d: 1,
        }
    }

    /// Delete a user effect: removes the .pfx and its .wgsl shader files, then rescans.
    pub fn delete_effect(&mut self, index: usize) -> Result<String> {
        let effect = self
            .effects
            .get(index)
            .ok_or_else(|| anyhow::anyhow!("Effect index {} out of range", index))?;
        if Self::is_builtin(effect) {
            anyhow::bail!("Cannot delete built-in effect '{}'", effect.name);
        }
        let name = effect.name.clone();

        // Delete .pfx file
        if let Some(ref pfx_path) = effect.source_path {
            if pfx_path.exists() {
                std::fs::remove_file(pfx_path)?;
                log::info!("Deleted .pfx: {}", pfx_path.display());
            }
        }

        // Delete shader files referenced by the effect
        let mut shader_files = Vec::new();
        if !effect.shader.is_empty() {
            shader_files.push(effect.shader.clone());
        }
        for pass in &effect.passes {
            if !pass.shader.is_empty() && !shader_files.contains(&pass.shader) {
                shader_files.push(pass.shader.clone());
            }
        }
        if let Some(ref particles) = effect.particles {
            if !particles.compute_shader.is_empty()
                && !shader_files.contains(&particles.compute_shader)
            {
                shader_files.push(particles.compute_shader.clone());
            }
            if let Some(ref rd) = particles.reaction_diffusion {
                if !rd.compute_shader.is_empty() && !shader_files.contains(&rd.compute_shader) {
                    shader_files.push(rd.compute_shader.clone());
                }
            }
        }
        for shader_rel in &shader_files {
            let path = self.resolve_shader_path(shader_rel);
            if path.exists() {
                std::fs::remove_file(&path)?;
                log::info!("Deleted shader: {}", path.display());
            }
        }

        // Rescan
        self.scan_effects_directory();
        Ok(name)
    }

    /// Copy a built-in effect to a new user effect with the given name.
    /// Returns (pfx_path, first_wgsl_path) so the caller can load + open editor.
    pub fn copy_builtin_effect(&self, index: usize, new_name: &str) -> Result<(PathBuf, PathBuf)> {
        let effect = self
            .effects
            .get(index)
            .ok_or_else(|| anyhow::anyhow!("Effect index {} out of range", index))?;
        if !Self::is_builtin(effect) {
            anyhow::bail!("Effect '{}' is not a built-in", effect.name);
        }

        let new_name = new_name.trim();
        if new_name.is_empty() {
            anyhow::bail!("Effect name cannot be empty");
        }

        // Sanitize to snake_case filename
        let snake: String = new_name
            .chars()
            .map(|c| {
                if c.is_alphanumeric() {
                    c.to_ascii_lowercase()
                } else {
                    '_'
                }
            })
            .collect();
        let snake = snake.trim_matches('_').to_string();
        if snake.is_empty() {
            anyhow::bail!("Invalid effect name");
        }

        let effects_dir = assets_dir().join("effects");
        let shaders_dir = assets_dir().join("shaders");
        let new_pfx_path = effects_dir.join(format!("{snake}.pfx"));
        if new_pfx_path.exists() {
            anyhow::bail!("Effect '{}' already exists", new_name);
        }

        // Collect all shader files from the effect and copy each
        let passes = effect.normalized_passes();
        let mut shader_map: Vec<(String, String)> = Vec::new(); // (old_rel, new_rel)
        for pass in &passes {
            if !pass.shader.is_empty() && !shader_map.iter().any(|(old, _)| old == &pass.shader) {
                let new_shader = format!("{snake}.wgsl");
                // If multi-pass, use {snake}_{pass_name}.wgsl
                let new_rel = if passes.len() > 1 {
                    let pass_snake: String = pass
                        .name
                        .chars()
                        .map(|c| {
                            if c.is_alphanumeric() {
                                c.to_ascii_lowercase()
                            } else {
                                '_'
                            }
                        })
                        .collect();
                    format!("{snake}_{pass_snake}.wgsl")
                } else {
                    new_shader
                };
                let new_path = shaders_dir.join(&new_rel);
                if new_path.exists() {
                    anyhow::bail!("Shader '{}' already exists", new_rel);
                }
                shader_map.push((pass.shader.clone(), new_rel));
            }
        }

        // Copy shader files
        let mut first_wgsl = PathBuf::new();
        for (old_rel, new_rel) in &shader_map {
            let src = self.resolve_shader_path(old_rel);
            let dst = shaders_dir.join(new_rel);
            std::fs::copy(&src, &dst)?;
            log::info!("Copied shader: {} -> {}", src.display(), dst.display());
            if first_wgsl.as_os_str().is_empty() {
                first_wgsl = dst;
            }
        }

        // Build new .pfx with updated name, author, and shader references
        let mut new_effect = effect.clone();
        new_effect.name = new_name.to_string();
        new_effect.author = String::new(); // user effect
        new_effect.source_path = None;

        // Update shader references
        for (old_rel, new_rel) in &shader_map {
            if new_effect.shader == *old_rel {
                new_effect.shader = new_rel.clone();
            }
            for pass in &mut new_effect.passes {
                if pass.shader == *old_rel {
                    pass.shader = new_rel.clone();
                }
            }
        }

        // Also update compute_shader and R-D shader if present in particles
        if let Some(ref mut particles) = new_effect.particles {
            if !particles.compute_shader.is_empty() {
                let compute_new = format!("{snake}_sim.wgsl");
                let compute_src = self.resolve_shader_path(&particles.compute_shader);
                let compute_dst = shaders_dir.join(&compute_new);
                if !compute_dst.exists() {
                    std::fs::copy(&compute_src, &compute_dst)?;
                    log::info!(
                        "Copied compute shader: {} -> {}",
                        compute_src.display(),
                        compute_dst.display()
                    );
                }
                particles.compute_shader = compute_new;
            }
            if let Some(ref mut rd) = particles.reaction_diffusion {
                if !rd.compute_shader.is_empty() {
                    let rd_new = format!("{snake}_rd.wgsl");
                    let rd_src = self.resolve_shader_path(&rd.compute_shader);
                    let rd_dst = shaders_dir.join(&rd_new);
                    if !rd_dst.exists() {
                        std::fs::copy(&rd_src, &rd_dst)?;
                        log::info!(
                            "Copied R-D shader: {} -> {}",
                            rd_src.display(),
                            rd_dst.display()
                        );
                    }
                    rd.compute_shader = rd_new;
                }
            }
        }

        let pfx_json = serde_json::to_string_pretty(&new_effect)?;
        std::fs::write(&new_pfx_path, pfx_json)?;
        log::info!(
            "Created effect copy: {} -> {}",
            effect.name,
            new_pfx_path.display()
        );

        Ok((new_pfx_path, first_wgsl))
    }
}

#[cfg(test)]
mod composite_decay_guard {
    //! The guard under `ParticleDef::composite_decay` (#2349).
    //!
    //! `composite_decay` restates, in the `.pfx`, a fact the background shader
    //! already encodes: which param holds the feedback retention `k`. That
    //! duplication is only safe while something checks it, so this walks every
    //! shipped effect and proves three-way agreement between the shader's
    //! `frame_decay(param(N u))`, the effect's `inputs[N].name`, and the
    //! declaration. Editing one side alone fails here.

    use crate::effect::format::PfxEffect;
    use crate::effect::loader::shipped_effects_for_test;
    use std::path::Path;

    fn shader_src(rel: &str) -> Option<String> {
        let p = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/shaders")
            .join(rel);
        std::fs::read_to_string(p).ok()
    }

    /// The expression handed to `frame_decay`/`frame_decay3`, resolved through a
    /// `let decay = ...;` binding when there is one. Returns the source text, not
    /// a value — the assertions below are about which param it names.
    fn decay_expr(src: &str) -> Option<String> {
        let at = src
            .find("frame_decay(")
            .or_else(|| src.find("frame_decay3("))?;
        let inner = &src[at..];
        let open = inner.find('(')?;
        let mut depth = 0usize;
        let mut end = None;
        for (i, c) in inner[open..].char_indices() {
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(open + i);
                        break;
                    }
                }
                _ => {}
            }
        }
        let arg = inner[open + 1..end?].trim().to_string();
        // `frame_decay(decay)` / `frame_decay3(vec3f(decay*0.97, ...))` both point
        // at a `let decay = ...` binding; resolve it so the param lookup sees the
        // real expression rather than the identifier.
        if arg.contains("decay") && !arg.contains("param(") {
            if let Some(b) = src.find("let decay = ") {
                let rest = &src[b + "let decay = ".len()..];
                if let Some(semi) = rest.find(';') {
                    return Some(rest[..semi].trim().to_string());
                }
            }
        }
        Some(arg)
    }

    fn param_index(expr: &str) -> Option<u32> {
        let at = expr.find("param(")?;
        let rest = &expr[at + 6..];
        let end = rest.find('u')?;
        rest[..end].trim().parse().ok()
    }

    /// The background pass shader carrying this effect's feedback decay, if any.
    fn feedback_bg(effect: &PfxEffect) -> Option<(String, String)> {
        effect.normalized_passes().into_iter().find_map(|p| {
            let src = shader_src(&p.shader)?;
            src.contains("frame_decay").then_some((p.shader, src))
        })
    }

    #[test]
    fn composite_decay_matches_shaders() {
        let effects = shipped_effects_for_test();
        let mut declared = 0usize;

        for e in &effects {
            let Some(particles) = e.particles.as_ref() else {
                continue;
            };
            let Some((shader, src)) = feedback_bg(e) else {
                assert!(
                    particles.composite_decay.is_none(),
                    "{}: declares composite_decay but no pass shader calls frame_decay — \
                     the declaration corrects a steady state that does not exist",
                    e.name,
                );
                continue;
            };

            // Only additive compositing has the independent source gain the
            // correction scales. See compute_raster_resolve.wgsl for the algebra
            // that rules out alpha (Morph, Raster) and wboit (Genesis).
            let additive = particles.blend == "additive";
            let Some(cd) = particles.composite_decay.as_ref() else {
                assert!(
                    !additive,
                    "{} ({shader}): additive particles over a frame_decay background, \
                     but no composite_decay — its source gain is still per-frame (#2349)",
                    e.name,
                );
                continue;
            };
            assert!(
                additive,
                "{} ({shader}): composite_decay declared on blend={:?}. The correction \
                 is only valid for additive compositing.",
                e.name, particles.blend,
            );
            declared += 1;

            let expr = decay_expr(&src)
                .unwrap_or_else(|| panic!("{} ({shader}): no frame_decay argument", e.name));

            match (cd.param, cd.constant) {
                (Some(idx), None) => {
                    let shader_idx = param_index(&expr).unwrap_or_else(|| {
                        panic!(
                            "{} ({shader}): declares param {idx}, shader reads no param: {expr}",
                            e.name
                        )
                    });
                    assert_eq!(
                        idx, shader_idx,
                        "{} ({shader}): declares param {idx}, shader decays by param({shader_idx}u): {expr}",
                        e.name,
                    );
                    let name = e
                        .inputs
                        .get(idx as usize)
                        .map(|p| p.name())
                        .unwrap_or_else(|| {
                            panic!("{}: composite_decay param {idx} is out of range", e.name)
                        });
                    assert_eq!(
                        cd.input, name,
                        "{}: composite_decay names {:?} but inputs[{idx}] is {name:?}",
                        e.name, cd.input,
                    );
                    // Params above 7 never reach the particle system: only
                    // effect_params[0..8] is forwarded.
                    assert!(
                        idx < 8,
                        "{}: param {idx} is not forwarded to particles",
                        e.name
                    );
                }
                (None, Some(_)) => assert!(
                    param_index(&expr).is_none(),
                    "{} ({shader}): declares a constant, shader reads a param: {expr}",
                    e.name,
                ),
                _ => panic!(
                    "{}: composite_decay needs exactly one of `param` or `constant`",
                    e.name
                ),
            }

            // A `mix(lo, hi, ...)` in the shader must be mirrored, or `k` is read
            // on the wrong scale entirely.
            let shader_remap = expr.starts_with("mix(");
            assert_eq!(
                shader_remap,
                cd.remap.is_some(),
                "{} ({shader}): remap declared={:?} but shader expression is: {expr}",
                e.name,
                cd.remap,
            );
            if let Some([lo, hi]) = cd.remap {
                assert!(
                    expr.contains(&format!("{lo}")) && expr.contains(&format!("{hi}")),
                    "{} ({shader}): declares remap [{lo}, {hi}], shader says: {expr}",
                    e.name,
                );
            }

            // Anything else modulating k must be named in `ignores`, so an
            // approximation is a recorded decision rather than an oversight.
            let audio_modulated =
                expr.contains("u.beat") || expr.contains("u.rms") || expr.contains("u.buildup");
            assert_eq!(
                audio_modulated,
                !cd.ignores.is_empty(),
                "{} ({shader}): audio-modulated={audio_modulated}, ignores={:?}. \
                 An untracked term must be declared.",
                e.name,
                cd.ignores,
            );
        }

        // A count, so deleting the declarations does not turn this into a
        // vacuous pass over zero effects.
        assert_eq!(
            declared, 14,
            "expected the 14 additive members of the feedback family to declare \
             composite_decay (Genesis is wboit, Morph and Raster are alpha)",
        );
    }

    /// `frame_gain` must exactly cancel the plateau error each effect actually has.
    ///
    /// Derived per effect from its own declaration and default slider rather than
    /// asserted against a fixed range: the steady state is `a/(1-k)`, so the error
    /// at frame time `dt` is `(1-k)/(1-k^n)` and the gain has to be its reciprocal.
    ///
    /// Note for anyone reconciling this against #2349's note, which quotes
    /// "0.505-0.588 at 30 fps and 1.837-1.990 at 120": that range does not
    /// reproduce from the shipped defaults. The real spread over the 14 declared
    /// effects is 0.526-0.625 and 1.775-1.949, because Cascade sits at k=0.60
    /// (plateau 0.625) and Array at k=0.70. The board figures are the right
    /// magnitude but were not derived from these defaults.
    #[test]
    fn frame_gain_cancels_each_effects_own_plateau_error() {
        use crate::gpu::particle::types::frame_gain;

        let effects = shipped_effects_for_test();
        let mut checked = 0usize;
        let (mut worst30, mut best30) = (f32::MAX, f32::MIN);

        for e in &effects {
            let Some(cd) = e
                .particles
                .as_ref()
                .and_then(|p| p.composite_decay.as_ref())
            else {
                continue;
            };
            // Defaults as shipped, in the slots the particle system is handed.
            let mut params = [0.0_f32; 8];
            for (i, slot) in params.iter_mut().enumerate() {
                *slot = match e.inputs.get(i) {
                    Some(crate::params::types::ParamDef::Float { default, .. }) => *default,
                    _ => 0.0,
                };
            }
            let k = cd.resolve(&params);

            for fps in [30.0_f32, 120.0] {
                let dt = 1.0 / fps;
                let n = (dt * 60.0).clamp(1e-4, 2.0);
                let plateau = (1.0 - k) / (1.0 - k.powf(n));
                let gain = frame_gain(1.0, k, dt);
                assert!(
                    (gain * plateau - 1.0).abs() < 1e-5,
                    "{} at {fps} fps (k={k}): gain {gain} does not cancel plateau {plateau}",
                    e.name,
                );
                if fps == 30.0 {
                    worst30 = worst30.min(plateau);
                    best30 = best30.max(plateau);
                }
            }
            checked += 1;
        }

        assert_eq!(checked, 14, "expected 14 declared effects");
        // Pin the magnitude so a declaration pointing at the wrong slider (which
        // would still self-consistently cancel) shows up as an implausible error.
        assert!(
            (0.52..=0.63).contains(&worst30) && (0.52..=0.63).contains(&best30),
            "30 fps plateau spread {worst30}..{best30} is not the ~0.53-0.63 measured \
             across the family — a declaration probably points at the wrong param",
        );
    }

    /// 60 fps must be untouched — every shipped look was authored there.
    #[test]
    fn frame_gain_is_identity_at_sixty() {
        use crate::gpu::particle::types::frame_gain;
        for k in [0.0_f32, 0.5, 0.82, 0.985, 1.0] {
            assert_eq!(
                frame_gain(1.0, k, 1.0 / 60.0),
                1.0,
                "k={k}: 60 fps must be bit-exact",
            );
        }
        // dt <= 0 means "no frame time recorded" -> behave as authored, and in
        // particular never 0.0, which would multiply every particle to black.
        assert_eq!(frame_gain(1.0, 0.82, 0.0), 1.0);
        assert_eq!(frame_gain(1.0, 0.82, -1.0), 1.0);
    }

    /// The bound of 2 normal frames is the lesson of the reverted 4b106dd
    /// (#1983): an unbounded exponent made one stalled frame a visible artefact.
    #[test]
    fn frame_gain_is_bounded_on_a_stall() {
        use crate::gpu::particle::types::frame_gain;
        let at_clamp = frame_gain(1.0, 0.82, 2.0 / 60.0);
        for dt in [0.05_f32, 0.2, 1.0, 10.0] {
            assert_eq!(
                frame_gain(1.0, 0.82, dt),
                at_clamp,
                "dt={dt}: must saturate at the 2-frame bound, not grow",
            );
        }
    }
}

#[cfg(test)]
mod tests;
