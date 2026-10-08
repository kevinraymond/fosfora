//! The desktop's single-pass fragment effects on room surfaces (board
//! #3489, step D2b of `docs/xr/SURFACES_DESIGN.md`): eight of the core's
//! effects run on a face through the surfaces pass, with no change to
//! the core. This module is the plain-number side (the eight, how a
//! source is composed, the wrapper, the uniform packing), so it builds and
//! tests on the desktop; `gfx.rs` holds the slots and draws them.
//!
//! **The eight** ([`PORTS`]): single pass, no feedback, no texture reads.
//! Each effect's shader and `.pfx` are read from the core's `assets/` with
//! `include_str!`, as `world.rs` reads the XR sims: the source stays in the
//! core's assets and nothing is copied.
//!
//! **The source** ([`compose`]): the effect's fragment entry becomes a
//! plain function ([`rename_entry`]: its two-line header, attributes and
//! all, replaced by `fn effect_main(frag_coord: vec4f) -> vec4f {`, which
//! must hit exactly once), the core's loader prepends the uniform block
//! and the libraries (`EffectLoader::prepend_library`), and
//! [`WRAPPER_WGSL`] is appended: the surfaces pass's vertex stage and a
//! fragment entry that calls the effect.
//!
//! **The wrapper.** Group 0 is the desktop's: the effect's uniform `u` at
//! binding 0, one buffer per lit face (the block also declares the
//! previous frame and the audio textures at bindings 1 to 6; none of the
//! eight reads them, so the layout has binding 0 only). Group 1 is the
//! wrapper's: the eye camera at binding 0 and the port block at binding 1
//! ([`rows`]: the four corners, `face`, `params`, `color`, `mode`). The
//! fragment maps the face to the effect's frame: `uv` in face meters over
//! the half extents gives 0..1 across the face, `v` is flipped (a frame's
//! y runs down, the face's `v` up) and scaled by `u.resolution`
//! ([`frame_coord`]). What the effect returns becomes light
//! ([`light`]): an overlay effect (`alpha: true` in its `.pfx`) keeps its
//! own premultiplied coverage; an opaque one is covered by its luminance,
//! so its blacks are the real surface and its highlights glow. Both are
//! scaled by the peak alpha ([`PEAK_ALPHA`]), the lane's strength and the
//! surfaces pass's 5 cm edge fade.
//!
//! **The tint** (board #3472, D3). The lane's color index tints the
//! effect: index 0 leaves its colors its own (white, `[1, 1, 1]`, so the
//! default look is bit-identical to D2b's), any other multiplies its rgb by
//! the surface color the index picks ([`tint`], the key's tint included).
//! The tint goes on after the coverage is taken, so it never changes an
//! alpha, the opaque effects' luminance coverage included: a tinted
//! effect covers the surface where it did, in the tint's color. The lane's
//! band is not read: a ported effect follows its own audio bindings.
//!
//! **The uniform** ([`uniforms`]): the core's `ShaderUniforms`, filled as
//! the desktop fills it: this frame's audio features mirrored in, the
//! surfaces' clock as the time, the frame's dt and index, the `.pfx`
//! defaults packed once ([`Effect::params`]; a [`Port`]'s overrides aside:
//! tessera runs without its black scrim) with the rates integrated
//! after them every frame (one state per effect, so two walls on aurora
//! drift in step), and a virtual resolution: the face's extents at
//! [`PX_PER_M`] pixels per meter, each axis clamped to [`RES_MIN`]..
//! [`RES_MAX`] ([`resolution`]), so the effect's aspect is the face's and
//! a 2 m table runs at 1024 across.
//!
//! **Which way is up** ([`port_face`]): on a wall the frame's top is up
//! the wall and its left the wearer's left; on a level face the box's own
//! axes are kept (they do not turn with the head), with `u` signed so the
//! frame is not mirrored seen from the face's side.

use glam::Vec2;

use crate::surface_fx::{LIFT_M, STREAM_EDGE_M};
use crate::surfaces::Face;

/// One ported effect: the `.pfx` stem (its catalogue name), its shader and
/// its `.pfx`, both the core's files, and the float inputs a surface runs
/// at another value than the `.pfx`'s default (name, value).
#[derive(Debug, Clone, Copy)]
pub struct Port {
    pub name: &'static str,
    pub shader: &'static str,
    pub pfx: &'static str,
    pub overrides: &'static [(&'static str, f32)],
}

macro_rules! port {
    ($name:literal) => {
        port!($name, &[])
    };
    ($name:literal, $overrides:expr) => {
        Port {
            name: $name,
            shader: include_str!(concat!("../../../assets/shaders/", $name, ".wgsl")),
            pfx: include_str!(concat!("../../../assets/effects/", $name, ".pfx")),
            overrides: $overrides,
        }
    };
}

/// The eight, in catalogue order: ids [`FIRST_ID`] to 15.
pub const PORTS: [Port; 8] = [
    port!("aurora"),
    port!("prism"),
    port!("shards"),
    port!("astrolabe"),
    port!("bezel"),
    port!("fenestra"),
    port!("reticle"),
    // Tessera's scrim is a black cover at 0.85 over every tile not yet
    // revealed: on the desktop it hides the layer beneath, on a surface it
    // would darken the real one. Without it the tiles' strokes and embers
    // are the light and the rest is the surface.
    port!("tessera", &[("scrim", 0.0)]),
];
/// The catalogue id of the first port (`SurfaceBehavior::Aurora`).
pub const FIRST_ID: u32 = 8;
/// A port's light at full strength and full coverage.
pub const PEAK_ALPHA: f32 = 0.5;
/// The virtual resolution: pixels per meter of the face, and each axis'
/// bounds.
pub const PX_PER_M: f32 = 512.0;
pub const RES_MIN: f32 = 256.0;
pub const RES_MAX: f32 = 2048.0;
/// Rows of [`rows`], `struct XrPort` in [`WRAPPER_WGSL`].
pub const UNIFORM_ROWS: usize = 8;
/// A face steeper than this (|normal.y| under it) is a wall for
/// [`port_face`]; anything flatter is level.
pub const WALL_NORMAL_Y: f32 = 0.7;
/// The Rec. 709 weights an opaque effect's luminance is taken with.
pub const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];

/// The header every ported shader's fragment entry has, and the plain
/// function it becomes.
const ENTRY: &str =
    "@fragment\nfn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {";
const PLAIN: &str = "fn effect_main(frag_coord: vec4f) -> vec4f {";

