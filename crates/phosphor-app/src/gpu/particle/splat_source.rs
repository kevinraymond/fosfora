//! Gaussian-splat scene loading for the Splat effect (#1800).
//!
//! Parses pre-trained 3DGS scenes from disk into a [`SplatCloud`]: the
//! canonical binary little-endian `.ply` export (INRIA property layout) and
//! the compact 32-byte-record `.splat` format. Files can reach ~1.5 GB, so
//! parsing streams in fixed-size chunks (never a whole-file read), culls
//! near-invisible splats, and reservoir-subsamples down to the particle
//! budget — deterministically, so the same file + target always produces the
//! same cloud. Runs on a background thread via [`SplatSceneLoader`]
//! (the `ParticleSourceLoader` shape): the effect loads instantly with an
//! empty scene and splats appear when the decode lands.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::thread;

use crossbeam_channel::{Receiver, TryRecvError, bounded};

use crate::gpu::half::f32_to_f16;

/// The `.pfx` `splat` block's load-time scene options, threaded from `SplatDef`
/// down to the decoder. Bundled rather than passed loose because they travel
/// together through five call sites, and a third bare `f32` alongside
/// `scene_scale` is exactly the kind of positional argument that gets
/// transposed. See [`SplatDef`](super::types::SplatDef) for what each means.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SceneOptions {
    pub scene_scale: f32,
    pub rotation_degrees: [f32; 3],
    pub far_clip: f32,
}

impl Default for SceneOptions {
    fn default() -> Self {
        Self {
            scene_scale: 1.0,
            rotation_degrees: [0.0; 3],
            far_clip: 0.0, // no clip unless a .pfx asks for one
        }
    }
}

impl From<&super::types::SplatDef> for SceneOptions {
    fn from(d: &super::types::SplatDef) -> Self {
        Self {
            scene_scale: d.scene_scale,
            rotation_degrees: d.rotation_degrees,
            far_clip: d.far_clip,
        }
    }
}

/// Transform applied to normalize the source scene (recorded for debugging /
/// status UI; the cloud itself is already transformed).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SplatTransform {
    /// Per-axis median center of the retained set (source units, subtracted).
    pub center: [f32; 3],
    /// Final uniform scale applied to positions and scales
    /// (normalization 1/r95 × the .pfx `scene_scale`).
    pub scale: f32,
    /// 95th-percentile radius of the retained set before scaling (source units).
    pub radius_p95: f32,
}

/// Normalized, subsampled splat scene, ready for GPU packing
/// (`ParticleSystem::upload_splat_cloud`).
///
/// Positions are recentered to the per-axis median and uniformly scaled so the
/// 95th-percentile radius == 1.0 (then `scene_scale`/`rotation_degrees` from
/// the .pfx applied on top). Axes are otherwise unchanged from the source file
/// (3DGS/COLMAP convention, typically Y-down) — the renderer owns any flip.
#[derive(Debug)]
pub struct SplatCloud {
    pub count: usize,
    pub positions: Vec<[f32; 3]>,
    /// LINEAR scales (exp already applied), × the same normalization scale.
    pub scales: Vec<[f32; 3]>,
    /// Unit quaternions `[x, y, z, w]` (glam order; PLY `rot_0` = w reordered).
    /// Zero-length source quats fall back to identity.
    pub rotations: Vec<[f32; 4]>,
    /// SH DC decoded: `0.28209479 · f_dc + 0.5`, floored at 0 (no upper clamp
    /// — the GPU packer clamps; `.splat` colors are stored display-ready).
    pub colors: Vec<[f32; 3]>,
    /// Post-sigmoid opacity, 0..1 (splats below 1/255 are culled at parse).
    pub opacities: Vec<f32>,
    /// View-dependent SH coefficients (bands 1–3), f16 bit patterns, laid out
    /// **channel-major**: `[0..15]` = R, `[15..30]` = G, `[30..45]` = B — the
    /// PLY `f_rest_*` order. Empty when [`sh_degree`](Self::sh_degree) is 0.
    /// f16 at parse time keeps a 1.3M-splat load near 90 MB instead of 180.
    pub sh: Vec<[u16; SH_COEFFS]>,
    /// 0 = DC only (no `f_rest`, or an unrecognized count), 1–3 = bands present.
    pub sh_degree: u8,
    /// Inverse of the `rotation_degrees` scene rotation. SH lobes are defined in
    /// the **source** frame, but `normalize_cloud` rotates positions and quats
    /// into the render frame — so the sim rotates the view direction back
    /// through this before evaluating, rather than SH-rotating 45 coefficients
    /// per splat (which needs Wigner-D matrices for bands 2–3).
    pub sh_rot_inv: glam::Mat3,
    /// Absolute path actually loaded.
    pub source_path: String,
    /// Vertex count in the file before cull/subsample (status UI).
    pub total_in_file: u32,
    pub transform: SplatTransform,
}

/// A downloadable demo scene. `file` lives under [`splat_dir`].
pub struct DemoScene {
    pub name: &'static str,
    pub file: &'static str,
    /// Download URL (raw `.ply`/`.splat`, not an archive). Hosted as a GitHub
    /// release asset on the non-version `demo-assets` tag; `ureq` follows the
    /// 302 to the CDN. Empty disables the Download button (shows a hint).
    pub url: &'static str,
    /// Approximate size shown in the confirm dialog.
    pub size_mb: u32,
}

pub const DEMO_SCENES: &[DemoScene] = &[DemoScene {
    name: "default",
    file: "phosphor_demo.ply",
    url: "https://github.com/kevinraymond/fosfora/releases/download/demo-assets/trooper.ply",
    size_mb: 42,
}];

pub fn demo_scene(name: &str) -> Option<&'static DemoScene> {
    DEMO_SCENES.iter().find(|d| d.name == name)
}

/// Is the named demo scene already on disk?
pub fn demo_scene_cached(name: &str) -> bool {
    demo_scene(name).is_some_and(|d| splat_dir().join(d.file).is_file())
}

/// Download the named demo scene on a background thread (mirrors
/// `depth::model::download_model`): .tmp → rename, cancellable, progress
/// 0–100 / 101 complete / 102 error.
pub fn download_demo_scene(name: &str) -> Arc<crate::download::DownloadProgress> {
    let progress = crate::download::DownloadProgress::new();
    let progress_clone = Arc::clone(&progress);
    let demo = demo_scene(name);
    let (url, file) = match demo {
        Some(d) if !d.url.is_empty() => (d.url.to_string(), d.file.to_string()),
        _ => {
            if let Ok(mut msg) = progress.error_message.lock() {
                *msg = Some(format!("demo scene '{name}' has no published URL yet"));
            }
            progress
                .progress
                .store(102, std::sync::atomic::Ordering::Relaxed);
            return progress;
        }
    };

    std::thread::Builder::new()
        .name("splat-demo-dl".into())
        .spawn(move || {
            let run = || -> anyhow::Result<()> {
                let dir = splat_dir();
                std::fs::create_dir_all(&dir)?;
                crate::download::download_file(&url, &dir.join(&file), &file, &progress_clone)?;
                Ok(())
            };
            match run() {
                Ok(()) => progress_clone
                    .progress
                    .store(101, std::sync::atomic::Ordering::Relaxed),
                Err(e) => {
                    log::error!("Splat demo download failed: {e}");
                    if let Ok(mut msg) = progress_clone.error_message.lock() {
                        *msg = Some(e.to_string());
                    }
                    progress_clone
                        .progress
                        .store(102, std::sync::atomic::Ordering::Relaxed);
                }
            }
        })
        .ok();

    progress
}

/// Where downloaded demo scenes live: `~/.config/fosfora/splats/`
/// (mirrors `depth::model::model_dir`).
pub fn splat_dir() -> PathBuf {
    crate::paths::config_root().join("splats")
}

