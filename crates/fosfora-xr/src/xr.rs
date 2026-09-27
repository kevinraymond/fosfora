//! OpenXR bring-up on Android: loader, instance, system, session lifecycle,
//! per-eye swapchains and the frame loop ("OpenXR bring-up" in
//! `docs/xr/XR_DESIGN.md`).

use android_activity::AndroidApp;
use anyhow::{Context, Result, anyhow, bail};
use ash::vk::{self, Handle as _};
use log::{info, warn};
use openxr as xr;

use crate::app::FrameStats;
use crate::gfx::{EyeCamera, Gfx};
use crate::math;
use crate::particles3d::Particles3d;

pub const VIEW_TYPE: xr::ViewConfigurationType = xr::ViewConfigurationType::PRIMARY_STEREO;
pub const EYE_COUNT: usize = 2;

const NEAR: f32 = 0.05;
const FAR: f32 = 100.0;

/// Instance-level state: created once, before any Vulkan object exists.
pub struct XrContext {
    pub instance: xr::Instance,
    pub system: xr::SystemId,
    pub blend_mode: xr::EnvironmentBlendMode,
    pub views: Vec<xr::ViewConfigurationView>,
    pub has_refresh_rate_ext: bool,
}

impl XrContext {
    pub fn new(app: &AndroidApp) -> Result<Self> {
        // SAFETY: `android-activity` hands us the process JavaVM and the live
        // `NativeActivity` JNI reference for the activity that owns this thread;
        // both stay valid until `android_main` returns.
        let platform =
            unsafe { xr::AndroidPlatformInfo::new(app.vm_as_ptr(), app.activity_as_ptr()) };
        // SAFETY: `libopenxr_loader.so` in the APK is the unmodified Khronos
        // loader (Apache-2.0, Maven `org.khronos.openxr:openxr_loader_for_android`),
        // which conforms to the specification. `Entry::load` also runs
        // `xrInitializeLoaderKHR` with the Android platform info.
        let entry = unsafe { xr::Entry::load(&platform) }
            .map_err(|e| anyhow!("loading libopenxr_loader.so: {e}"))?;

        let available = entry
            .enumerate_extensions()
            .context("xrEnumerateInstanceExtensionProperties")?;
        let names: Vec<String> = available
            .names()
            .iter()
            .map(|n| String::from_utf8_lossy(n).into_owned())
            .collect();
        info!("runtime extensions ({}): {}", names.len(), names.join(" "));
        if !available.khr_vulkan_enable2 {
            bail!("runtime lacks XR_KHR_vulkan_enable2");
        }
        if !available.khr_android_create_instance {
            bail!("runtime lacks XR_KHR_android_create_instance");
        }

        let mut enabled = xr::ExtensionSet::default();
        enabled.khr_vulkan_enable2 = true;
        enabled.khr_android_create_instance = true;
        // Optional: lets us read (and later request) the display refresh rate.
        enabled.fb_display_refresh_rate = available.fb_display_refresh_rate;

        let instance = entry
            .create_instance(
                &xr::ApplicationInfo {
                    application_name: "Fosfora VR",
                    application_version: 1,
                    engine_name: "fosfora",
                    engine_version: 1,
                    api_version: xr::Version::new(1, 0, 0),
                },
                &enabled,
                &[],
                &platform,
            )
            .context("xrCreateInstance")?;
        let props = instance.properties().context("xrGetInstanceProperties")?;
        info!("runtime: {} {}", props.runtime_name, props.runtime_version);

        let system = instance
            .system(xr::FormFactor::HEAD_MOUNTED_DISPLAY)
            .context("xrGetSystem(HEAD_MOUNTED_DISPLAY)")?;
        let sys_props = instance
            .system_properties(system)
            .context("xrGetSystemProperties")?;
        info!(
            "system: {} (vendor {:#x}) · max swapchain {}x{} · max layers {} · orientation {} position {}",
            sys_props.system_name,
            sys_props.vendor_id,
            sys_props.graphics_properties.max_swapchain_image_width,
            sys_props.graphics_properties.max_swapchain_image_height,
            sys_props.graphics_properties.max_layer_count,
            sys_props.tracking_properties.orientation_tracking,
            sys_props.tracking_properties.position_tracking,
        );

        let blend_modes = instance
            .enumerate_environment_blend_modes(system, VIEW_TYPE)
            .context("xrEnumerateEnvironmentBlendModes")?;
        info!("blend modes: {blend_modes:?}");
        let blend_mode = if blend_modes.contains(&xr::EnvironmentBlendMode::OPAQUE) {
            xr::EnvironmentBlendMode::OPAQUE
        } else {
            *blend_modes
                .first()
                .ok_or_else(|| anyhow!("no environment blend modes"))?
        };

        let views = instance
            .enumerate_view_configuration_views(system, VIEW_TYPE)
            .context("xrEnumerateViewConfigurationViews")?;
        if views.len() != EYE_COUNT {
            bail!(
                "expected {EYE_COUNT} views, runtime reports {}",
                views.len()
            );
        }
        for (i, v) in views.iter().enumerate() {
            info!(
                "view {i}: recommended {}x{} (max {}x{}) samples {} (max {})",
                v.recommended_image_rect_width,
                v.recommended_image_rect_height,
                v.max_image_rect_width,
                v.max_image_rect_height,
                v.recommended_swapchain_sample_count,
                v.max_swapchain_sample_count,
            );
        }

        let reqs = instance
            .graphics_requirements::<xr::Vulkan>(system)
            .context("xrGetVulkanGraphicsRequirements2KHR")?;
        info!(
            "vulkan requirements: {} .. {}",
            reqs.min_api_version_supported, reqs.max_api_version_supported
        );
        let wanted = xr::Version::new(1, 2, 0);
        if wanted < reqs.min_api_version_supported
            || wanted.major() > reqs.max_api_version_supported.major()
        {
            bail!(
                "runtime wants Vulkan {} .. {}, we target 1.2",
                reqs.min_api_version_supported,
                reqs.max_api_version_supported
            );
        }

        Ok(Self {
            instance,
            system,
            blend_mode,
            views,
            has_refresh_rate_ext: enabled.fb_display_refresh_rate,
        })
    }
}

