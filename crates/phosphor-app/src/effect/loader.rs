use std::path::{Path, PathBuf};

use anyhow::Result;

use super::format::PfxEffect;

const EFFECTS_DIR: &str = "assets/effects";
const SHADERS_DIR: &str = "assets/shaders";

/// WGSL standard library files, prepended to all effect shaders.
const LIB_FILES: &[&str] = &[
    "assets/shaders/lib/noise.wgsl",
    "assets/shaders/lib/palette.wgsl",
    "assets/shaders/lib/sdf.wgsl",
    "assets/shaders/lib/tonemap.wgsl",
];

/// Standard uniform block prepended to all effect shaders.
const UNIFORM_BLOCK: &str = r#"
struct PhosphorUniforms {
    time: f32,
    delta_time: f32,
    resolution: vec2f,
    bass: f32,
    mid: f32,
    treble: f32,
    rms: f32,
    phase: f32,
    onset: f32,
    centroid: f32,
    flux: f32,
    flatness: f32,
    rolloff: f32,
    bandwidth: f32,
    zcr: f32,
    params: array<vec4f, 4>,
    feedback_decay: f32,
    frame_index: f32,
}

@group(0) @binding(0) var<uniform> u: PhosphorUniforms;
@group(0) @binding(1) var prev_frame: texture_2d<f32>;
@group(0) @binding(2) var prev_sampler: sampler;

fn param(i: u32) -> f32 {
    return u.params[i / 4u][i % 4u];
}

fn feedback(uv: vec2f) -> vec4f {
    return textureSample(prev_frame, prev_sampler, uv);
}
"#;

pub struct EffectLoader {
    pub effects: Vec<PfxEffect>,
    pub current_effect: Option<usize>,
    lib_source: String,
}

impl EffectLoader {
    pub fn new() -> Self {
        // Load library sources
        let mut lib_source = String::new();
        for path in LIB_FILES {
            match std::fs::read_to_string(path) {
                Ok(src) => {
                    lib_source.push_str(&src);
                    lib_source.push('\n');
                }
                Err(e) => {
                    log::warn!("Failed to load shader library {path}: {e}");
                }
            }
        }

        Self {
            effects: Vec::new(),
            current_effect: None,
            lib_source,
        }
    }

    pub fn scan_effects_directory(&mut self) {
        self.effects.clear();
        let dir = Path::new(EFFECTS_DIR);
        if !dir.exists() {
            log::warn!("Effects directory not found: {}", dir.display());
            return;
        }

        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path()
                    .extension()
                    .is_some_and(|ext| ext == "pfx")
            })
            .collect();
        entries.sort_by_key(|e| e.file_name());

        for entry in entries {
            match std::fs::read_to_string(entry.path()) {
                Ok(json) => match serde_json::from_str::<PfxEffect>(&json) {
                    Ok(effect) => {
                        log::info!("Found effect: {} ({})", effect.name, entry.path().display());
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
        Path::new(SHADERS_DIR).join(shader_rel)
    }

    pub fn load_effect_source(&self, shader_rel: &str) -> Result<String> {
        let path = self.resolve_shader_path(shader_rel);
        let source = std::fs::read_to_string(&path)?;
        Ok(self.prepend_library(&source))
    }

    /// Load a compute shader source. Prepends the noise library but NOT the fragment
    /// uniform block (compute shaders have their own uniform struct).
    pub fn load_compute_source(&self, shader_rel: &str) -> Result<String> {
        let path = self.resolve_shader_path(shader_rel);
        let source = std::fs::read_to_string(&path)?;
        // Compute shaders get the noise library but not the fragment uniform block
        Ok(format!("{}\n{}", self.lib_source, source))
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
}
