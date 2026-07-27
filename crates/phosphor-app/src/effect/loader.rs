use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::Result;

use super::format::PfxEffect;

/// Resolve the assets directory once (CWD-relative → exe-relative → macOS bundle).
pub fn assets_dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
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
];

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

pub struct EffectLoader {
    pub effects: Vec<PfxEffect>,
    pub current_effect: Option<usize>,
    lib_source: String,
    /// Particle library source (structs, bindings, helpers) prepended to compute shaders.
    particle_lib_source: String,
    /// Spatial hash grid dimensions, patched into particle_lib SH_GRID_W/H constants.
    /// Updated when a particle system with interaction is created.
    pub grid_dims: (u32, u32),
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
    /// Patches spatial hash grid constants (SH_GRID_W/H) to match current grid_dims.
    pub fn prepend_compute_libraries(&self, source: &str) -> String {
        let (w, h) = self.grid_dims;
        let patched_plib = self
            .particle_lib_source
            .replace(
                "const SH_GRID_W: u32 = 40u;",
                &format!("const SH_GRID_W: u32 = {w}u;"),
            )
            .replace(
                "const SH_GRID_H: u32 = 40u;",
                &format!("const SH_GRID_H: u32 = {h}u;"),
            );
        format!("{}\n{}\n{}", self.lib_source, patched_plib, source)
    }

    /// Prepend the uniform block and library functions to a shader source.
    pub fn prepend_library(&self, source: &str) -> String {
        // Check if the source already has the uniform block
        if source.contains("PhosphorUniforms") {
            // Already has uniforms, just prepend library
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
mod tests {
    use super::*;
    use crate::gpu::test_gpu::{gpu_guard, test_gpu};

    // Multi-pass graph (#1481): the injected WGSL declares one texture+sampler pair
    // and one accessor per input, at bindings 7+2i / 8+2i, and nothing when there
    // are no inputs.
    #[test]
    fn input_bindings_wgsl_shape() {
        assert!(build_input_bindings(0).is_empty());

        let two = build_input_bindings(2);
        // input0 at 7/8, input1 at 9/10, each with a raw texture, sampler, accessor.
        assert!(two.contains("@binding(7) var input0_tex: texture_2d<f32>;"));
        assert!(two.contains("@binding(8) var input0_sampler: sampler;"));
        assert!(two.contains("fn input0(uv: vec2f) -> vec4f"));
        assert!(two.contains("@binding(9) var input1_tex: texture_2d<f32>;"));
        assert!(two.contains("@binding(10) var input1_sampler: sampler;"));
        assert!(two.contains("fn input1(uv: vec2f) -> vec4f"));

        // The inputs-aware preamble carries the uniform block AND the input decls;
        // input_count 0 is byte-identical to the plain library preamble.
        let loader = EffectLoader::for_test("");
        let frag = "@fragment fn fs_main() -> @location(0) vec4f { return vec4f(0.0); }";
        assert_eq!(
            loader.prepend_library_with_inputs(frag, 0),
            loader.prepend_library(frag),
        );
        let with_one = loader.prepend_library_with_inputs(frag, 1);
        assert!(with_one.contains("PhosphorUniforms"));
        assert!(with_one.contains("fn input0(uv: vec2f) -> vec4f"));
    }

    fn make_effect(author: &str) -> PfxEffect {
        serde_json::from_str(&format!(
            r#"{{"name":"Test","author":"{}","shader":"test.wgsl"}}"#,
            author
        ))
        .unwrap()
    }

    #[test]
    fn is_builtin_true_for_fosfora_author() {
        assert!(EffectLoader::is_builtin(&make_effect("Fosfora")));
    }

    #[test]
    fn is_builtin_false_for_user_author() {
        assert!(!EffectLoader::is_builtin(&make_effect("User")));
        assert!(!EffectLoader::is_builtin(&make_effect("")));
    }

    #[test]
    fn prepend_library_without_uniforms() {
        let loader = EffectLoader::for_test("// lib code\n");
        let source = "fn main() {}";
        let result = loader.prepend_library(source);
        // Should contain UNIFORM_BLOCK, lib, and source
        assert!(result.contains("PhosphorUniforms"));
        assert!(result.contains("// lib code"));
        assert!(result.contains("fn main() {}"));
    }

    #[test]
    fn prepend_library_with_existing_uniforms() {
        let loader = EffectLoader::for_test("// lib code\n");
        let source = "struct PhosphorUniforms { time: f32 }\nfn main() {}";
        let result = loader.prepend_library(source);
        // Should NOT double-prepend UNIFORM_BLOCK
        let count = result.matches("PhosphorUniforms").count();
        assert_eq!(count, 1); // only the one in source
        assert!(result.contains("// lib code"));
    }

    // Discovery silently drops a .pfx that fails to deserialize
    // (scan_effects_directory warn-logs and moves on), so a schema typo in a
    // shipped builtin would just make it vanish from the browser. Parse the
    // real file in CI instead.
    #[test]
    fn tide_pfx_parses_as_builtin() {
        let effect: PfxEffect =
            serde_json::from_str(include_str!("../../../../assets/effects/tide.pfx"))
                .expect("tide.pfx must deserialize");
        assert!(EffectLoader::is_builtin(&effect));
        assert_eq!(effect.inputs.len(), 8); // exactly the 8 compute param slots
        let particles = effect.particles.expect("tide is a particle effect");
        assert_eq!(particles.render_mode, "billboard"); // trails need billboard
        assert!(particles.trail_length >= 2); // ribbon renderer enable gate
        assert!(particles.max_scaled_count <= 300_000); // quality scaler cap
    }

    // Generic offscreen preview for ANY particle effect, through the production
    // ParticleSystem — the repo previously had only splat- and frost-specific
    // probes, so a change to a shared sim helper had no cheap before/after.
    //
    // Renders the particle layer alone (no bg pass, no obstacle, no image
    // source), which is exactly the layer where spawn-distribution changes show.
    // Prints a per-render SIGNATURE (mean + quadrant means + alive count) so an
    // A/B can be diffed numerically instead of by eye.
    //
    // PARTICLE_PFX=vessel,tesla  selects effects (default: every particle .pfx)
    // PARTICLE_PNG_DIR=/path     where PNGs land (default /tmp)
    // Run: cargo test -p phosphor-app -- --ignored particle_effect_previews --nocapture
    #[test]
    #[ignore = "requires a GPU/software adapter; writes PNGs"]
    fn particle_effect_previews() {
        use crate::gpu::frame_capture::FrameCapture;
        use crate::gpu::particle::ParticleSystem;

        let out_dir = std::env::var("PARTICLE_PNG_DIR").unwrap_or_else(|_| "/tmp".to_string());
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");

        let noise = include_str!("../../../../assets/shaders/lib/noise.wgsl");
        let palette = include_str!("../../../../assets/shaders/lib/palette.wgsl");
        let sdf = include_str!("../../../../assets/shaders/lib/sdf.wgsl");
        let tonemap = include_str!("../../../../assets/shaders/lib/tonemap.wgsl");
        let chronoflow = include_str!("../../../../assets/shaders/lib/chronoflow.wgsl");
        let plib = include_str!("../../../../assets/shaders/lib/particle_lib.wgsl");
        let libs = format!("{noise}\n{palette}\n{sdf}\n{tonemap}\n{chronoflow}\n{plib}");

        let wanted: Option<Vec<String>> = std::env::var("PARTICLE_PFX")
            .ok()
            .map(|v| v.split(',').map(|s| s.trim().to_string()).collect());

        let mut names: Vec<String> = std::fs::read_dir(root.join("effects"))
            .expect("effects dir")
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "pfx"))
            .filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().to_string()))
            .collect();
        names.sort();
        if let Some(w) = &wanted {
            names.retain(|n| w.contains(n));
            assert!(!names.is_empty(), "PARTICLE_PFX matched no .pfx: {w:?}");
        }

        let _guard = gpu_guard();
        let (device, queue) = test_gpu();
        let (w, h) = (640u32, 360u32);
        let fmt = wgpu::TextureFormat::Rgba8UnormSrgb;

        // Silence matters here: Vessel's degenerate emit gate showed up as a
        // fountain that ran with no audio at all, so "idle" is a real probe.
        struct State {
            name: &'static str,
            rms: f32,
            onset_every: u32,
        }
        let states = [
            State {
                name: "idle",
                rms: 0.02,
                onset_every: 0,
            },
            State {
                name: "groove",
                rms: 0.55,
                onset_every: 15,
            },
        ];

        let frames = 120u32;
        let dt = 1.0 / 60.0;
        let mut rendered = 0usize;