/// What the main loop should do after draining OpenXR events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    Continue,
    Exit,
}

struct Eye {
    /// wgpu wrappers first: they must go before the swapchain that owns the
    /// images (field drop order).
    images: Vec<(wgpu::Texture, wgpu::TextureView)>,
    swapchain: xr::Swapchain<xr::Vulkan>,
    extent: vk::Extent2D,
}

/// Session-level state and the frame loop.
///
/// Field order matters: fields drop in declaration order, and the swapchains
/// and space must be destroyed before the session they belong to.
pub struct XrSession {
    eyes: Vec<Eye>,
    space: xr::Space,
    stream: xr::FrameStream<xr::Vulkan>,
    waiter: xr::FrameWaiter,
    session: xr::Session<xr::Vulkan>,
    blend_mode: xr::EnvironmentBlendMode,
    state: xr::SessionState,
    running: bool,
    events: xr::EventDataBuffer,
    has_refresh_rate_ext: bool,
    /// Display rate to request when the session becomes ready (S5 sweep).
    wanted_hz: Option<f32>,
}

impl XrSession {
    /// `eye_scale` scales the runtime's recommended per-eye swapchain size
    /// (1.0 = recommended); the compositor resamples to the display.
    pub fn new(ctx: &XrContext, gfx: &mut Gfx, eye_scale: f32) -> Result<Self> {
        // SAFETY: every handle comes from `Gfx`, which created the instance and
        // device through XR_KHR_vulkan_enable2 for this system and keeps them
        // alive longer than the session (drop order in `app::run_inner`). The
        // queue is the one wgpu submits to.
        let (session, waiter, stream) = unsafe {
            ctx.instance.create_session::<xr::Vulkan>(
                ctx.system,
                &xr::vulkan::SessionCreateInfo {
                    instance: gfx.raw.instance.as_raw() as _,
                    physical_device: gfx.raw.physical_device.as_raw() as _,
                    device: gfx.raw.device.as_raw() as _,
                    queue_family_index: gfx.raw.queue_family_index,
                    queue_index: 0,
                },
            )
        }
        .context("xrCreateSession")?;

        let spaces = session
            .enumerate_reference_spaces()
            .context("xrEnumerateReferenceSpaces")?;
        info!("reference spaces: {spaces:?}");
        let space_type = if spaces.contains(&xr::ReferenceSpaceType::STAGE) {
            xr::ReferenceSpaceType::STAGE
        } else {
            xr::ReferenceSpaceType::LOCAL
        };
        let space = session
            .create_reference_space(space_type, xr::Posef::IDENTITY)
            .context("xrCreateReferenceSpace")?;
        info!("reference space: {space_type:?}");

        let formats = session
            .enumerate_swapchain_formats()
            .context("xrEnumerateSwapchainFormats")?;
        let format_names: Vec<String> = formats
            .iter()
            .map(|f| format!("{:?}", vk::Format::from_raw(*f as i32)))
            .collect();
        info!("swapchain formats: {}", format_names.join(" "));
        let format = gfx.swapchain_vk_format;
        if !formats.contains(&(format.as_raw() as u32)) {
            bail!("runtime does not offer swapchain format {format:?}");
        }

        // One 2D swapchain per eye rather than a two-layer array: each image
        // is wrapped as a plain wgpu 2D texture, and the compute raster in S5
        // runs once per eye anyway.
        let mut eyes = Vec::with_capacity(EYE_COUNT);
        for (i, view) in ctx.views.iter().enumerate() {
            let scale = |v: u32| {
                (((v as f32) * eye_scale).round() as u32).clamp(
                    64,
                    view.max_image_rect_width.max(view.max_image_rect_height),
                )
            };
            let extent = vk::Extent2D {
                width: scale(view.recommended_image_rect_width),
                height: scale(view.recommended_image_rect_height),
            };
            let swapchain = session
                .create_swapchain(&xr::SwapchainCreateInfo {
                    create_flags: xr::SwapchainCreateFlags::EMPTY,
                    usage_flags: xr::SwapchainUsageFlags::COLOR_ATTACHMENT,
                    format: format.as_raw() as u32,
                    sample_count: 1,
                    width: extent.width,
                    height: extent.height,
                    face_count: 1,
                    array_size: 1,
                    mip_count: 1,
                })
                .with_context(|| format!("xrCreateSwapchain (eye {i})"))?;
            let images = swapchain
                .enumerate_images()
                .context("xrEnumerateSwapchainImages")?
                .into_iter()
                .map(|raw| {
                    gfx.wrap_swapchain_image(vk::Image::from_raw(raw), extent.width, extent.height)
                })
                .collect::<Result<Vec<_>>>()?;
            info!(
                "eye {i}: swapchain {}x{} {format:?} (scale {eye_scale}), {} images wrapped as wgpu textures",
                extent.width,
                extent.height,
                images.len()
            );
            gfx.set_eye_extent(i, extent.width, extent.height);
            eyes.push(Eye {
                images,
                swapchain,
                extent,
            });
        }

        Ok(Self {
            eyes,
            space,
            stream,
            waiter,
            session,
            blend_mode: ctx.blend_mode,
            state: xr::SessionState::UNKNOWN,
            running: false,
            events: xr::EventDataBuffer::new(),
            has_refresh_rate_ext: ctx.has_refresh_rate_ext,
            wanted_hz: None,
        })
    }