/// Map a .pfx `splat.source` string to a filesystem path (no existence check):
/// `"demo:<name>"` → [`splat_dir`]`/<file>`, absolute paths verbatim, anything
/// else relative to `assets/splats/`.
fn resolve_source_path(source: &str) -> Result<PathBuf, String> {
    if let Some(name) = source.strip_prefix("demo:") {
        let demo = DEMO_SCENES
            .iter()
            .find(|d| d.name == name)
            .ok_or_else(|| format!("unknown demo scene '{name}'"))?;
        return Ok(splat_dir().join(demo.file));
    }
    let p = PathBuf::from(source);
    if p.is_absolute() {
        Ok(p)
    } else {
        Ok(crate::effect::loader::assets_dir()
            .join("splats")
            .join(source))
    }
}

/// Resolve a .pfx `splat.source` to an existing file, with actionable errors
/// (a missing demo names the exact path the downloader will fill).
pub fn resolve_source(source: &str) -> Result<PathBuf, String> {
    if source.is_empty() {
        return Err("splat source is empty".to_string());
    }
    let path = resolve_source_path(source)?;
    if path.exists() {
        Ok(path)
    } else if source.starts_with("demo:") {
        Err(format!(
            "demo scene not downloaded — expected at {}",
            path.display()
        ))
    } else {
        Err(format!("splat scene not found: {}", path.display()))
    }
}

// ---------------------------------------------------------------------------
// Deterministic RNG + reservoir subsampling
// ---------------------------------------------------------------------------

/// Fixed seed: same file + same target ⇒ bit-identical cloud (unit-tested).
const RESERVOIR_SEED: u64 = 0x5EED_5147_2026_0720;

fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Splats below this post-sigmoid opacity contribute nothing on screen and
/// are culled before subsampling (typically 5–20% of a trained scene).
const OPACITY_CULL: f32 = 1.0 / 255.0;

/// Number of view-dependent SH coefficients at degree 3: 15 bands × 3 channels.
/// Lower-degree captures fill a prefix per channel and leave the rest zero, so
/// one fixed-width record serves every degree.
pub const SH_COEFFS: usize = 45;

/// One decoded source splat, prior to normalization.
struct RawSplat {
    pos: [f32; 3],
    scale: [f32; 3],
    rot: [f32; 4],
    color: [f32; 3],
    opacity: f32,
    /// f16 bit patterns, channel-major; all zero when the file has no `f_rest`.
    sh: [u16; SH_COEFFS],
}

/// Reservoir sampler (Algorithm R) writing straight into the cloud's SoA
/// vectors — no intermediate AoS copy of the full scene.
struct Reservoir {
    target: usize,
    seen: u64,
    rng: u64,
}

impl Reservoir {
    fn new(target: usize) -> Self {
        Self {
            target: target.max(1),
            seen: 0,
            rng: RESERVOIR_SEED,
        }
    }

