//! OpenXR bring-up on Android: loader, instance, system, session lifecycle,
//! per-eye swapchains and the frame loop ("OpenXR bring-up" in
//! `docs/xr/XR_DESIGN.md`).

use android_activity::AndroidApp;
use anyhow::{Context, Result, anyhow, bail};
use ash::vk::{self, Handle as _};
use log::{info, warn};
use openxr as xr;

use crate::app::FrameStats;
use crate::gfx::Gfx;

pub const VIEW_TYPE: xr::ViewConfigurationType = xr::ViewConfigurationType::PRIMARY_STEREO;
pub const EYE_COUNT: usize = 2;

/// Swapchain formats in order of preference. sRGB matches the desktop surface
/// choice (`gpu/context.rs`); the runtime then handles the transfer function.
const PREFERRED_FORMATS: [vk::Format; 3] = [
    vk::Format::R8G8B8A8_SRGB,
    vk::Format::B8G8R8A8_SRGB,
    vk::Format::R8G8B8A8_UNORM,
];

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
        let wanted = xr::Version::new(1, 1, 0);
        if wanted < reqs.min_api_version_supported
            || wanted.major() > reqs.max_api_version_supported.major()
        {
            bail!(
                "runtime wants Vulkan {} .. {}, we target 1.1",
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
    swapchain: xr::Swapchain<xr::Vulkan>,
    images: Vec<vk::Image>,
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
    frame_slot: usize,
    has_refresh_rate_ext: bool,
}

impl XrSession {
    pub fn new(ctx: &XrContext, gfx: &Gfx) -> Result<Self> {
        // SAFETY: every handle comes from `Gfx`, which created the instance and
        // device through XR_KHR_vulkan_enable2 for this system and keeps them
        // alive longer than the session (drop order in `app::run_inner`). The
        // queue is the one `Gfx` submits to.
        let (session, waiter, stream) = unsafe {
            ctx.instance.create_session::<xr::Vulkan>(
                ctx.system,
                &xr::vulkan::SessionCreateInfo {
                    instance: gfx.instance.handle().as_raw() as _,
                    physical_device: gfx.physical_device.as_raw() as _,
                    device: gfx.device.handle().as_raw() as _,
                    queue_family_index: gfx.queue_family_index,
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
        let format = PREFERRED_FORMATS
            .iter()
            .copied()
            .find(|f| formats.contains(&(f.as_raw() as u32)))
            .ok_or_else(|| anyhow!("no preferred swapchain format offered"))?;

        // One 2D swapchain per eye rather than a two-layer array: S2 wraps each
        // image as a plain wgpu 2D texture, and the compute raster in S5 runs
        // once per eye anyway.
        let mut eyes = Vec::with_capacity(EYE_COUNT);
        for (i, view) in ctx.views.iter().enumerate() {
            let extent = vk::Extent2D {
                width: view.recommended_image_rect_width,
                height: view.recommended_image_rect_height,
            };
            let swapchain = session
                .create_swapchain(&xr::SwapchainCreateInfo {
                    create_flags: xr::SwapchainCreateFlags::EMPTY,
                    // TRANSFER_DST for S1's vkCmdClearColorImage; S2 renders.
                    usage_flags: xr::SwapchainUsageFlags::COLOR_ATTACHMENT
                        | xr::SwapchainUsageFlags::TRANSFER_DST,
                    format: format.as_raw() as u32,
                    sample_count: 1,
                    width: extent.width,
                    height: extent.height,
                    face_count: 1,
                    array_size: 1,
                    mip_count: 1,
                })
                .with_context(|| format!("xrCreateSwapchain (eye {i})"))?;
            let images: Vec<vk::Image> = swapchain
                .enumerate_images()
                .context("xrEnumerateSwapchainImages")?
                .into_iter()
                .map(vk::Image::from_raw)
                .collect();
            info!(
                "eye {i}: swapchain {}x{} {format:?}, {} images",
                extent.width,
                extent.height,
                images.len()
            );
            eyes.push(Eye {
                swapchain,
                images,
                extent,
            });
        }

        Ok(Self {
            session,
            waiter,
            stream,
            space,
            eyes,
            blend_mode: ctx.blend_mode,
            state: xr::SessionState::UNKNOWN,
            running: false,
            events: xr::EventDataBuffer::new(),
            frame_slot: 0,
            has_refresh_rate_ext: ctx.has_refresh_rate_ext,
        })
    }

    pub fn is_running(&self) -> bool {
        self.running
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
                            self.frame_slot = 0;
                            self.log_refresh_rate();
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

    /// One frame: wait, begin, locate views, clear both eyes, submit a
    /// projection layer. Must only be called while the session is running.
    pub fn frame(&mut self, gfx: &Gfx, color: [f32; 4], stats: &mut FrameStats) -> Result<()> {
        let frame_state = self.waiter.wait().context("xrWaitFrame")?;
        self.stream.begin().context("xrBeginFrame")?;
        let period = std::time::Duration::from_nanos(
            frame_state.predicted_display_period.as_nanos().max(0) as u64,
        );
        stats.record(period, frame_state.should_render);

        if !frame_state.should_render
            || self.state != xr::SessionState::VISIBLE && self.state != xr::SessionState::FOCUSED
        {
            // Nothing to composite (e.g. the system overlay is in front).
            self.stream
                .end(frame_state.predicted_display_time, self.blend_mode, &[])
                .context("xrEndFrame (empty)")?;
            return Ok(());
        }

        let (_flags, views) = self
            .session
            .locate_views(VIEW_TYPE, frame_state.predicted_display_time, &self.space)
            .context("xrLocateViews")?;

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

        let cmd = gfx.begin_commands(self.frame_slot)?;
        for (eye, &index) in self.eyes.iter().zip(image_indices.iter()) {
            gfx.record_clear(cmd, eye.images[index as usize], color);
        }
        gfx.submit(cmd, self.frame_slot)?;
        self.frame_slot = (self.frame_slot + 1) % crate::gfx::FRAMES_IN_FLIGHT;

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