        for name in &names {
            let json = std::fs::read_to_string(root.join("effects").join(format!("{name}.pfx")))
                .expect("read .pfx");
            let effect: PfxEffect = serde_json::from_str(&json).expect("parse .pfx");
            let Some(mut def) = effect.particles.clone() else {
                continue;
            };
            if def.compute_shader.is_empty() {
                continue;
            }
            // Splat needs an uploaded cloud to show anything — it has its own probe.
            if def.splat.is_some() {
                continue;
            }
            let sim = match std::fs::read_to_string(root.join("shaders").join(&def.compute_shader))
            {
                Ok(s) => s,
                Err(e) => panic!("{name}: missing sim {}: {e}", def.compute_shader),
            };
            // Probe-sized: the 2M-particle effects would dominate runtime and the
            // distribution artefacts are visible far below full count.
            def.max_count = def.max_count.min(200_000);
            def.max_scaled_count = 0;

            // Interaction effects read the spatial hash (group 3), whose grid
            // constants the production loader patches into particle_lib for the
            // actual particle count — mirror that or the pipeline layout mismatches.
            let sim_src = if def.interaction {
                let (gw, gh) =
                    crate::gpu::particle::spatial_hash::grid_dims(def.max_count, def.grid_max);
                let patched = libs
                    .replace(
                        "const SH_GRID_W: u32 = 40u;",
                        &format!("const SH_GRID_W: u32 = {gw}u;"),
                    )
                    .replace(
                        "const SH_GRID_H: u32 = 40u;",
                        &format!("const SH_GRID_H: u32 = {gh}u;"),
                    );
                format!("{patched}\n{sim}")
            } else {
                format!("{libs}\n{sim}")
            };

            // Param slots 0–7 from the .pfx defaults, so the probe renders each
            // effect as shipped rather than at an arbitrary setting.
            let mut params = [0.0f32; 8];
            for (i, p) in effect.inputs.iter().take(8).enumerate() {
                params[i] = match p {
                    crate::params::ParamDef::Float { default, .. } => *default,
                    crate::params::ParamDef::Bool { default: true, .. } => 1.0,
                    _ => 0.0,
                };
            }

            let mut ps = ParticleSystem::new(&device, &queue, fmt, &def, &sim_src, def.interaction);
            if def.trail_length >= 2 {
                ps.setup_trails(&device, fmt, def.trail_length, def.trail_width);
            }

            for s in &states {
                for f in 0..frames {
                    ps.poll_counter_readback();
                    ps.update_uniforms(dt, f as f32 * dt, [w as f32, h as f32], 0.0);
                    ps.uniforms.rms = s.rms;
                    ps.uniforms.centroid = 0.5;
                    ps.uniforms.onset = if s.onset_every > 0 && f % s.onset_every == 0 {
                        0.7
                    } else {
                        0.0
                    };
                    ps.uniforms.beat = if s.onset_every > 0 && f % s.onset_every == 0 {
                        1.0
                    } else {
                        0.0
                    };
                    ps.uniforms.buildup = if s.onset_every > 0 { 0.5 } else { 0.0 };
                    ps.uniforms.effect_params = params;

                    let is_last = f == frames - 1;
                    let mut fc =
                        is_last.then(|| FrameCapture::new(&device, w, h, fmt, "particle-capture"));

                    let mut enc = device.create_command_encoder(&Default::default());
                    ps.dispatch(&mut enc, &queue);
                    let target = fc.as_ref().map(|fc| &fc.view);
                    if let Some(view) = target {
                        {
                            let _pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                                label: Some("particle-preview-bg"),
                                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                    view,
                                    depth_slice: None,
                                    resolve_target: None,
                                    ops: wgpu::Operations {
                                        load: wgpu::LoadOp::Clear(wgpu::Color {
                                            r: 0.01,
                                            g: 0.01,
                                            b: 0.015,
                                            a: 1.0,
                                        }),
                                        store: wgpu::StoreOp::Store,
                                    },
                                })],
                                depth_stencil_attachment: None,
                                timestamp_writes: None,
                                occlusion_query_set: None,
                            });
                        }
                        ps.render(&mut enc, &queue, view);
                    }
                    if let Some(fc) = fc.as_ref() {
                        fc.copy_to_staging(&mut enc);
                    }
                    queue.submit([enc.finish()]);
                    ps.request_counter_readback();
                    ps.flip();

                    if let Some(fc) = fc.as_mut() {
                        device
                            .poll(wgpu::PollType::Wait {
                                submission_index: None,
                                timeout: None,
                            })
                            .unwrap();
                        fc.request_map();
                        let data = loop {
                            device
                                .poll(wgpu::PollType::Wait {
                                    submission_index: None,
                                    timeout: None,
                                })
                                .unwrap();
                            if let Some(d) = fc.take_mapped_data(&device) {
                                break d;
                            }
                        };
                        // Signature: overall mean plus quadrant means. A spawn
                        // distribution that shifts (clustered → uniform) moves the
                        // quadrant spread even when the overall mean barely budges.
                        let lum = |i: usize| {
                            (data[i] as f64 + data[i + 1] as f64 + data[i + 2] as f64) / 765.0
                        };
                        let mut q = [0f64; 4];
                        let mut qn = [0f64; 4];
                        for y in 0..h as usize {
                            for x in 0..w as usize {
                                let qi = (y >= h as usize / 2) as usize * 2
                                    + (x >= w as usize / 2) as usize;
                                q[qi] += lum((y * w as usize + x) * 4);
                                qn[qi] += 1.0;
                            }
                        }
                        for i in 0..4 {
                            q[i] /= qn[i];
                        }
                        let mean = (q[0] + q[1] + q[2] + q[3]) / 4.0;
                        let path = format!("{out_dir}/{name}_{}.png", s.name);
                        image::RgbaImage::from_raw(w, h, data)
                            .expect("raw->image")
                            .save(&path)
                            .expect("save png");
                        println!(
                            "SIG {name:<14} {:<7} mean={mean:.5} q=[{:.5} {:.5} {:.5} {:.5}]",
                            s.name, q[0], q[1], q[2], q[3]
                        );
                        rendered += 1;
                    }
                }
            }
        }

        assert!(rendered > 0, "no particle effects rendered");
        eprintln!("rendered {rendered} previews into {out_dir}");
    }

    // The f32-spacing half of the degenerate-hash finding, provable on the CPU:
    // decorrelated draws must NEVER be taken as hash(x), hash(x + 1.0), because
    // at realistic seeds the offset rounds away entirely and both calls return
    // the same number.
    #[test]
    fn float_offset_seeds_collapse_at_particle_scale() {
        // seed_base = u.seed + f32(idx) * 17.31, the phosphor/builtin convention,
        // at the 2,000,000 particles those effects actually ship.
        let seed_base = 30_000.0f32 + 2_000_000.0f32 * 17.31;
        assert!(
            seed_base > 33_554_432.0, // 2^25, where the f32 ULP reaches 4.0
            "test premise moved: seed_base = {seed_base}"
        );
        assert_eq!(
            seed_base + 1.0,
            seed_base,
            "the +1.0 offset must be shown to vanish — this is why rand_vec2 \
             returns x == y and why 5-draw emitters collapse to one value"
        );
        assert_eq!(seed_base + 2.0, seed_base);

        // The integer path keeps every draw distinct at the same scale.
        let idx = 2_000_000u32;
        let a = uhash_ref(idx);
        let b = uhash_ref(idx ^ 0x9e37_79b9);
        assert_ne!(a, b, "XOR-salted draws must stay independent");
    }

    // Mirror of the library's uhash, used by the statistical probes below. The
    // test immediately after this one asserts it has not drifted from the WGSL.
    fn uhash_ref(x: u32) -> u32 {
        let mut h = x;
        h ^= h >> 16;
        h = h.wrapping_mul(0x7feb_352d);
        h ^= h >> 15;
        h = h.wrapping_mul(0x846c_a68b);
        h ^= h >> 16;
        h
    }

    // Guards uhash_ref (and the inline copy in the GPU probe) against drifting
    // away from the shipped WGSL, which is the thing actually under test.
    #[test]
    fn particle_lib_exports_the_integer_hash() {
        let plib = include_str!("../../../../assets/shaders/lib/particle_lib.wgsl");
        for needle in [
            "fn uhash(x: u32) -> u32 {",
            "h = h ^ (h >> 16u);",
            "h = h * 0x7feb352du;",
            "h = h ^ (h >> 15u);",
            "h = h * 0x846ca68bu;",
            "fn uhash_f(x: u32) -> f32 {",
            "return f32(uhash(x)) / 4294967296.0;",
        ] {
            assert!(
                plib.contains(needle),
                "particle_lib.wgsl no longer contains `{needle}` — the integer \
                 hash moved or changed; update uhash_ref and the GPU probe"
            );
        }
        // The duplicated per-effect copies were folded into the library; a new
        // one creeping back in would be a redefinition error at load, but this
        // catches it at test time with a clearer message.
        let shaders = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/shaders");
        for sim in [
            "cleave_sim",
            "tide_sim",
            "ascend_sim",
            "panorama_sim",
            "splat_sim",
        ] {
            let src = std::fs::read_to_string(shaders.join(format!("{sim}.wgsl"))).unwrap();
            assert!(
                !src.contains("fn uhash(x: u32) -> u32 {"),
                "{sim}.wgsl redefines uhash — it comes from particle_lib now"
            );
        }
    }

    // Statistical probe on the REAL GPU: the integer hash must be uniform and
    // decorrelated over a contiguous index band at the magnitudes particle sims
    // actually reach. This is the guard on the fix.
    //
    // Deliberately one-sided: it does NOT assert that fract-sin misbehaves.
    // sin() accuracy is a driver/hardware property (lavapipe's is accurate,
    // the RTX fast path is not), so demanding the bug reproduce would be flaky.
    // The fract-sin numbers are printed for the record instead.
    // Run: cargo test -p phosphor-app -- --ignored integer_hash_is_uniform_at_particle_scale
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn integer_hash_is_uniform_at_particle_scale() {
        const N: u32 = 65536;
        const BASE: u32 = 1_000_000; // a contiguous band deep in a 2M-particle sim
        const SEED: f32 = 30_000.0; // u.seed is time*1000 % 65536

        let _guard = gpu_guard();
        let (device, queue) = test_gpu();

        let shader = r#"
struct Params { seed: f32, base: u32, pad0: u32, pad1: u32 };
@group(0) @binding(0) var<uniform> pr: Params;
@group(0) @binding(1) var<storage, read_write> out_sin: array<f32>;
@group(0) @binding(2) var<storage, read_write> out_int: array<f32>;

fn uhash(x: u32) -> u32 {
    var h = x;
    h = h ^ (h >> 16u);
    h = h * 0x7feb352du;
    h = h ^ (h >> 15u);
    h = h * 0x846ca68bu;
    h = h ^ (h >> 16u);
    return h;
}
fn uhash_f(x: u32) -> f32 { return f32(uhash(x)) / 4294967296.0; }
fn hash(n: f32) -> f32 { return fract(sin(n) * 43758.5453123); }

@compute @workgroup_size(64)
fn cs_main(@builtin(global_invocation_id) gid: vec3u) {
    let i = gid.x;
    if i >= arrayLength(&out_sin) { return; }
    let idx = pr.base + i;
    // The exact expression vessel_sim used before the fix.
    out_sin[i] = hash(pr.seed + f32(idx) * 3.7);
    out_int[i] = uhash_f(idx + uhash(u32(pr.seed * 256.0)));
}
"#;

        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("hash-probe"),
            source: wgpu::ShaderSource::Wgsl(shader.into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("hash-probe"),
            layout: None,
            module: &module,
            entry_point: Some("cs_main"),
            compilation_options: Default::default(),
            cache: None,
        });

        let bytes = (N as u64) * 4;
        let mk = || {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: bytes,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            })
        };
        let (buf_sin, buf_int) = (mk(), mk());
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut pbytes = [0u8; 16];
        pbytes[0..4].copy_from_slice(&SEED.to_ne_bytes());
        pbytes[4..8].copy_from_slice(&BASE.to_ne_bytes());
        queue.write_buffer(&params, 0, &pbytes);

        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: buf_sin.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: buf_int.as_entire_binding(),
                },
            ],
        });

        let mut enc =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: None,
                timestamp_writes: None,
            });
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bg, &[]);
            pass.dispatch_workgroups(N / 64, 1, 1);
        }
        let stage_sin = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: bytes,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let stage_int = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: bytes,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        enc.copy_buffer_to_buffer(&buf_sin, 0, &stage_sin, 0, bytes);
        enc.copy_buffer_to_buffer(&buf_int, 0, &stage_int, 0, bytes);
        queue.submit([enc.finish()]);

        let read = |b: &wgpu::Buffer| -> Vec<f32> {
            b.slice(..).map_async(wgpu::MapMode::Read, |_| {});
            device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: None,
                })
                .unwrap();
            let v = bytemuck::cast_slice::<u8, f32>(&b.slice(..).get_mapped_range()).to_vec();
            b.unmap();
            v
        };
        let sins = read(&stage_sin);
        let ints = read(&stage_int);

        let stats = |v: &[f32]| {
            let n = v.len() as f64;
            let mean = v.iter().map(|&x| x as f64).sum::<f64>() / n;
            let below_05 = v.iter().filter(|&&x| x < 0.05).count() as f64 / n;
            let tiny = v.iter().filter(|&&x| x < 1e-4).count() as f64 / n;
            (mean, below_05, tiny)
        };
        let (sm, sb, st) = stats(&sins);
        let (im, ib, it) = stats(&ints);
        eprintln!(
            "fract-sin: mean={sm:.4} p(<0.05)={sb:.4} p(<1e-4)={st:.6}\n\
             integer  : mean={im:.4} p(<0.05)={ib:.4} p(<1e-4)={it:.6}\n\
             (uniform expects 0.5 / 0.05 / 0.0001)"
        );

        // The integer hash must be uniform at this scale.
        assert!(
            (im - 0.5).abs() < 0.01,
            "integer hash mean should be 0.5, got {im}"
        );
        assert!(
            (ib - 0.05).abs() < 0.01,
            "integer hash should put 5% below 0.05, got {ib}"
        );
        assert!(
            it < 0.001,
            "integer hash near-zero tail should stay ~1e-4, got {it} — this is \
             the band that made gates fire unconditionally"
        );

        // ...and adjacent indices must be independent, since sims hash idx and
        // idx+1 for values that must not correlate.
        let pairs = ints.len() / 2;
        let corr = {
            let (mut sx, mut sy, mut sxy, mut sxx, mut syy) = (0f64, 0f64, 0f64, 0f64, 0f64);
            for i in 0..pairs {
                let (x, y) = (ints[i * 2] as f64, ints[i * 2 + 1] as f64);
                sx += x;
                sy += y;
                sxy += x * y;
                sxx += x * x;
                syy += y * y;
            }
            let n = pairs as f64;
            (sxy - sx * sy / n) / (((sxx - sx * sx / n) * (syy - sy * sy / n)).sqrt())
        };
        assert!(
            corr.abs() < 0.02,
            "adjacent indices should be uncorrelated, got r={corr}"
        );
    }

    // Sweep: every particle sim a shipped .pfx names, compiled through the
    // production compute concatenation. The per-effect probes below cover only
    // 8 effects, so a library change (a new helper, a renamed function) could
    // break the other dozen sims with nothing failing until launch —
    // all_effect_pass_shaders_compile deliberately skips compute sims.
    //
    // Auto-layout (`layout: None`) is what makes this cheap: pipeline creation
    // still forces full validation of bindings and the entry point without the
    // harness having to build any particle buffers.
    // Run: cargo test -p phosphor-app -- --ignored all_particle_sim_shaders_compile
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn all_particle_sim_shaders_compile() {
        let noise = include_str!("../../../../assets/shaders/lib/noise.wgsl");
        let palette = include_str!("../../../../assets/shaders/lib/palette.wgsl");
        let sdf = include_str!("../../../../assets/shaders/lib/sdf.wgsl");
        let tonemap = include_str!("../../../../assets/shaders/lib/tonemap.wgsl");
        let chronoflow = include_str!("../../../../assets/shaders/lib/chronoflow.wgsl");
        let plib = include_str!("../../../../assets/shaders/lib/particle_lib.wgsl");
        let libs = format!("{noise}\n{palette}\n{sdf}\n{tonemap}\n{chronoflow}\n{plib}");

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let mut entries: Vec<_> = std::fs::read_dir(root.join("effects"))
            .expect("effects dir")
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "pfx"))
            .collect();
        entries.sort();

        // Several effects share a sim (e.g. the lattice family); compile once each.
        let mut seen = std::collections::BTreeSet::new();
        for path in &entries {
            let json = std::fs::read_to_string(path).expect("read .pfx");
            let effect: PfxEffect = serde_json::from_str(&json)
                .unwrap_or_else(|e| panic!("{}: bad JSON: {e}", path.display()));
            if let Some(p) = &effect.particles {
                if !p.compute_shader.is_empty() {
                    seen.insert(p.compute_shader.clone());
                }
            }
        }
        assert!(
            seen.len() > 15,
            "suspiciously few particle sims found ({}) — did .pfx discovery change?",
            seen.len()
        );

        let _guard = gpu_guard();
        let (device, _queue) = test_gpu();

        let mut failures: Vec<String> = Vec::new();
        for rel in &seen {
            let src_path = root.join("shaders").join(rel);
            let src = match std::fs::read_to_string(&src_path) {
                Ok(s) => s,
                Err(e) => {
                    failures.push(format!("missing sim {}: {e}", src_path.display()));
                    continue;
                }
            };
            device.push_error_scope(wgpu::ErrorFilter::Validation);
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(rel),
                source: wgpu::ShaderSource::Wgsl(format!("{libs}\n{src}").into()),
            });
            let _ = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(rel),
                layout: None,
                module: &module,
                entry_point: Some("cs_main"),
                compilation_options: Default::default(),
                cache: None,
            });
            if let Some(err) = pollster::block_on(device.pop_error_scope()) {
                failures.push(format!("{rel}: {err:?}"));
            }
        }

        eprintln!("compiled {} particle sims", seen.len());
        assert!(
            failures.is_empty(),
            "particle sims failed to compile:\n{}",
            failures.join("\n")
        );
    }

    // Compile probe for the Tide sim + bg shaders through the production
    // concatenation (lib_source = noise + palette, then particle_lib for
    // compute). Catches WGSL errors without launching the app.
    // Run: cargo test -p phosphor-app -- --ignored tide_shaders_compile
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn tide_shaders_compile() {
        let noise = include_str!("../../../../assets/shaders/lib/noise.wgsl");
        let palette = include_str!("../../../../assets/shaders/lib/palette.wgsl");
        let plib = include_str!("../../../../assets/shaders/lib/particle_lib.wgsl");
        let sim = include_str!("../../../../assets/shaders/tide_sim.wgsl");
        let bg = include_str!("../../../../assets/shaders/tide_bg.wgsl");

        let _guard = gpu_guard();
        let (device, _queue) = test_gpu();

        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let sim_src = format!("{noise}\n{palette}\n{plib}\n{sim}");
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("tide-sim-probe"),
            source: wgpu::ShaderSource::Wgsl(sim_src.into()),
        });
        // Pipeline creation forces full validation (entry point, bindings).
        let _ = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("tide-sim-probe"),
            layout: None,
            module: &module,
            entry_point: Some("cs_main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let err = pollster::block_on(device.pop_error_scope());
        assert!(err.is_none(), "tide_sim.wgsl failed validation: {err:?}");

        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let bg_src = format!("{UNIFORM_BLOCK}\n{noise}\n{palette}\n{bg}");
        let _ = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("tide-bg-probe"),
            source: wgpu::ShaderSource::Wgsl(bg_src.into()),
        });
        let err = pollster::block_on(device.pop_error_scope());
        assert!(err.is_none(), "tide_bg.wgsl failed validation: {err:?}");
    }

    // Same guard as tide_pfx_parses_as_builtin: discovery silently drops a
    // .pfx that fails to deserialize, so parse the real file in CI.
    #[test]
    fn vessel_pfx_parses_as_builtin() {
        let effect: PfxEffect =
            serde_json::from_str(include_str!("../../../../assets/effects/vessel.pfx"))
                .expect("vessel.pfx must deserialize");
        assert!(EffectLoader::is_builtin(&effect));
        assert_eq!(effect.inputs.len(), 8); // exactly the 8 compute param slots
        let particles = effect.particles.expect("vessel is a particle effect");
        assert_eq!(particles.render_mode, "billboard"); // trails need billboard
        assert!(particles.trail_length >= 2); // ribbon renderer enable gate
        assert!(particles.max_scaled_count <= 300_000); // quality scaler cap
    }

    // Compile probe for the Vessel sim + bg shaders. Unlike Tide's probe this
    // includes the sdf lib in the sim concatenation — Vessel's fallback
    // amphora uses phosphor_sd_segment2 (production lib_source is
    // noise + palette + sdf + tonemap, see LIBRARY_FILES). Also a
    // pre-launch check that the WGSL ParticleUniforms mirror matches the
    // Rust layout (896 bytes since the #1800 ABI bump).
    // Run: cargo test -p phosphor-app -- --ignored vessel_shaders_compile
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn vessel_shaders_compile() {
        let noise = include_str!("../../../../assets/shaders/lib/noise.wgsl");
        let palette = include_str!("../../../../assets/shaders/lib/palette.wgsl");
        let sdf = include_str!("../../../../assets/shaders/lib/sdf.wgsl");
        let plib = include_str!("../../../../assets/shaders/lib/particle_lib.wgsl");
        let sim = include_str!("../../../../assets/shaders/vessel_sim.wgsl");
        let bg = include_str!("../../../../assets/shaders/vessel_bg.wgsl");

        let _guard = gpu_guard();
        let (device, _queue) = test_gpu();

        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let sim_src = format!("{noise}\n{palette}\n{sdf}\n{plib}\n{sim}");
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("vessel-sim-probe"),
            source: wgpu::ShaderSource::Wgsl(sim_src.into()),
        });
        // Pipeline creation forces full validation (entry point, bindings).
        let _ = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("vessel-sim-probe"),
            layout: None,
            module: &module,
            entry_point: Some("cs_main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let err = pollster::block_on(device.pop_error_scope());
        assert!(err.is_none(), "vessel_sim.wgsl failed validation: {err:?}");

        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let bg_src = format!("{UNIFORM_BLOCK}\n{noise}\n{palette}\n{bg}");
        let _ = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("vessel-bg-probe"),
            source: wgpu::ShaderSource::Wgsl(bg_src.into()),
        });
        let err = pollster::block_on(device.pop_error_scope());
        assert!(err.is_none(), "vessel_bg.wgsl failed validation: {err:?}");
    }

    // Same guard as tide_pfx_parses_as_builtin: discovery silently drops a
    // .pfx that fails to deserialize, so parse the real file in CI.
    #[test]
    fn cleave_pfx_parses_as_builtin() {
        let effect: PfxEffect =
            serde_json::from_str(include_str!("../../../../assets/effects/cleave.pfx"))
                .expect("cleave.pfx must deserialize");
        assert!(EffectLoader::is_builtin(&effect));
        assert_eq!(effect.inputs.len(), 8); // exactly the 8 compute param slots
        let particles = effect.particles.expect("cleave is a particle effect");
        assert_eq!(particles.render_mode, "billboard"); // trails need billboard
        assert!(particles.trail_length >= 2); // ribbon renderer enable gate
        assert!(particles.max_scaled_count <= 300_000); // quality scaler cap
    }

    // Compile probe for the Cleave sim + bg shaders (no sdf lib — Cleave uses
    // no SDF helpers). Also validates the two-cohort sim's atomicAdd on
    // counters[3] (the shard emission sub-budget) against the particle_lib
    // binding layout.
    // Run: cargo test -p phosphor-app -- --ignored cleave_shaders_compile
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn cleave_shaders_compile() {
        let noise = include_str!("../../../../assets/shaders/lib/noise.wgsl");
        let palette = include_str!("../../../../assets/shaders/lib/palette.wgsl");
        let plib = include_str!("../../../../assets/shaders/lib/particle_lib.wgsl");
        let sim = include_str!("../../../../assets/shaders/cleave_sim.wgsl");
        let bg = include_str!("../../../../assets/shaders/cleave_bg.wgsl");

        let _guard = gpu_guard();
        let (device, _queue) = test_gpu();

        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let sim_src = format!("{noise}\n{palette}\n{plib}\n{sim}");
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("cleave-sim-probe"),
            source: wgpu::ShaderSource::Wgsl(sim_src.into()),
        });
        // Pipeline creation forces full validation (entry point, bindings).
        let _ = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("cleave-sim-probe"),
            layout: None,
            module: &module,
            entry_point: Some("cs_main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let err = pollster::block_on(device.pop_error_scope());
        assert!(err.is_none(), "cleave_sim.wgsl failed validation: {err:?}");

        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let bg_src = format!("{UNIFORM_BLOCK}\n{noise}\n{palette}\n{bg}");
        let _ = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("cleave-bg-probe"),
            source: wgpu::ShaderSource::Wgsl(bg_src.into()),
        });
        let err = pollster::block_on(device.pop_error_scope());
        assert!(err.is_none(), "cleave_bg.wgsl failed validation: {err:?}");
    }

    // Helix is a volume effect like Lattice: no compute shader, so
    // `particle_effect_previews` skips it and `gpu::helix::helix_render_previews`
    // is its probe. What still has to hold here is that the .pfx loads as a
    // builtin and its background pass exists — every particle effect needs at
    // least one pass, and Helix's whole render is the ray marcher compositing
    // over it. Plus the #1855 injection-trap assert.
    #[test]
    fn helix_pfx_parses_as_builtin() {
        let effect: PfxEffect =
            serde_json::from_str(include_str!("../../../../assets/effects/helix.pfx"))
                .expect("helix.pfx must deserialize");
        assert!(EffectLoader::is_builtin(&effect));
        assert_eq!(effect.passes.len(), 1, "Helix needs its background pass");
        let particles = effect.particles.expect("helix is a particle effect");
        assert!(
            particles.helix.is_some(),
            "the helix def block is what turns the effect on"
        );
        assert!(
            particles.compute_shader.is_empty(),
            "Helix renders a volume, not particles — a sim shader would be dead weight"
        );
        let bg = include_str!("../../../../assets/shaders/helix_bg.wgsl");
        assert!(
            !bg.contains("PhosphorUniforms"),
            "helix_bg.wgsl must not mention the uniform struct name — it suppresses injection"
        );
    }

    // Compile probe for the Helix background pass through the production
    // concatenation. `helix_bg.wgsl` reads `u.resolution`, so it only compiles if
    // the uniform block is actually injected — this is what would catch the #1855
    // trap turning the backdrop into a load error at runtime.
    // Run: cargo test -p phosphor-app -- --ignored helix_shaders_compile
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn helix_shaders_compile() {
        let noise = include_str!("../../../../assets/shaders/lib/noise.wgsl");
        let palette = include_str!("../../../../assets/shaders/lib/palette.wgsl");
        let bg = include_str!("../../../../assets/shaders/helix_bg.wgsl");

        let _guard = gpu_guard();
        let (device, _queue) = test_gpu();

        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let src = format!("{UNIFORM_BLOCK}\n{noise}\n{palette}\n{bg}");
        let _ = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("helix-bg-probe"),
            source: wgpu::ShaderSource::Wgsl(src.into()),
        });
        let err = pollster::block_on(device.pop_error_scope());
        assert!(err.is_none(), "helix_bg.wgsl failed validation: {err:?}");
    }

    // Same guard as tide_pfx_parses_as_builtin, plus the uniform-injection
    // trap: prepend_library suppresses UNIFORM_BLOCK if the shader source
    // contains the struct name anywhere (even a comment), and the compile
    // probe below concatenates the block unconditionally so it can't catch
    // that — assert the string is absent here instead.
    #[test]
    fn frost_pfx_parses_as_builtin() {
        let effect: PfxEffect =
            serde_json::from_str(include_str!("../../../../assets/effects/frost.pfx"))
                .expect("frost.pfx must deserialize");
        assert!(EffectLoader::is_builtin(&effect));
        assert_eq!(effect.inputs.len(), 9); // 8 floats + drift Point2D = 10 slots
        assert!(effect.particles.is_none()); // pure fragment + feedback effect
        let shader = include_str!("../../../../assets/shaders/frost.wgsl");
        assert!(
            !shader.contains("PhosphorUniforms"),
            "frost.wgsl must not mention the uniform struct name — it suppresses injection"
        );
    }

    // Compile probe for the Frost fragment shader through the production
    // concatenation (UNIFORM_BLOCK + noise + palette). Fragment-only effect,
    // so no compute-pipeline step.
    // Run: cargo test -p phosphor-app -- --ignored frost_shaders_compile
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn frost_shaders_compile() {
        let noise = include_str!("../../../../assets/shaders/lib/noise.wgsl");
        let palette = include_str!("../../../../assets/shaders/lib/palette.wgsl");
        let frost = include_str!("../../../../assets/shaders/frost.wgsl");

        let _guard = gpu_guard();
        let (device, _queue) = test_gpu();

        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let src = format!("{UNIFORM_BLOCK}\n{noise}\n{palette}\n{frost}");
        let _ = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("frost-probe"),
            source: wgpu::ShaderSource::Wgsl(src.into()),
        });
        let err = pollster::block_on(device.pop_error_scope());
        assert!(err.is_none(), "frost.wgsl failed validation: {err:?}");
    }

    // Offscreen render probe for Frost's two material states: run the real
    // fragment pipeline with synthetic audio uniforms (tonal vs noisy) through
    // 90 feedback frames and capture PNGs. Guards against a black screen or a
    // feedback blowout and asserts the crystal and sand states actually differ.
    // Run: FROST_PNG_DIR=/path cargo test -p phosphor-app -- --ignored frost_render_previews
    #[test]
    #[ignore = "requires a GPU/software adapter; writes PNGs"]
    fn frost_render_previews() {
        use crate::gpu::frame_capture::FrameCapture;
        use crate::gpu::pipeline::ShaderPipeline;
        use crate::gpu::uniforms::{ShaderUniforms, UniformBuffer};

        let out_dir = std::env::var("FROST_PNG_DIR").unwrap_or_else(|_| "/tmp".to_string());
        let _guard = gpu_guard();
        let (device, queue) = test_gpu();

        // Production concatenation: uniform block + libs + effect fragment.
        let noise = include_str!("../../../../assets/shaders/lib/noise.wgsl");
        let palette = include_str!("../../../../assets/shaders/lib/palette.wgsl");
        let frost = include_str!("../../../../assets/shaders/frost.wgsl");
        let fragment_source = format!("{UNIFORM_BLOCK}\n{noise}\n{palette}\n{frost}");

        let (w, h) = (960u32, 540u32);
        let fmt = wgpu::TextureFormat::Rgba8UnormSrgb;
        let pipeline =
            ShaderPipeline::new(&device, fmt, &fragment_source, None, 0).expect("frost pipeline");

        // Ping-pong pair for the feedback loop.
        let mk_target = |label: &str| {
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: fmt,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = tex.create_view(&Default::default());
            (tex, view)
        };
        let targets = [mk_target("frost-ping"), mk_target("frost-pong")];

        // 1x1 placeholder audio textures matching the production bindings.
        let mk_audio = |label: &str, format: wgpu::TextureFormat| {
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            tex.create_view(&Default::default())
        };
        let waveform = mk_audio("frost-waveform", wgpu::TextureFormat::Rg16Float);
        let spectrum = mk_audio("frost-spectrum", wgpu::TextureFormat::R16Float);
        let spectrogram = mk_audio("frost-spectrogram", wgpu::TextureFormat::R8Unorm);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let ubuf = UniformBuffer::new(&device);
        let bind_groups: Vec<_> = targets
            .iter()
            .map(|(_, view)| {
                ubuf.create_bind_group(
                    &device,
                    &pipeline.bind_group_layout,
                    view,
                    &sampler,
                    &waveform,
                    &spectrum,
                    &spectrogram,
                    &sampler,
                    &[],
                )
            })
            .collect();

        struct ProbeState {
            name: &'static str,
            flatness: f32,
            zcr: f32,
            bandwidth: f32,
            bass: f32,
            centroid: f32,
            rms: f32,
            /// Onset / kick applied on the final (captured) frame only.
            onset: f32,
            kick: f32,
        }
        let state = |name, flatness, zcr, bandwidth, bass, centroid, rms, onset, kick| ProbeState {
            name,
            flatness,
            zcr,
            bandwidth,
            bass,
            centroid,
            rms,
            onset,
            kick,
        };
        let states = [
            state("crystal", 0.05, 0.02, 0.20, 0.30, 0.60, 0.40, 0.0, 0.0),
            state("mid", 0.50, 0.20, 0.45, 0.35, 0.50, 0.45, 0.0, 0.0),
            state("sand", 0.92, 0.45, 0.70, 0.40, 0.40, 0.50, 0.0, 0.0),
            state(
                "crystal_shatter",
                0.05,
                0.02,
                0.20,
                0.30,
                0.60,
                0.40,
                1.0,
                0.8,
            ),
        ];
        let frames = 90u32;
        let dt = 1.0 / 60.0;
        let mut means = std::collections::HashMap::new();

        for s in states {
            let name = s.name;
            let mut u = ShaderUniforms::zeroed();
            u.resolution = [w as f32, h as f32];
            u.delta_time = dt;
            u.feedback_decay = 0.88;
            u.params = [
                0.5, 0.0, 0.5, 0.5, 0.6, 0.6, 0.5, 1.0, 0.0, -0.6, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
            ];
            u.flatness = s.flatness;
            u.zcr = s.zcr;
            u.bandwidth = s.bandwidth;
            u.sub_bass = s.bass * 0.7;
            u.bass = s.bass;
            u.centroid = s.centroid;
            u.rms = s.rms;

            let mut src = 0usize;
            for f in 0..frames {
                u.time = f as f32 * dt;
                u.frame_index = f as f32;
                u.beat_phase = (u.time * 2.0).fract();
                if f == frames - 1 {
                    u.onset = s.onset;
                    u.kick = s.kick;
                }
                ubuf.update(&queue, &u);
                let dst = 1 - src;
                let mut enc = device.create_command_encoder(&Default::default());
                {
                    let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("frost-preview-pass"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &targets[dst].1,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                    });
                    pass.set_pipeline(&pipeline.pipeline);
                    pass.set_bind_group(0, &bind_groups[src], &[]);
                    pass.draw(0..3, 0..1);
                }
                queue.submit([enc.finish()]);
                src = dst;
            }

            // Re-render the final frame into the capture target and read it back.
            let mut fc = FrameCapture::new(&device, w, h, fmt, "frost-capture");
            let mut enc = device.create_command_encoder(&Default::default());
            {
                let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("frost-capture-pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &fc.view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                pass.set_pipeline(&pipeline.pipeline);
                pass.set_bind_group(0, &bind_groups[1 - src], &[]);
                pass.draw(0..3, 0..1);
            }
            fc.copy_to_staging(&mut enc);
            queue.submit([enc.finish()]);
            device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: None,
                })
                .unwrap();
            fc.request_map();
            let data = loop {
                device
                    .poll(wgpu::PollType::Wait {
                        submission_index: None,
                        timeout: None,
                    })
                    .unwrap();
                if let Some(d) = fc.take_mapped_data(&device) {
                    break d;
                }
            };

            let mean = data.iter().map(|&b| b as f64).sum::<f64>() / (data.len() as f64 * 255.0);
            means.insert(name, mean);
            let path = format!("{out_dir}/frost_{name}.png");
            image::RgbaImage::from_raw(w, h, data)
                .expect("raw->image")
                .save(&path)
                .expect("save png");
            eprintln!("wrote {path} (mean {mean:.4})");

            // Not black, not blown out.
            assert!(mean > 0.005, "{name} rendered near-black (mean {mean:.4})");
            assert!(mean < 0.90, "{name} blew out (mean {mean:.4})");
        }

        // The two material states must actually look different.
        let diff = (means["crystal"] - means["sand"]).abs();
        assert!(
            diff > 0.01,
            "crystal and sand states are indistinguishable (means {:?})",
            means
        );
    }

    // Same guard as frost_pfx_parses_as_builtin: discovery silently drops a .pfx
    // that fails to deserialize, and the #1855 injection trap suppresses the
    // uniform block if the shader mentions the struct name anywhere.
    #[test]
    fn chromatica_pfx_parses_as_builtin() {
        let effect: PfxEffect =
            serde_json::from_str(include_str!("../../../../assets/effects/chromatica.pfx"))
                .expect("chromatica.pfx must deserialize");
        assert!(EffectLoader::is_builtin(&effect));
        assert_eq!(effect.inputs.len(), 12); // 12 float params
        assert!(effect.particles.is_none()); // pure fragment + feedback effect
        let shader = include_str!("../../../../assets/shaders/chromatica.wgsl");
        assert!(
            !shader.contains("PhosphorUniforms"),
            "chromatica.wgsl must not mention the uniform struct name — it suppresses injection"
        );
    }

    // Compile probe for the Chromatica fragment shader through the production
    // concatenation. It uses phosphor_sd_segment2, so sdf.wgsl must be in the
    // concat (production prepends it via LIB_FILENAMES; the compile probe must too).
    // Run: cargo test -p phosphor-app -- --ignored chromatica_shaders_compile
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn chromatica_shaders_compile() {
        let noise = include_str!("../../../../assets/shaders/lib/noise.wgsl");
        let palette = include_str!("../../../../assets/shaders/lib/palette.wgsl");
        let sdf = include_str!("../../../../assets/shaders/lib/sdf.wgsl");
        let chromatica = include_str!("../../../../assets/shaders/chromatica.wgsl");

        let _guard = gpu_guard();
        let (device, _queue) = test_gpu();

        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let src = format!("{UNIFORM_BLOCK}\n{noise}\n{palette}\n{sdf}\n{chromatica}");
        let _ = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("chromatica-probe"),
            source: wgpu::ShaderSource::Wgsl(src.into()),
        });
        let err = pollster::block_on(device.pop_error_scope());
        assert!(err.is_none(), "chromatica.wgsl failed validation: {err:?}");
    }

    // Same silent-drop guard, for the first true multi-pass pass-graph effect (#1481).
    // Also pins the two pieces of infra the solver depends on: the pressure pass's
    // Jacobi `iterations` and the cross-pass `prev_inputs` edge that cuts the
    // velocity→divergence→pressure→velocity cycle.
    #[test]
    fn sumi_pfx_parses_as_builtin() {
        let effect: PfxEffect =
            serde_json::from_str(include_str!("../../../../assets/effects/sumi.pfx"))
                .expect("sumi.pfx must deserialize");
        assert!(EffectLoader::is_builtin(&effect));
        assert_eq!(effect.inputs.len(), 10); // 10 float param sliders
        assert!(effect.particles.is_none()); // pure fragment pass-graph effect
        let passes = effect.normalized_passes();
        assert_eq!(passes.len(), 4);
        let pressure = passes.iter().find(|p| p.name == "pressure").unwrap();
        assert_eq!(pressure.iterations, 24);
        assert!(pressure.feedback);
        let divergence = passes.iter().find(|p| p.name == "divergence").unwrap();
        assert!(!divergence.feedback);
        assert_eq!(divergence.prev_inputs, vec!["velocity".to_string()]);
        let velocity = passes.iter().find(|p| p.name == "velocity").unwrap();
        assert_eq!(velocity.inputs, vec!["pressure".to_string()]);
        assert_eq!(velocity.prev_inputs, vec!["dye".to_string()]);

        // Mirror PassExecutor::new's resolver so a mistyped pass name in the .pfx fails
        // in CI, not only in the ignored GPU probe: every `inputs` entry must name an
        // EARLIER pass; every `prev_inputs` entry must name some FEEDBACK pass.
        for (idx, pass) in passes.iter().enumerate() {
            for name in &pass.inputs {
                assert!(
                    passes[..idx].iter().any(|p| &p.name == name),
                    "pass '{}' input '{name}' must name an earlier pass",
                    pass.name
                );
            }
            for name in &pass.prev_inputs {
                assert!(
                    passes.iter().any(|p| &p.name == name && p.feedback),
                    "pass '{}' prev_input '{name}' must name a feedback pass",
                    pass.name
                );
            }
        }
        // None of the pass shaders may mention the uniform struct name (#1855 trap).
        for shader in [
            include_str!("../../../../assets/shaders/sumi_divergence.wgsl"),
            include_str!("../../../../assets/shaders/sumi_pressure.wgsl"),
            include_str!("../../../../assets/shaders/sumi_velocity.wgsl"),
            include_str!("../../../../assets/shaders/sumi_dye.wgsl"),
        ] {
            assert!(
                !shader.contains("PhosphorUniforms"),
                "a Sumi shader mentions the uniform struct name — it suppresses injection"
            );
        }
    }

    // Compile probe for all four Sumi pass shaders through the real pipeline path
    // (ShaderPipeline::new = reflection layout + render-pipeline creation), each with
    // its production input_count so the injected input0/input1 bindings validate.
    // Run: cargo test -p phosphor-app -- --ignored sumi_shaders_compile
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn sumi_shaders_compile() {
        use crate::gpu::pipeline::ShaderPipeline;

        let noise = include_str!("../../../../assets/shaders/lib/noise.wgsl");
        let palette = include_str!("../../../../assets/shaders/lib/palette.wgsl");
        let sdf = include_str!("../../../../assets/shaders/lib/sdf.wgsl");
        let tonemap = include_str!("../../../../assets/shaders/lib/tonemap.wgsl");
        let libs = format!("{noise}\n{palette}\n{sdf}\n{tonemap}");
        let loader = EffectLoader::for_test(&libs);

        let _guard = gpu_guard();
        let (device, _queue) = test_gpu();
        let fmt = wgpu::TextureFormat::Rgba16Float;

        // (name, source, input_count) — velocity reads pressure + prev dye (2).
        let cases = [
            (
                "sumi_divergence",
                include_str!("../../../../assets/shaders/sumi_divergence.wgsl"),
                1usize,
            ),
            (
                "sumi_pressure",
                include_str!("../../../../assets/shaders/sumi_pressure.wgsl"),
                1,
            ),
            (
                "sumi_velocity",
                include_str!("../../../../assets/shaders/sumi_velocity.wgsl"),
                2,
            ),
            (
                "sumi_dye",
                include_str!("../../../../assets/shaders/sumi_dye.wgsl"),
                1,
            ),
        ];
        for (name, shader, count) in cases {
            let src = loader.prepend_library_with_inputs(shader, count);
            ShaderPipeline::new(&device, fmt, &src, None, count)
                .unwrap_or_else(|e| panic!("{name}.wgsl failed to compile: {e}"));
        }
    }

    // Offscreen render probe: drive the real fragment pipeline with synthetic
    // chroma/key uniforms (C major, A minor, silence, edges-off) through feedback
    // frames and capture PNGs. Guards against a black screen or a feedback blowout,
    // and asserts a lit chord is brighter than silence and the consonance edges add light.
    // Run: CHROMATICA_PNG_DIR=/path cargo test -p phosphor-app --release -- --ignored chromatica_render_previews
    #[test]
    #[ignore = "requires a GPU/software adapter; writes PNGs"]
    fn chromatica_render_previews() {
        use crate::gpu::frame_capture::FrameCapture;
        use crate::gpu::pipeline::ShaderPipeline;
        use crate::gpu::uniforms::{ShaderUniforms, UniformBuffer};

        let out_dir = std::env::var("CHROMATICA_PNG_DIR").unwrap_or_else(|_| "/tmp".to_string());
        let _guard = gpu_guard();
        let (device, queue) = test_gpu();

        // Production concatenation: uniform block + libs (incl. sdf) + effect fragment.
        let noise = include_str!("../../../../assets/shaders/lib/noise.wgsl");
        let palette = include_str!("../../../../assets/shaders/lib/palette.wgsl");
        let sdf = include_str!("../../../../assets/shaders/lib/sdf.wgsl");
        let chromatica = include_str!("../../../../assets/shaders/chromatica.wgsl");
        let fragment_source = format!("{UNIFORM_BLOCK}\n{noise}\n{palette}\n{sdf}\n{chromatica}");

        let (w, h) = (960u32, 540u32);
        let fmt = wgpu::TextureFormat::Rgba8UnormSrgb;
        let pipeline = ShaderPipeline::new(&device, fmt, &fragment_source, None, 0)
            .expect("chromatica pipeline");

        // Ping-pong pair for the feedback loop.
        let mk_target = |label: &str| {
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: fmt,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = tex.create_view(&Default::default());
            (tex, view)
        };
        let targets = [mk_target("chroma-ping"), mk_target("chroma-pong")];

        // 1x1 placeholder audio textures matching the production bindings.
        let mk_audio = |label: &str, format: wgpu::TextureFormat| {
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            tex.create_view(&Default::default())
        };
        let waveform = mk_audio("chroma-waveform", wgpu::TextureFormat::Rg16Float);
        let spectrum = mk_audio("chroma-spectrum", wgpu::TextureFormat::R16Float);
        let spectrogram = mk_audio("chroma-spectrogram", wgpu::TextureFormat::R8Unorm);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let ubuf = UniformBuffer::new(&device);
        let bind_groups: Vec<_> = targets
            .iter()
            .map(|(_, view)| {
                ubuf.create_bind_group(
                    &device,
                    &pipeline.bind_group_layout,
                    view,
                    &sampler,
                    &waveform,
                    &spectrum,
                    &spectrogram,
                    &sampler,
                    &[],
                )
            })
            .collect();

        struct ProbeState {
            name: &'static str,
            chroma: [f32; 12],
            key_class: f32,
            key_is_minor: f32,
            key_confidence: f32,
            dominant_chroma: f32,
            edges: f32,
        }
        // Chord chroma vectors (C=0 .. B=11).
        let c_major = {
            let mut c = [0.0f32; 12];
            c[0] = 1.0; // C
            c[4] = 0.85; // E
            c[7] = 0.9; // G
            c
        };
        let a_minor = {
            let mut c = [0.0f32; 12];
            c[9] = 1.0; // A
            c[0] = 0.85; // C
            c[4] = 0.8; // E
            c
        };
        let states = [
            ProbeState {
                name: "c_major",
                chroma: c_major,
                key_class: 0.0,
                key_is_minor: 0.0,
                key_confidence: 0.9,
                dominant_chroma: 0.0,
                edges: 1.0,
            },
            ProbeState {
                name: "a_minor",
                chroma: a_minor,
                key_class: 9.0 / 11.0,
                key_is_minor: 1.0,
                key_confidence: 0.85,
                dominant_chroma: 9.0 / 11.0,
                edges: 1.0,
            },
            ProbeState {
                name: "no_edges",
                chroma: c_major,
                key_class: 0.0,
                key_is_minor: 0.0,
                key_confidence: 0.9,
                dominant_chroma: 0.0,
                edges: 0.0,
            },
            ProbeState {
                name: "silence",
                chroma: [0.0; 12],
                key_class: 0.0,
                key_is_minor: 0.0,
                key_confidence: 0.0,
                dominant_chroma: 0.0,
                edges: 1.0,
            },
        ];
        let frames = 60u32;
        let dt = 1.0 / 60.0;
        let mut means = std::collections::HashMap::new();

        for s in states {
            let name = s.name;
            let mut u = ShaderUniforms::zeroed();
            u.resolution = [w as f32, h as f32];
            u.delta_time = dt;
            u.feedback_decay = 0.88;
            // ring_spacing, bloom_gain, arc_thickness, rotation_speed, palette_shift,
            // minor_droop, feedback_amount, interval_edges, consonance_gain, glow,
            // edge_spin, edge_breath
            u.params = [
                0.4, 0.6, 0.5, 0.4, 0.0, 0.6, 0.5, s.edges, 0.6, 0.6, 0.65, 0.5, 0.0, 0.0, 0.0, 0.0,
            ];
            u.chroma = s.chroma;
            u.key_class = s.key_class;
            u.key_is_minor = s.key_is_minor;
            u.key_confidence = s.key_confidence;
            u.dominant_chroma = s.dominant_chroma;
            u.bandwidth = 0.4;

            let mut src = 0usize;
            for f in 0..frames {
                u.time = f as f32 * dt;
                u.frame_index = f as f32;
                u.beat_phase = (u.time * 2.0).fract();
                ubuf.update(&queue, &u);
                let dst = 1 - src;
                let mut enc = device.create_command_encoder(&Default::default());
                {
                    let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("chroma-preview-pass"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &targets[dst].1,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                    });
                    pass.set_pipeline(&pipeline.pipeline);
                    pass.set_bind_group(0, &bind_groups[src], &[]);
                    pass.draw(0..3, 0..1);
                }
                queue.submit([enc.finish()]);
                src = dst;
            }

            // Re-render the final frame into the capture target and read it back.
            let mut fc = FrameCapture::new(&device, w, h, fmt, "chroma-capture");
            let mut enc = device.create_command_encoder(&Default::default());
            {
                let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("chroma-capture-pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &fc.view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                pass.set_pipeline(&pipeline.pipeline);
                pass.set_bind_group(0, &bind_groups[1 - src], &[]);
                pass.draw(0..3, 0..1);
            }
            fc.copy_to_staging(&mut enc);
            queue.submit([enc.finish()]);
            device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: None,
                })
                .unwrap();
            fc.request_map();
            let data = loop {
                device
                    .poll(wgpu::PollType::Wait {
                        submission_index: None,
                        timeout: None,
                    })
                    .unwrap();
                if let Some(d) = fc.take_mapped_data(&device) {
                    break d;
                }
            };

            let mean = data.iter().map(|&b| b as f64).sum::<f64>() / (data.len() as f64 * 255.0);
            means.insert(name, mean);
            let path = format!("{out_dir}/chromatica_{name}.png");
            image::RgbaImage::from_raw(w, h, data)
                .expect("raw->image")
                .save(&path)
                .expect("save png");
            eprintln!("wrote {path} (mean {mean:.4})");

            // Not blown out.
            assert!(mean < 0.90, "{name} blew out (mean {mean:.4})");
        }

        // A lit chord is not black.
        assert!(means["c_major"] > 0.005, "c_major near-black");
        assert!(means["a_minor"] > 0.005, "a_minor near-black");
        // A chord is brighter than silence (the rings bloom).
        assert!(
            means["c_major"] > means["silence"] + 0.002,
            "chord not brighter than silence (means {means:?})"
        );
        // The consonance edges add light: edges-on is brighter than edges-off.
        assert!(
            means["c_major"] > means["no_edges"],
            "consonance edges added no light (means {means:?})"
        );
    }

    // Same guard as tide_pfx_parses_as_builtin, plus the frozen Splat param
    // ABI: the sim reads slots 0–7 by index and the CPU driver reads 8–11
    // (app.rs forwards them into splat_ui_params), so count and order are
    // load-bearing. Also the #1855 injection-trap assert for the bg shader.
    #[test]
    fn splat_pfx_parses_as_builtin() {
        let effect: PfxEffect =
            serde_json::from_str(include_str!("../../../../assets/effects/splat.pfx"))
                .expect("splat.pfx must deserialize");
        assert!(EffectLoader::is_builtin(&effect));
        assert_eq!(effect.inputs.len(), 13); // 8 sim slots + 4 CPU camera slots + roundness
        let particles = effect.particles.expect("splat is a particle effect");
        assert_eq!(particles.render_mode, "compute"); // splats need the raster
        assert_eq!(particles.blend, "oit"); // weighted-average OIT resolve
        assert!(particles.trail_length < 2); // trails share group 2 — forbidden
        let splat = particles.splat.expect("splat def block required");
        assert!(splat.source.starts_with("demo:"));
        assert!(particles.max_scaled_count <= 3_000_000); // go/no-go budget
        assert!((particles.emit_rate - 0.0).abs() < f32::EPSILON); // persistent slots, no emission
        let bg = include_str!("../../../../assets/shaders/splat_bg.wgsl");
        assert!(
            !bg.contains("PhosphorUniforms"),
            "splat_bg.wgsl must not mention the uniform struct name — it suppresses injection"
        );
    }

    // Compile probe for the Splat sim + bg through the production
    // concatenation. The sim declares its own @group(2) @binding(1) splat
    // static buffer next to the lib's unconditional @group(2) @binding(0)
    // trail declaration — this probe is the pre-launch check that naga
    // accepts that coexistence (auto layout only materializes statically
    // used bindings) and that the 896-byte uniform mirror still matches.
    // Run: cargo test -p phosphor-app -- --ignored splat_shaders_compile
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn splat_shaders_compile() {
        let noise = include_str!("../../../../assets/shaders/lib/noise.wgsl");
        let palette = include_str!("../../../../assets/shaders/lib/palette.wgsl");
        let plib = include_str!("../../../../assets/shaders/lib/particle_lib.wgsl");
        let sim = include_str!("../../../../assets/shaders/splat_sim.wgsl");
        let bg = include_str!("../../../../assets/shaders/splat_bg.wgsl");
        let resolve =
            include_str!("../../../../assets/shaders/builtin/compute_raster_resolve.wgsl");

        let _guard = gpu_guard();
        let (device, _queue) = test_gpu();

        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let sim_src = format!("{noise}\n{palette}\n{plib}\n{sim}");
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("splat-sim-probe"),
            source: wgpu::ShaderSource::Wgsl(sim_src.into()),
        });
        let _ = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("splat-sim-probe"),
            layout: None,
            module: &module,
            entry_point: Some("cs_main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let err = pollster::block_on(device.pop_error_scope());
        assert!(err.is_none(), "splat_sim.wgsl failed validation: {err:?}");

        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let bg_src = format!("{UNIFORM_BLOCK}\n{noise}\n{palette}\n{bg}");
        let _ = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("splat-bg-probe"),
            source: wgpu::ShaderSource::Wgsl(bg_src.into()),
        });
        let err = pollster::block_on(device.pop_error_scope());
        assert!(err.is_none(), "splat_bg.wgsl failed validation: {err:?}");

        // The patched resolve (OIT mode 2 branch) must still validate.
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let _ = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("splat-resolve-probe"),
            source: wgpu::ShaderSource::Wgsl(resolve.into()),
        });
        let err = pollster::block_on(device.pop_error_scope());
        assert!(
            err.is_none(),
            "compute_raster_resolve.wgsl failed validation: {err:?}"
        );
    }

    // Offscreen render probe (frost_render_previews pattern) driving the
    // PRODUCTION ParticleSystem: a procedural torus-knot scene (no asset
    // download — CI never needs a real capture) uploaded via
    // upload_splat_cloud, simulated + rasterized through the "oit" resolve
    // for four synthetic audio states, PNGs captured and sanity-asserted.
    // A wrapped i32 accumulator (overflow) shows up as garbage colors and
    // fails the mean bound; a broken projection/OIT renders black.
    // Run: cargo test -p phosphor-app -- --ignored splat_render_previews
    // PNGs land in $SPLAT_PNG_DIR (default /tmp).
    #[test]
    #[ignore = "requires a GPU/software adapter; writes PNGs"]
    fn splat_render_previews() {
        use crate::gpu::frame_capture::FrameCapture;
        use crate::gpu::particle::ParticleSystem;
        use crate::gpu::particle::splat::generate_test_scene;

        let out_dir = std::env::var("SPLAT_PNG_DIR").unwrap_or_else(|_| "/tmp".to_string());
        let _guard = gpu_guard();
        let (device, queue) = test_gpu();

        // Production sim concatenation + the shipped .pfx def, probe-sized.
        let noise = include_str!("../../../../assets/shaders/lib/noise.wgsl");
        let palette = include_str!("../../../../assets/shaders/lib/palette.wgsl");
        let plib = include_str!("../../../../assets/shaders/lib/particle_lib.wgsl");
        let sim = include_str!("../../../../assets/shaders/splat_sim.wgsl");
        let sim_src = format!("{noise}\n{palette}\n{plib}\n{sim}");
        let effect: PfxEffect =
            serde_json::from_str(include_str!("../../../../assets/effects/splat.pfx")).unwrap();
        let mut def = effect.particles.unwrap();
        // Real-scene override: SPLAT_PLY=/path/to/scene.ply runs the production
        // decode path (parse + cull + normalize) instead of the synthetic
        // torus-knot, so tuning happens against actual capture geometry (the
        // synthetic scene is too thin/front-facing to reproduce dense-figure
        // artefacts). SPLAT_CAM_DIST overrides the orbit radius (default 1.6 =
        // whole figure; smaller = zoom in).
        let ply_path = std::env::var("SPLAT_PLY").ok();
        let env_f = |k: &str, d: f32| {
            std::env::var(k)
                .ok()
                .and_then(|v| v.parse::<f32>().ok())
                .unwrap_or(d)
        };
        let cam_dist = env_f("SPLAT_CAM_DIST", 1.6);
        let splat_scale = env_f("SPLAT_SCALE", 1.0);
        let opacity_gain = env_f("SPLAT_OPACITY", 1.0);
        let exposure = env_f("SPLAT_EXPOSURE", 0.33);
        // SPLAT_SORT=0 forces the OIT fallback for an A/B against the sorted path.
        if let Ok(v) = std::env::var("SPLAT_SORT") {
            if let Some(splat) = def.splat.as_mut() {
                splat.sort = v != "0";
            }
        }
        if ply_path.is_some() {
            def.max_count = 1_000_000; // keep every splat of a ~800k capture
        } else {
            // 60k: above TILED_THRESHOLD once alive, so early frames exercise
            // the direct path and steady state exercises the tiled path.
            def.max_count = 60_000;
        }
        def.max_scaled_count = 0;

        // SPLAT_W/SPLAT_H override the probe resolution — the 8px-cap regression
        // only shows at real res (1920×1080), not the 960×540 default.
        let env_u = |k: &str, d: u32| {
            std::env::var(k)
                .ok()
                .and_then(|v| v.parse::<u32>().ok())
                .unwrap_or(d)
        };
        let (w, h) = (env_u("SPLAT_W", 960), env_u("SPLAT_H", 540));
        let fmt = wgpu::TextureFormat::Rgba8UnormSrgb;
        // Use the effect's own scene transform (incl. the Y-down→Y-up 180°-X flip)
        // so the offscreen A/B exercises the real load path, not an unrotated one.
        let scene_scale = def.splat.as_ref().map_or(1.0, |s| s.scene_scale);
        // SPLAT_FAR_CLIP overrides the .pfx far-field cull (0 = keep everything).
        let far_clip = def.splat.as_ref().map_or(0.0, |s| s.far_clip);
        // SPLAT_ROT=x,y,z overrides the .pfx Euler offsets — the SH path is the
        // one thing that reads the scene rotation back out (it un-rotates the
        // view direction), so testing it needs the rotation to be a variable.
        let scene_rot = match std::env::var("SPLAT_ROT") {
            Ok(v) => {
                let e: Vec<f32> = v.split(',').filter_map(|c| c.trim().parse().ok()).collect();
                assert_eq!(e.len(), 3, "SPLAT_ROT wants three comma-separated degrees");
                [e[0], e[1], e[2]]
            }
            Err(_) => def
                .splat
                .as_ref()
                .map_or([0.0, 0.0, 0.0], |s| s.rotation_degrees),
        };
        let mut ps = ParticleSystem::new(&device, &queue, fmt, &def, &sim_src, false);
        ps.resize_compute_raster(&device, w, h);
        let mut cloud = if let Some(p) = ply_path.as_ref() {
            use std::sync::atomic::{AtomicBool, AtomicU8};
            let prog = AtomicU8::new(0);
            let cancel = AtomicBool::new(false);
            crate::gpu::particle::splat_source::load_splat_file(
                std::path::Path::new(p),
                1_000_000,
                crate::gpu::particle::splat_source::SceneOptions {
                    scene_scale,
                    rotation_degrees: scene_rot,
                    far_clip: env_f("SPLAT_FAR_CLIP", far_clip),
                },
                &prog,
                &cancel,
            )
            .expect("load SPLAT_PLY")
        } else {
            generate_test_scene(50_000)
        };
        // SPLAT_SH=0 drops a capture to DC only — the A/B that isolates the
        // view-dependent contribution at identical geometry (#1862).
        if std::env::var("SPLAT_SH").is_ok_and(|v| v == "0") {
            cloud.sh_degree = 0;
            cloud.sh = Vec::new();
        }
        eprintln!(
            "scene splats: {} (SH degree {})",
            cloud.count, cloud.sh_degree
        );
        ps.upload_splat_cloud(&device, &queue, &cloud);

        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("splat-preview-target"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: fmt,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let target_view = target.create_view(&Default::default());
        // Stand-in for the bg pass: the plate's base vignette color.
        let bg_clear = wgpu::Color {
            r: 0.02,
            g: 0.022,
            b: 0.032,
            a: 1.0,
        };

        struct ProbeState {
            name: &'static str,
            rms: f32,
            centroid: f32,
            focus: f32,
            onset_every: u32, // 0 = never
            drop_at: u32,     // frame index, u32::MAX = never
        }
        let states = [
            ProbeState {
                name: "idle",
                rms: 0.1,
                centroid: 0.4,
                focus: 0.5,
                onset_every: 0,
                drop_at: u32::MAX,
            },
            ProbeState {
                name: "groove",
                rms: 0.5,
                centroid: 0.5,
                focus: 0.5,
                onset_every: 15,
                drop_at: u32::MAX,
            },
            ProbeState {
                name: "drop_explode",
                rms: 0.6,
                centroid: 0.5,
                focus: 0.5,
                onset_every: 15,
                drop_at: 60,
            },
            ProbeState {
                name: "defocus",
                rms: 0.3,
                centroid: 1.0,
                focus: 1.0,
                onset_every: 0,
                drop_at: u32::MAX,
            },
        ];

        let frames = 90u32;
        let dt = 1.0 / 60.0;
        let mut captures: std::collections::HashMap<&str, Vec<u8>> =
            std::collections::HashMap::new();

        for s in &states {
            for f in 0..frames {
                ps.poll_counter_readback();
                ps.update_uniforms(dt, f as f32 * dt, [w as f32, h as f32], 0.0);
                ps.uniforms.rms = s.rms;
                ps.uniforms.centroid = s.centroid;
                ps.uniforms.onset = if s.onset_every > 0 && f % s.onset_every == 0 {
                    0.7
                } else {
                    0.0
                };
                ps.uniforms.drop = if f == s.drop_at { 1.0 } else { 0.0 };
                ps.uniforms.buildup = if s.drop_at != u32::MAX && f < s.drop_at {
                    f as f32 / s.drop_at as f32
                } else {
                    0.0
                };
                // Frozen param slots 0–7 (sim) — see splat.pfx.
                ps.uniforms.effect_params = [
                    0.8,
                    0.75,
                    0.5,
                    s.focus,
                    splat_scale,
                    opacity_gain,
                    0.3,
                    exposure,
                ];
                // Slots 8–12 (CPU driver): orbit, distance, pitch, focal bias,
                // roundness. SPLAT_ORBIT=0 + SPLAT_YAW/SPLAT_PITCH freezes a
                // viewing angle; SPLAT_ROUNDNESS morphs shard→sphere.
                ps.splat_ui_params = [
                    env_f("SPLAT_ORBIT", 0.3),
                    cam_dist,
                    env_f("SPLAT_PITCH", 0.15),
                    0.0,
                    env_f("SPLAT_ROUNDNESS", 0.0),
                ];
                ps.update_splat_driver();
                if std::env::var("SPLAT_YAW").is_ok() {
                    ps.uniforms.cam_yaw = env_f("SPLAT_YAW", 0.0);
                }

                // On the final frame render into the capture target instead
                // (its texture is RENDER_ATTACHMENT | COPY_SRC, not COPY_DST).
                let is_last = f == frames - 1;
                let mut fc =
                    is_last.then(|| FrameCapture::new(&device, w, h, fmt, "splat-capture"));
                let frame_view = fc.as_ref().map_or(&target_view, |fc| &fc.view);

                let mut enc = device.create_command_encoder(&Default::default());
                ps.dispatch(&mut enc, &queue);
                {
                    // Clear to the bg color; ps.render composites (LoadOp::Load).
                    let _pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("splat-preview-bg"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: frame_view,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(bg_clear),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                    });
                }
                ps.render(&mut enc, &queue, frame_view);
                if let Some(fc) = fc.as_ref() {
                    fc.copy_to_staging(&mut enc);
                }
                queue.submit([enc.finish()]);
                ps.request_counter_readback();
                ps.flip();

                if let Some(fc) = fc.as_mut() {
                    device
                        .poll(wgpu::PollType::Wait {
                            submission_index: None,
                            timeout: None,
                        })
                        .unwrap();
                    fc.request_map();
                    let data = loop {
                        device
                            .poll(wgpu::PollType::Wait {
                                submission_index: None,
                                timeout: None,
                            })
                            .unwrap();
                        if let Some(d) = fc.take_mapped_data(&device) {
                            break d;
                        }
                    };
                    let mean =
                        data.iter().map(|&b| b as f64).sum::<f64>() / (data.len() as f64 * 255.0);
                    let path = format!("{out_dir}/splat_{}.png", s.name);
                    image::RgbaImage::from_raw(w, h, data.clone())
                        .expect("raw->image")
                        .save(&path)
                        .expect("save png");
                    eprintln!("wrote {path} (mean {mean:.4})");

                    // Sanity guards apply to the synthetic scene only; a real
                    // SPLAT_PLY render (possibly zoomed) legitimately fills the
                    // corner and varies in mean — it is a debug capture.
                    if ply_path.is_none() {
                        // Not black (scene visible), not blown out (an i32
                        // accumulator wrap reads as garbage brightness).
                        assert!(
                            mean > 0.004,
                            "{} rendered near-black (mean {mean:.4})",
                            s.name
                        );
                        assert!(mean < 0.90, "{} blew out (mean {mean:.4})", s.name);
                        // Background must show through empty space: the scene is
                        // centered, so the top-left corner is bg-only (the 0.02
                        // linear clear ≈ 40/255 in sRGB — allow slack).
                        assert!(
                            data[0] < 70 && data[1] < 70 && data[2] < 70,
                            "{}: corner not background ({:?})",
                            s.name,
                            &data[0..4]
                        );
                    }
                    captures.insert(s.name, data);
                }
            }
        }

        // The drop must visibly shatter the scene vs. the same state without
        // it (mean absolute per-pixel difference). Synthetic scene only.
        if ply_path.is_none() {
            let a = &captures["groove"];
            let b = &captures["drop_explode"];
            let diff = a
                .iter()
                .zip(b.iter())
                .map(|(&x, &y)| (x as f64 - y as f64).abs())
                .sum::<f64>()
                / (a.len() as f64 * 255.0);
            assert!(
                diff > 0.003,
                "drop_explode is indistinguishable from groove (mean |Δ| {diff:.5})"
            );
        }
    }

    // Headless wall-clock perf run for the #1800 go/no-go gate (≥60 FPS at
    // 1–3M splats): 600 frames of the production dispatch+raster+resolve at
    // 1080p, GPU-bound via a blocking poll per frame. Reports mean / p99
    // frame time. Splat count via SPLAT_PERF_COUNT (default 1_000_000).
    // Run: SPLAT_PERF_COUNT=3000000 cargo test -p phosphor-app --release -- --ignored --nocapture splat_perf_600_frames
    #[test]
    #[ignore = "requires a GPU; perf measurement, run --release"]
    fn splat_perf_600_frames() {
        use crate::gpu::particle::ParticleSystem;
        use crate::gpu::particle::splat::generate_test_scene;

        let count: u32 = std::env::var("SPLAT_PERF_COUNT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1_000_000);

        let noise = include_str!("../../../../assets/shaders/lib/noise.wgsl");
        let palette = include_str!("../../../../assets/shaders/lib/palette.wgsl");
        let plib = include_str!("../../../../assets/shaders/lib/particle_lib.wgsl");
        let sim = include_str!("../../../../assets/shaders/splat_sim.wgsl");
        let sim_src = format!("{noise}\n{palette}\n{plib}\n{sim}");
        let effect: PfxEffect =
            serde_json::from_str(include_str!("../../../../assets/effects/splat.pfx")).unwrap();
        let mut def = effect.particles.unwrap();
        def.max_count = count;
        def.max_scaled_count = 0;

        let _guard = gpu_guard();
        let (device, queue) = test_gpu();

        let (w, h) = (1920u32, 1080u32);
        let fmt = wgpu::TextureFormat::Rgba8UnormSrgb;
        let mut ps = ParticleSystem::new(&device, &queue, fmt, &def, &sim_src, false);
        ps.resize_compute_raster(&device, w, h);
        // SPLAT_PLY measures a REAL capture instead of the procedural knot —
        // needed for the two things the synthetic scene cannot show: the cost of
        // view-dependent SH (the knot has none) and the close-zoom overdraw the
        // sorted path's 1024 px radius cap allows (#1862).
        let cloud = match std::env::var("SPLAT_PLY") {
            Ok(p) => {
                use std::sync::atomic::{AtomicBool, AtomicU8};
                eprintln!("loading {p}…");
                let (prog, cancel) = (AtomicU8::new(0), AtomicBool::new(false));
                let scene_rot = def
                    .splat
                    .as_ref()
                    .map_or([0.0, 0.0, 0.0], |s| s.rotation_degrees);
                crate::gpu::particle::splat_source::load_splat_file(
                    std::path::Path::new(&p),
                    count,
                    crate::gpu::particle::splat_source::SceneOptions {
                        rotation_degrees: scene_rot,
                        far_clip: def.splat.as_ref().map_or(0.0, |s| s.far_clip),
                        ..Default::default()
                    },
                    &prog,
                    &cancel,
                )
                .expect("load SPLAT_PLY")
            }
            Err(_) => {
                eprintln!("generating {count} procedural splats…");
                generate_test_scene(count as usize)
            }
        };
        if std::env::var("SPLAT_SH").is_ok_and(|v| v == "0") {
            // A/B the SH evaluation cost at identical geometry.
            let mut c = cloud;
            c.sh_degree = 0;
            c.sh = Vec::new();
            ps.upload_splat_cloud(&device, &queue, &c);
        } else {
            ps.upload_splat_cloud(&device, &queue, &cloud);
        }

        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("splat-perf-target"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: fmt,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = target.create_view(&Default::default());

        let frames = 600u32;
        let dt = 1.0 / 60.0;
        let mut times_ms: Vec<f64> = Vec::with_capacity(frames as usize);
        for f in 0..frames {
            ps.poll_counter_readback();
            ps.update_uniforms(dt, f as f32 * dt, [w as f32, h as f32], 0.0);
            ps.uniforms.rms = 0.5;
            ps.uniforms.onset = if f % 20 == 0 { 0.7 } else { 0.0 };
            ps.uniforms.drop = if f % 240 == 100 { 1.0 } else { 0.0 }; // periodic worst-case explode
            let scale_ov: f32 = std::env::var("SPLAT_PERF_SCALE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1.0);
            let focus_ov: f32 = std::env::var("SPLAT_PERF_FOCUS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.5);
            ps.uniforms.effect_params = [0.8, 0.75, 0.5, focus_ov, scale_ov, 1.0, 0.3, 0.33];
            // SPLAT_CAM_DIST < 1.6 zooms in — the r_cap overdraw stress case.
            let dist: f32 = std::env::var("SPLAT_CAM_DIST")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1.6);
            ps.splat_ui_params = [0.3, dist, 0.15, 0.0, 0.0];
            ps.update_splat_driver();

            let t0 = std::time::Instant::now();
            let mut enc = device.create_command_encoder(&Default::default());
            ps.dispatch(&mut enc, &queue);
            ps.render(&mut enc, &queue, &view);
            queue.submit([enc.finish()]);
            device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: None,
                })
                .unwrap();
            let ms = t0.elapsed().as_secs_f64() * 1e3;
            ps.request_counter_readback();
            ps.flip();
            if f >= 30 {
                times_ms.push(ms); // skip warm-up (pipeline compiles, first tiled frames)
            }
            if f % 100 == 0 {
                eprintln!("  frame {f}: {ms:.2} ms, alive {}", ps.alive_count);
            }
        }
        times_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let mean = times_ms.iter().sum::<f64>() / times_ms.len() as f64;
        let p99 = times_ms[(times_ms.len() as f64 * 0.99) as usize - 1];
        let max = times_ms.last().unwrap();
        eprintln!(
            "splat perf @ {count} splats, 1080p: mean {mean:.2} ms ({:.0} FPS), p99 {p99:.2} ms, max {max:.2} ms",
            1000.0 / mean
        );
    }

    // Panorama + Ascend (#1801): the two MIR-pack effects. Same guards the
    // other particle effects carry — the .pfx must parse as a builtin, and
    // neither shader may mention the uniform struct name, since
    // prepend_library suppresses UNIFORM_BLOCK injection if it appears
    // anywhere including a comment (#1855) and the compile probe below
    // concatenates the block unconditionally, so it cannot catch that.
    #[test]
    fn panorama_pfx_parses_as_builtin() {
        let effect: PfxEffect =
            serde_json::from_str(include_str!("../../../../assets/effects/panorama.pfx"))
                .expect("panorama.pfx must deserialize");
        assert!(EffectLoader::is_builtin(&effect));
        assert_eq!(effect.inputs.len(), 8);
        let particles = effect.particles.expect("panorama is a particle effect");
        assert_eq!(particles.compute_shader, "panorama_sim.wgsl");
        for src in [
            include_str!("../../../../assets/shaders/panorama_sim.wgsl"),
            include_str!("../../../../assets/shaders/panorama_bg.wgsl"),
        ] {
            assert!(
                !src.contains("PhosphorUniforms"),
                "panorama shaders must not mention the uniform struct name — it suppresses injection"
            );
        }
    }

    #[test]
    fn ascend_pfx_parses_as_builtin() {
        let effect: PfxEffect =
            serde_json::from_str(include_str!("../../../../assets/effects/ascend.pfx"))
                .expect("ascend.pfx must deserialize");
        assert!(EffectLoader::is_builtin(&effect));
        assert_eq!(effect.inputs.len(), 8);
        let particles = effect.particles.expect("ascend is a particle effect");
        assert_eq!(particles.compute_shader, "ascend_sim.wgsl");
        for src in [
            include_str!("../../../../assets/shaders/ascend_sim.wgsl"),
            include_str!("../../../../assets/shaders/ascend_bg.wgsl"),
        ] {
            assert!(
                !src.contains("PhosphorUniforms"),
                "ascend shaders must not mention the uniform struct name — it suppresses injection"
            );
        }
    }

    /// Compile probe for both #1801 effects through the production concatenation.
    /// Run: cargo test -p phosphor-app -- --ignored mir_pack_shaders_compile
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn mir_pack_shaders_compile() {
        let noise = include_str!("../../../../assets/shaders/lib/noise.wgsl");
        let palette = include_str!("../../../../assets/shaders/lib/palette.wgsl");
        let plib = include_str!("../../../../assets/shaders/lib/particle_lib.wgsl");

        let _guard = gpu_guard();
        let (device, _queue) = test_gpu();

        let sims = [
            (
                "panorama_sim.wgsl",
                include_str!("../../../../assets/shaders/panorama_sim.wgsl"),
            ),
            (
                "ascend_sim.wgsl",
                include_str!("../../../../assets/shaders/ascend_sim.wgsl"),
            ),
        ];
        for (name, sim) in sims {
            device.push_error_scope(wgpu::ErrorFilter::Validation);
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(name),
                source: wgpu::ShaderSource::Wgsl(
                    format!("{noise}\n{palette}\n{plib}\n{sim}").into(),
                ),
            });
            // Pipeline creation forces full validation (entry point, bindings).
            let _ = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(name),
                layout: None,
                module: &module,
                entry_point: Some("cs_main"),
                compilation_options: Default::default(),
                cache: None,
            });
            let err = pollster::block_on(device.pop_error_scope());
            assert!(err.is_none(), "{name} failed validation: {err:?}");
        }

        let bgs = [
            (
                "panorama_bg.wgsl",
                include_str!("../../../../assets/shaders/panorama_bg.wgsl"),
            ),
            (
                "ascend_bg.wgsl",
                include_str!("../../../../assets/shaders/ascend_bg.wgsl"),
            ),
        ];
        for (name, bg) in bgs {
            device.push_error_scope(wgpu::ErrorFilter::Validation);
            let _ = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(name),
                source: wgpu::ShaderSource::Wgsl(
                    format!("{UNIFORM_BLOCK}\n{noise}\n{palette}\n{bg}").into(),
                ),
            });
            let err = pollster::block_on(device.pop_error_scope());
            assert!(err.is_none(), "{name} failed validation: {err:?}");
        }
    }

    /// Offscreen render of the reworked Ascend ridgeline (#1441) under a few
    /// synthetic spectra, so the terrain shape can be eyeballed without the app.
    /// Renders only the particle pass (no bg feedback / bloom) — enough to read
    /// the ridge silhouette and confirm the bands sculpt it. Writes PNGs to
    /// ASCEND_PNG_DIR (default /tmp); the asserts only guard not-black / not-blown.
    /// Run: ASCEND_PNG_DIR=/some/dir cargo test -p phosphor-app --release -- --ignored ascend_render_previews
    #[test]
    #[ignore = "requires a GPU/software adapter; writes PNGs"]
    fn ascend_render_previews() {
        use crate::audio::features::AudioFeatures;
        use crate::gpu::frame_capture::FrameCapture;
        use crate::gpu::particle::ParticleSystem;

        let out_dir = std::env::var("ASCEND_PNG_DIR").unwrap_or_else(|_| "/tmp".to_string());
        let _guard = gpu_guard();
        let (device, queue) = test_gpu();

        let noise = include_str!("../../../../assets/shaders/lib/noise.wgsl");
        let palette = include_str!("../../../../assets/shaders/lib/palette.wgsl");
        let plib = include_str!("../../../../assets/shaders/lib/particle_lib.wgsl");
        let sim = include_str!("../../../../assets/shaders/ascend_sim.wgsl");
        let sim_src = format!("{noise}\n{palette}\n{plib}\n{sim}");
        let effect: PfxEffect =
            serde_json::from_str(include_str!("../../../../assets/effects/ascend.pfx")).unwrap();
        let mut def = effect.particles.unwrap();
        def.max_scaled_count = 0; // keep max_count as authored

        // The 8 sim params in .pfx order (altitude, relief, shimmer, flow, hue,
        // glow, baseline, trail_decay).
        let params: [f32; 8] = [0.7, 0.6, 0.45, 0.4, 0.5, 0.55, 0.3, 0.84];

        let (w, h) = (960u32, 540u32);
        let fmt = wgpu::TextureFormat::Rgba8UnormSrgb;

        // Each scene is a band spectrum + brightness, chosen to show the ridge
        // respond: bass sinks it low-left, bright lifts a right-leaning range,
        // full raises the whole massif.
        let bands = |v: [f32; 7]| v;
        let scenes: [(&str, [f32; 7], f32, f32); 3] = [
            // name, [sub_bass..brilliance], rolloff, rms
            (
                "bass",
                bands([0.85, 0.7, 0.35, 0.15, 0.08, 0.05, 0.03]),
                0.18,
                0.6,
            ),
            (
                "bright",
                bands([0.05, 0.1, 0.2, 0.35, 0.55, 0.8, 0.9]),
                0.85,
                0.6,
            ),
            (
                "full",
                bands([0.5, 0.55, 0.5, 0.6, 0.5, 0.55, 0.5]),
                0.5,
                0.7,
            ),
        ];

        for (name, b, rolloff, rms) in scenes {
            let mut ps = ParticleSystem::new(&device, &queue, fmt, &def, &sim_src, false);

            let feat = AudioFeatures {
                sub_bass: b[0],
                bass: b[1],
                low_mid: b[2],
                mid: b[3],
                upper_mid: b[4],
                presence: b[5],
                brilliance: b[6],
                rolloff,
                rms,
                bandwidth: 0.4,
                centroid: rolloff,
                zcr: 0.2,
                ..Default::default()
            };

            let target = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("ascend-preview-target"),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: fmt,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let target_view = target.create_view(&Default::default());

            // ~2.5 s of frames at 60 Hz so the field fully populates and settles
            // onto the ridge (lifetime is 3 s).
            let frames = 150u32;
            for f in 0..frames {
                let time = f as f32 / 60.0;
                ps.update_uniforms(1.0 / 60.0, time, [w as f32, h as f32], 0.0);
                ps.update_audio(&feat);
                ps.uniforms.effect_params = params;

                let is_last = f == frames - 1;
                let mut fc =
                    is_last.then(|| FrameCapture::new(&device, w, h, fmt, "ascend-capture"));
                let frame_view = fc.as_ref().map_or(&target_view, |fc| &fc.view);

                let mut enc = device.create_command_encoder(&Default::default());
                ps.dispatch(&mut enc, &queue);
                {
                    // Clear to near-black; ps.render composites additively (LoadOp::Load).
                    let _pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("ascend-preview-clear"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: frame_view,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color {
                                    r: 0.01,
                                    g: 0.01,
                                    b: 0.02,
                                    a: 1.0,
                                }),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                    });
                }
                ps.render(&mut enc, &queue, frame_view);
                if let Some(fc) = fc.as_ref() {
                    fc.copy_to_staging(&mut enc);
                }
                queue.submit([enc.finish()]);
                ps.flip();

                if let Some(fc) = fc.as_mut() {
                    device
                        .poll(wgpu::PollType::Wait {
                            submission_index: None,
                            timeout: None,
                        })
                        .unwrap();
                    fc.request_map();
                    let data = loop {
                        device
                            .poll(wgpu::PollType::Wait {
                                submission_index: None,
                                timeout: None,
                            })
                            .unwrap();
                        if let Some(d) = fc.take_mapped_data(&device) {
                            break d;
                        }
                    };
                    let mean =
                        data.iter().map(|&x| x as f64).sum::<f64>() / (data.len() as f64 * 255.0);
                    let path = format!("{out_dir}/ascend_{name}.png");
                    image::RgbaImage::from_raw(w, h, data.clone())
                        .expect("raw->image")
                        .save(&path)
                        .expect("save png");
                    eprintln!("wrote {path} (mean {mean:.4})");
                    assert!(mean > 0.001, "{name} rendered near-black (mean {mean:.4})");
                    assert!(mean < 0.80, "{name} blew out (mean {mean:.4})");
                }
            }
        }
    }

    /// Proves the Rust `ParticleUniforms` and the WGSL mirror in `particle_lib.wgsl` agree at the
    /// **field** level, not just in total size.
    ///
    /// The `*_shaders_compile` probes above all create pipelines with `layout: None`, so wgpu
    /// derives the layout *from the shader* — a WGSL struct that has drifted smaller than the Rust
    /// one still validates, and every sim then reads shifted offsets. That is the failure mode the
    /// A13b bump (896 -> 944 B, #1801) could introduce silently, and Panorama is about to read
    /// `band_pan` directly.
    ///
    /// Writes a distinctive value into one field per block, runs a real dispatch that copies them
    /// out through the production `particle_lib` accessors, and reads them back. `splat_sh_degree`
    /// is asserted alongside the new fields specifically to pin the "append, never insert"
    /// invariant (#1505): if the new tail had been spliced in mid-struct, it would move.
    ///
    /// Run: cargo test -p phosphor-app -- --ignored particle_uniforms_wgsl_layout_matches_rust
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn particle_uniforms_wgsl_layout_matches_rust() {
        use crate::gpu::particle::types::ParticleUniforms;
        use wgpu::util::DeviceExt;

        // Production concatenation order — particle_lib calls into noise.wgsl.
        let noise = include_str!("../../../../assets/shaders/lib/noise.wgsl");
        let palette = include_str!("../../../../assets/shaders/lib/palette.wgsl");
        let plib = include_str!("../../../../assets/shaders/lib/particle_lib.wgsl");
        let probe = r#"
@group(0) @binding(1) var<storage, read_write> out: array<f32>;
@compute @workgroup_size(1)
fn cs_main() {
    out[0] = u.delta_time;       // first block — must not have moved
    out[1] = u.seed;             // mid-struct anchor
    out[2] = u.splat_sh_degree;  // last pre-A13b field — pins "append, never insert"
    out[3] = u.pan;
    out[4] = u.stereo_width;
    out[5] = u.stereo_corr;
    for (var i = 0u; i < 7u; i = i + 1u) {
        out[6u + i] = band_pan(i);
    }
}
"#;
        const N: usize = 13;

        let _guard = gpu_guard();
        let (device, queue) = test_gpu();

        // Distinctive, unequal values so a shifted read cannot coincidentally match.
        let mut u: ParticleUniforms = bytemuck::Zeroable::zeroed();
        u.delta_time = 0.125;
        u.seed = 0.375;
        u.splat_sh_degree = 3.0;
        u.pan = 0.25;
        u.stereo_width = 0.75;
        u.stereo_corr = 0.125;
        u.band_pan = [0.11, 0.22, 0.33, 0.44, 0.55, 0.66, 0.77, 0.0];

        let ubuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("probe-uniforms"),
            contents: bytemuck::bytes_of(&u),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let obuf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("probe-out"),
            size: (N * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let rbuf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("probe-readback"),
            size: (N * 4) as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("uniform-layout-probe"),
            source: wgpu::ShaderSource::Wgsl(format!("{noise}\n{palette}\n{plib}\n{probe}").into()),
        });
        // `layout: None` is fine here: the bind group below supplies the *real* Rust-sized buffer,
        // so wgpu checks it against the minimum binding size the WGSL struct implies.
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("uniform-layout-probe"),
            layout: None,
            module: &module,
            entry_point: Some("cs_main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("uniform-layout-probe"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: ubuf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: obuf.as_entire_binding(),
                },
            ],
        });

        let mut enc = device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_compute_pass(&Default::default());
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bg, &[]);
            pass.dispatch_workgroups(1, 1, 1);
        }
        enc.copy_buffer_to_buffer(&obuf, 0, &rbuf, 0, (N * 4) as u64);
        queue.submit([enc.finish()]);

        rbuf.slice(..)
            .map_async(wgpu::MapMode::Read, |r| r.unwrap());
        device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .unwrap();
        let got: Vec<f32> = bytemuck::cast_slice(&rbuf.slice(..).get_mapped_range()).to_vec();

        let expect = [
            ("delta_time", u.delta_time),
            ("seed", u.seed),
            ("splat_sh_degree", u.splat_sh_degree),
            ("pan", u.pan),
            ("stereo_width", u.stereo_width),
            ("stereo_corr", u.stereo_corr),
        ];
        for (i, (name, want)) in expect.iter().enumerate() {
            assert_eq!(
                got[i],
                *want,
                "{name}: WGSL read {} but Rust wrote {want} — particle_lib.wgsl has drifted from \
                 ParticleUniforms ({}-byte struct)",
                got[i],
                std::mem::size_of::<ParticleUniforms>()
            );
        }
        for i in 0..7 {
            assert_eq!(
                got[6 + i],
                u.band_pan[i],
                "band_pan({i}): WGSL read {} but Rust wrote {}",
                got[6 + i],
                u.band_pan[i]
            );
        }
    }
}