    fn offer(&mut self, cloud: &mut SplatCloud, s: RawSplat) {
        // SH rides the same reservoir decisions as the rest of the attributes,
        // so a subsampled scene keeps each retained splat's own coefficients.
        let keep_sh = cloud.sh_degree > 0;
        self.seen += 1;
        if cloud.positions.len() < self.target {
            cloud.positions.push(s.pos);
            cloud.scales.push(s.scale);
            cloud.rotations.push(s.rot);
            cloud.colors.push(s.color);
            cloud.opacities.push(s.opacity);
            if keep_sh {
                cloud.sh.push(s.sh);
            }
            return;
        }
        // Replace a random slot with probability target/seen.
        let j = (splitmix64(&mut self.rng) % self.seen) as usize;
        if j < self.target {
            cloud.positions[j] = s.pos;
            cloud.scales[j] = s.scale;
            cloud.rotations[j] = s.rot;
            cloud.colors[j] = s.color;
            cloud.opacities[j] = s.opacity;
            if keep_sh {
                cloud.sh[j] = s.sh;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Shared decode helpers
// ---------------------------------------------------------------------------

const SH_C0: f32 = 0.282_094_79;

fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

fn read_f32(rec: &[u8], off: usize) -> f32 {
    f32::from_le_bytes([rec[off], rec[off + 1], rec[off + 2], rec[off + 3]])
}

/// Normalize `[x,y,z,w]`; zero-length (corrupt/pruned splat) → identity.
fn normalize_quat(q: [f32; 4]) -> [f32; 4] {
    let len_sq = q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3];
    if len_sq < 1e-12 {
        return [0.0, 0.0, 0.0, 1.0];
    }
    let inv = len_sq.sqrt().recip();
    [q[0] * inv, q[1] * inv, q[2] * inv, q[3] * inv]
}

fn empty_cloud(path: &Path, total: u32) -> SplatCloud {
    SplatCloud {
        count: 0,
        positions: Vec::new(),
        scales: Vec::new(),
        rotations: Vec::new(),
        colors: Vec::new(),
        opacities: Vec::new(),
        sh: Vec::new(),
        sh_degree: 0,
        sh_rot_inv: glam::Mat3::IDENTITY,
        source_path: path.to_string_lossy().to_string(),
        total_in_file: total,
        transform: SplatTransform::default(),
    }
}

// ---------------------------------------------------------------------------
// PLY (3DGS binary little-endian)
// ---------------------------------------------------------------------------

/// Byte offsets of the required 3DGS properties within one vertex record.
/// Found by property NAME, so property-order variance between exporters is
/// handled; normals just contribute to the stride.
struct PlyLayout {
    vertex_count: u32,
    stride: usize,
    x: usize,
    y: usize,
    z: usize,
    f_dc: [usize; 3],
    opacity: usize,
    scale: [usize; 3],
    rot: [usize; 4],
    /// Byte offsets of `f_rest_0..N`, indexed by N (empty when absent, i.e.
    /// [`sh_degree`](Self::sh_degree) 0). Sorted by the numeric suffix, not by
    /// header order, for the same exporter-variance reason as the rest.
    f_rest: Vec<usize>,
    /// 0 (DC only) or 1–3, from the per-channel coefficient count.
    sh_degree: u8,
}

/// SH degree from the total `f_rest` count: coefficients are 3 channels ×
/// (`(deg+1)² − 1`) bands, so 9 → 1, 24 → 2, 45 → 3. Anything else is a format
/// we do not recognize; treat it as DC-only rather than mis-decode it.
fn sh_degree_from_count(total: usize) -> u8 {
    match total {
        9 => 1,
        24 => 2,
        SH_COEFFS => 3,
        _ => 0,
    }
}

fn ply_type_size(ty: &str) -> Option<usize> {
    match ty {
        "char" | "uchar" | "int8" | "uint8" => Some(1),
        "short" | "ushort" | "int16" | "uint16" => Some(2),
        "int" | "uint" | "int32" | "uint32" | "float" | "float32" => Some(4),
        "double" | "float64" => Some(8),
        _ => None,
    }
}

fn parse_ply_header(reader: &mut impl BufRead) -> Result<PlyLayout, String> {
    let mut line = String::new();
    let mut read_line = |line: &mut String| -> Result<(), String> {
        line.clear();
        reader
            .read_line(line)
            .map_err(|e| format!("PLY header read error: {e}"))?;
        if line.is_empty() {
            return Err("unexpected EOF in PLY header".to_string());
        }
        Ok(())
    };

    read_line(&mut line)?;
    if line.trim_end() != "ply" {
        return Err("not a PLY file (missing 'ply' magic)".to_string());
    }

    let mut vertex_count: Option<u32> = None;
    let mut in_vertex_element = false;
    let mut stride = 0usize;
    let mut offsets: Vec<(String, usize, String)> = Vec::new(); // (name, offset, type)
    let mut format_seen = false;

    loop {
        read_line(&mut line)?;
        let trimmed = line.trim_end();
        let mut parts = trimmed.split_ascii_whitespace();
        match parts.next() {
            Some("format") => {
                match parts.next() {
                    Some("binary_little_endian") => {}
                    Some("ascii") => {
                        return Err(
                            "ASCII PLY not supported — re-export as binary_little_endian"
                                .to_string(),
                        );
                    }
                    Some("binary_big_endian") => {
                        return Err("big-endian PLY not supported".to_string());
                    }
                    other => return Err(format!("unknown PLY format {other:?}")),
                }
                format_seen = true;
            }
            Some("comment" | "obj_info") => {}
            Some("element") => {
                let name = parts.next().unwrap_or("");
                if name == "vertex" {
                    let n: u32 = parts
                        .next()
                        .and_then(|c| c.parse().ok())
                        .ok_or("PLY vertex element has no count")?;
                    vertex_count = Some(n);
                    in_vertex_element = true;
                } else if vertex_count.is_none() {
                    // Vertex data must come first in the payload for us to
                    // stride-read it; 3DGS exports always satisfy this.
                    return Err(format!(
                        "unsupported PLY: element '{name}' precedes vertex data"
                    ));
                } else {
                    in_vertex_element = false; // trailing elements: ignored
                }
            }
            Some("property") => {
                if !in_vertex_element {
                    continue;
                }
                let ty = parts.next().unwrap_or("");
                if ty == "list" {
                    return Err("PLY list properties not supported (not a 3DGS export)".to_string());
                }
                let size =
                    ply_type_size(ty).ok_or_else(|| format!("unknown PLY property type '{ty}'"))?;
                let name = parts.next().unwrap_or("").to_string();
                offsets.push((name, stride, ty.to_string()));
                stride += size;
            }
            Some("end_header") => break,
            _ => {}
        }
    }

    if !format_seen {
        return Err("PLY header has no format line".to_string());
    }
    let vertex_count = vertex_count.ok_or("PLY has no vertex element")?;

    let find = |name: &str| -> Result<usize, String> {
        let (_, off, ty) = offsets
            .iter()
            .find(|(n, _, _)| n == name)
            .ok_or_else(|| format!("PLY missing required 3DGS property '{name}'"))?;
        if ply_type_size(ty) != Some(4) || !(ty == "float" || ty == "float32") {
            return Err(format!(
                "PLY property '{name}' must be float32 (got '{ty}')"
            ));
        }
        Ok(*off)
    };

    // View-dependent SH: gather every float32 `f_rest_N` and order by N. A
    // non-float or out-of-range suffix means this is not an INRIA-style export,
    // so drop the whole set to DC-only instead of decoding garbage.
    let mut f_rest: Vec<(usize, usize)> = Vec::new(); // (index, offset)
    for (name, off, ty) in &offsets {
        let Some(suffix) = name.strip_prefix("f_rest_") else {
            continue;
        };
        match suffix.parse::<usize>() {
            Ok(i) if i < SH_COEFFS && (ty == "float" || ty == "float32") => f_rest.push((i, *off)),
            _ => {
                f_rest.clear();
                break;
            }
        }
    }
    f_rest.sort_unstable_by_key(|(i, _)| *i);
    let sh_degree = if f_rest.iter().enumerate().all(|(n, (i, _))| n == *i) {
        sh_degree_from_count(f_rest.len())
    } else {
        0 // gaps in the sequence — not a layout we can index
    };
    let f_rest: Vec<usize> = if sh_degree > 0 {
        f_rest.into_iter().map(|(_, off)| off).collect()
    } else {
        Vec::new()
    };

    Ok(PlyLayout {
        vertex_count,
        stride,
        x: find("x")?,
        y: find("y")?,
        z: find("z")?,
        f_dc: [find("f_dc_0")?, find("f_dc_1")?, find("f_dc_2")?],
        opacity: find("opacity")?,
        scale: [find("scale_0")?, find("scale_1")?, find("scale_2")?],
        rot: [
            find("rot_0")?,
            find("rot_1")?,
            find("rot_2")?,
            find("rot_3")?,
        ],
        f_rest,
        sh_degree,
    })
}

/// Vertices decoded per streaming chunk (× stride ≈ 15 MB at the 3DGS
/// stride of 248 B — bounds peak memory regardless of file size).
const CHUNK_VERTS: usize = 65_536;

fn parse_ply_stream(
    reader: &mut impl BufRead,
    path: &Path,
    target_count: u32,
    progress: &AtomicU8,
    cancel: &AtomicBool,
) -> Result<SplatCloud, String> {
    let layout = parse_ply_header(reader)?;
    let mut cloud = empty_cloud(path, layout.vertex_count);
    cloud.sh_degree = layout.sh_degree;
    let mut reservoir = Reservoir::new(target_count as usize);

    let total = layout.vertex_count as usize;
    let mut remaining = total;
    let mut chunk = vec![0u8; CHUNK_VERTS * layout.stride];
    while remaining > 0 {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".to_string());
        }
        let n = remaining.min(CHUNK_VERTS);
        let bytes = &mut chunk[..n * layout.stride];
        reader
            .read_exact(bytes)
            .map_err(|e| format!("PLY truncated ({remaining} vertices missing): {e}"))?;
        for rec in bytes.chunks_exact(layout.stride) {
            let opacity = sigmoid(read_f32(rec, layout.opacity));
            if opacity < OPACITY_CULL {
                continue;
            }
            let rot = normalize_quat([
                read_f32(rec, layout.rot[1]), // PLY rot_0 = w (real first) →
                read_f32(rec, layout.rot[2]), // reorder to glam [x, y, z, w]
                read_f32(rec, layout.rot[3]),
                read_f32(rec, layout.rot[0]),
            ]);
            reservoir.offer(
                &mut cloud,
                RawSplat {
                    pos: [
                        read_f32(rec, layout.x),
                        read_f32(rec, layout.y),
                        read_f32(rec, layout.z),
                    ],
                    scale: [
                        read_f32(rec, layout.scale[0]).exp(),
                        read_f32(rec, layout.scale[1]).exp(),
                        read_f32(rec, layout.scale[2]).exp(),
                    ],
                    rot,
                    color: [
                        (SH_C0 * read_f32(rec, layout.f_dc[0]) + 0.5).max(0.0),
                        (SH_C0 * read_f32(rec, layout.f_dc[1]) + 0.5).max(0.0),
                        (SH_C0 * read_f32(rec, layout.f_dc[2]) + 0.5).max(0.0),
                    ],
                    opacity,
                    sh: {
                        // Raw coefficients — the SH basis constants are applied
                        // GPU-side at evaluation, exactly as the reference
                        // renderers do. Stays all-zero for DC-only files.
                        let mut sh = [0u16; SH_COEFFS];
                        for (dst, &off) in sh.iter_mut().zip(layout.f_rest.iter()) {
                            *dst = f32_to_f16(read_f32(rec, off));
                        }
                        sh
                    },
                },
            );
        }
        remaining -= n;
        let done = total - remaining;
        progress.store(((done * 100) / total.max(1)) as u8, Ordering::Relaxed);
    }

    cloud.count = cloud.positions.len();
    Ok(cloud)
}

// ---------------------------------------------------------------------------
// .splat (antimatter15): 32-byte records, no header
// ---------------------------------------------------------------------------

const SPLAT_RECORD: usize = 32;

fn parse_splat_stream(
    reader: &mut impl BufRead,
    path: &Path,
    file_len: u64,
    target_count: u32,
    progress: &AtomicU8,
    cancel: &AtomicBool,
) -> Result<SplatCloud, String> {
    if file_len == 0 || !file_len.is_multiple_of(SPLAT_RECORD as u64) {
        return Err(format!(
            ".splat length {file_len} is not a multiple of the 32-byte record"
        ));
    }
    let total = (file_len / SPLAT_RECORD as u64) as usize;
    let mut cloud = empty_cloud(path, total as u32);
    let mut reservoir = Reservoir::new(target_count as usize);

    let mut remaining = total;
    let mut chunk = vec![0u8; CHUNK_VERTS * SPLAT_RECORD];
    while remaining > 0 {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".to_string());
        }
        let n = remaining.min(CHUNK_VERTS);
        let bytes = &mut chunk[..n * SPLAT_RECORD];
        reader
            .read_exact(bytes)
            .map_err(|e| format!(".splat truncated: {e}"))?;
        for rec in bytes.chunks_exact(SPLAT_RECORD) {
            // Layout: pos 3×f32 | scale 3×f32 (linear) | rgba 4×u8 | quat 4×u8
            let opacity = rec[27] as f32 / 255.0;
            if opacity < OPACITY_CULL {
                continue;
            }
            let q = |b: u8| (b as f32 - 128.0) / 128.0;
            let rot = normalize_quat([
                q(rec[29]), // byte order (w, x, y, z) → [x, y, z, w]
                q(rec[30]),
                q(rec[31]),
                q(rec[28]),
            ]);
            reservoir.offer(
                &mut cloud,
                RawSplat {
                    pos: [read_f32(rec, 0), read_f32(rec, 4), read_f32(rec, 8)],
                    scale: [read_f32(rec, 12), read_f32(rec, 16), read_f32(rec, 20)],
                    rot,
                    color: [
                        rec[24] as f32 / 255.0,
                        rec[25] as f32 / 255.0,
                        rec[26] as f32 / 255.0,
                    ],
                    opacity,
                    // The .splat format carries no view-dependent bands by
                    // design — colors are stored display-ready.
                    sh: [0u16; SH_COEFFS],
                },
            );
        }
        remaining -= n;
        let done = total - remaining;
        progress.store(((done * 100) / total.max(1)) as u8, Ordering::Relaxed);
    }

    cloud.count = cloud.positions.len();
    Ok(cloud)
}

// ---------------------------------------------------------------------------
// Normalization
// ---------------------------------------------------------------------------

fn median_of(values: &mut [f32]) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    let mid = values.len() / 2;
    let (_, m, _) = values.select_nth_unstable_by(mid, f32::total_cmp);
    *m
}

