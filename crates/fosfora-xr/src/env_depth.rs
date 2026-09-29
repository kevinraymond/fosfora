//! The live environment depth as a depth occluder (board #3324, phase 1).
//!
//! `XR_META_environment_depth` hands the app a depth map from the
//! passthrough cameras every frame: a two-layer `D16_UNORM` array (layer 0
//! the left eye, layer 1 the right), each layer rendered from its own pose
//! and fov with an OpenGL-convention projection between `near_z` and
//! `far_z`. The scanned room's boxes and the hand mesh only hide the
//! sprites behind what the scan knows; this map also covers an unscanned
//! chair, a person, a pet, the wearer's own body and anything moved since.
//!
//! The eye pass draws one full-screen triangle first, depth-only: each
//! fragment takes the point [`RAY_DISTANCE_M`] along its eye ray, looks the
//! depth map up where that point lands in the depth camera, scales the
//! depth camera's ray to the stored distance and writes that point's depth
//! in the eye as `frag_depth`. The sprites, drawn later with depth compare
//! `Less`, are then hidden behind the real room. One sample, no march: at
//! the predicted display time the depth camera's pose and the eye's differ
//! by millimeters, so the parallax between the eye ray and the depth ray is
//! negligible for a surface near the lookup distance and small elsewhere
//! (the tests below bound it). Texels with no data (`d >= 1`) and distances
//! under [`NEAR_CUT_M`] (the depth API is unreliable there) are discarded.
//!
//! Plain numbers in, so the math and the shader build and test on the
//! desktop; [`EyeReprojection::frag_depth`] is the CPU mirror of
//! [`ENV_DEPTH_WGSL`]'s fragment stage. The OpenXR provider, its swapchain
//! and the pipelines are Android-only (`runtime` below).

use glam::{Mat4, Quat, Vec2, Vec3};

use crate::math::Fov;

/// Distance along the eye ray at which the depth map is looked up (m).
pub const RAY_DISTANCE_M: f32 = 2.0;
/// Depth under this is discarded (m): the API is unreliable below ~0.2 m.
/// `debug.fosfora.envdepthnear`.
pub const NEAR_CUT_M: f32 = 0.2;
/// The diagnostic's gray scale: 0 m black to this white (m).
pub const SHOW_FAR_M: f32 = 4.0;
/// Rows of [`EyeReprojection::uniform`], `struct EnvEye` in
/// [`ENV_DEPTH_WGSL`].
pub const UNIFORM_ROWS: usize = 21;

/// What the knobs ask of the environment depth.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EnvDepthOptions {
    /// Draw the occluder (`debug.fosfora.envdepth`).
    pub occlude: bool,
    /// Draw the depth map as gray instead, still writing its depth
    /// (`debug.fosfora.envdepthshow`).
    pub show: bool,
    /// Ask the runtime to replace hand pixels with the background
    /// (`debug.fosfora.envdepthhands`), when it supports that.
    pub hand_removal: bool,
    /// Discard distance (m), `debug.fosfora.envdepthnear`.
    pub near_cut_m: f32,
    /// Texture row 0 is the top of the view (`debug.fosfora.envdepthflipv`,
    /// default on); off reads row 0 as the bottom.
    pub flip_v: bool,
}

impl Default for EnvDepthOptions {
    fn default() -> Self {
        Self {
            occlude: false,
            show: false,
            hand_removal: true,
            near_cut_m: NEAR_CUT_M,
            flip_v: true,
        }
    }
}

/// One layer of an acquired depth image: the pose it was rendered from (in
/// the acquire call's space) and its fov.
#[derive(Debug, Clone, Copy)]
pub struct DepthView {
    /// `(x, y, z, w)` quaternion.
    pub orientation: [f32; 4],
    pub position: [f32; 3],
    pub fov: Fov,
}

impl DepthView {
    /// World (the acquire space) to this depth camera: the inverse of its
    /// pose.
    pub fn view_matrix(&self) -> Mat4 {
        let [x, y, z, w] = self.orientation;
        Mat4::from_rotation_translation(Quat::from_xyzw(x, y, z, w), self.position.into()).inverse()
    }
}

/// Whether `near`/`far` describe an infinite projection: the spec uses
/// one when `far < near`, and Meta documents `far` may be infinity.
pub fn is_infinite(near: f32, far: f32) -> bool {
    !far.is_finite() || far <= near
}

/// Metric distance along the depth camera's -Z for the stored value `d`
/// (0..1): the depth-buffer value of an OpenGL projection with planes at
/// `near` and `far` (NDC z = 2d - 1).
pub fn decode_distance(d: f32, near: f32, far: f32) -> f32 {
    if is_infinite(near, far) {
        // Rows [-1, -2n; -1, 0]: ndc = 1 - 2n / dist.
        near / (1.0 - d)
    } else {
        2.0 * near * far / (far + near - (2.0 * d - 1.0) * (far - near))
    }
}

/// Everything the occluder needs for one eye this frame, and the CPU
/// mirror of the fragment stage.
#[derive(Debug, Clone, Copy)]
pub struct EyeReprojection {
    /// The eye's view-projection (wgpu clip space, depth 0..1).
    pub view_proj: Mat4,
    pub eye_pos: Vec3,
    /// World to the depth camera for this eye's layer.
    pub depth_view: Mat4,
    pub depth_fov: Fov,
    pub near: f32,
    pub far: f32,
    /// Depth map size (texels).
    pub depth_size: [u32; 2],
    /// The eye target's size (pixels), for NDC from the fragment position.
    pub target_size: [u32; 2],
    /// Array layer: 0 left, 1 right.
    pub layer: u32,
    pub near_cut_m: f32,
    pub flip_v: bool,
}

