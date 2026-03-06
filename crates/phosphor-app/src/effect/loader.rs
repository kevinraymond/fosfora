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
    _pad_align: f32,
    mfcc: array<vec4f, 4>,     // 13 MFCCs (indices 0-12 used, 13-15 padding)
    chroma: array<vec4f, 3>,   // 12 pitch class energies (C=0, C#=1, ..., B=11)
}

@group(0) @binding(0) var<uniform> u: PhosphorUniforms;
@group(0) @binding(1) var prev_frame: texture_2d<f32>;
@group(0) @binding(2) var prev_sampler: sampler;

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
        effect.author == "Phosphor"
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
    fn is_builtin_true_for_phosphor_author() {
        assert!(EffectLoader::is_builtin(&make_effect("Phosphor")));
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
}