/// Drop every splat further than `far_clip` × `radius_p95` from `center`.
/// Returns the number removed. All SoA vectors move in lockstep — a splat that
/// loses only some of its attributes is worse than one that stays.
fn clip_far_field(
    cloud: &mut SplatCloud,
    center: [f32; 3],
    radius_p95: f32,
    far_clip: f32,
) -> usize {
    if far_clip <= 0.0 || radius_p95 <= 1e-6 {
        return 0;
    }
    let limit = far_clip * radius_p95;
    let limit_sq = limit * limit;
    let keep: Vec<usize> = (0..cloud.count)
        .filter(|&i| {
            let p = cloud.positions[i];
            let d = [p[0] - center[0], p[1] - center[1], p[2] - center[2]];
            d[0] * d[0] + d[1] * d[1] + d[2] * d[2] <= limit_sq
        })
        .collect();
    let dropped = cloud.count - keep.len();
    if dropped == 0 {
        return 0;
    }
    let pick = |src: &[[f32; 3]]| -> Vec<[f32; 3]> { keep.iter().map(|&i| src[i]).collect() };
    cloud.positions = pick(&cloud.positions);
    cloud.scales = pick(&cloud.scales);
    cloud.colors = pick(&cloud.colors);
    cloud.rotations = keep.iter().map(|&i| cloud.rotations[i]).collect();
    cloud.opacities = keep.iter().map(|&i| cloud.opacities[i]).collect();
    if !cloud.sh.is_empty() {
        cloud.sh = keep.iter().map(|&i| cloud.sh[i]).collect();
    }
    cloud.count = keep.len();
    dropped
}

/// Recenter to the per-axis median, scale so the 95th-percentile radius is
/// 1.0 × `scene_scale`, then apply the .pfx Euler rotation offsets. Runs on
/// the retained (subsampled) set — a representative sample by construction.
/// `far_clip` drops the unbounded-capture far field first (see [`SplatDef`]).
fn normalize_cloud(cloud: &mut SplatCloud, opts: SceneOptions) {
    let SceneOptions {
        scene_scale,
        rotation_degrees,
        far_clip,
    } = opts;
    if cloud.count == 0 {
        return;
    }
    let mut axis: Vec<f32> = Vec::with_capacity(cloud.count);
    let mut center = [0.0f32; 3];
    for (a, c) in center.iter_mut().enumerate() {
        axis.clear();
        axis.extend(cloud.positions.iter().map(|p| p[a]));
        *c = median_of(&mut axis);
    }

    let mut dist_sq: Vec<f32> = cloud
        .positions
        .iter()
        .map(|p| {
            let d = [p[0] - center[0], p[1] - center[1], p[2] - center[2]];
            d[0] * d[0] + d[1] * d[1] + d[2] * d[2]
        })
        .collect();
    let p95_idx = ((dist_sq.len() - 1) as f32 * 0.95) as usize;
    let (_, v, _) = dist_sq.select_nth_unstable_by(p95_idx, f32::total_cmp);
    let radius_p95 = v.sqrt();

    // Cull the far field BEFORE scaling, and keep the p95 measured on the full
    // set: recomputing it afterwards would zoom the framing by however much was
    // dropped, so an unbounded capture would change size the moment far_clip is
    // touched. The clip only removes outliers, never rescales what remains.
    let dropped = clip_far_field(cloud, center, radius_p95, far_clip);
    if dropped > 0 {
        log::info!(
            "Splat far-field clip: dropped {dropped} splats beyond {far_clip}× the p95 radius \
             ({} kept)",
            cloud.count
        );
    }

    let norm = if radius_p95 > 1e-6 {
        radius_p95.recip()
    } else {
        1.0
    };
    let s = norm * if scene_scale > 0.0 { scene_scale } else { 1.0 };

    let rotate = rotation_degrees != [0.0, 0.0, 0.0];
    let rot_q = glam::Quat::from_euler(
        glam::EulerRot::XYZ,
        rotation_degrees[0].to_radians(),
        rotation_degrees[1].to_radians(),
        rotation_degrees[2].to_radians(),
    );

    for p in &mut cloud.positions {
        let mut v = glam::Vec3::new(
            (p[0] - center[0]) * s,
            (p[1] - center[1]) * s,
            (p[2] - center[2]) * s,
        );
        if rotate {
            v = rot_q * v;
        }
        *p = v.to_array();
    }
    for sc in &mut cloud.scales {
        sc[0] *= s;
        sc[1] *= s;
        sc[2] *= s;
    }
    if rotate {
        for r in &mut cloud.rotations {
            let q = rot_q * glam::Quat::from_xyzw(r[0], r[1], r[2], r[3]);
            *r = q.normalize().to_array();
        }
    }
    // SH coefficients are NOT rotated (bands 2–3 would need Wigner-D matrices);
    // the sim rotates the view direction back into the source frame instead.
    cloud.sh_rot_inv = glam::Mat3::from_quat(rot_q.inverse());

    cloud.transform = SplatTransform {
        center,
        scale: s,
        radius_p95,
    };
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Parse + cull + subsample + normalize a scene file. Blocking — call from a
/// background thread ([`SplatSceneLoader`] does). `progress` is 0–100 over
/// the file's vertex payload; `cancel` aborts at the next chunk boundary.
pub fn load_splat_file(
    path: &Path,
    target_count: u32,
    opts: SceneOptions,
    progress: &AtomicU8,
    cancel: &AtomicBool,
) -> Result<SplatCloud, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    let file_len = file
        .metadata()
        .map_err(|e| format!("stat {}: {e}", path.display()))?
        .len();
    let mut reader = BufReader::with_capacity(1 << 20, file);

    // Extension first, then header sniff: a .ply payload starts with "ply".
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let head = reader
        .fill_buf()
        .map_err(|e| format!("read {}: {e}", path.display()))?;
    let looks_ply = head.starts_with(b"ply");

    let mut cloud = if ext == "ply" || (ext != "splat" && looks_ply) {
        parse_ply_stream(&mut reader, path, target_count, progress, cancel)?
    } else {
        parse_splat_stream(&mut reader, path, file_len, target_count, progress, cancel)?
    };

    if cloud.count == 0 {
        return Err("scene contains no visible splats".to_string());
    }
    normalize_cloud(&mut cloud, opts);
    progress.store(100, Ordering::Relaxed);
    Ok(cloud)
}