impl EyeReprojection {
    /// The rows of `struct EnvEye` in [`ENV_DEPTH_WGSL`].
    pub fn uniform(&self) -> [[f32; 4]; UNIFORM_ROWS] {
        let mut rows = [[0.0f32; 4]; UNIFORM_ROWS];
        let mats = [
            self.view_proj,
            self.view_proj.inverse(),
            self.depth_view,
            self.depth_view.inverse(),
        ];
        for (k, m) in mats.iter().enumerate() {
            for (c, col) in m.to_cols_array_2d().into_iter().enumerate() {
                rows[4 * k + c] = col;
            }
        }
        let e = self.eye_pos;
        rows[16] = [e.x, e.y, e.z, RAY_DISTANCE_M];
        let f = self.depth_fov;
        rows[17] = [f.left.tan(), f.right.tan(), f.up.tan(), f.down.tan()];
        let infinite = is_infinite(self.near, self.far);
        rows[18] = [
            self.near,
            if infinite { 0.0 } else { self.far },
            if infinite { 1.0 } else { 0.0 },
            self.near_cut_m,
        ];
        rows[19] = [
            self.depth_size[0] as f32,
            self.depth_size[1] as f32,
            self.target_size[0] as f32,
            self.target_size[1] as f32,
        ];
        rows[20] = [
            self.layer as f32,
            if self.flip_v { 1.0 } else { 0.0 },
            SHOW_FAR_M,
            0.0,
        ];
        rows
    }

    /// The depth map coordinates (0..1 each, `v` down the texture) where
    /// the depth camera sees depth-camera-space point `pd`; `None` behind it
    /// or outside its fov.
    pub fn depth_uv(&self, pd: Vec3) -> Option<Vec2> {
        if pd.z > -1e-4 {
            return None;
        }
        let t = pd.truncate() / -pd.z;
        let (l, r) = (self.depth_fov.left.tan(), self.depth_fov.right.tan());
        let (up, down) = (self.depth_fov.up.tan(), self.depth_fov.down.tan());
        let u = (t.x - l) / (r - l);
        let v_up = (t.y - down) / (up - down);
        let v = if self.flip_v { 1.0 - v_up } else { v_up };
        ((0.0..1.0).contains(&u) && (0.0..1.0).contains(&v)).then_some(Vec2::new(u, v))
    }

    /// The texel the shader loads for depth map coordinates `uv`.
    pub fn texel(&self, uv: Vec2) -> [u32; 2] {
        let s = Vec2::new(self.depth_size[0] as f32, self.depth_size[1] as f32);
        let t = (uv * s).floor();
        [
            (t.x as u32).min(self.depth_size[0].saturating_sub(1)),
            (t.y as u32).min(self.depth_size[1].saturating_sub(1)),
        ]
    }

    /// The world point the occluder places at fragment position `frag`
    /// (pixels, y down), reading the stored value through `sample` (given
    /// the depth map coordinates; the shader loads the texel
    /// [`Self::texel`] of them). `None` where the shader discards.
    pub fn reproject(&self, frag: Vec2, sample: impl Fn(Vec2) -> f32) -> Option<Vec3> {
        let size = Vec2::new(self.target_size[0] as f32, self.target_size[1] as f32);
        let ndc = Vec2::new(frag.x / size.x * 2.0 - 1.0, 1.0 - frag.y / size.y * 2.0);
        let inv = self.view_proj.inverse();
        let a = inv.project_point3(ndc.extend(0.0));
        let b = inv.project_point3(ndc.extend(1.0));
        let p = self.eye_pos + (b - a).normalize() * RAY_DISTANCE_M;
        let pd = self.depth_view.transform_point3(p);
        let uv = self.depth_uv(pd)?;
        let d = sample(uv);
        if d >= 1.0 {
            return None;
        }
        let dist = decode_distance(d, self.near, self.far);
        if dist < self.near_cut_m {
            return None;
        }
        let q = pd * (dist / -pd.z);
        Some(self.depth_view.inverse().transform_point3(q))
    }

    /// The `frag_depth` the shader writes at `frag` (see [`Self::reproject`]).
    pub fn frag_depth(&self, frag: Vec2, sample: impl Fn(Vec2) -> f32) -> Option<f32> {
        let w = self.reproject(frag, sample)?;
        let clip = self.view_proj * w.extend(1.0);
        (clip.w > 0.0).then(|| (clip.z / clip.w).clamp(0.0, 1.0))
    }
}

/// The occluder's shader. Group 0: binding 0 the eye's `EnvEye` rows
/// ([`EyeReprojection::uniform`]), binding 1 the depth map. The vertex
/// stage is one full-screen triangle; the fragment stage mirrors
/// [`EyeReprojection::frag_depth`] and also returns the decoded distance as
/// gray for the diagnostic pipeline (the occluder masks color off).
pub const ENV_DEPTH_WGSL: &str = r"
struct EnvEye {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    // World to the depth camera of this eye's layer, and back.
    depth_view: mat4x4<f32>,
    inv_depth_view: mat4x4<f32>,
    // xyz the eye position, w the lookup distance along the eye ray (m)
    eye: vec4<f32>,
    // tangents of the depth fov: left, right, up, down
    depth_tan: vec4<f32>,
    // near, far (0 when infinite), 1 when infinite, discard distance (m)
    depth_range: vec4<f32>,
    // depth map width, height, eye target width, height
    sizes: vec4<f32>,
    // x layer (0 left, 1 right), y 1 = texture row 0 is the top,
    // z distance shown as white (m)
    misc: vec4<f32>,
}
@group(0) @binding(0) var<uniform> env: EnvEye;
@group(0) @binding(1) var depth_map: texture_depth_2d_array;

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    // (-1, -1), (3, -1), (-1, 3): one triangle over the whole target.
    let x = f32((i << 1u) & 2u) * 2.0 - 1.0;
    let y = f32(i & 2u) * 2.0 - 1.0;
    return vec4<f32>(x, y, 0.0, 1.0);
}

// Metric distance along the depth camera's -Z for the stored value (an
// OpenGL-convention depth-buffer value, NDC z = 2d - 1).
fn decode(d: f32) -> f32 {
    let n = env.depth_range.x;
    if env.depth_range.z > 0.5 {
        return n / (1.0 - d);
    }
    let f = env.depth_range.y;
    return 2.0 * n * f / (f + n - (2.0 * d - 1.0) * (f - n));
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        return c / 12.92;
    }
    return pow((c + 0.055) / 1.055, 2.4);
}