/// `source` with its fragment entry `fs_main` turned into the plain
/// function `effect_main` the wrapper calls: the entry's header replaced,
/// its `@fragment`, `@builtin(position)` and `@location(0)` with it. An
/// error unless the header is there exactly once and no other `fs_main`
/// is left.
pub fn rename_entry(source: &str) -> Result<String, String> {
    let source = source.replace("\r\n", "\n");
    let hits = source.matches(ENTRY).count();
    if hits != 1 {
        return Err(format!("the fragment entry's header is there {hits} times"));
    }
    let out = source.replacen(ENTRY, PLAIN, 1);
    if out.contains("fs_main") {
        return Err("another `fs_main` is left after the rename".into());
    }
    Ok(out)
}

/// The virtual resolution of a face of half extents `half` (m).
pub fn resolution(half: [f32; 2]) -> [f32; 2] {
    half.map(|h| (2.0 * h * PX_PER_M).clamp(RES_MIN, RES_MAX))
}

/// `face` as a port draws on it. A wall (a face steeper than
/// [`WALL_NORMAL_Y`]): `v` the in-plane axis closest to up, signed up. A
/// level face keeps its axes, which are the box's and do not turn with
/// the head. Either way `u` is signed so `u x v` is the normal: seen from
/// the face's side the frame is not mirrored, and on a wall its left is
/// the wearer's left.
pub fn port_face(face: Face) -> Face {
    let mut f = if face.normal.y.abs() < WALL_NORMAL_Y {
        crate::canvas::spectrum_face(face).0
    } else {
        face
    };
    if f.axes[0].cross(f.axes[1]).dot(f.normal) < 0.0 {
        f.axes[0] = -f.axes[0];
    }
    f
}

/// The port block's rows for `face` ([`port_face`] already applied) at
/// `strength`, for an effect that is an `overlay` or opaque, of catalogue
/// id `id`, tinted `tint` ([`tint`]): the corners (lifted as the surfaces
/// pass lifts them); `face` (the half extents, the lift, the id); `params`
/// (the strength); `color` (the tint, the peak alpha); `mode` (x 1 for an
/// overlay).
pub fn rows(
    face: &Face,
    strength: f32,
    overlay: bool,
    id: u32,
    tint: [f32; 3],
) -> [[f32; 4]; UNIFORM_ROWS] {
    let mut rows = [[0.0f32; 4]; UNIFORM_ROWS];
    for (row, c) in rows.iter_mut().zip(face.corners(LIFT_M)) {
        *row = [c.x, c.y, c.z, 1.0];
    }
    rows[4] = [face.half[0], face.half[1], LIFT_M, id as f32];
    rows[5] = [strength.clamp(0.0, 1.0), 0.0, 0.0, 0.0];
    rows[6] = [tint[0], tint[1], tint[2], PEAK_ALPHA];
    rows[7] = [f32::from(u8::from(overlay)), 0.0, 0.0, 0.0];
    rows
}

/// The tint of a ported effect on a surface of `kind` whose lane's color
/// index is `color`, while the key's tint is `key`: white for index 0 (the
/// effect's colors are its own), else the surface color the index picks
/// (`surfaces::surface_color`, as the surfaces pass's own faces resolve
/// it through `surface_fx::color_of`).
pub fn tint(color: u32, kind: u32, key: [f32; 3]) -> [f32; 3] {
    if color == 0 {
        [1.0; 3]
    } else {
        crate::surfaces::surface_color(color, kind, key)
    }
}

/// The frame coordinate the wrapper hands the effect for face point `uv`
/// (m) on a face of half extents `half` at `resolution`: 0..1 across the
/// face, `v` flipped, in pixels. The WGSL's `fc`.
pub fn frame_coord(uv: Vec2, half: [f32; 2], resolution: [f32; 2]) -> Vec2 {
    let n = uv / Vec2::from(half) * 0.5 + 0.5;
    Vec2::new(n.x, 1.0 - n.y) * Vec2::from(resolution)
}

/// What the wrapper writes for the effect's output `c` at a scale of `k`
/// (the peak alpha times the strength times the edge fade), tinted `tint`:
/// premultiplied light. An overlay's coverage is its alpha; an opaque
/// effect's is its luminance (of the untinted color), so black is no
/// coverage; the tint then multiplies the color alone. The WGSL's
/// `fs_main` after the call.
pub fn light(c: [f32; 4], overlay: bool, k: f32, tint: [f32; 3]) -> [f32; 4] {
    let rgb = [c[0], c[1], c[2]].map(|x| x.clamp(0.0, 1.0));
    let cover = if overlay {
        c[3]
    } else {
        rgb.iter().zip(LUMA).map(|(x, w)| x * w).sum()
    };
    [
        rgb[0] * tint[0] * k,
        rgb[1] * tint[1] * k,
        rgb[2] * tint[2] * k,
        cover.clamp(0.0, 1.0) * k,
    ]
}

/// The surfaces pass's edge fade at face point `uv` (m): 0 at the face's
/// edges, 1 from 5 cm in. The WGSL's `edge`.
pub fn edge_fade(uv: Vec2, half: [f32; 2]) -> f32 {
    let s = |d: f32| {
        let t = (d / STREAM_EDGE_M).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    };
    s(half[0] - uv.x.abs()) * s(half[1] - uv.y.abs())
}

/// Appended to every ported effect ([`compose`]): the port block, the
/// surfaces pass's vertex stage and the fragment entry that calls the
/// effect (`effect_main`, [`rename_entry`]) and turns its output into
/// premultiplied light. The names carry an `xr` prefix so they cannot
/// meet one of the core's.
pub const WRAPPER_WGSL: &str = r"
struct XrEye { view_proj: mat4x4<f32> }
struct XrPort {
    // -a-b, +a-b, +a+b, -a+b around the face
    corners: array<vec4<f32>, 4>,
    // x, y the half extents along the face's axes (m), z the lift (m),
    // w the behavior id
    face: vec4<f32>,
    // x strength 0..1
    params: vec4<f32>,
    // rgb the tint (white: the effect's own colors), w the peak alpha
    color: vec4<f32>,
    // x 1 for an overlay (its own premultiplied alpha), 0 for an opaque
    // effect (its luminance is the coverage)
    mode: vec4<f32>,
}
@group(1) @binding(0) var<uniform> xr_eye: XrEye;
@group(1) @binding(1) var<uniform> xr_port: XrPort;

// The surfaces pass's edge fade (m) and the luminance weights.
const XR_EDGE_M: f32 = 0.05;
const XR_LUMA: vec3<f32> = vec3<f32>(0.2126, 0.7152, 0.0722);