// ---------------------------------------------------------------------------
// Background loader
// ---------------------------------------------------------------------------

/// Result from background splat scene loading. Carries the target layer
/// (effect-load and preset-restore target specific layers, unlike the
/// active-layer `ParticleSourceLoader`).
pub enum SplatLoadResult {
    Loaded {
        layer_idx: usize,
        cloud: Box<SplatCloud>,
    },
    Error {
        layer_idx: usize,
        message: String,
    },
}

/// Background scene loader — single in-flight load; a new request cancels the
/// previous one (generation counter + cancel flag). Failure changes nothing
/// GPU-side: the layer keeps rendering its previous (or empty) scene.
pub struct SplatSceneLoader {
    result_rx: Receiver<(u64, SplatLoadResult)>,
    generation: u64,
    cancel: Arc<AtomicBool>,
    pub progress: Arc<AtomicU8>,
    /// A load (or picker dialog) is in flight — panel spinner state.
    pub loading: bool,
    /// Filename being decoded (or "choosing file…"), for the panel.
    pub loading_name: String,
    /// Most recent load failure, cleared by the next successful load.
    pub last_error: Option<String>,
}

impl SplatSceneLoader {
    pub fn new() -> Self {
        let (_tx, rx) = bounded(1);
        Self {
            result_rx: rx,
            generation: 0,
            cancel: Arc::new(AtomicBool::new(false)),
            progress: Arc::new(AtomicU8::new(0)),
            loading: false,
            loading_name: String::new(),
            last_error: None,
        }
    }

    fn begin_request(&mut self, name: String) -> u64 {
        self.cancel.store(true, Ordering::Relaxed); // abort any previous load
        self.cancel = Arc::new(AtomicBool::new(false));
        self.generation += 1;
        self.progress.store(0, Ordering::Relaxed);
        self.loading = true;
        self.loading_name = name;
        self.generation
    }

    /// Start loading a scene in the background for `layer_idx`, subsampled to
    /// `target_count` with the .pfx scene transform applied.
    pub fn load(&mut self, path: PathBuf, target_count: u32, opts: SceneOptions, layer_idx: usize) {
        let load_gen = self.begin_request(
            path.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default(),
        );

        let (tx, rx) = bounded(1);
        self.result_rx = rx;
        let cancel = Arc::clone(&self.cancel);
        let progress = Arc::clone(&self.progress);

        thread::Builder::new()
            .name("splat-scene-loader".into())
            .spawn(move || {
                let result = match load_splat_file(&path, target_count, opts, &progress, &cancel) {
                    Ok(cloud) => SplatLoadResult::Loaded {
                        layer_idx,
                        cloud: Box::new(cloud),
                    },
                    Err(message) => SplatLoadResult::Error { layer_idx, message },
                };
                let _ = tx.send((load_gen, result));
            })
            .expect("failed to spawn splat scene loader thread");
    }

    /// Open a file dialog (background thread, `ParticleSourceLoader` pattern)
    /// then decode the chosen scene. A cancelled dialog drops the sender →
    /// `try_recv` sees Disconnected and resets `loading`.
    pub fn open_dialog(&mut self, target_count: u32, opts: SceneOptions, layer_idx: usize) {
        let load_gen = self.begin_request("choosing file…".to_string());

        let (tx, rx) = bounded(1);
        self.result_rx = rx;
        let cancel = Arc::clone(&self.cancel);
        let progress = Arc::clone(&self.progress);

        thread::Builder::new()
            .name("splat-scene-dialog".into())
            .spawn(move || {
                let dialog = rfd::FileDialog::new()
                    .set_title("Load Gaussian Splat Scene")
                    .add_filter("Splat scenes", &["ply", "splat"]);
                if let Some(path) = dialog.pick_file() {
                    let result =
                        match load_splat_file(&path, target_count, opts, &progress, &cancel) {
                            Ok(cloud) => SplatLoadResult::Loaded {
                                layer_idx,
                                cloud: Box::new(cloud),
                            },
                            Err(message) => SplatLoadResult::Error { layer_idx, message },
                        };
                    let _ = tx.send((load_gen, result));
                }
            })
            .expect("failed to spawn splat scene dialog thread");
    }