struct FragOut {
    @location(0) color: vec4<f32>,
    @builtin(frag_depth) depth: f32,
}

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> FragOut {
    // Framebuffer y runs down, NDC y up.
    let ndc = vec2<f32>(pos.x / env.sizes.z * 2.0 - 1.0, 1.0 - pos.y / env.sizes.w * 2.0);
    let a = env.inv_view_proj * vec4<f32>(ndc, 0.0, 1.0);
    let b = env.inv_view_proj * vec4<f32>(ndc, 1.0, 1.0);
    let dir = normalize(b.xyz / b.w - a.xyz / a.w);
    let p = env.eye.xyz + dir * env.eye.w;
    let pd = (env.depth_view * vec4<f32>(p, 1.0)).xyz;
    if pd.z > -1e-4 {
        discard;
    }
    let t = pd.xy / -pd.z;
    let u = (t.x - env.depth_tan.x) / (env.depth_tan.y - env.depth_tan.x);
    var v = (t.y - env.depth_tan.w) / (env.depth_tan.z - env.depth_tan.w);
    if env.misc.y > 0.5 {
        v = 1.0 - v;
    }
    if u < 0.0 || u >= 1.0 || v < 0.0 || v >= 1.0 {
        discard;
    }
    let size = vec2<i32>(env.sizes.xy);
    let texel = min(vec2<i32>(floor(vec2<f32>(u, v) * env.sizes.xy)), size - vec2<i32>(1));
    let d = textureLoad(depth_map, texel, i32(env.misc.x), 0);
    // 1 is no data (or infinity): nothing to occlude with.
    if d >= 1.0 {
        discard;
    }
    let dist = decode(d);
    if dist < env.depth_range.w {
        discard;
    }
    let q = pd * (dist / -pd.z);
    let w = env.inv_depth_view * vec4<f32>(q, 1.0);
    let clip = env.view_proj * vec4<f32>(w.xyz / w.w, 1.0);
    if clip.w <= 0.0 {
        discard;
    }
    var out: FragOut;
    out.depth = clamp(clip.z / clip.w, 0.0, 1.0);
    // Diagnostic gray, linear in distance in the stored (sRGB) bytes: a
    // screencap's gray level / 255 x misc.z is the distance in meters.
    let g = srgb_to_linear(clamp(dist / env.misc.z, 0.0, 1.0));
    out.color = vec4<f32>(g, g, g, 1.0);
    return out;
}
";

#[cfg(target_os = "android")]
pub use runtime::{EnvDepth, EnvDepthDraw, EnvDepthFrame};

/// The OpenXR provider, its swapchain wrapped as wgpu textures, and the
/// occluder pipeline. The `openxr` crate binds the extension's function
/// table but has no safe wrapper, so the calls go through it raw (the
/// pattern of `perf.rs`).
#[cfg(target_os = "android")]
mod runtime {
    use std::ptr;
    use std::time::Instant;

    use anyhow::{Context, Result, bail};
    use ash::vk::{self, Handle as _};
    use log::{info, warn};
    use openxr as xr;
    use xr::sys;
    use xr::sys::Handle as _;

    use super::{DepthView, ENV_DEPTH_WGSL, EnvDepthOptions, EyeReprojection, UNIFORM_ROWS};
    use crate::gfx::{EyeCamera, Gfx, SWAPCHAIN_FORMAT};
    use crate::math::Fov;
    use crate::particles3d::DEPTH_FORMAT;
    use crate::room::check;

    /// Frames between retries of a provider start that failed.
    const START_RETRY_FRAMES: u32 = 144;
    /// Acquired frames between the running-count log lines (~10 s at 72 Hz).
    const LOG_EVERY_FRAMES: u64 = 720;

    /// One acquired depth image: which swapchain image, its projection's
    /// planes and the pose and fov of each layer.
    #[derive(Debug, Clone, Copy)]
    pub struct EnvDepthFrame {
        pub index: u32,
        pub near: f32,
        pub far: f32,
        pub views: [DepthView; 2],
    }