    pub fn is_running(&self) -> bool {
        self.running
    }

    /// Ask the runtime for this display rate once the session runs
    /// (`XR_FB_display_refresh_rate`; the runtime may refuse or change it
    /// later, which `DisplayRefreshRateChangedFB` logs).
    pub fn request_refresh_rate(&mut self, hz: f32) {
        self.wanted_hz = Some(hz);
    }

    /// Drain the OpenXR event queue and drive the session state machine:
    /// READY begins the session, STOPPING ends it, EXITING / LOSS_PENDING and
    /// instance loss leave the main loop. Android pause and resume arrive here
    /// as STOPPING and READY, so the activity lifecycle needs no separate path.
    pub fn poll_events(&mut self) -> Result<Flow> {
        while let Some(event) = self
            .session
            .instance()
            .poll_event(&mut self.events)
            .context("xrPollEvent")?
        {
            match event {
                xr::Event::SessionStateChanged(e) => {
                    let state = e.state();
                    info!("session state {:?} -> {state:?}", self.state);
                    self.state = state;
                    match state {
                        xr::SessionState::READY => {
                            self.session.begin(VIEW_TYPE).context("xrBeginSession")?;
                            self.running = true;
                            self.log_refresh_rate();
                            if let Some(hz) = self.wanted_hz {
                                if self.has_refresh_rate_ext {
                                    match self.session.request_display_refresh_rate(hz) {
                                        Ok(()) => info!("requested display refresh rate {hz} Hz"),
                                        Err(e) => warn!("xrRequestDisplayRefreshRateFB({hz}): {e}"),
                                    }
                                } else {
                                    warn!(
                                        "cannot request {hz} Hz: XR_FB_display_refresh_rate missing"
                                    );
                                }
                            }
                        }
                        xr::SessionState::STOPPING => {
                            self.session.end().context("xrEndSession")?;
                            self.running = false;
                        }
                        xr::SessionState::EXITING | xr::SessionState::LOSS_PENDING => {
                            info!("session {state:?}: leaving the main loop");
                            return Ok(Flow::Exit);
                        }
                        _ => {}
                    }
                }
                xr::Event::InstanceLossPending(_) => {
                    warn!("instance loss pending: leaving the main loop");
                    return Ok(Flow::Exit);
                }
                xr::Event::EventsLost(e) => warn!("lost {} events", e.lost_event_count()),
                xr::Event::DisplayRefreshRateChangedFB(e) => {
                    info!(
                        "display refresh rate {} -> {} Hz",
                        e.from_display_refresh_rate(),
                        e.to_display_refresh_rate()
                    );
                }
                _ => {}
            }
        }
        Ok(Flow::Continue)
    }