struct XrVsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> XrVsOut {
    var order = array<u32, 6>(0u, 1u, 2u, 0u, 2u, 3u);
    var signs = array<vec2<f32>, 4>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0),
    );
    let k = order[i];
    var out: XrVsOut;
    out.pos = xr_eye.view_proj * vec4<f32>(xr_port.corners[k].xyz, 1.0);
    out.uv = signs[k] * xr_port.face.xy;
    return out;
}

@fragment
fn fs_main(in: XrVsOut) -> @location(0) vec4<f32> {
    // 0..1 across the face, then the effect's frame: y down, in pixels.
    let n = in.uv / xr_port.face.xy * 0.5 + 0.5;
    let fc = vec4<f32>(vec2<f32>(n.x, 1.0 - n.y) * u.resolution, 0.0, 1.0);
    let c = effect_main(fc);
    let edge = smoothstep(0.0, XR_EDGE_M, xr_port.face.x - abs(in.uv.x))
        * smoothstep(0.0, XR_EDGE_M, xr_port.face.y - abs(in.uv.y));
    let rgb = clamp(c.rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    var cover = dot(rgb, XR_LUMA);
    if xr_port.mode.x > 0.5 {
        cover = c.a;
    }
    let k = xr_port.color.w * clamp(xr_port.params.x, 0.0, 1.0) * edge;
    // The tint after the coverage: it colors the light, never its alpha.
    return vec4<f32>(rgb * xr_port.color.rgb, clamp(cover, 0.0, 1.0)) * k;
}
";

#[cfg(any(target_os = "android", test))]
pub use core_side::*;

/// The side that reads the core: on the device, and in the desktop tests
/// (the core is a dev-dependency there).
#[cfg(any(target_os = "android", test))]
mod core_side {
    use anyhow::{Context, Result, anyhow, ensure};
    use fosfora_app::audio::AudioFeatures;
    use fosfora_app::effect::EffectLoader;
    use fosfora_app::effect::format::PfxEffect;
    use fosfora_app::effect::rates::{self, RateState};
    use fosfora_app::gpu::uniforms::{ShaderUniforms, mirror_audio_features, shader_time};
    use fosfora_app::params::{ParamStore, ParamValue};

    use super::{PORTS, Port, WRAPPER_WGSL, rename_entry, resolution};

    /// `source` (an effect's shader) as a port's module: its entry renamed,
    /// the loader's uniform block and libraries before it, the wrapper
    /// after it.
    pub fn compose(loader: &EffectLoader, source: &str) -> Result<String> {
        let renamed = rename_entry(source).map_err(|e| anyhow!(e))?;
        Ok(format!(
            "{}\n{WRAPPER_WGSL}",
            loader.prepend_library(&renamed)
        ))
    }

    /// One ported effect, ready for a pipeline: its composed source,
    /// whether it is an overlay (`alpha: true` in its `.pfx`), and its
    /// packed parameters with their rates.
    pub struct Effect {
        pub name: &'static str,
        pub overlay: bool,
        pub source: String,
        defaults: [f32; 16],
        rates: RateState,
        params: [f32; 16],
    }

    impl Effect {
        /// `port` composed with `loader`'s libraries: the `.pfx` parsed
        /// for the overlay flag, the inputs (packed at their defaults, as
        /// `ParamStore` packs them, the port's overrides set) and the
        /// rates' layout. An error when the `.pfx` is not the single pass
        /// without feedback the port expects, or an override names no
        /// float input.
        pub fn new(loader: &EffectLoader, port: &Port) -> Result<Self> {
            let name = port.name;
            let pfx: PfxEffect =
                serde_json::from_str(port.pfx).with_context(|| format!("{name}.pfx"))?;
            let file = format!("{name}.wgsl");
            match pfx.passes.as_slice() {
                [] => ensure!(pfx.shader == file, "{name}.pfx runs `{}`", pfx.shader),
                [pass] => ensure!(
                    pass.shader == file && !pass.feedback && pass.inputs.is_empty(),
                    "{name}.pfx: its pass is not `{file}` alone, without feedback"
                ),
                passes => return Err(anyhow!("{name}.pfx has {} passes", passes.len())),
            }
            let mut store = ParamStore::new();
            store.load_from_defs(&pfx.inputs);
            for &(input, value) in port.overrides {
                ensure!(
                    matches!(store.get(input), Some(ParamValue::Float(_))),
                    "{name}.pfx has no float input `{input}`"
                );
                store.set(input, ParamValue::Float(value));
            }
            let defaults = store.pack_to_buffer();
            let slots =
                rates::layout(&pfx.inputs, &pfx.rates).map_err(|e| anyhow!("{name}.pfx: {e}"))?;
            Ok(Self {
                name,
                overlay: pfx.alpha,
                source: compose(loader, port.shader).with_context(|| format!("{name}.wgsl"))?,
                defaults,
                rates: RateState::new(slots),
                params: defaults,
            })
        }

        /// A frame of `dt` seconds: the rates integrate into their slots
        /// after the packed defaults. Once per frame, whatever is lit.
        pub fn advance(&mut self, dt: f32) {
            let mut buf = self.defaults;
            self.rates.advance(&mut buf, dt.max(0.0));
            self.params = buf;
        }

        /// The effect's `u.params` this frame.
        pub fn params(&self) -> [f32; 16] {
            self.params
        }
    }

    /// The eight ([`PORTS`]), composed with the libraries `loader` read
    /// from the assets directory.
    pub fn load(loader: &EffectLoader) -> Result<Vec<Effect>> {
        PORTS.iter().map(|p| Effect::new(loader, p)).collect()
    }

    /// The effect's uniform for one lit face this frame: `features`
    /// mirrored in as the desktop does, `clock` (the surfaces' clock, s)
    /// as the time, the frame's `dt` and index, the virtual resolution of
    /// a face of half extents `half`, and the effect's `params`.
    /// Everything else zero.
    pub fn uniforms(
        features: &AudioFeatures,
        clock: f32,
        dt: f32,
        frame: u64,
        half: [f32; 2],
        params: [f32; 16],
    ) -> ShaderUniforms {
        let mut u: ShaderUniforms = bytemuck::Zeroable::zeroed();
        mirror_audio_features(&mut u, features);
        u.time = shader_time(f64::from(clock));
        u.delta_time = dt;
        u.frame_index = frame as f32;
        u.resolution = resolution(half);
        u.params = params;
        u
    }

    /// The bind group layouts of a port's pipeline: group 0 the effect's
    /// uniform (`size_of::<ShaderUniforms>()` at binding 0, the only one
    /// of the desktop's seven the eight use), group 1 the eye camera
    /// (binding 0) and the port block (binding 1).
    pub fn layouts(device: &wgpu::Device) -> [wgpu::BindGroupLayout; 2] {
        let uniform = |binding, visibility, bytes: usize| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: wgpu::BufferSize::new(bytes as u64),
            },
            count: None,
        };
        let both = wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT;
        [
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("xr-port-effect"),
                entries: &[uniform(
                    0,
                    wgpu::ShaderStages::FRAGMENT,
                    size_of::<ShaderUniforms>(),
                )],
            }),
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("xr-port"),
                entries: &[
                    uniform(0, wgpu::ShaderStages::VERTEX, 64),
                    uniform(1, both, 16 * super::UNIFORM_ROWS),
                ],
            }),
        ]
    }

    /// One port's pipeline over [`layouts`]: `source` ([`compose`]) drawn
    /// as the surfaces pass draws (premultiplied over the target, no
    /// culling), into `format` with the depth state the caller's pass
    /// uses.
    pub fn pipeline(
        device: &wgpu::Device,
        layouts: &[wgpu::BindGroupLayout; 2],
        label: &str,
        source: &str,
        format: wgpu::TextureFormat,
        depth_stencil: Option<wgpu::DepthStencilState>,
    ) -> wgpu::RenderPipeline {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(label),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some(label),
            bind_group_layouts: &[&layouts[0], &layouts[1]],
            push_constant_ranges: &[],
        });
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..wgpu::PrimitiveState::default()
            },
            depth_stencil,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
            cache: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surfaces::acting_face;
    use fosfora_app::effect::EffectLoader;
    use fosfora_app::effect::format::PfxEffect;
    use fosfora_app::gpu::uniforms::ShaderUniforms;
    use fosfora_app::params::ParamStore;
    use glam::{Quat, Vec3};

    /// The core's loader over the workspace's `assets/` (the tests run in
    /// the crate's directory, so the loader's own search would miss it).
    fn loader() -> EffectLoader {
        let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        // Already pinned by another test: the same directory.
        let _ = fosfora_app::effect::loader::set_assets_dir(assets);
        EffectLoader::new()
    }

    fn validate(source: &str, what: &str) -> (naga::Module, naga::valid::ModuleInfo) {
        let module = naga::front::wgsl::parse_str(source)
            .unwrap_or_else(|e| panic!("{what} parses: {}", e.emit_to_string(source)));
        let info = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::default(),
        )
        .validate(&module)
        .unwrap_or_else(|e| panic!("{what} validates: {e:?}"));
        (module, info)
    }

    fn struct_span(module: &naga::Module, name: &str) -> usize {
        let (_, ty) = module
            .types
            .iter()
            .find(|(_, t)| t.name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("struct {name}"));
        let naga::TypeInner::Struct { span, .. } = ty.inner else {
            panic!("{name} is not a struct");
        };
        span as usize
    }

    /// The names of the globals entry point `entry` reads.
    fn globals_used(
        module: &naga::Module,
        info: &naga::valid::ModuleInfo,
        entry: &str,
    ) -> Vec<String> {
        let i = module
            .entry_points
            .iter()
            .position(|e| e.name == entry)
            .unwrap_or_else(|| panic!("entry point {entry}"));
        let used = info.get_entry_point(i);
        let mut names: Vec<_> = module
            .global_variables
            .iter()
            .filter(|(h, _)| !used[*h].is_empty())
            .map(|(_, v)| v.name.clone().unwrap_or_default())
            .collect();
        names.sort();
        names
    }

    /// A wall plane 3 m wide and 2.4 m high at z = -2, seen from the room.
    fn wall() -> Face {
        acting_face(
            Vec3::new(0.0, 1.2, -2.0),
            Quat::IDENTITY,
            Vec3::new(1.5, 1.2, 0.02),
            Vec3::new(0.0, 1.5, 0.0),
        )
    }

    /// A table as the runtime reports one: local +Z up, top at 0.75 m.
    fn table() -> Face {
        acting_face(
            Vec3::new(0.0, 0.4, -0.8),
            Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2),
            Vec3::new(0.6, 0.4, 0.35),
            Vec3::new(0.3, 1.2, 0.4),
        )
    }

    #[test]
    fn the_eight_are_the_briefs_in_catalogue_order() {
        let names: Vec<_> = PORTS.iter().map(|p| p.name).collect();
        assert_eq!(
            names,
            [
                "aurora",
                "prism",
                "shards",
                "astrolabe",
                "bezel",
                "fenestra",
                "reticle",
                "tessera"
            ]
        );
        // None reads a texture or the previous frame: binding 0 alone.
        for p in &PORTS {
            for word in [
                "textureSample",
                "textureLoad",
                "prev_frame",
                "audio_s",
                "audio_w",
            ] {
                assert!(!p.shader.contains(word), "{}: {word}", p.name);
            }
        }
    }

    #[test]
    fn each_port_composes_validates_and_has_one_fragment_entry() {
        let loader = loader();
        for port in &PORTS {
            let name = port.name;
            // The rename hits exactly once and leaves a plain function.
            let renamed = rename_entry(port.shader).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(renamed.matches("fn effect_main(").count(), 1, "{name}");
            assert!(!renamed.contains("@fragment"), "{name}");
            assert!(!renamed.contains("@builtin"), "{name}");

            let effect = Effect::new(&loader, port).unwrap_or_else(|e| panic!("{name}: {e:#}"));
            let (module, info) = validate(&effect.source, name);
            let stages: Vec<_> = module
                .entry_points
                .iter()
                .map(|e| (e.name.as_str(), e.stage))
                .collect();
            assert_eq!(
                stages,
                [
                    ("vs_main", naga::ShaderStage::Vertex),
                    ("fs_main", naga::ShaderStage::Fragment)
                ],
                "{name}"
            );
            // The layouts' sizes are the module's.
            assert_eq!(
                struct_span(&module, "PhosphorUniforms"),
                size_of::<ShaderUniforms>(),
                "{name}"
            );
            assert_eq!(struct_span(&module, "XrPort"), 16 * UNIFORM_ROWS, "{name}");
            assert_eq!(struct_span(&module, "XrEye"), 64, "{name}");
            // Of the desktop's seven bindings the entries read `u` alone,
            // so group 0's layout is binding 0 and nothing stands in for
            // the textures.
            assert_eq!(
                globals_used(&module, &info, "fs_main"),
                ["u", "xr_port"],
                "{name}"
            );
            assert_eq!(
                globals_used(&module, &info, "vs_main"),
                ["xr_eye", "xr_port"],
                "{name}"
            );
        }
    }

    #[test]
    fn the_rename_refuses_a_missing_or_a_second_entry() {
        assert!(rename_entry("fn helper() -> f32 { return 1.0; }").is_err());
        let one = format!("{ENTRY}\n    return vec4f(0.0);\n}}\n");
        assert!(rename_entry(&one).is_ok());
        // Windows line endings between the attribute and the function.
        assert!(rename_entry(&one.replace('\n', "\r\n")).is_ok());
        assert!(rename_entry(&format!("{one}{one}")).is_err());
        // A second `fs_main` under another header would shadow the
        // wrapper's.
        let other = format!("{one}@fragment\nfn fs_main() -> @location(0) vec4f {{ }}\n");
        assert!(rename_entry(&other).is_err());
    }

    #[test]
    fn the_overlay_flag_is_the_pfxs() {
        let loader = loader();
        let overlays: Vec<_> = load(&loader)
            .expect("the eight load")
            .iter()
            .filter(|e| e.overlay)
            .map(|e| e.name)
            .collect();
        assert_eq!(
            overlays,
            ["astrolabe", "bezel", "fenestra", "reticle", "tessera"]
        );
    }

    #[test]
    fn the_packed_defaults_are_param_stores_and_the_rates_resolve() {
        let loader = loader();
        let mut effects = load(&loader).expect("the eight load");
        for (effect, port) in effects.iter().zip(&PORTS) {
            let pfx: PfxEffect = serde_json::from_str(port.pfx).expect("pfx");
            let mut store = ParamStore::new();
            store.load_from_defs(&pfx.inputs);
            let mut packed = store.pack_to_buffer();
            // The one override: tessera's scrim, its seventh float.
            if port.name == "tessera" {
                assert_close!(packed[6], 0.85);
                packed[6] = 0.0;
            } else {
                assert!(port.overrides.is_empty(), "{}", port.name);
            }
            assert_close!(effect.params(), packed, "{}", port.name);
            // Every input is packed: none runs past the 16 slots.
            let (offsets, _) = ParamStore::packed_offsets(&pfx.inputs);
            assert!(offsets.iter().all(Option::is_some), "{}", port.name);
        }
        // A few by hand: aurora's three floats; prism's four floats and
        // three toggles; astrolabe's tint after its six floats.
        assert_close!(effects[0].params()[..4], [0.5, 0.5, 0.3, 0.0]);
        assert_close!(
            effects[1].params()[..8],
            [0.4, 0.3, 0.4, 0.5, 1.0, 1.0, 1.0, 0.0]
        );
        assert_close!(effects[3].params()[6..10], [0.75, 0.95, 1.0, 1.0]);

        // The rates integrate after the packed inputs, where the shaders
        // read them: aurora's curtain speed (0.5) in slot 3, prism's
        // rotation speed (0.3) in slot 7; the others have none.
        for e in &mut effects {
            e.advance(0.5);
            e.advance(0.5);
        }
        assert_close!(effects[0].params()[..4], [0.5, 0.5, 0.3, 0.5]);
        assert_close!(effects[1].params()[7], 0.3);
        for (i, e) in effects.iter().enumerate().skip(2) {
            let pfx: PfxEffect = serde_json::from_str(PORTS[i].pfx).expect("pfx");
            assert!(pfx.rates.is_empty(), "{}", e.name);
            let fresh = Effect::new(&loader, &PORTS[i]).expect("the port loads");
            assert_close!(e.params(), fresh.params(), "{}", e.name);
        }
        // An override that names no float input is refused.
        let mut wrong = PORTS[7];
        wrong.overrides = &[("punch", 0.0)];
        assert!(Effect::new(&loader, &wrong).is_err());
        wrong.overrides = &[("missing", 0.0)];
        assert!(Effect::new(&loader, &wrong).is_err());
    }

    #[test]
    fn the_uniform_carries_the_features_the_clock_and_the_faces_resolution() {
        let f = fosfora_app::headless::loop_driver::synth_features(100, 72, 120.0);
        let mut params = [0.0; 16];
        params[2] = 0.25;
        let u = uniforms(&f, 12.5, 1.0 / 72.0, 100, [1.0, 0.4], params);
        assert_close!(u.time, 12.5);
        assert_close!(u.delta_time, 1.0 / 72.0);
        assert_close!(u.frame_index, 100.0);
        assert_close!(u.resolution, [1024.0, 409.6]);
        assert_close!(u.params, params);
        assert_close!(
            [u.beat_phase, u.bar_phase, u.bar_index, u.beat_index],
            [f.beat_phase, f.bar_phase, f.bar_index, f.beat_index]
        );
        assert_close!([u.rms, u.bass], [f.rms, f.bass]);
        assert_close!(u.feedback_decay, 0.0);
    }

    #[test]
    fn the_virtual_resolution_is_512_px_per_meter_clamped() {
        // A 2 m by 0.8 m table; a 10 cm frame; a 6 m floor.
        assert_close!(resolution([1.0, 0.4]), [1024.0, 409.6]);
        assert_close!(resolution([0.05, 0.3]), [256.0, 307.2]);
        assert_close!(resolution([3.0, 2.5]), [2048.0, 2048.0]);
    }

    #[test]
    fn the_faces_corners_map_to_the_frames_with_v_flipped() {
        let half = [1.0, 0.4];
        let res = resolution(half);
        // The corners in the vertex stage's order: -a-b, +a-b, +a+b, -a+b.
        let corners = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)];
        // 0..1 across the face: (0,0), (1,0), (1,1), (0,1); the frame's y
        // runs down, so the face's +v edge is the frame's top row.
        let expected = [(0.0, 1.0), (1.0, 1.0), (1.0, 0.0), (0.0, 0.0)];
        for ((su, sv), (x, y)) in corners.into_iter().zip(expected) {
            let uv = Vec2::new(su * half[0], sv * half[1]);
            let fc = frame_coord(uv, half, res);
            assert_close!(fc.to_array(), [x * res[0], y * res[1]]);
        }
        assert_close!(
            frame_coord(Vec2::ZERO, half, res).to_array(),
            [512.0, 204.8]
        );
    }

    #[test]
    fn an_opaque_effects_black_is_no_coverage_and_an_overlay_keeps_its_own() {
        // Opaque: the luminance covers; black is the real surface.
        const WHITE: [f32; 3] = [1.0; 3];
        assert_close!(light([0.0, 0.0, 0.0, 1.0], false, 0.5, WHITE), [0.0; 4]);
        assert_close!(light([1.0, 1.0, 1.0, 1.0], false, 0.5, WHITE), [0.5; 4]);
        let green = light([0.0, 1.0, 0.0, 1.0], false, 0.5, WHITE);
        assert_close!(green, [0.0, 0.5, 0.0, 0.5 * LUMA[1]]);
        // Past 1 the light clips at the peak.
        assert_close!(light([3.0, 3.0, 3.0, 1.0], false, 0.5, WHITE), [0.5; 4]);
        // Overlay: premultiplied in, premultiplied out, scaled.
        assert_close!(
            light([0.2, 0.1, 0.0, 0.4], true, 0.5, WHITE),
            [0.1, 0.05, 0.0, 0.2]
        );
        assert_close!(light([0.0, 0.0, 0.0, 0.0], true, 0.5, WHITE), [0.0; 4]);
        // The edge fade: 0 at an edge, 1 from 5 cm in.
        let half = [0.6, 0.4];
        assert_close!(edge_fade(Vec2::new(0.6, 0.0), half), 0.0);
        assert_close!(edge_fade(Vec2::new(0.55, 0.35), half), 1.0);
        // Halfway in (to the float's rounding of the 2.5 cm).
        assert!((edge_fade(Vec2::new(0.575, 0.0), half) - 0.5).abs() < 1e-4);
    }

    #[test]
    fn a_tint_colors_the_light_and_never_its_alpha() {
        use crate::surfaces::{AMBER, COLOR_KEY, KIND_TABLE, TEAL, palette};
        let key = [0.9, 0.3, 0.2];
        // Index 0: white, the effect's own colors; the others the color
        // the surfaces pass draws that index in, the key's tint included.
        assert_close!(tint(0, KIND_TABLE, key), [1.0; 3]);
        assert_close!(tint(4, KIND_TABLE, key), AMBER);
        assert_close!(tint(6, KIND_TABLE, key), TEAL);
        assert_close!(tint(COLOR_KEY, KIND_TABLE, key), key);
        for c in 1..=COLOR_KEY {
            let behavior = crate::surfaces::SurfaceBehavior::Aurora;
            assert_close!(
                tint(c, KIND_TABLE, key),
                crate::surface_fx::color_of(KIND_TABLE, behavior, c, key)
            );
        }
        assert!(!crate::test_util::close(
            &tint(0, KIND_TABLE, key),
            &palette(KIND_TABLE)
        ));
        // White is bit-identical to D2b's light, which had no tint: the
        // default look.
        let untinted = |c: [f32; 4], overlay: bool, k: f32| {
            let rgb = [c[0], c[1], c[2]].map(|x| x.clamp(0.0, 1.0));
            let cover: f32 = if overlay {
                c[3]
            } else {
                rgb.iter().zip(LUMA).map(|(x, w)| x * w).sum()
            };
            [
                rgb[0] * k,
                rgb[1] * k,
                rgb[2] * k,
                cover.clamp(0.0, 1.0) * k,
            ]
        };
        let samples = [
            [0.3, 0.6, 0.9, 1.0],
            [0.71, 0.13, 0.02, 0.4],
            [1.7, 0.5, 0.25, 0.8],
        ];
        for c in samples {
            for overlay in [false, true] {
                let a = light(c, overlay, 0.37, [1.0; 3]);
                let b = untinted(c, overlay, 0.37);
                assert_eq!(a.map(f32::to_bits), b.map(f32::to_bits), "{c:?}");
                // Tinted: the color times the tint, the alpha unchanged,
                // for an opaque effect's luminance coverage too.
                let t = light(c, overlay, 0.37, AMBER);
                assert_close!(t[3], a[3]);
                for i in 0..3 {
                    assert_close!(t[i], a[i] * AMBER[i]);
                }
            }
        }
    }

    #[test]
    fn the_wrappers_constants_are_the_modules() {
        let source = format!(
            "struct U {{ resolution: vec2<f32> }}\n@group(0) @binding(0) var<uniform> u: U;\n\
             fn effect_main(fc: vec4<f32>) -> vec4<f32> {{ return fc; }}\n{WRAPPER_WGSL}"
        );
        let (module, _) = validate(&source, "the wrapper");
        let constant = |name: &str| {
            let (_, c) = module
                .constants
                .iter()
                .find(|(_, c)| c.name.as_deref() == Some(name))
                .unwrap_or_else(|| panic!("const {name}"));
            &module.global_expressions[c.init]
        };
        let naga::Expression::Literal(naga::Literal::F32(edge)) = *constant("XR_EDGE_M") else {
            panic!("XR_EDGE_M");
        };
        assert_close!(edge, STREAM_EDGE_M);
        let naga::Expression::Compose { ref components, .. } = *constant("XR_LUMA") else {
            panic!("XR_LUMA");
        };
        let luma: Vec<f32> = components
            .iter()
            .map(|c| match module.global_expressions[*c] {
                naga::Expression::Literal(naga::Literal::F32(v)) => v,
                ref e => panic!("XR_LUMA: {e:?}"),
            })
            .collect();
        assert_close!(luma, LUMA);
        assert_close!(LUMA.iter().sum::<f32>(), 1.0);
    }

    #[test]
    fn the_rows_carry_the_face_the_strength_the_color_and_the_mode() {
        let face = port_face(table());
        let r = rows(&face, 0.8, true, FIRST_ID + 4, [1.0; 3]);
        for (row, c) in r.iter().zip(face.corners(LIFT_M)) {
            assert_close!(*row, [c.x, c.y, c.z, 1.0]);
        }
        assert_close!(r[4], [face.half[0], face.half[1], LIFT_M, 12.0]);
        assert_close!(r[5], [0.8, 0.0, 0.0, 0.0]);
        assert_close!(r[6], [1.0, 1.0, 1.0, PEAK_ALPHA]);
        assert_close!(r[7], [1.0, 0.0, 0.0, 0.0]);
        let amber = crate::surfaces::AMBER;
        let opaque = rows(&face, 2.0, false, FIRST_ID, amber);
        assert_close!(opaque[5][0], 1.0);
        assert_close!(opaque[6], [amber[0], amber[1], amber[2], PEAK_ALPHA]);
        assert_close!(opaque[7][0], 0.0);
    }

    #[test]
    fn a_walls_frame_is_upright_and_a_level_faces_keeps_the_boxs_axes() {
        // The wall, from the room (the wearer looks down -Z): the frame's
        // up is the world's, its u the wearer's right (+X).
        let w = port_face(wall());
        assert_close!(w.axes[1].to_array(), [0.0, 1.0, 0.0]);
        assert_close!(w.axes[0].to_array(), [1.0, 0.0, 0.0]);
        assert_close!(w.half, [1.5, 1.2]);
        // The same wall seen from behind: u turns with the wearer.
        let back = port_face(acting_face(
            Vec3::new(0.0, 1.2, -2.0),
            Quat::IDENTITY,
            Vec3::new(1.5, 1.2, 0.02),
            Vec3::new(0.0, 1.5, -4.0),
        ));
        assert_close!(back.axes[1].to_array(), [0.0, 1.0, 0.0]);
        assert_close!(back.axes[0].to_array(), [-1.0, 0.0, 0.0]);
        // A wall whose runtime axes are turned a quarter (x up): still
        // upright, the extents with the axes.
        let turned = port_face(acting_face(
            Vec3::new(0.0, 1.2, -2.0),
            Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),
            Vec3::new(1.2, 1.5, 0.02),
            Vec3::new(0.0, 1.5, 0.0),
        ));
        assert!(turned.axes[1].dot(Vec3::Y) > 0.999);
        assert!(turned.axes[0].dot(Vec3::X) > 0.999);
        assert_close!(turned.half, [1.5, 1.2]);
        // A table: the box's axes, wherever the head is, right-handed
        // about the normal.
        let t = table();
        let p = port_face(t);
        assert_close!(p.axes[1].to_array(), t.axes[1].to_array());
        assert_close!(p.half, t.half);
        assert!(p.axes[0].cross(p.axes[1]).dot(p.normal) > 0.999);
        for f in [w, back, turned] {
            assert!(f.axes[0].cross(f.axes[1]).dot(f.normal) > 0.999);
        }
    }

    /// The ports on a real device (`#[ignore]`d like the repo's other GPU
    /// tests: it needs an adapter; run with `cargo test -p fosfora-xr
    /// --lib surface_port -- --ignored`).
    #[cfg(not(target_os = "android"))]
    mod gpu {
        use super::*;

        /// Pixels per meter of the test's target.
        const PX: f32 = 200.0;

        struct Gpu {
            device: wgpu::Device,
            queue: wgpu::Queue,
            layouts: [wgpu::BindGroupLayout; 2],
        }

        fn gpu() -> Gpu {
            let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
                backends: wgpu::Backends::VULKAN | wgpu::Backends::METAL,
                ..Default::default()
            });
            let adapter =
                pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::LowPower,
                    compatible_surface: None,
                    force_fallback_adapter: false,
                }))
                .expect("no wgpu adapter");
            eprintln!("adapter: {:?}", adapter.get_info());
            let (device, queue) =
                pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                    label: Some("xr-surface-port-test"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                    experimental_features: wgpu::ExperimentalFeatures::default(),
                    memory_hints: wgpu::MemoryHints::Performance,
                    trace: wgpu::Trace::Off,
                }))
                .expect("no wgpu device");
            let layouts = layouts(&device);
            Gpu {
                device,
                queue,
                layouts,
            }
        }

        /// A face `half` in size in the z = 0 plane, u along +X and v
        /// along +Y, looked at down -Z by a camera that fits it to the
        /// target: the target's top row is the face's +v edge.
        fn flat(half: [f32; 2]) -> (Face, glam::Mat4) {
            let face = Face {
                normal: Vec3::Z,
                center: Vec3::ZERO,
                axes: [Vec3::X, Vec3::Y],
                half,
            };
            let fit = glam::Mat4::from_scale(Vec3::new(1.0 / half[0], 1.0 / half[1], 1.0));
            (face, fit)
        }

        fn uniform(gpu: &Gpu, bytes: &[u8]) -> wgpu::Buffer {
            let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: bytes.len() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            gpu.queue.write_buffer(&buffer, 0, bytes);
            buffer
        }

        /// `source` drawn once on a face of `half` with the uniform `u`:
        /// the target's premultiplied RGBA floats, row 0 the top, and its
        /// width. Panics on a validation error.
        fn draw(
            gpu: &Gpu,
            source: &str,
            overlay: bool,
            u: &ShaderUniforms,
            half: [f32; 2],
        ) -> (Vec<[f32; 4]>, usize) {
            let (w, h) = (
                (2.0 * half[0] * PX) as u32 / 32 * 32,
                (2.0 * half[1] * PX) as u32,
            );
            let format = wgpu::TextureFormat::Rgba16Float;
            gpu.device.push_error_scope(wgpu::ErrorFilter::Validation);
            let pipeline = pipeline(&gpu.device, &gpu.layouts, "port", source, format, None);
            let (face, fit) = flat(half);
            let u_buffer = uniform(gpu, bytemuck::bytes_of(u));
            let eye = uniform(gpu, bytemuck::cast_slice(&fit.to_cols_array()));
            let port = uniform(
                gpu,
                bytemuck::cast_slice(&rows(&face, 1.0, overlay, FIRST_ID, [1.0; 3])),
            );
            fn entry(binding: u32, buffer: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
                wgpu::BindGroupEntry {
                    binding,
                    resource: buffer.as_entire_binding(),
                }
            }
            let group0 = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &gpu.layouts[0],
                entries: &[entry(0, &u_buffer)],
            });
            let group1 = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &gpu.layouts[1],
                entries: &[entry(0, &eye), entry(1, &port)],
            });
            let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = target.create_view(&wgpu::TextureViewDescriptor::default());
            let row_bytes = w * 8;
            assert_eq!(row_bytes % wgpu::COPY_BYTES_PER_ROW_ALIGNMENT, 0);
            let readback = gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: u64::from(row_bytes * h),
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut encoder = gpu
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: None,
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                pass.set_pipeline(&pipeline);
                pass.set_bind_group(0, &group0, &[]);
                pass.set_bind_group(1, &group1, &[]);
                pass.draw(0..6, 0..1);
            }
            encoder.copy_texture_to_buffer(
                target.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &readback,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(row_bytes),
                        rows_per_image: None,
                    },
                },
                target.size(),
            );
            gpu.queue.submit([encoder.finish()]);
            readback
                .slice(..)
                .map_async(wgpu::MapMode::Read, |r| r.expect("map"));
            gpu.device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: None,
                })
                .expect("poll");
            if let Some(e) = pollster::block_on(gpu.device.pop_error_scope()) {
                panic!("validation: {e}");
            }
            let data = readback.slice(..).get_mapped_range();
            let halves: &[u16] = bytemuck::cast_slice(&data);
            let pixels = halves
                .chunks_exact(4)
                .map(|p| [p[0], p[1], p[2], p[3]].map(half_to_f32))
                .collect();
            (pixels, w as usize)
        }

        /// An IEEE half as an f32 (the target's texels).
        fn half_to_f32(h: u16) -> f32 {
            let sign = if h & 0x8000 == 0 { 1.0 } else { -1.0 };
            let (e, m) = (i32::from((h >> 10) & 0x1f), f32::from(h & 0x3ff));
            sign * match e {
                0 => m * 2f32.powi(-24),
                31 => f32::INFINITY,
                _ => (1.0 + m / 1024.0) * 2f32.powi(e - 15),
            }
        }

        /// A fragment effect with the ported shaders' header that returns
        /// `body`.
        fn probe(loader: &EffectLoader, body: &str) -> String {
            compose(loader, &format!("{ENTRY}\n    {body}\n}}\n")).expect("the probe composes")
        }

        fn features(frame: u32) -> fosfora_app::audio::AudioFeatures {
            fosfora_app::headless::loop_driver::synth_features_accented(frame, 72, 120.0, 4)
        }

        /// The mean alpha over the rows `from..to` (fractions of the
        /// height) of the middle third of the columns.
        fn mean_alpha(pixels: &[[f32; 4]], w: usize, from: f32, to: f32) -> f32 {
            let h = pixels.len() / w;
            let rows = (from * h as f32) as usize..(to * h as f32) as usize;
            let cols = w / 3..2 * w / 3;
            let n = (rows.len() * cols.len()) as f32;
            rows.flat_map(|y| cols.clone().map(move |x| (x, y)))
                .map(|(x, y)| pixels[y * w + x][3])
                .sum::<f32>()
                / n
        }

        #[test]
        #[ignore = "needs a GPU adapter"]
        fn the_eight_pipelines_build_and_draw_on_a_device() {
            let gpu = gpu();
            let loader = loader();
            let mut effects = load(&loader).expect("the eight load");
            let half = [0.6, 0.4];
            let dump = std::env::var_os("FOSFORA_PORT_DUMP").map(std::path::PathBuf::from);

            // Each of the eight: its pipeline on the two layouts (group 0
            // binding 0 alone) and a draw, with no validation error, and
            // light somewhere in a bar of the synthetic groove.
            for e in &mut effects {
                let mut lit = 0usize;
                for frame in [20u32, 150, 290, 430] {
                    e.advance(frame as f32 / 72.0);
                    let u = uniforms(
                        &features(frame),
                        frame as f32 / 72.0,
                        1.0 / 72.0,
                        u64::from(frame),
                        half,
                        e.params(),
                    );
                    let (pixels, w) = draw(&gpu, &e.source, e.overlay, &u, half);
                    for p in &pixels {
                        assert!(p.iter().all(|x| x.is_finite()), "{}", e.name);
                        assert!((0.0..=PEAK_ALPHA + 1e-3).contains(&p[3]), "{}", e.name);
                    }
                    lit += pixels.iter().filter(|p| p[3] > 0.02).count();
                    if let Some(dir) = &dump {
                        write_ppm(&dir.join(format!("{}_{frame}.ppm", e.name)), &pixels, w);
                    }
                }
                assert!(lit > 200, "{}: {lit} lit pixels", e.name);
            }

            // The wrapper on the device. An opaque effect's black is no
            // coverage anywhere; its white is the peak inside the edge
            // fade and nothing on the edge.
            let black = probe(&loader, "return vec4f(0.0, 0.0, 0.0, 1.0);");
            let white = probe(&loader, "return vec4f(1.0);");
            let u = uniforms(&features(0), 0.0, 1.0 / 72.0, 0, half, [0.0; 16]);
            let (pixels, _) = draw(&gpu, &black, false, &u, half);
            assert!(pixels.iter().all(|p| p.iter().all(|x| x.abs() < 1e-6)));
            let (pixels, w) = draw(&gpu, &white, false, &u, half);
            let h = pixels.len() / w;
            assert!((pixels[h / 2 * w + w / 2][3] - PEAK_ALPHA).abs() < 1e-3);
            assert!(pixels[h / 2 * w][3] < 0.1 * PEAK_ALPHA);
            // The same black as an overlay with full alpha is a dark
            // cover: the mode is what tells them apart.
            let (pixels, w) = draw(&gpu, &black, true, &u, half);
            assert!((pixels[h / 2 * w + w / 2][3] - PEAK_ALPHA).abs() < 1e-3);

            // Which way is up: the effect's uv (0, 0) is the frame's top
            // left, which lands on the face's -u +v corner: the target's
            // top left when v is up. Red is uv.x, green uv.y.
            let uv = probe(
                &loader,
                "let uv = frag_coord.xy / u.resolution; return vec4f(uv, 0.0, 1.0);",
            );
            let (pixels, w) = draw(&gpu, &uv, true, &u, half);
            let h = pixels.len() / w;
            let at = |fx: f32, fy: f32| {
                let p = pixels[(fy * h as f32) as usize * w + (fx * w as f32) as usize];
                [p[0] / PEAK_ALPHA, p[1] / PEAK_ALPHA]
            };
            let near =
                |a: [f32; 2], b: [f32; 2]| (a[0] - b[0]).abs() < 0.02 && (a[1] - b[1]).abs() < 0.02;
            assert!(near(at(0.25, 0.25), [0.25, 0.25]), "{:?}", at(0.25, 0.25));
            assert!(near(at(0.75, 0.25), [0.75, 0.25]), "{:?}", at(0.75, 0.25));
            assert!(near(at(0.25, 0.75), [0.25, 0.75]), "{:?}", at(0.25, 0.75));

            // And on a real effect with an obvious top: bezel's sweep arc
            // sits at the top center of its frame, with nothing like it at
            // the bottom.
            let bezel = &effects[4];
            assert_eq!(bezel.name, "bezel");
            let u = uniforms(&features(150), 2.0, 1.0 / 72.0, 150, half, bezel.params());
            let (pixels, w) = draw(&gpu, &bezel.source, true, &u, half);
            let top = mean_alpha(&pixels, w, 0.06, 0.24);
            let bottom = mean_alpha(&pixels, w, 0.76, 0.94);
            assert!(top > 1.2 * bottom, "top {top}, bottom {bottom}");
        }

        /// `pixels` (premultiplied light) over a dim gray room, as a PPM
        /// for looking at (`FOSFORA_PORT_DUMP=<dir>`).
        fn write_ppm(path: &std::path::Path, pixels: &[[f32; 4]], w: usize) {
            let mut out = format!("P6\n{w} {}\n255\n", pixels.len() / w).into_bytes();
            for p in pixels {
                for c in &p[..3] {
                    let lin = (c + (1.0 - p[3]) * 0.12).clamp(0.0, 1.0);
                    out.push((lin.powf(1.0 / 2.2) * 255.0) as u8);
                }
            }
            std::fs::write(path, out).expect("writing the dump");
        }
    }
}