    /// The occluder draw for one frame: its uniforms are written, the
    /// bind groups point at the acquired image.
    pub struct EnvDepthDraw<'a> {
        pipeline: &'a wgpu::RenderPipeline,
        bind_groups: &'a [wgpu::BindGroup; 2],
    }

    impl EnvDepthDraw<'_> {
        /// One full-screen triangle for `eye` (0 left, 1 right).
        pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, eye: usize) {
            let Some(group) = self.bind_groups.get(eye) else {
                return;
            };
            pass.set_pipeline(self.pipeline);
            pass.set_bind_group(0, group, &[]);
            pass.draw(0..3, 0..1);
        }
    }

    /// The raw handles, destroyed on drop: the provider stopped, then the
    /// swapchain, then the provider.
    struct Provider {
        fp: xr::raw::EnvironmentDepthMETA,
        handle: sys::EnvironmentDepthProviderMETA,
        swapchain: sys::EnvironmentDepthSwapchainMETA,
        running: bool,
    }

    impl Provider {
        fn start(&mut self) -> Result<()> {
            // SAFETY: the provider handle is live (destroyed only in `drop`).
            check(unsafe { (self.fp.start_environment_depth_provider)(self.handle) })
                .context("xrStartEnvironmentDepthProviderMETA")?;
            self.running = true;
            Ok(())
        }
    }

    impl Drop for Provider {
        fn drop(&mut self) {
            if self.running {
                // SAFETY: the provider handle is live until the destroy below.
                let res = unsafe { (self.fp.stop_environment_depth_provider)(self.handle) };
                if let Err(e) = check(res) {
                    warn!("xrStopEnvironmentDepthProviderMETA: {e:#}");
                }
            }
            if self.swapchain.into_raw() != 0 {
                // SAFETY: the swapchain was created from this provider and is
                // destroyed once; the wgpu wrappers of its images are
                // dropped before this (field order in `EnvDepth`).
                let res = unsafe { (self.fp.destroy_environment_depth_swapchain)(self.swapchain) };
                if let Err(e) = check(res) {
                    warn!("xrDestroyEnvironmentDepthSwapchainMETA: {e:#}");
                }
            }
            // SAFETY: the provider handle is live and destroyed once, after
            // its swapchain; the session it belongs to outlives it
            // (`XrSession` field order).
            let res = unsafe { (self.fp.destroy_environment_depth_provider)(self.handle) };
            if let Err(e) = check(res) {
                warn!("xrDestroyEnvironmentDepthProviderMETA: {e:#}");
            }
        }
    }

    /// The live environment depth: provider, swapchain, the images as wgpu
    /// textures and the occluder (or diagnostic) pipeline. Session-owned.
    pub struct EnvDepth {
        // wgpu objects first: they reference the runtime's images, which
        // the swapchain in `provider` owns (fields drop in order).
        /// Per swapchain image, per eye: the eye's uniform and the image.
        bind_groups: Vec<[wgpu::BindGroup; 2]>,
        _images: Vec<(wgpu::Texture, wgpu::TextureView)>,
        uniforms: [wgpu::Buffer; 2],
        pipeline: wgpu::RenderPipeline,
        provider: Provider,
        size: [u32; 2],
        opts: EnvDepthOptions,
        created: Instant,
        acquired: u64,
        not_available: u64,
        errors: u64,
        range: Option<(f32, f32)>,
        start_retry_in: u32,
    }

    impl EnvDepth {
        /// Bring the provider up and start it. `None` when the extension is
        /// not enabled or anything fails (logged): the app runs without it.
        pub fn new(
            session: &xr::Session<xr::Vulkan>,
            system: xr::SystemId,
            gfx: &Gfx,
            opts: EnvDepthOptions,
        ) -> Option<Self> {
            match Self::try_new(session, system, gfx, opts) {
                Ok(Some(d)) => Some(d),
                Ok(None) => {
                    warn!(
                        "environment depth requested but XR_META_environment_depth is not enabled"
                    );
                    None
                }
                Err(e) => {
                    warn!("environment depth unavailable: {e:#}");
                    None
                }
            }
        }

        fn try_new(
            session: &xr::Session<xr::Vulkan>,
            system: xr::SystemId,
            gfx: &Gfx,
            opts: EnvDepthOptions,
        ) -> Result<Option<Self>> {
            let instance = session.instance();
            let Some(fp) = instance.exts().meta_environment_depth else {
                return Ok(None);
            };
            let (supported, hand_removal_supported) = system_support(instance, system)?;
            if !supported {
                bail!("the system reports no environment depth support");
            }
            let create = sys::EnvironmentDepthProviderCreateInfoMETA {
                ty: sys::EnvironmentDepthProviderCreateInfoMETA::TYPE,
                next: ptr::null(),
                create_flags: sys::EnvironmentDepthProviderCreateFlagsMETA::EMPTY,
            };
            let mut handle = sys::EnvironmentDepthProviderMETA::NULL;
            // SAFETY: `create` is a fully initialized input struct and
            // `handle` an output slot, both outliving the call; the session
            // is live.
            check(unsafe {
                (fp.create_environment_depth_provider)(session.as_raw(), &create, &mut handle)
            })
            .context("xrCreateEnvironmentDepthProviderMETA")?;
            // From here the provider (and later its swapchain) is destroyed
            // on any early return.
            let mut provider = Provider {
                fp,
                handle,
                swapchain: sys::EnvironmentDepthSwapchainMETA::NULL,
                running: false,
            };
            let create = sys::EnvironmentDepthSwapchainCreateInfoMETA {
                ty: sys::EnvironmentDepthSwapchainCreateInfoMETA::TYPE,
                next: ptr::null(),
                create_flags: sys::EnvironmentDepthSwapchainCreateFlagsMETA::EMPTY,
            };
            // SAFETY: as above; the provider handle is live.
            check(unsafe {
                (fp.create_environment_depth_swapchain)(
                    provider.handle,
                    &create,
                    &mut provider.swapchain,
                )
            })
            .context("xrCreateEnvironmentDepthSwapchainMETA")?;
            let mut state = sys::EnvironmentDepthSwapchainStateMETA {
                ty: sys::EnvironmentDepthSwapchainStateMETA::TYPE,
                next: ptr::null_mut(),
                width: 0,
                height: 0,
            };
            // SAFETY: `state` is an initialized output struct of the type
            // the call fills; the swapchain is live.
            check(unsafe {
                (fp.get_environment_depth_swapchain_state)(provider.swapchain, &mut state)
            })
            .context("xrGetEnvironmentDepthSwapchainStateMETA")?;
            let (width, height) = (state.width, state.height);
            let mut count = 0u32;
            // SAFETY: the two-call idiom's sizing call: capacity 0 and a
            // null array, so the runtime writes only the count.
            check(unsafe {
                (fp.enumerate_environment_depth_swapchain_images)(
                    provider.swapchain,
                    0,
                    &mut count,
                    ptr::null_mut(),
                )
            })
            .context("xrEnumerateEnvironmentDepthSwapchainImagesMETA (count)")?;
            let blank = sys::SwapchainImageVulkanKHR {
                ty: sys::SwapchainImageVulkanKHR::TYPE,
                next: ptr::null_mut(),
                image: 0,
            };
            let mut raw = vec![blank; count as usize];
            // SAFETY: `raw` holds exactly `count` Vulkan image structs with
            // their type set, the capacity passed; every one starts with
            // the base header the call takes, and the runtime writes at most
            // that many.
            check(unsafe {
                (fp.enumerate_environment_depth_swapchain_images)(
                    provider.swapchain,
                    count,
                    &mut count,
                    raw.as_mut_ptr().cast(),
                )
            })
            .context("xrEnumerateEnvironmentDepthSwapchainImagesMETA")?;
            raw.truncate(count as usize);
            if raw.is_empty() {
                bail!("the depth swapchain has no images");
            }
            let images = raw
                .iter()
                .map(|i| gfx.wrap_env_depth_image(vk::Image::from_raw(i.image), width, height))
                .collect::<Result<Vec<_>>>()?;

            let hand_removal = hand_removal_supported && opts.hand_removal;
            if hand_removal_supported {
                let set = sys::EnvironmentDepthHandRemovalSetInfoMETA {
                    ty: sys::EnvironmentDepthHandRemovalSetInfoMETA::TYPE,
                    next: ptr::null(),
                    enabled: hand_removal.into(),
                };
                // SAFETY: `set` is a fully initialized input struct that
                // outlives the call; the provider is live.
                let res = unsafe { (fp.set_environment_depth_hand_removal)(provider.handle, &set) };
                if let Err(e) = check(res) {
                    warn!("xrSetEnvironmentDepthHandRemovalMETA({hand_removal}): {e:#}");
                }
            }
            // A start that fails here (the session not running yet, say) is
            // retried from `acquire`.
            let start_retry_in = match provider.start() {
                Ok(()) => 0,
                Err(e) => {
                    warn!("environment depth: {e:#} (retrying from the frame loop)");
                    START_RETRY_FRAMES
                }
            };

            let (pipeline, layout) = build_pipeline(&gfx.device, opts.show);
            let uniforms = [0, 1].map(|eye| {
                gfx.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(if eye == 0 {
                        "xr-env-depth-eye-0"
                    } else {
                        "xr-env-depth-eye-1"
                    }),
                    size: (UNIFORM_ROWS * 16) as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            });
            let bind_groups = images
                .iter()
                .map(|(_, view)| {
                    [0, 1].map(|eye| {
                        gfx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                            label: Some("xr-env-depth"),
                            layout: &layout,
                            entries: &[
                                wgpu::BindGroupEntry {
                                    binding: 0,
                                    resource: uniforms[eye].as_entire_binding(),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 1,
                                    resource: wgpu::BindingResource::TextureView(view),
                                },
                            ],
                        })
                    })
                })
                .collect();
            info!(
                "environment depth: swapchain {} images, {width}x{height} D16 x 2 layers · hand removal {} (supported {hand_removal_supported}, asked {}) · {} · discard under {} m · v flip {}",
                images.len(),
                if hand_removal { "on" } else { "off" },
                opts.hand_removal,
                if opts.show {
                    "diagnostic gray (writes depth too)"
                } else {
                    "occluder"
                },
                opts.near_cut_m,
                opts.flip_v,
            );
            Ok(Some(Self {
                bind_groups,
                _images: images,
                uniforms,
                pipeline,
                provider,
                size: [width, height],
                opts,
                created: Instant::now(),
                acquired: 0,
                not_available: 0,
                errors: 0,
                range: None,
                start_retry_in,
            }))
        }

        /// The depth image for display time `time`, located in `space`.
        /// `None` until the provider has produced one
        /// (`ENVIRONMENT_DEPTH_NOT_AVAILABLE_META`, counted) or on an
        /// error (logged, rate-limited).
        pub fn acquire(&mut self, space: &xr::Space, time: xr::Time) -> Option<EnvDepthFrame> {
            if !self.provider.running {
                self.start_retry_in = self.start_retry_in.saturating_sub(1);
                if self.start_retry_in > 0 {
                    return None;
                }
                match self.provider.start() {
                    Ok(()) => info!("environment depth: provider started on retry"),
                    Err(e) => {
                        warn!("environment depth: {e:#}");
                        self.start_retry_in = START_RETRY_FRAMES;
                        return None;
                    }
                }
            }
            let info = sys::EnvironmentDepthImageAcquireInfoMETA {
                ty: sys::EnvironmentDepthImageAcquireInfoMETA::TYPE,
                next: ptr::null(),
                space: space.as_raw(),
                display_time: time,
            };
            let view = sys::EnvironmentDepthImageViewMETA {
                ty: sys::EnvironmentDepthImageViewMETA::TYPE,
                next: ptr::null(),
                fov: sys::Fovf {
                    angle_left: 0.0,
                    angle_right: 0.0,
                    angle_up: 0.0,
                    angle_down: 0.0,
                },
                pose: sys::Posef::IDENTITY,
            };
            let mut image = sys::EnvironmentDepthImageMETA {
                ty: sys::EnvironmentDepthImageMETA::TYPE,
                next: ptr::null(),
                swapchain_index: 0,
                near_z: 0.0,
                far_z: 0.0,
                views: [view; 2],
            };
            // SAFETY: `info` is a fully initialized input struct and `image`
            // an output struct with its own and both views' types set, all
            // outliving the call; the provider and the space are live.
            let res = unsafe {
                (self.provider.fp.acquire_environment_depth_image)(
                    self.provider.handle,
                    &info,
                    &mut image,
                )
            };
            if res == sys::Result::ENVIRONMENT_DEPTH_NOT_AVAILABLE_META {
                self.not_available += 1;
                return None;
            }
            if let Err(e) = check(res) {
                self.errors += 1;
                if self.errors == 1 || self.errors.is_multiple_of(LOG_EVERY_FRAMES) {
                    warn!(
                        "xrAcquireEnvironmentDepthImageMETA: {e:#} ({} so far)",
                        self.errors
                    );
                }
                return None;
            }
            if image.swapchain_index as usize >= self.bind_groups.len() {
                self.errors += 1;
                warn!(
                    "environment depth: image index {} past the swapchain's {}",
                    image.swapchain_index,
                    self.bind_groups.len()
                );
                return None;
            }
            self.acquired += 1;
            let (near, far) = (image.near_z, image.far_z);
            if self.acquired == 1 {
                info!(
                    "environment depth: first frame after {} not-available frames, {} ms after creation · near {near} far {far} ({}) · {}x{} · layer 0 fov deg [{:.1} {:.1} {:.1} {:.1}]",
                    self.not_available,
                    self.created.elapsed().as_millis(),
                    if super::is_infinite(near, far) {
                        "infinite"
                    } else {
                        "finite"
                    },
                    self.size[0],
                    self.size[1],
                    image.views[0].fov.angle_left.to_degrees(),
                    image.views[0].fov.angle_right.to_degrees(),
                    image.views[0].fov.angle_up.to_degrees(),
                    image.views[0].fov.angle_down.to_degrees(),
                );
            } else if self
                .range
                .is_some_and(|(n, f)| n.to_bits() != near.to_bits() || f.to_bits() != far.to_bits())
            {
                info!("environment depth: near/far now {near} / {far}");
            }
            self.range = Some((near, far));
            if self.acquired.is_multiple_of(LOG_EVERY_FRAMES) {
                info!(
                    "environment depth: {} frames acquired, {} not available, {} errors",
                    self.acquired, self.not_available, self.errors
                );
            }
            let depth_view = |v: &sys::EnvironmentDepthImageViewMETA| DepthView {
                orientation: [
                    v.pose.orientation.x,
                    v.pose.orientation.y,
                    v.pose.orientation.z,
                    v.pose.orientation.w,
                ],
                position: [v.pose.position.x, v.pose.position.y, v.pose.position.z],
                fov: Fov {
                    left: v.fov.angle_left,
                    right: v.fov.angle_right,
                    up: v.fov.angle_up,
                    down: v.fov.angle_down,
                },
            };
            Some(EnvDepthFrame {
                index: image.swapchain_index,
                near,
                far,
                views: [depth_view(&image.views[0]), depth_view(&image.views[1])],
            })
        }

        /// Write both eyes' uniforms for `frame` and return the draw.
        /// `cameras` and `extents` (the eye targets' sizes) are in eye
        /// order, left first, like the depth map's layers.
        pub fn prepare(
            &self,
            queue: &wgpu::Queue,
            frame: &EnvDepthFrame,
            cameras: &[EyeCamera],
            extents: &[[u32; 2]],
        ) -> EnvDepthDraw<'_> {
            for (eye, (cam, extent)) in cameras.iter().zip(extents).enumerate().take(2) {
                let depth = frame.views[eye];
                let r = EyeReprojection {
                    view_proj: cam.view_proj(),
                    eye_pos: cam.view.inverse().w_axis.truncate(),
                    depth_view: depth.view_matrix(),
                    depth_fov: depth.fov,
                    near: frame.near,
                    far: frame.far,
                    depth_size: self.size,
                    target_size: *extent,
                    layer: eye as u32,
                    near_cut_m: self.opts.near_cut_m,
                    flip_v: self.opts.flip_v,
                };
                queue.write_buffer(&self.uniforms[eye], 0, bytemuck::cast_slice(&r.uniform()));
            }
            EnvDepthDraw {
                pipeline: &self.pipeline,
                bind_groups: &self.bind_groups[frame.index as usize],
            }
        }
    }

    /// `supports_environment_depth` and `supports_hand_removal` from the
    /// system properties.
    fn system_support(instance: &xr::Instance, system: xr::SystemId) -> Result<(bool, bool)> {
        let mut depth = sys::SystemEnvironmentDepthPropertiesMETA {
            ty: sys::SystemEnvironmentDepthPropertiesMETA::TYPE,
            next: ptr::null_mut(),
            supports_environment_depth: sys::FALSE,
            supports_hand_removal: sys::FALSE,
        };
        let mut props = sys::SystemProperties::out(ptr::from_mut(&mut depth).cast());
        // SAFETY: `props` has its header written (type, and `next` pointing
        // at `depth`, an initialized output struct of the chained type);
        // both outlive the call, and the runtime fills the rest. The
        // instance is live and `system` came from it.
        check(unsafe {
            (instance.fp().get_system_properties)(instance.as_raw(), system, props.as_mut_ptr())
        })
        .context("xrGetSystemProperties (environment depth)")?;
        Ok((
            depth.supports_environment_depth.into(),
            depth.supports_hand_removal.into(),
        ))
    }

    /// The occluder (`show` false: color writes off) or the diagnostic
    /// (`show` true: the gray written over the view), off the same shader;
    /// either writes depth with compare `Always`.
    fn build_pipeline(
        device: &wgpu::Device,
        show: bool,
    ) -> (wgpu::RenderPipeline, wgpu::BindGroupLayout) {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("xr-env-depth"),
            source: wgpu::ShaderSource::Wgsl(ENV_DEPTH_WGSL.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("xr-env-depth"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new((UNIFORM_ROWS * 16) as u64),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("xr-env-depth"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(if show {
                "xr-env-depth-show"
            } else {
                "xr-env-depth-occluder"
            }),
            layout: Some(&pipeline_layout),
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
            // First in the pass, over the cleared depth: Always, so every
            // fragment the shader keeps lands.
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Always,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: SWAPCHAIN_FORMAT,
                    blend: None,
                    write_mask: if show {
                        wgpu::ColorWrites::ALL
                    } else {
                        wgpu::ColorWrites::empty()
                    },
                })],
            }),
            multiview: None,
            cache: None,
        });
        (pipeline, layout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math;

    const FOV: Fov = Fov {
        left: -0.9,
        right: 0.8,
        up: 0.85,
        down: -0.9,
    };
    const NEAR: f32 = 0.05;
    const FAR: f32 = 100.0;

    /// The stored value for metric distance `dist` through the OpenGL
    /// projection's z rows (finite, or the infinite limit), as the runtime
    /// writes it.
    fn encode(dist: f32, n: f32, f: f32) -> f32 {
        let z = -dist;
        let (a, b) = if is_infinite(n, f) {
            (-1.0, -2.0 * n)
        } else {
            (-(f + n) / (f - n), -2.0 * f * n / (f - n))
        };
        let ndc = (a * z + b) / -z;
        (ndc + 1.0) * 0.5
    }

    #[test]
    fn decode_hits_the_planes_and_round_trips() {
        let (n, f) = (0.1, 10.0);
        assert!((decode_distance(0.0, n, f) - n).abs() < 1e-6);
        assert!((decode_distance(1.0, n, f) - f).abs() < 1e-4);
        for dist in [0.15, 0.5, 1.0, 2.0, 3.7, 9.0] {
            let d = encode(dist, n, f);
            assert!((0.0..1.0).contains(&d), "{d}");
            let back = decode_distance(d, n, f);
            assert!((back - dist).abs() < dist * 1e-4, "{dist} -> {d} -> {back}");
        }
    }

    #[test]
    fn infinite_far_decodes_to_infinity_at_one() {
        let n = 0.1;
        for far in [f32::INFINITY, 0.05, n] {
            assert!(is_infinite(n, far));
            assert!((decode_distance(0.0, n, far) - n).abs() < 1e-6);
            assert!(decode_distance(1.0, n, far).is_infinite());
            for dist in [0.3, 1.0, 2.5, 6.0] {
                let back = decode_distance(encode(dist, n, far), n, far);
                assert!((back - dist).abs() < dist * 1e-3, "{dist} -> {back}");
            }
        }
        assert!(!is_infinite(n, 10.0));
    }

    fn eye(orientation: [f32; 4], position: [f32; 3]) -> (Mat4, Vec3) {
        (
            math::view_projection(orientation, position, FOV, NEAR, FAR),
            Vec3::from(position),
        )
    }

    fn reprojection(eye_pos: [f32; 3], depth: DepthView) -> EyeReprojection {
        let rot = [0.0, 0.0, 0.0, 1.0];
        let (view_proj, eye_pos) = eye(rot, eye_pos);
        EyeReprojection {
            view_proj,
            eye_pos,
            depth_view: depth.view_matrix(),
            depth_fov: depth.fov,
            near: 0.1,
            far: f32::INFINITY,
            depth_size: [320, 320],
            target_size: [1680, 1760],
            layer: 0,
            near_cut_m: NEAR_CUT_M,
            flip_v: true,
        }
    }

    const FRAGS: [[f32; 2]; 5] = [
        [840.0, 880.0],
        [100.5, 200.5],
        [1600.5, 1700.5],
        [300.5, 1500.5],
        [1400.5, 400.5],
    ];

    #[test]
    fn same_pose_writes_the_eye_depth_of_the_stored_distance() {
        let pos = [0.03, 1.2, -0.4];
        let r = reprojection(
            pos,
            DepthView {
                orientation: [0.0, 0.0, 0.0, 1.0],
                position: pos,
                fov: FOV,
            },
        );
        let view = Mat4::from_translation(Vec3::from(pos)).inverse();
        let proj = math::projection(FOV, NEAR, FAR);
        for z in [0.4, 1.0, 2.0, 3.5] {
            let stored = encode(z, r.near, r.far);
            for [x, y] in FRAGS {
                let frag = Vec2::new(x, y);
                let got = r.frag_depth(frag, |_| stored).expect("inside both fovs");
                // The point on this pixel's eye ray at view-space z = -z.
                let ndc = Vec2::new(
                    x / r.target_size[0] as f32 * 2.0 - 1.0,
                    1.0 - y / r.target_size[1] as f32 * 2.0,
                );
                let ray = proj.inverse().project_point3(ndc.extend(0.5));
                let p = ray * (z / -ray.z);
                let clip = proj * p.extend(1.0);
                let want = clip.z / clip.w;
                assert!(
                    (got - want).abs() < 1e-5,
                    "z {z} frag {frag}: {got} vs {want}"
                );
                // And the world point sits on the eye's ray at that depth.
                let w = r.reproject(frag, |_| stored).unwrap();
                assert!((view.transform_point3(w).z + z).abs() < 1e-4);
            }
        }
    }

    #[test]
    fn no_data_near_and_outside_are_discarded() {
        let pos = [0.0, 1.5, 0.0];
        let r = reprojection(
            pos,
            DepthView {
                orientation: [0.0, 0.0, 0.0, 1.0],
                position: pos,
                fov: FOV,
            },
        );
        let frag = Vec2::new(840.0, 880.0);
        assert!(r.frag_depth(frag, |_| 1.0).is_none());
        assert!(
            r.frag_depth(frag, |_| encode(0.15, r.near, r.far))
                .is_none()
        );
        assert!(
            r.frag_depth(frag, |_| encode(0.25, r.near, r.far))
                .is_some()
        );
        // A depth camera turned away sees none of the eye's rays.
        let away = reprojection(
            pos,
            DepthView {
                orientation: Quat::from_rotation_y(std::f32::consts::PI).to_array(),
                position: pos,
                fov: FOV,
            },
        );
        assert!(away.frag_depth(frag, |_| 0.5).is_none());
    }

    #[test]
    fn texture_rows_run_down_the_view() {
        let pos = [0.0, 0.0, 0.0];
        let mut r = reprojection(
            pos,
            DepthView {
                orientation: [0.0, 0.0, 0.0, 1.0],
                position: pos,
                fov: FOV,
            },
        );
        // Just inside the top-left corner of the depth fov.
        let top_left = Vec3::new(FOV.left.tan() + 1e-3, FOV.up.tan() - 1e-3, -1.0);
        assert_eq!(r.texel(r.depth_uv(top_left).unwrap()), [0, 0]);
        let bottom_right = Vec3::new(FOV.right.tan() - 1e-3, FOV.down.tan() + 1e-3, -1.0);
        assert_eq!(r.texel(r.depth_uv(bottom_right).unwrap()), [319, 319]);
        r.flip_v = false;
        assert_eq!(r.texel(r.depth_uv(top_left).unwrap()), [0, 319]);
        // Behind the camera or outside the fov: nothing.
        assert!(r.depth_uv(Vec3::new(0.0, 0.0, 1.0)).is_none());
        assert!(r.depth_uv(Vec3::new(5.0, 0.0, -1.0)).is_none());
    }

    /// Intersect the ray `o + t d` (t > 0) with the plane through `c` with
    /// normal `n`.
    fn hit(o: Vec3, d: Vec3, c: Vec3, n: Vec3) -> Vec3 {
        let t = (c - o).dot(n) / d.dot(n);
        assert!(t > 0.0);
        o + d * t
    }

    /// Eye-space depth error (m) of the reprojected point against the true
    /// hit of the eye ray through `frag`, for a plane with normal `n` that
    /// crosses that ray `along` meters from the eye; the depth camera sits
    /// `offset` from the eye (same orientation, same fov).
    fn parallax_error(offset: Vec3, along: f32, n: Vec3, frag: Vec2) -> f32 {
        let eye_pos = Vec3::new(0.0, 1.5, 0.0);
        let depth_pos = eye_pos + offset;
        let r = reprojection(
            eye_pos.into(),
            DepthView {
                orientation: [0.0, 0.0, 0.0, 1.0],
                position: depth_pos.into(),
                fov: FOV,
            },
        );
        let size = Vec2::new(r.target_size[0] as f32, r.target_size[1] as f32);
        let ndc = Vec2::new(frag.x / size.x * 2.0 - 1.0, 1.0 - frag.y / size.y * 2.0);
        let inv = r.view_proj.inverse();
        let ray =
            (inv.project_point3(ndc.extend(1.0)) - inv.project_point3(ndc.extend(0.0))).normalize();
        let truth = eye_pos + ray * along;
        // The depth map: along the depth ray through `uv`, the plane's
        // distance down -Z, stored as the runtime would. Sampled at the
        // exact coordinates, so this measures the parallax alone.
        let (l, rt) = (FOV.left.tan(), FOV.right.tan());
        let (up, down) = (FOV.up.tan(), FOV.down.tan());
        let sample = |uv: Vec2| {
            let v_up = 1.0 - uv.y;
            let dir = Vec3::new(l + uv.x * (rt - l), down + v_up * (up - down), -1.0);
            let q = hit(depth_pos, dir, truth, n) - depth_pos;
            encode(-q.z, r.near, r.far)
        };
        let w = r.reproject(frag, sample).expect("inside the depth fov");
        // The eye looks down -Z: view-space depth is the world z offset.
        (w.z - truth.z).abs()
    }

    #[test]
    fn a_centimeter_of_parallax_costs_under_a_millimeter_at_two_meters() {
        let offset = Vec3::new(0.01, 0.0, 0.0);
        let frags = [
            Vec2::new(840.0, 880.0),
            Vec2::new(200.5, 300.5),
            Vec2::new(1500.5, 1600.5),
        ];
        let turned = |deg: f32| Quat::from_rotation_y(deg.to_radians()) * Vec3::Z;
        // A surface 2 m along the ray, facing the eye or turned 45° and
        // 60°: the lookup point lies on it, so only the sampling differs.
        for deg in [0.0f32, 45.0, 60.0] {
            for frag in frags {
                let e = parallax_error(offset, RAY_DISTANCE_M, turned(deg), frag);
                assert!(e < 1e-3, "{deg}° at {frag}: {e} m");
            }
        }
        // Away from 2 m the depth ray crosses the surface beside the eye's
        // hit, by about offset x |D - 2| / 2 sideways. A fronto-parallel
        // wall does not care (zero up to float noise); a 45° one turns it
        // into depth: 4.5 mm at 1 m and 3 m, 6 mm at 0.6 m, 9 mm at 4 m
        // for a 1 cm offset. The map's own texel footprint (not modeled
        // here) comes on top.
        for along in [0.6f32, 1.0, 3.0, 4.0] {
            let e = parallax_error(offset, along, Vec3::Z, frags[0]);
            assert!(e < 1e-4, "frontal at {along} m: {e} m");
            let e = parallax_error(offset, along, turned(45.0), frags[0]);
            let bound = 0.01 * (along - 2.0).abs() / 2.0 * 1.5 + 1e-4;
            assert!(e < bound, "45° at {along} m: {e} m (bound {bound})");
        }
    }

    #[test]
    fn the_shader_validates_and_matches_the_rows() {
        let module = naga::front::wgsl::parse_str(ENV_DEPTH_WGSL).expect("env depth WGSL parses");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::default(),
        )
        .validate(&module)
        .expect("env depth WGSL validates");
        let (_, ty) = module
            .types
            .iter()
            .find(|(_, t)| t.name.as_deref() == Some("EnvEye"))
            .expect("struct EnvEye");
        let naga::TypeInner::Struct { span, .. } = ty.inner else {
            panic!("EnvEye is not a struct");
        };
        assert_eq!(span as usize, UNIFORM_ROWS * 16);
    }

    #[test]
    fn the_uniform_packs_the_matrices_and_flags() {
        let pos = [0.1, 1.4, 0.2];
        let mut r = reprojection(
            pos,
            DepthView {
                orientation: [0.0, 0.0, 0.0, 1.0],
                position: [0.0, 1.4, 0.2],
                fov: FOV,
            },
        );
        let rows = r.uniform();
        assert_eq!(rows[0..4], r.view_proj.to_cols_array_2d());
        assert_eq!(rows[8..12], r.depth_view.to_cols_array_2d());
        assert_eq!(rows[16], [0.1, 1.4, 0.2, RAY_DISTANCE_M]);
        assert_eq!(rows[18], [0.1, 0.0, 1.0, NEAR_CUT_M]);
        assert_eq!(rows[19], [320.0, 320.0, 1680.0, 1760.0]);
        assert_eq!(rows[20], [0.0, 1.0, SHOW_FAR_M, 0.0]);
        r.far = 20.0;
        r.layer = 1;
        let rows = r.uniform();
        assert_eq!(rows[18][..3], [0.1, 20.0, 0.0]);
        assert_eq!(rows[20][0], 1.0);
        // The inverse view-projection really inverts.
        let inv = Mat4::from_cols_array_2d(&[rows[4], rows[5], rows[6], rows[7]]);
        assert!((inv * r.view_proj).abs_diff_eq(Mat4::IDENTITY, 1e-4));
    }
}