    fn log_refresh_rate(&self) {
        if !self.has_refresh_rate_ext {
            return;
        }
        match (
            self.session.enumerate_display_refresh_rates(),
            self.session.get_display_refresh_rate(),
        ) {
            (Ok(rates), Ok(current)) => {
                info!("display refresh rate {current} Hz (available {rates:?})");
            }
            (r, c) => warn!("XR_FB_display_refresh_rate query failed: {r:?} {c:?}"),
        }
    }

    /// One frame: wait, begin, run `before_render` (the effect step), locate
    /// views, render both eyes through wgpu, submit a projection layer. Must only be called while the session is
    /// running.
    pub fn frame(
        &mut self,
        gfx: &Gfx,
        clear: [f32; 4],
        stats: &mut FrameStats,
        particles: Option<&Particles3d>,
        before_render: impl FnOnce(),
    ) -> Result<()> {
        let frame_state = self.waiter.wait().context("xrWaitFrame")?;
        self.stream.begin().context("xrBeginFrame")?;
        let period = std::time::Duration::from_nanos(
            frame_state.predicted_display_period.as_nanos().max(0) as u64,
        );
        stats.record(period, frame_state.should_render);
        let cpu_start = std::time::Instant::now();

        if !frame_state.should_render
            || self.state != xr::SessionState::VISIBLE && self.state != xr::SessionState::FOCUSED
        {
            // Nothing to composite (e.g. the system overlay is in front).
            self.stream
                .end(frame_state.predicted_display_time, self.blend_mode, &[])
                .context("xrEndFrame (empty)")?;
            return Ok(());
        }
        before_render();

        let (view_flags, views) = self
            .session
            .locate_views(VIEW_TYPE, frame_state.predicted_display_time, &self.space)
            .context("xrLocateViews")?;
        let cameras: Vec<EyeCamera> = views.iter().map(camera).collect();
        log_stereo(view_flags, &views, &cameras);

        let mut image_indices = [0u32; EYE_COUNT];
        for (eye, index) in self.eyes.iter_mut().zip(image_indices.iter_mut()) {
            *index = eye
                .swapchain
                .acquire_image()
                .context("xrAcquireSwapchainImage")?;
            eye.swapchain
                .wait_image(xr::Duration::INFINITE)
                .context("xrWaitSwapchainImage")?;
        }

        let targets: Vec<&wgpu::TextureView> = self
            .eyes
            .iter()
            .zip(image_indices.iter())
            .map(|(eye, &index)| &eye.images[index as usize].1)
            .collect();
        gfx.render(&targets, &cameras, clear, particles);

        for eye in &mut self.eyes {
            eye.swapchain
                .release_image()
                .context("xrReleaseSwapchainImage")?;
        }

        let projection_views: Vec<xr::CompositionLayerProjectionView<'_, xr::Vulkan>> = self
            .eyes
            .iter()
            .zip(views.iter())
            .map(|(eye, view)| {
                xr::CompositionLayerProjectionView::new()
                    .pose(view.pose)
                    .fov(view.fov)
                    .sub_image(
                        xr::SwapchainSubImage::new()
                            .swapchain(&eye.swapchain)
                            .image_array_index(0)
                            .image_rect(xr::Rect2Di {
                                offset: xr::Offset2Di { x: 0, y: 0 },
                                extent: xr::Extent2Di {
                                    width: eye.extent.width as i32,
                                    height: eye.extent.height as i32,
                                },
                            }),
                    )
            })
            .collect();
        let layer = xr::CompositionLayerProjection::new()
            .space(&self.space)
            .views(&projection_views);
        stats.record_cpu(cpu_start.elapsed());
        self.stream
            .end(
                frame_state.predicted_display_time,
                self.blend_mode,
                &[&layer],
            )
            .context("xrEndFrame")?;
        Ok(())
    }
}

fn camera(view: &xr::View) -> EyeCamera {
    let q = view.pose.orientation;
    let p = view.pose.position;
    let (view, proj) = math::view_and_projection(
        [q.x, q.y, q.z, q.w],
        [p.x, p.y, p.z],
        math::Fov {
            left: view.fov.angle_left,
            right: view.fov.angle_right,
            up: view.fov.angle_up,
            down: view.fov.angle_down,
        },
        NEAR,
        FAR,
    );
    EyeCamera { view, proj }
}

