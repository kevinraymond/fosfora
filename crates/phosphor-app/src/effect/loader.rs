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

    pub fn load_effect_source(&self, shader_rel: &str) -> Result<String> {
        let path = self.resolve_shader_path(shader_rel);
        let source = std::fs::read_to_string(&path)?;
        Ok(self.prepend_library(&source))
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

        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..Default::default()
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
        .expect("no wgpu adapter");
        eprintln!("probe adapter: {:?}", adapter.get_info());
        let (device, _queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("tide-compile-probe"),
                required_features: wgpu::Features::empty(),
                required_limits: adapter.limits(),
                experimental_features: wgpu::ExperimentalFeatures::default(),
                memory_hints: wgpu::MemoryHints::Performance,
                trace: wgpu::Trace::Off,
            }))
            .expect("no wgpu device");

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

        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..Default::default()
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
        .expect("no wgpu adapter");
        eprintln!("probe adapter: {:?}", adapter.get_info());
        let (device, _queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("vessel-compile-probe"),
                required_features: wgpu::Features::empty(),
                required_limits: adapter.limits(),
                experimental_features: wgpu::ExperimentalFeatures::default(),
                memory_hints: wgpu::MemoryHints::Performance,
                trace: wgpu::Trace::Off,
            }))
            .expect("no wgpu device");

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

        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..Default::default()
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
        .expect("no wgpu adapter");
        eprintln!("probe adapter: {:?}", adapter.get_info());
        let (device, _queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("cleave-compile-probe"),
                required_features: wgpu::Features::empty(),
                required_limits: adapter.limits(),
                experimental_features: wgpu::ExperimentalFeatures::default(),
                memory_hints: wgpu::MemoryHints::Performance,
                trace: wgpu::Trace::Off,
            }))
            .expect("no wgpu device");

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

        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..Default::default()
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
        .expect("no wgpu adapter");
        eprintln!("probe adapter: {:?}", adapter.get_info());
        let (device, _queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("frost-compile-probe"),
                required_features: wgpu::Features::empty(),
                required_limits: adapter.limits(),
                experimental_features: wgpu::ExperimentalFeatures::default(),
                memory_hints: wgpu::MemoryHints::Performance,
                trace: wgpu::Trace::Off,
            }))
            .expect("no wgpu device");

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
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..Default::default()
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
        .expect("no wgpu adapter");
        eprintln!("probe adapter: {:?}", adapter.get_info());
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("frost-preview"),
            required_features: wgpu::Features::empty(),
            required_limits: adapter.limits(),
            experimental_features: wgpu::ExperimentalFeatures::default(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .expect("no wgpu device");

        // Production concatenation: uniform block + libs + effect fragment.
        let noise = include_str!("../../../../assets/shaders/lib/noise.wgsl");
        let palette = include_str!("../../../../assets/shaders/lib/palette.wgsl");
        let frost = include_str!("../../../../assets/shaders/frost.wgsl");
        let fragment_source = format!("{UNIFORM_BLOCK}\n{noise}\n{palette}\n{frost}");

        let (w, h) = (960u32, 540u32);
        let fmt = wgpu::TextureFormat::Rgba8UnormSrgb;
        let pipeline =
            ShaderPipeline::new(&device, fmt, &fragment_source, None).expect("frost pipeline");

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
        assert_eq!(effect.inputs.len(), 12); // 8 sim slots + 4 CPU camera slots
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

        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..Default::default()
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
        .expect("no wgpu adapter");
        eprintln!("probe adapter: {:?}", adapter.get_info());
        let (device, _queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("splat-compile-probe"),
                required_features: wgpu::Features::empty(),
                required_limits: adapter.limits(),
                experimental_features: wgpu::ExperimentalFeatures::default(),
                memory_hints: wgpu::MemoryHints::Performance,
                trace: wgpu::Trace::Off,
            }))
            .expect("no wgpu device");

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
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..Default::default()
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
        .expect("no wgpu adapter");
        eprintln!("probe adapter: {:?}", adapter.get_info());
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("splat-preview"),
            required_features: wgpu::Features::empty(),
            required_limits: adapter.limits(),
            experimental_features: wgpu::ExperimentalFeatures::default(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .expect("no wgpu device");

        // Production sim concatenation + the shipped .pfx def, probe-sized.
        let noise = include_str!("../../../../assets/shaders/lib/noise.wgsl");
        let palette = include_str!("../../../../assets/shaders/lib/palette.wgsl");
        let plib = include_str!("../../../../assets/shaders/lib/particle_lib.wgsl");
        let sim = include_str!("../../../../assets/shaders/splat_sim.wgsl");
        let sim_src = format!("{noise}\n{palette}\n{plib}\n{sim}");
        let effect: PfxEffect =
            serde_json::from_str(include_str!("../../../../assets/effects/splat.pfx")).unwrap();
        let mut def = effect.particles.unwrap();
        // 60k: above TILED_THRESHOLD once alive, so early frames exercise the
        // direct path and steady state exercises the tiled path.
        def.max_count = 60_000;
        def.max_scaled_count = 0;

        let (w, h) = (960u32, 540u32);
        let fmt = wgpu::TextureFormat::Rgba8UnormSrgb;
        let mut ps = ParticleSystem::new(&device, &queue, fmt, &def, &sim_src, false);
        ps.resize_compute_raster(&device, w, h);
        let cloud = generate_test_scene(50_000);
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
                ps.uniforms.effect_params = [0.8, 0.75, 0.5, s.focus, 1.0, 1.0, 0.3, 0.33];
                // Slots 8–11 (CPU driver): orbit, distance, pitch, focal bias.
                ps.splat_ui_params = [0.3, 1.6, 0.15, 0.0];
                ps.update_splat_driver();

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
                    captures.insert(s.name, data);
                }
            }
        }

        // The drop must visibly shatter the scene vs. the same state without
        // it (mean absolute per-pixel difference).
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

        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..Default::default()
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
        .expect("no wgpu adapter");
        eprintln!("perf adapter: {:?}", adapter.get_info());
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("splat-perf"),
            required_features: wgpu::Features::empty(),
            required_limits: adapter.limits(),
            experimental_features: wgpu::ExperimentalFeatures::default(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .expect("no wgpu device");

        let (w, h) = (1920u32, 1080u32);
        let fmt = wgpu::TextureFormat::Rgba8UnormSrgb;
        let mut ps = ParticleSystem::new(&device, &queue, fmt, &def, &sim_src, false);
        ps.resize_compute_raster(&device, w, h);
        eprintln!("generating {count} procedural splats…");
        let cloud = generate_test_scene(count as usize);
        ps.upload_splat_cloud(&device, &queue, &cloud);

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
            ps.uniforms.effect_params = [0.8, 0.75, 0.5, 0.5, 1.0, 1.0, 0.3, 0.33];
            ps.splat_ui_params = [0.3, 1.6, 0.15, 0.0];
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
}