    /// Check for a completed load. Stale results from cancelled loads are
    /// dropped by generation; a cancelled dialog resets the loading state.
    pub fn try_recv(&mut self) -> Option<SplatLoadResult> {
        match self.result_rx.try_recv() {
            Ok((load_gen, result)) if load_gen == self.generation => {
                self.loading = false;
                self.loading_name.clear();
                match &result {
                    SplatLoadResult::Loaded { .. } => self.last_error = None,
                    SplatLoadResult::Error { message, .. } => {
                        self.last_error = Some(message.clone());
                    }
                }
                Some(result)
            }
            Ok(_) => None,
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.loading = false;
                self.loading_name.clear();
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Canonical 3DGS property list (INRIA export order).
    const CANONICAL_PROPS: &[&str] = &[
        "x",
        "y",
        "z",
        "nx",
        "ny",
        "nz",
        "f_dc_0",
        "f_dc_1",
        "f_dc_2",
        "f_rest_all",
        "opacity",
        "scale_0",
        "scale_1",
        "scale_2",
        "rot_0",
        "rot_1",
        "rot_2",
        "rot_3",
    ];

    /// Distinct, f16-exact value for `f_rest_n` (1/8 steps round-trip through
    /// f16 without loss, so tests can assert equality rather than a tolerance).
    fn sh_probe_value(n: usize) -> f32 {
        1.0 + n as f32 / 8.0
    }

    /// Raw (pre-activation) source values for one test vertex.
    #[derive(Clone, Copy, Default)]
    struct TestSplat {
        pos: [f32; 3],
        f_dc: [f32; 3],
        opacity: f32,    // pre-sigmoid
        scale: [f32; 3], // pre-exp
        rot: [f32; 4],   // (w, x, y, z) as stored
    }

    /// Build binary PLY bytes. "f_rest_all" in `props` expands to the 45
    /// degree-1..3 SH floats (stride documentation doubles as a test).
    fn make_test_ply(props: &[&str], verts: &[TestSplat]) -> Vec<u8> {
        use std::fmt::Write as _;
        let mut header = String::from("ply\nformat binary_little_endian 1.0\ncomment splat test\n");
        let _ = writeln!(header, "element vertex {}", verts.len());
        let mut expanded: Vec<String> = Vec::new();
        for p in props {
            if *p == "f_rest_all" {
                for i in 0..45 {
                    expanded.push(format!("f_rest_{i}"));
                }
            } else {
                expanded.push((*p).to_string());
            }
        }
        for name in &expanded {
            let _ = writeln!(header, "property float {name}");
        }
        header.push_str("end_header\n");

        let mut bytes = header.into_bytes();
        for v in verts {
            for name in &expanded {
                let val: f32 = match name.as_str() {
                    "x" => v.pos[0],
                    "y" => v.pos[1],
                    "z" => v.pos[2],
                    "f_dc_0" => v.f_dc[0],
                    "f_dc_1" => v.f_dc[1],
                    "f_dc_2" => v.f_dc[2],
                    "opacity" => v.opacity,
                    "scale_0" => v.scale[0],
                    "scale_1" => v.scale[1],
                    "scale_2" => v.scale[2],
                    "rot_0" => v.rot[0],
                    "rot_1" => v.rot[1],
                    "rot_2" => v.rot[2],
                    "rot_3" => v.rot[3],
                    // f_rest_N gets a value that encodes N, so an SH decode
                    // that transposes channels or loses ordering is visible.
                    other => match other.strip_prefix("f_rest_") {
                        Some(n) => sh_probe_value(n.parse::<usize>().unwrap()),
                        None => 0.123, // normals filler
                    },
                };
                bytes.extend_from_slice(&val.to_le_bytes());
            }
        }
        bytes
    }

    fn parse_bytes(bytes: &[u8], target: u32) -> Result<SplatCloud, String> {
        let progress = AtomicU8::new(0);
        let cancel = AtomicBool::new(false);
        parse_ply_stream(
            &mut std::io::Cursor::new(bytes),
            Path::new("test.ply"),
            target,
            &progress,
            &cancel,
        )
    }

    fn visible_splat() -> TestSplat {
        TestSplat {
            opacity: 4.0, // sigmoid ≈ 0.982
            rot: [1.0, 0.0, 0.0, 0.0],
            ..Default::default()
        }
    }

    #[test]
    fn ply_header_parses_canonical_order() {
        let v = visible_splat();
        let bytes = make_test_ply(CANONICAL_PROPS, &[v]);
        let cloud = parse_bytes(&bytes, 10).unwrap();
        assert_eq!(cloud.count, 1);
        assert_eq!(cloud.total_in_file, 1);
        // Stride = 62 floats (17 named + 45 f_rest) — a decode error would
        // misread every field of vertex 0.
        assert!((cloud.opacities[0] - sigmoid(4.0)).abs() < 1e-6);
    }

    #[test]
    fn ply_header_property_order_variance() {
        // opacity/scale/rot before position; parser must find by name.
        let props = &[
            "opacity", "scale_0", "scale_1", "scale_2", "rot_0", "rot_1", "rot_2", "rot_3",
            "f_dc_0", "f_dc_1", "f_dc_2", "x", "y", "z",
        ];
        let mut v = visible_splat();
        v.pos = [1.0, 2.0, 3.0];
        let bytes = make_test_ply(props, &[v, v]);
        let cloud = parse_bytes(&bytes, 10).unwrap();
        assert_eq!(cloud.count, 2);
        assert_eq!(cloud.positions[0], [1.0, 2.0, 3.0]);
    }

    #[test]
    fn ply_rejects_ascii_with_clear_error() {
        let bytes = b"ply\nformat ascii 1.0\nelement vertex 0\nend_header\n";
        let err = parse_bytes(bytes, 10).unwrap_err();
        assert!(err.contains("ASCII"), "unhelpful error: {err}");
        assert!(err.contains("binary_little_endian"), "no fix hint: {err}");
    }

    #[test]
    fn ply_rejects_big_endian() {
        let bytes = b"ply\nformat binary_big_endian 1.0\nelement vertex 0\nend_header\n";
        assert!(parse_bytes(bytes, 10).unwrap_err().contains("big-endian"));
    }

    #[test]
    fn ply_missing_property_names_it() {
        let props = &["x", "y", "z", "f_dc_0", "f_dc_1", "f_dc_2", "opacity"];
        let bytes = make_test_ply(props, &[]);
        let err = parse_bytes(&bytes, 10).unwrap_err();
        assert!(
            err.contains("scale_0"),
            "should name the missing prop: {err}"
        );
    }

    #[test]
    fn ply_stride_skips_f_rest() {
        // Vertex 1 only decodes correctly if the 45 f_rest floats (and
        // normals) contribute exactly their bytes to the stride.
        let mut a = visible_splat();
        a.pos = [1.0, 0.0, 0.0];
        let mut b = visible_splat();
        b.pos = [0.0, 5.0, 0.0];
        b.f_dc = [0.5, 0.0, 0.0];
        let bytes = make_test_ply(CANONICAL_PROPS, &[a, b]);
        let cloud = parse_bytes(&bytes, 10).unwrap();
        assert_eq!(cloud.positions[1], [0.0, 5.0, 0.0]);
        assert!((cloud.colors[1][0] - (SH_C0 * 0.5 + 0.5)).abs() < 1e-6);
    }

    /// A canonical property list whose `f_rest` block has exactly `n` entries.
    fn props_with_f_rest(n: usize) -> Vec<String> {
        CANONICAL_PROPS
            .iter()
            .flat_map(|p| {
                if *p == "f_rest_all" {
                    (0..n).map(|i| format!("f_rest_{i}")).collect()
                } else {
                    vec![(*p).to_string()]
                }
            })
            .collect()
    }

    /// PLY bytes for the canonical layout with an `n`-entry `f_rest` block.
    fn make_ply_with_f_rest(n: usize, verts: &[TestSplat]) -> Vec<u8> {
        let props = props_with_f_rest(n);
        let refs: Vec<&str> = props.iter().map(String::as_str).collect();
        make_test_ply(&refs, verts)
    }

    fn parse_with_f_rest(n: usize) -> SplatCloud {
        // Two vertices: the second only decodes if the f_rest block contributed
        // exactly its bytes to the stride.
        let bytes = make_ply_with_f_rest(n, &[visible_splat(), visible_splat()]);
        parse_bytes(&bytes, 10).unwrap()
    }

    #[test]
    fn ply_sh_decodes_all_45_in_source_order() {
        let cloud = parse_with_f_rest(SH_COEFFS);
        assert_eq!(cloud.sh_degree, 3);
        assert_eq!(cloud.sh.len(), 2);
        // Channel-major: f_rest_0..14 = R, 15..29 = G, 30..44 = B. A transposed
        // decode would still fill 45 slots — only the ORDER catches it, and
        // only if it is checked on every index.
        for v in 0..2 {
            for i in 0..SH_COEFFS {
                assert_eq!(
                    cloud.sh[v][i],
                    f32_to_f16(sh_probe_value(i)),
                    "vertex {v} coefficient {i} out of order"
                );
            }
        }
    }

    #[test]
    fn ply_sh_degree_from_coefficient_count() {
        assert_eq!(parse_with_f_rest(9).sh_degree, 1);
        assert_eq!(parse_with_f_rest(24).sh_degree, 2);
        assert_eq!(parse_with_f_rest(45).sh_degree, 3);
        // Degree 1 fills a 3-wide prefix per channel and leaves the rest zero.
        let deg1 = parse_with_f_rest(9);
        assert_eq!(deg1.sh[0][8], f32_to_f16(sh_probe_value(8)));
        assert_eq!(deg1.sh[0][9], 0);
    }

    #[test]
    fn ply_without_f_rest_is_dc_only() {
        let cloud = parse_with_f_rest(0);
        assert_eq!(cloud.sh_degree, 0);
        // No per-splat allocation at all for a DC-only capture.
        assert!(cloud.sh.is_empty());
    }

    #[test]
    fn ply_unrecognized_f_rest_count_falls_back_to_dc() {
        // 12 coefficients is not 3×(3|8|15) — decoding it as if it were would
        // shift every band. Must degrade to DC, not guess.
        let cloud = parse_with_f_rest(12);
        assert_eq!(cloud.sh_degree, 0);
        assert!(cloud.sh.is_empty());
        // ...and the stride is still right, so geometry is unaffected.
        assert_eq!(cloud.count, 2);
    }

    #[test]
    fn ply_sh_gap_in_sequence_falls_back_to_dc() {
        // f_rest_0..7 plus f_rest_9: the right COUNT (9) but a hole, so index N
        // no longer means band N. Must not be decoded as degree 1.
        let mut props: Vec<String> = props_with_f_rest(8);
        let at = props.iter().position(|p| p == "opacity").unwrap();
        props.insert(at, "f_rest_9".to_string());
        let refs: Vec<&str> = props.iter().map(String::as_str).collect();
        let bytes = make_test_ply(&refs, &[visible_splat()]);
        let cloud = parse_bytes(&bytes, 10).unwrap();
        assert_eq!(cloud.sh_degree, 0);
    }

    #[test]
    fn sh_survives_subsampling() {
        // The reservoir must move SH in lockstep with the other attributes, or
        // splat i renders with splat j's view-dependent colour.
        let mut verts = Vec::new();
        for i in 0..64 {
            let mut v = visible_splat();
            v.pos = [i as f32, 0.0, 0.0];
            verts.push(v);
        }
        let bytes = make_ply_with_f_rest(SH_COEFFS, &verts);
        let cloud = parse_bytes(&bytes, 8).unwrap();
        assert_eq!(cloud.count, 8);
        assert_eq!(cloud.sh.len(), 8, "SH must be subsampled with the rest");
    }

    #[test]
    fn normalization_records_inverse_sh_rotation() {
        // SH is evaluated in the SOURCE frame, so the recorded matrix must undo
        // the rotation the loader applied to the geometry.
        let cloud = {
            let bytes = make_ply_with_f_rest(SH_COEFFS, &[visible_splat()]);
            let mut c = parse_bytes(&bytes, 10).unwrap();
            normalize_cloud(
                &mut c,
                SceneOptions {
                    rotation_degrees: [90.0, 0.0, 0.0],
                    ..Default::default()
                },
            );
            c
        };
        // A source-frame +Y direction must come back out of the render frame.
        let fwd = glam::Quat::from_euler(glam::EulerRot::XYZ, 90f32.to_radians(), 0.0, 0.0);
        let rendered = fwd * glam::Vec3::Y;
        let back = cloud.sh_rot_inv * rendered;
        assert!((back - glam::Vec3::Y).length() < 1e-5, "got {back:?}");
    }

    #[test]
    fn ply_activation_math() {
        let mut v = visible_splat();
        v.opacity = 0.0; // sigmoid → 0.5
        v.f_dc = [0.0, 1.0, -0.5];
        v.scale = [0.0, 2.0_f32.ln(), 1.0];
        v.rot = [0.0, 0.0, 0.0, 1.0]; // (w,x,y,z) → glam [0,0,1,0]
        let bytes = make_test_ply(CANONICAL_PROPS, &[v]);
        let cloud = parse_bytes(&bytes, 10).unwrap();
        assert!((cloud.opacities[0] - 0.5).abs() < 1e-5);
        assert!((cloud.colors[0][0] - 0.5).abs() < 1e-5);
        assert!((cloud.colors[0][1] - (SH_C0 + 0.5)).abs() < 1e-5);
        assert!((cloud.colors[0][2] - (0.5 - SH_C0 * 0.5)).abs() < 1e-5);
        assert!((cloud.scales[0][0] - 1.0).abs() < 1e-5);
        assert!((cloud.scales[0][1] - 2.0).abs() < 1e-5);
        assert!((cloud.scales[0][2] - std::f32::consts::E).abs() < 1e-4);
        assert_eq!(cloud.rotations[0], [0.0, 0.0, 1.0, 0.0]);
    }

    #[test]
    fn quat_zero_length_falls_back_identity() {
        let mut v = visible_splat();
        v.rot = [0.0, 0.0, 0.0, 0.0];
        let bytes = make_test_ply(CANONICAL_PROPS, &[v]);
        let cloud = parse_bytes(&bytes, 10).unwrap();
        assert_eq!(cloud.rotations[0], [0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn opacity_cull_drops_invisible() {
        let mut dim = visible_splat();
        dim.opacity = -12.0; // sigmoid ≈ 6e-6 < 1/255
        let bytes = make_test_ply(CANONICAL_PROPS, &[visible_splat(), dim, visible_splat()]);
        let cloud = parse_bytes(&bytes, 10).unwrap();
        assert_eq!(cloud.count, 2);
        assert_eq!(cloud.total_in_file, 3);
    }

    fn many_splats(n: usize) -> Vec<TestSplat> {
        (0..n)
            .map(|i| {
                let mut v = visible_splat();
                v.pos = [i as f32, (i * 7 % 13) as f32, -(i as f32) * 0.5];
                v
            })
            .collect()
    }

    #[test]
    fn subsample_deterministic() {
        let bytes = make_test_ply(CANONICAL_PROPS, &many_splats(500));
        let a = parse_bytes(&bytes, 100).unwrap();
        let b = parse_bytes(&bytes, 100).unwrap();
        assert_eq!(a.positions, b.positions);
        assert_eq!(a.rotations, b.rotations);
        assert_eq!(a.opacities, b.opacities);
    }

    #[test]
    fn subsample_hits_target() {
        let bytes = make_test_ply(CANONICAL_PROPS, &many_splats(500));
        let cloud = parse_bytes(&bytes, 100).unwrap();
        assert_eq!(cloud.count, 100);
        assert_eq!(cloud.total_in_file, 500);
    }

    #[test]
    fn subsample_keeps_all_when_under_target() {
        let bytes = make_test_ply(CANONICAL_PROPS, &many_splats(50));
        let cloud = parse_bytes(&bytes, 100).unwrap();
        assert_eq!(cloud.count, 50);
        // Under-target parse preserves file order exactly.
        assert_eq!(cloud.positions[10][0], 10.0);
    }

    #[test]
    fn splat_format_parses_32byte_records() {
        // One record: pos (1,2,3), scale (0.5,0.5,0.5), color (255,128,0,255),
        // quat bytes (w,x,y,z) = (255,128,128,128) → ~identity.
        let mut bytes = Vec::new();
        for v in [1.0f32, 2.0, 3.0, 0.5, 0.5, 0.5] {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        bytes.extend_from_slice(&[255, 128, 0, 255]);
        bytes.extend_from_slice(&[255, 128, 128, 128]);
        let progress = AtomicU8::new(0);
        let cancel = AtomicBool::new(false);
        let cloud = parse_splat_stream(
            &mut std::io::Cursor::new(&bytes),
            Path::new("test.splat"),
            bytes.len() as u64,
            10,
            &progress,
            &cancel,
        )
        .unwrap();
        assert_eq!(cloud.count, 1);
        assert_eq!(cloud.positions[0], [1.0, 2.0, 3.0]);
        assert_eq!(cloud.scales[0], [0.5, 0.5, 0.5]);
        assert!((cloud.colors[0][0] - 1.0).abs() < 1e-6);
        assert!((cloud.colors[0][1] - 128.0 / 255.0).abs() < 1e-6);
        assert!((cloud.opacities[0] - 1.0).abs() < 1e-6);
        // w byte 255 → (255-128)/128 ≈ 0.992, xyz ≈ 0 → normalizes to identity.
        let r = cloud.rotations[0];
        assert!(r[3] > 0.99 && r[0].abs() < 1e-6);
    }

    #[test]
    fn splat_format_rejects_bad_length() {
        let bytes = vec![0u8; 33];
        let progress = AtomicU8::new(0);
        let cancel = AtomicBool::new(false);
        let err = parse_splat_stream(
            &mut std::io::Cursor::new(&bytes),
            Path::new("test.splat"),
            33,
            10,
            &progress,
            &cancel,
        )
        .unwrap_err();
        assert!(err.contains("32-byte"));
    }

    /// A compact cloud of `n` splats plus `far` outliers parked at `dist` × the
    /// cluster's own extent — a miniature of the unbounded-capture shape.
    fn cloud_with_outliers(n: usize, far: usize, dist: f32) -> SplatCloud {
        let mut verts = Vec::new();
        for i in 0..n {
            let mut v = visible_splat();
            let t = i as f32 / n as f32;
            v.pos = [t, t * 0.5, -t];
            verts.push(v);
        }
        for i in 0..far {
            let mut v = visible_splat();
            v.pos = [dist + i as f32, 0.0, 0.0];
            verts.push(v);
        }
        let bytes = make_ply_with_f_rest(SH_COEFFS, &verts);
        parse_bytes(&bytes, (n + far) as u32).unwrap()
    }

    #[test]
    fn far_clip_needs_the_far_field_under_5_percent() {
        // The threshold is a multiple of the p95 radius, so if MORE than 5% of a
        // capture is far field the p95 itself lands out there and the clip
        // becomes a no-op. Real unbounded captures sit well inside that (ladder
        // .ply: 2.24%), but the limit is structural, so pin it.
        let mut cloud = cloud_with_outliers(200, 12, 500.0); // 5.7% — too many
        normalize_cloud(
            &mut cloud,
            SceneOptions {
                far_clip: 10.0,
                ..Default::default()
            },
        );
        assert_eq!(
            cloud.count, 212,
            "p95 lands in the far field; nothing culled"
        );
    }

    #[test]
    fn far_clip_drops_the_far_field_and_keeps_the_scene() {
        let mut cloud = cloud_with_outliers(200, 4, 500.0); // 2.0%, like ladder.ply
        assert_eq!(cloud.count, 204);
        normalize_cloud(
            &mut cloud,
            SceneOptions {
                far_clip: 10.0,
                ..Default::default()
            },
        );
        assert_eq!(cloud.count, 200, "the 4 far-field splats must be dropped");
        // Every SoA vector moves in lockstep — a splat keeping its position but
        // inheriting a neighbour's colour or SH would be far worse than one that
        // simply stayed.
        assert_eq!(cloud.positions.len(), 200);
        assert_eq!(cloud.scales.len(), 200);
        assert_eq!(cloud.rotations.len(), 200);
        assert_eq!(cloud.colors.len(), 200);
        assert_eq!(cloud.opacities.len(), 200);
        assert_eq!(cloud.sh.len(), 200, "SH must be culled with the rest");
    }

    #[test]
    fn far_clip_zero_keeps_everything() {
        let mut cloud = cloud_with_outliers(200, 4, 500.0);
        normalize_cloud(&mut cloud, SceneOptions::default()); // far_clip 0
        assert_eq!(cloud.count, 204);
    }

    #[test]
    fn far_clip_is_a_noop_on_a_compact_capture() {
        // The default must not touch an object capture. trooper.ply's furthest
        // splat is 1.4× its p95 radius; nothing here exceeds that either.
        let mut cloud = cloud_with_outliers(200, 0, 0.0);
        let before = cloud.count;
        normalize_cloud(
            &mut cloud,
            SceneOptions {
                far_clip: 10.0,
                ..Default::default()
            },
        );
        assert_eq!(cloud.count, before);
    }

    #[test]
    fn far_clip_does_not_rescale_what_remains() {
        // The clip must not change framing: p95 is measured on the FULL set and
        // reused, so toggling far_clip cannot make the scene jump in size.
        let mut kept = cloud_with_outliers(200, 4, 500.0);
        let mut clipped = cloud_with_outliers(200, 4, 500.0);
        normalize_cloud(&mut kept, SceneOptions::default());
        normalize_cloud(
            &mut clipped,
            SceneOptions {
                far_clip: 10.0,
                ..Default::default()
            },
        );
        assert!(
            (kept.transform.scale - clipped.transform.scale).abs() < 1e-9,
            "clipping changed the scene scale: {} vs {}",
            kept.transform.scale,
            clipped.transform.scale
        );
        assert_eq!(kept.positions[0], clipped.positions[0]);
    }

    #[test]
    fn normalization_recenters_and_scales() {
        let bytes = make_test_ply(CANONICAL_PROPS, &many_splats(200));
        let mut cloud = parse_bytes(&bytes, 200).unwrap();
        let pre_scale = cloud.scales[0][0];
        normalize_cloud(&mut cloud, SceneOptions::default());
        // Center ≈ 0 (median-recentred): the median position maps to origin.
        let mut xs: Vec<f32> = cloud.positions.iter().map(|p| p[0]).collect();
        let med_x = median_of(&mut xs);
        assert!(med_x.abs() < 1e-3, "median x after recenter: {med_x}");
        // p95 radius ≈ 1.
        let mut d: Vec<f32> = cloud
            .positions
            .iter()
            .map(|p| (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt())
            .collect();
        d.sort_by(f32::total_cmp);
        let p95 = d[((d.len() - 1) as f32 * 0.95) as usize];
        assert!((p95 - 1.0).abs() < 0.05, "p95 radius: {p95}");
        // Scales scaled by the same factor.
        assert!((cloud.scales[0][0] - pre_scale * cloud.transform.scale).abs() < 1e-6);
    }

    #[test]
    fn normalization_applies_rotation() {
        // Symmetric cluster: median center = origin, p95 radius = 1, so the
        // normalize step is identity and only the rotation moves points.
        let mut splats = [visible_splat(), visible_splat(), visible_splat()];
        splats[0].pos = [1.0, 0.0, 0.0];
        splats[1].pos = [-1.0, 0.0, 0.0];
        splats[2].pos = [0.0, 0.0, 0.0];
        let bytes = make_test_ply(CANONICAL_PROPS, &splats);
        let mut cloud = parse_bytes(&bytes, 10).unwrap();
        normalize_cloud(
            &mut cloud,
            SceneOptions {
                rotation_degrees: [0.0, 0.0, 90.0],
                ..Default::default()
            },
        ); // Z+90°: x → y
        let p = cloud.positions[0];
        assert!(p[0].abs() < 1e-4 && (p[1] - 1.0).abs() < 1e-4, "{p:?}");
        // Quaternion rotated too (was identity → now Z+90°).
        let r = cloud.rotations[0];
        assert!((r[2] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-5);
        assert!((r[3] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-5);
    }

    #[test]
    fn resolve_source_demo_and_relative_and_absolute() {
        // Unknown demo errors by name.
        let err = resolve_source_path("demo:nope").unwrap_err();
        assert!(err.contains("nope"));
        // Known demo maps under splat_dir.
        let p = resolve_source_path("demo:default").unwrap();
        assert!(p.starts_with(splat_dir()));
        assert!(p.ends_with("phosphor_demo.ply"));
        // Absolute passes through.
        let abs = resolve_source_path("/tmp/scene.ply").unwrap();
        assert_eq!(abs, PathBuf::from("/tmp/scene.ply"));
        // Relative resolves under assets/splats/.
        let rel = resolve_source_path("scene.ply").unwrap();
        assert!(rel.to_string_lossy().contains("splats"));
    }

    #[test]
    fn format_detection_header_sniff() {
        // A "ply"-magic payload with a .bin extension routes to the PLY
        // parser (clear header error beats a silent 32-byte misparse).
        let bytes = make_test_ply(CANONICAL_PROPS, &[visible_splat()]);
        let dir = std::env::temp_dir().join("fosfora_splat_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sniff_test.bin");
        std::fs::write(&path, &bytes).unwrap();
        let progress = AtomicU8::new(0);
        let cancel = AtomicBool::new(false);
        let cloud =
            load_splat_file(&path, 10, SceneOptions::default(), &progress, &cancel).unwrap();
        assert_eq!(cloud.count, 1);
        assert_eq!(progress.load(Ordering::Relaxed), 100);
        let _ = std::fs::remove_file(&path);
    }
}