/// Center of the world-locked quad (`app::QUAD_CENTER`), for the stereo log.
const STEREO_TARGET: glam::Vec3 = glam::Vec3::new(0.0, 1.5, -1.5);

/// Numeric convergence check, every 5 s: the horizontal angle at which each
/// eye sees the quad center, against the angle IPD/distance predicts.
/// Stereo correctness can't be judged by a one-eyed wearer, so this is the
/// evidence for it (board #3221).
fn log_stereo(flags: xr::ViewStateFlags, views: &[xr::View], cameras: &[EyeCamera]) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static FRAME: AtomicU64 = AtomicU64::new(0);
    if !FRAME.fetch_add(1, Ordering::Relaxed).is_multiple_of(360) || views.len() != 2 {
        return;
    }
    let view_projs: Vec<glam::Mat4> = cameras.iter().map(EyeCamera::view_proj).collect();
    let pos =
        |v: &xr::View| glam::Vec3::new(v.pose.position.x, v.pose.position.y, v.pose.position.z);
    let (p0, p1) = (pos(&views[0]), pos(&views[1]));
    let ipd = p0.distance(p1);
    let mid = (p0 + p1) * 0.5;
    let dist = STEREO_TARGET.distance(mid);
    let expected = (ipd / dist).atan().to_degrees();
    // Horizontal view angle of the target in each eye, from its NDC x and
    // that eye's asymmetric fov.
    let azimuth = |i: usize| {
        let c = view_projs[i] * STEREO_TARGET.extend(1.0);
        let x = c.x / c.w;
        let l = views[i].fov.angle_left.tan();
        let r = views[i].fov.angle_right.tan();
        (x, (l + (x + 1.0) * 0.5 * (r - l)).atan().to_degrees())
    };
    let (x0, az0) = azimuth(0);
    let (x1, az1) = azimuth(1);
    info!(
        "stereo: flags {flags:?} · ipd {:.1} mm · target {:.2} m from eye midpoint · expected disparity {expected:.2}° · eye0 ndc.x {x0:.3} az {az0:.2}° · eye1 ndc.x {x1:.3} az {az1:.2}° · measured disparity {:.2}° · eye0 pos ({:.3},{:.3},{:.3}) · eye1 pos ({:.3},{:.3},{:.3}) · fov0 deg [{:.1} {:.1} {:.1} {:.1}]",
        ipd * 1000.0,
        dist,
        az0 - az1,
        p0.x,
        p0.y,
        p0.z,
        p1.x,
        p1.y,
        p1.z,
        views[0].fov.angle_left.to_degrees(),
        views[0].fov.angle_right.to_degrees(),
        views[0].fov.angle_up.to_degrees(),
        views[0].fov.angle_down.to_degrees(),
    );
    log_depth_probes(&view_projs, expected);
}

/// S5 depth evidence: two probe points on the quad's line of sight, half a
/// meter nearer and farther than the quad center. Correct stereo depth means
/// the nearer probe has the larger disparity and the smaller depth-buffer
/// value in both eyes, so a particle there passes the quad's depth test and
/// one behind fails it.
fn log_depth_probes(view_projs: &[glam::Mat4], quad_disparity: f32) {
    let probe = |name: &str, dz: f32| {
        let p = STEREO_TARGET + glam::Vec3::new(0.0, 0.0, dz);
        let ndc = |i: usize| {
            let c = view_projs[i] * p.extend(1.0);
            (c.x / c.w, c.z / c.w)
        };
        let (x0, z0) = ndc(0);
        let (x1, z1) = ndc(1);
        let quad_z = |i: usize| {
            let c = view_projs[i] * STEREO_TARGET.extend(1.0);
            c.z / c.w
        };
        info!(
            "depth probe {name} (quad z {dz:+.2} m): eye0 ndc.x {x0:.3} depth {z0:.5} (quad {:.5}) · eye1 ndc.x {x1:.3} depth {z1:.5} (quad {:.5}) · ndc.x disparity {:.3} (quad {:.3}) · quad disparity {quad_disparity:.2}°",
            quad_z(0),
            quad_z(1),
            x0 - x1,
            {
                let q0 = view_projs[0] * STEREO_TARGET.extend(1.0);
                let q1 = view_projs[1] * STEREO_TARGET.extend(1.0);
                q0.x / q0.w - q1.x / q1.w
            }
        );
    };
    probe("near", 0.5);
    probe("far", -0.5);
}
