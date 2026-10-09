//! OpenXR bring-up on Android: loader, instance, system, session lifecycle,
//! per-eye swapchains and the frame loop ("OpenXR bring-up" in
//! `docs/xr/XR_DESIGN.md`).

use android_activity::AndroidApp;
use anyhow::{Context, Result, anyhow, bail};
use ash::vk::{self, Handle as _};
use log::{info, warn};
use openxr as xr;
use xr::sys::Handle as _;

use crate::app::FrameStats;
use crate::env_depth::{EnvDepthOptions, EnvDepthPasses, EnvDepthSlot};
use crate::faces_layer::{FacesOptions, Layer};
use crate::gfx::{EyeCamera, Gfx};
use crate::input::{Hands, HandsFrame};
use crate::math;
use crate::microgestures::{MicroFrame, Microgestures};
use crate::particles3d::{HandMeshData, ObstacleBox, Particles3d};
use crate::perf::{PerfMetrics, PerfSample};
use crate::room::{Passthrough, Room};
use crate::scene::XrScene;

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
    /// S7 extensions the runtime offered and the instance enabled.
    pub has_passthrough: bool,
    pub has_hand_tracking: bool,
    /// `XR_FB_hand_tracking_aim` is enabled: the aim state rides on the
    /// joint locate.
    pub has_hand_aim: bool,
    /// `XR_FB_scene` + `XR_FB_spatial_entity` + `XR_FB_spatial_entity_query`.
    pub has_scene: bool,
    /// `XR_META_environment_depth` (board #3324).
    pub has_env_depth: bool,
    /// `XR_META_hand_tracking_microgestures` together with the
    /// `XR_EXT_hand_interaction` profile its input paths extend (board
    /// #3336).
    pub has_microgestures: bool,
    /// `XR_FB_composition_layer_settings`: the faces layer's sharpening
    /// (board #3793).
    pub has_layer_settings: bool,
}

/// Which S7 features to bring up with the session.
#[derive(Debug, Clone, Copy, Default)]
pub struct MrOptions {
    pub passthrough: bool,
    pub hands: bool,
    /// Chain `XR_FB_hand_tracking_aim`'s state onto the joint locate
    /// (`debug.fosfora.aim`, board #3336).
    pub aim: bool,
    /// Lead the hand joints along the runtime's velocities by this many
    /// seconds (`debug.fosfora.handlead` ms, board #3753); 0 is off.
    pub hand_lead_s: f32,
    pub room: bool,
    /// Launch Space Setup when the room query finds no anchors.
    pub scene_capture: bool,
    /// Rescan (or only requery) the room once the first query has
    /// returned anchors (`debug.fosfora.rescan 1|query`).
    pub rescan_at_start: crate::room::Rescan,
    /// Replay the saved room when the query finds no anchors
    /// (`debug.fosfora.anchors replay`, board #3536).
    pub anchor_replay: bool,
    /// The live environment depth as an occluder or its diagnostic
    /// (`debug.fosfora.envdepth*`); `None` creates no provider at all.
    pub env_depth: Option<EnvDepthOptions>,
    /// Thumb microgestures through the app's action set
    /// (`debug.fosfora.micro`, board #3336); off, no action set exists.
    pub micro: bool,
    /// The surfaces in their own composition layer
    /// (`debug.fosfora.faceslayer`, `facescale`, `facesharpen`; board
    /// #3793); created only over passthrough.
    pub faces: FacesOptions,
}

/// Per-frame input the frame loop hands to `before_render`, by value.
#[derive(Debug, Clone, Default)]
pub struct FrameInput {
    /// Midpoint of the two eyes in the reference space this frame.
    pub head: [f32; 3],
    /// Orientation of the first view (x, y, z, w) in the reference space.
    pub head_rot: [f32; 4],
    pub hands: HandsFrame,
    /// This frame's thumb microgestures, rising edges per hand (empty
    /// without the action set or while the session is not focused).
    pub micro: MicroFrame,
    /// Scene anchors as oriented boxes in the reference space (empty until
    /// the query returns, or when the room is off).
    pub room_boxes: Vec<ObstacleBox>,
    /// Each room box's semantic labels, in `room_boxes` order (for the
    /// log).
    pub room_labels: std::sync::Arc<[String]>,
    /// The room id of the anchor set (`room_file::room_id`); `None`
    /// without anchors.
    pub room_id: Option<u64>,
    /// The runtime's frame timing (`XR_META_performance_metrics`), latest.
    pub perf: PerfSample,
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
        // The debug panel's frame timing (public extension, no NDA).
        enabled.meta_performance_metrics = available.meta_performance_metrics;
        // S7: passthrough, hands, scene. Each behind a runtime check; the
        // scene trio is enabled only when all three are offered (the query
        // needs them together).
        enabled.fb_passthrough = available.fb_passthrough;
        enabled.ext_hand_tracking = available.ext_hand_tracking;
        // The runtime's skinned hand mesh as the depth occluder (needs the
        // hand tracker it is queried through).
        enabled.fb_hand_tracking_mesh =
            available.ext_hand_tracking && available.fb_hand_tracking_mesh;
        // The runtime's own pinch and system-gesture flags, chained onto the
        // joint locate (board #3336, `input.rs`).
        enabled.fb_hand_tracking_aim =
            available.ext_hand_tracking && available.fb_hand_tracking_aim;
        let has_scene =
            available.fb_scene && available.fb_spatial_entity && available.fb_spatial_entity_query;
        enabled.fb_scene = has_scene;
        enabled.fb_spatial_entity = has_scene;
        enabled.fb_spatial_entity_query = has_scene;
        enabled.fb_spatial_entity_container = has_scene && available.fb_spatial_entity_container;
        enabled.meta_spatial_entity_mesh = has_scene && available.meta_spatial_entity_mesh;
        enabled.fb_spatial_entity_storage = has_scene && available.fb_spatial_entity_storage;
        enabled.fb_scene_capture = has_scene && available.fb_scene_capture;
        // The live depth map from the passthrough cameras (board #3324).
        enabled.meta_environment_depth = available.meta_environment_depth;
        // Board #3336: thumb microgestures. Their input paths extend the
        // EXT hand interaction profile, so the pair is enabled together or
        // not at all.
        let has_microgestures =
            available.ext_hand_interaction && available.meta_hand_tracking_microgestures;
        enabled.ext_hand_interaction = has_microgestures;
        enabled.meta_hand_tracking_microgestures = has_microgestures;
        // Board #3793: the faces layer's upsample sharpening, asked for
        // per layer only by `debug.fosfora.facesharpen`.
        enabled.fb_composition_layer_settings = available.fb_composition_layer_settings;
        info!(
            "hand aim (XR_FB_hand_tracking_aim): {}",
            enabled.fb_hand_tracking_aim
        );
        info!(
            "S7 extensions: passthrough {} · hand tracking {} (mesh {}) · scene {} (XR_FB_scene {}, XR_FB_spatial_entity {}, XR_FB_spatial_entity_query {}, container {}, mesh {}) · plane tracking EXT {} (base XR_EXT_spatial_entity {}) · room mesh META {} · environment depth {}",
            available.fb_passthrough,
            available.ext_hand_tracking,
            available.fb_hand_tracking_mesh,
            has_scene,
            available.fb_scene,
            available.fb_spatial_entity,
            available.fb_spatial_entity_query,
            available.fb_spatial_entity_container,
            available.meta_spatial_entity_mesh,
            available.ext_spatial_plane_tracking,
            available.ext_spatial_entity,
            names
                .iter()
                .any(|n| n == "XR_META_spatial_entity_room_mesh"),
            available.meta_environment_depth,
        );
        info!(
            "microgestures (XR_META_hand_tracking_microgestures on XR_EXT_hand_interaction): {has_microgestures}"
        );
        if !has_microgestures {
            info!(
                "microgestures off: XR_EXT_hand_interaction {} · XR_META_hand_tracking_microgestures {}",
                available.ext_hand_interaction, available.meta_hand_tracking_microgestures
            );
        }

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
            has_passthrough: enabled.fb_passthrough,
            has_hand_tracking: enabled.ext_hand_tracking,
            has_hand_aim: enabled.fb_hand_tracking_aim,
            has_scene,
            has_env_depth: enabled.meta_environment_depth,
            has_microgestures,
            has_layer_settings: enabled.fb_composition_layer_settings,
        })
    }
}

/// What the main loop should do after draining OpenXR events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    Continue,
    Exit,
}

/// One eye's swapchain, its images wrapped for wgpu: the eye's own, or
/// its faces layer's (board #3793).
struct Eye {
    /// wgpu wrappers first: they must go before the swapchain that owns the
    /// images (field drop order).
    images: Vec<(wgpu::Texture, wgpu::TextureView)>,
    swapchain: xr::Swapchain<xr::Vulkan>,
    extent: vk::Extent2D,
}

impl Eye {
    /// A 2D color swapchain of `extent` in `format`, its images
    /// enumerated and wrapped as wgpu textures.
    fn create(
        session: &xr::Session<xr::Vulkan>,
        gfx: &Gfx,
        format: vk::Format,
        extent: vk::Extent2D,
        what: &str,
    ) -> Result<Self> {
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
            .with_context(|| format!("xrCreateSwapchain ({what})"))?;
        let images = swapchain
            .enumerate_images()
            .with_context(|| format!("xrEnumerateSwapchainImages ({what})"))?
            .into_iter()
            .map(|raw| {
                gfx.wrap_swapchain_image(vk::Image::from_raw(raw), extent.width, extent.height)
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            images,
            swapchain,
            extent,
        })
    }

    fn size(&self) -> [u32; 2] {
        [self.extent.width, self.extent.height]
    }

    /// The projection view showing this swapchain whole for `view`.
    fn projection_view(
        &self,
        view: &xr::View,
    ) -> xr::CompositionLayerProjectionView<'_, xr::Vulkan> {
        xr::CompositionLayerProjectionView::new()
            .pose(view.pose)
            .fov(view.fov)
            .sub_image(
                xr::SwapchainSubImage::new()
                    .swapchain(&self.swapchain)
                    .image_array_index(0)
                    .image_rect(xr::Rect2Di {
                        offset: xr::Offset2Di { x: 0, y: 0 },
                        extent: xr::Extent2Di {
                            width: self.extent.width as i32,
                            height: self.extent.height as i32,
                        },
                    }),
            )
    }
}

/// Session-level state and the frame loop.
///
/// Field order matters: fields drop in declaration order, and the swapchains
/// and space must be destroyed before the session they belong to.
pub struct XrSession {
    // S7 objects first: they hold session-owned handles (depth provider,
    // passthrough layer, hand trackers, anchor spaces) and must go before
    // the session.
    env_depth: Option<EnvDepthSlot>,
    passthrough: Option<Passthrough>,
    hands: Option<Hands>,
    room: Option<Room>,
    perf: Option<PerfMetrics>,
    /// The app's action set (board #3336); its actions belong to the
    /// instance, not the session, so its place here is only for tidiness.
    micro: Option<Microgestures>,
    /// `xrSyncActions` failures so far (logged at powers of two).
    micro_errors: u32,
    eyes: Vec<Eye>,
    /// The faces layer's swapchains, one per eye (board #3793); empty
    /// when it is off, and then the surfaces draw in the eye pass.
    faces: Vec<Eye>,
    /// Chain `NORMAL_SHARPENING` on the faces layer
    /// (`debug.fosfora.facesharpen`, with the extension enabled).
    faces_sharpen: bool,
    space: xr::Space,
    stream: xr::FrameStream<xr::Vulkan>,
    waiter: xr::FrameWaiter,
    session: xr::Session<xr::Vulkan>,
    blend_mode: xr::EnvironmentBlendMode,
    state: xr::SessionState,
    running: bool,
    events: xr::EventDataBuffer,
    has_refresh_rate_ext: bool,
    /// For the environment depth's creation retries.
    system: xr::SystemId,
    /// Display rate to request when the session becomes ready (S5 sweep).
    wanted_hz: Option<f32>,
}

impl XrSession {
    /// `eye_scale` scales the runtime's recommended per-eye swapchain size
    /// (1.0 = recommended); the compositor resamples to the display.
    /// `config` is the app's config dir (the room's saved anchors).
    pub fn new(
        ctx: &XrContext,
        gfx: &mut Gfx,
        eye_scale: f32,
        mr: MrOptions,
        config: &std::path::Path,
    ) -> Result<Self> {
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
            let recommended = [
                view.recommended_image_rect_width,
                view.recommended_image_rect_height,
            ];
            let max = view.max_image_rect_width.max(view.max_image_rect_height);
            let [width, height] = crate::faces_layer::scaled_extent(recommended, eye_scale, max);
            let extent = vk::Extent2D { width, height };
            let eye = Eye::create(&session, gfx, format, extent, &format!("eye {i}"))?;
            info!(
                "eye {i}: swapchain {}x{} {format:?} (scale {eye_scale}), {} images wrapped as wgpu textures",
                extent.width,
                extent.height,
                eye.images.len()
            );
            gfx.set_eye_extent(i, extent.width, extent.height);
            eyes.push(eye);
        }

        // S7 bring-up. A missing extension is logged, not fatal: the gate
        // records what the runtime offers.
        let passthrough = if mr.passthrough {
            if ctx.has_passthrough {
                Some(Passthrough::new(&session)?)
            } else {
                warn!("passthrough requested but XR_FB_passthrough is missing");
                None
            }
        } else {
            None
        };
        // Board #3793: the surfaces' own layer, a second per-eye swapchain
        // at a fraction of the recommended size, only over passthrough.
        let faces_on = crate::faces_layer::faces_layer_on(passthrough.is_some(), mr.faces.layer);
        let mut faces = Vec::new();
        if faces_on {
            for (i, view) in ctx.views.iter().enumerate() {
                let recommended = [
                    view.recommended_image_rect_width,
                    view.recommended_image_rect_height,
                ];
                let max = view.max_image_rect_width.max(view.max_image_rect_height);
                let [width, height] =
                    crate::faces_layer::faces_extent(recommended, mr.faces.scale, max);
                let extent = vk::Extent2D { width, height };
                let eye = Eye::create(&session, gfx, format, extent, &format!("faces {i}"))?;
                gfx.set_faces_extent(i, width, height);
                faces.push(eye);
            }
        }
        let faces_sharpen = faces_on && mr.faces.sharpen && ctx.has_layer_settings;
        match faces.first() {
            Some(f) => info!(
                "faces layer: {}x{} per eye (scale {:.2}) · {} images · sharpen {}",
                f.extent.width,
                f.extent.height,
                crate::faces_layer::face_scale(Some(mr.faces.scale)),
                f.images.len(),
                if faces_sharpen {
                    "on"
                } else if mr.faces.sharpen {
                    "asked, XR_FB_composition_layer_settings missing"
                } else {
                    "off"
                },
            ),
            None => info!(
                "faces layer: off ({}), the surfaces draw in the eye pass",
                if passthrough.is_none() {
                    "no passthrough"
                } else {
                    "debug.fosfora.faceslayer 0"
                }
            ),
        }
        let hands = if mr.hands {
            if ctx.has_hand_tracking {
                Some(Hands::new(
                    &session,
                    ctx.has_hand_aim && mr.aim,
                    mr.hand_lead_s,
                )?)
            } else {
                warn!("hands requested but XR_EXT_hand_tracking is missing");
                None
            }
        } else {
            None
        };
        let room = if mr.room {
            if ctx.has_scene {
                match Room::new(
                    &session,
                    &space,
                    mr.scene_capture,
                    mr.rescan_at_start,
                    config,
                    mr.anchor_replay,
                ) {
                    Ok(r) => Some(r),
                    Err(e) => {
                        warn!("scene query failed to start: {e:#}");
                        None
                    }
                }
            } else {
                warn!("room requested but the XR_FB_scene extensions are missing");
                None
            }
        } else {
            None
        };

        // Board #3324: created only when a knob asks for it, so the
        // baseline run is the app without it.
        let env_depth = match mr.env_depth {
            Some(opts) if ctx.has_env_depth => EnvDepthSlot::new(&session, ctx.system, gfx, opts),
            Some(_) => {
                warn!("environment depth requested but XR_META_environment_depth is missing");
                None
            }
            None => None,
        };

        let perf = match PerfMetrics::new(&session) {
            Ok(p) => p,
            Err(e) => {
                warn!("performance metrics unavailable: {e:#}");
                None
            }
        };

        // Board #3336: thumb microgestures. The action set is attached
        // here, after xrCreateSession and before the READY event that
        // begins the session (`poll_events`), because
        // xrAttachSessionActionSets must precede xrBeginSession and can run
        // only once per session. A refused binding is logged and the run
        // goes on without the feature.
        let micro = match (mr.micro, ctx.has_microgestures) {
            (false, _) => {
                info!("microgestures off (debug.fosfora.micro 0)");
                None
            }
            (true, false) => {
                warn!(
                    "microgestures requested but XR_EXT_hand_interaction + XR_META_hand_tracking_microgestures are missing"
                );
                None
            }
            (true, true) => match Microgestures::new(&ctx.instance, &session) {
                Ok(m) => Some(m),
                Err(e) => {
                    warn!("microgestures off: {e:#}");
                    None
                }
            },
        };

        Ok(Self {
            env_depth,
            system: ctx.system,
            passthrough,
            hands,
            room,
            perf,
            micro,
            micro_errors: 0,
            eyes,
            faces,
            faces_sharpen,
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
                        xr::SessionState::FOCUSED => {
                            if let Some(room) = self.room.as_mut() {
                                room.start();
                            }
                        }
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
                xr::Event::SpaceQueryResultsAvailableFB(e) => {
                    let id = e.request_id();
                    if let Some(room) = self.room.as_mut() {
                        room.results_available(id);
                    }
                }
                xr::Event::SpaceSetStatusCompleteFB(e) => {
                    let (space, component, enabled, result) =
                        (e.space(), e.component_type(), e.enabled(), e.result());
                    if let Some(room) = self.room.as_mut() {
                        room.set_status_complete(space, component, enabled, result);
                    }
                }
                xr::Event::SceneCaptureCompleteFB(e) => {
                    let (id, result) = (e.request_id(), e.result());
                    if let Some(room) = self.room.as_mut() {
                        room.capture_complete(id, result);
                    }
                }
                xr::Event::SpaceQueryCompleteFB(e) => {
                    let (id, result) = (e.request_id(), e.result());
                    if let Some(room) = self.room.as_mut() {
                        room.query_complete(id, result);
                    }
                }
                xr::Event::PassthroughStateChangedFB(e) => {
                    info!("passthrough state changed: {:?}", e.flags());
                }
                xr::Event::InteractionProfileChanged(_) => {
                    if let Some(m) = self.micro.as_ref() {
                        m.log_profiles(self.session.instance(), &self.session);
                    }
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

    pub fn has_passthrough(&self) -> bool {
        self.passthrough.is_some()
    }

    pub fn has_hands(&self) -> bool {
        self.hands.is_some()
    }

    /// Keep the runtime's performance counters only while something reads
    /// them (the debug panel, the thermal governor): turn them on, or off
    /// and stop polling. The hand menu's toggle can flip this during the
    /// session.
    pub fn set_perf_metrics(&mut self, on: bool) {
        if on {
            if self.perf.is_none() {
                self.perf = match PerfMetrics::new(&self.session) {
                    Ok(p) => p,
                    Err(e) => {
                        warn!("performance metrics unavailable: {e:#}");
                        None
                    }
                };
            }
        } else if let Some(p) = self.perf.take() {
            p.disable();
        }
    }

    /// Ask for the hands in or out of the live depth map (board #3352);
    /// see `EnvDepthSlot::set_hand_removal`. Nothing without the depth.
    pub fn set_env_depth_hand_removal(&mut self, on: bool) {
        match self.env_depth.as_mut() {
            Some(slot) => slot.set_hand_removal(on),
            None => info!(
                "environment depth: hand removal asked {}: no depth provider in this run",
                if on { "on" } else { "off" }
            ),
        }
    }

    /// Relaunch Space Setup and requery the room's anchors
    /// (`Room::rescan`); nothing without a room.
    pub fn rescan_room(&mut self) {
        match self.room.as_mut() {
            Some(room) => room.rescan(),
            None => warn!("rescan: the room is off (debug.fosfora.room 0 or no scene support)"),
        }
    }

    /// Query the room again after a late `USE_SCENE` grant
    /// (`Room::requery_after_grant`, board #3264). False while a query or
    /// Space Setup is in flight (try again); true once issued, or with
    /// nothing to do.
    pub fn requery_room_after_grant(&mut self) -> bool {
        self.room.as_mut().is_none_or(Room::requery_after_grant)
    }

    pub fn has_room(&self) -> bool {
        self.room.is_some()
    }

    /// Where the room's scene query is (`Room::scan_state`); `None`
    /// without a room.
    pub fn scan_state(&self) -> Option<crate::label::ScanState> {
        self.room.as_ref().map(Room::scan_state)
    }

    /// The runtime's hand meshes (left, right), when hands are on and
    /// `XR_FB_hand_tracking_mesh` delivered them.
    pub fn hand_meshes(&self) -> [Option<&HandMeshData>; 2] {
        self.hands.as_ref().map_or([None, None], Hands::meshes)
    }

    /// Labels of the room anchors found so far, for the once-a-second log.
    pub fn room_summary(&self) -> Option<String> {
        self.room.as_ref().map(Room::anchor_summary)
    }

    /// One frame: wait, begin, locate hands and room anchors at the
    /// predicted display time, run `before_render` (the effect step) with
    /// them and `scene`, locate views, acquire the environment depth (when
    /// on), render both eyes through wgpu (a world-mode `scene` draws into
    /// them too), submit the passthrough layer (if any), the faces layer
    /// (if any, board #3793) and the main projection layer, bottom first.
    /// Must only be called while the session is running.
    pub fn frame(
        &mut self,
        gfx: &Gfx,
        clear: [f32; 4],
        stats: &mut FrameStats,
        particles: Option<&Particles3d>,
        mut scene: Option<&mut XrScene>,
        before_render: impl FnOnce(&FrameInput, Option<&mut XrScene>),
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
        let time = frame_state.predicted_display_time;
        let (view_flags, views) = self
            .session
            .locate_views(VIEW_TYPE, time, &self.space)
            .context("xrLocateViews")?;
        let tracked = xr::ViewStateFlags::ORIENTATION_VALID | xr::ViewStateFlags::POSITION_VALID;
        if !view_flags.contains(tracked) {
            // Just put on, or tracking lost: the runtime rejects a projection
            // layer built from these poses (ERROR_POSE_INVALID at
            // xrEndFrame). Show passthrough alone, or nothing.
            let passthrough_layer = self.passthrough_layer();
            let layers: Vec<&xr::CompositionLayerBase<'_, xr::Vulkan>> =
                passthrough_layer.iter().map(as_layer_base).collect();
            self.stream
                .end(frame_state.predicted_display_time, self.blend_mode, &layers)
                .context("xrEndFrame (views not tracked)")?;
            return Ok(());
        }
        let head = {
            let p = |i: usize| views[i].pose.position;
            let (a, b) = (p(0), p(views.len() - 1));
            [(a.x + b.x) * 0.5, (a.y + b.y) * 0.5, (a.z + b.z) * 0.5]
        };
        let head_rot = {
            let q = views[0].pose.orientation;
            [q.x, q.y, q.z, q.w]
        };
        if let Some(perf) = self.perf.as_mut() {
            perf.poll();
        }
        let hands = self
            .hands
            .as_mut()
            .map(|h| h.locate(&self.space, time))
            .unwrap_or_default();
        // Board #3336: the action set synced once a frame, after the hands,
        // and only while focused (the actions are inactive otherwise).
        let micro = match self.micro.as_ref() {
            Some(m) if self.state == xr::SessionState::FOCUSED => {
                m.sync(&self.session).unwrap_or_else(|e| {
                    self.micro_errors += 1;
                    if self.micro_errors.is_power_of_two() {
                        warn!(
                            "microgestures: sync failed ({} so far): {e:#}",
                            self.micro_errors
                        );
                    }
                    MicroFrame::default()
                })
            }
            _ => MicroFrame::default(),
        };
        let input = FrameInput {
            head,
            head_rot,
            perf: self.perf.as_ref().map(|p| p.latest).unwrap_or_default(),
            hands,
            micro,
            room_boxes: match self.room.as_mut() {
                Some(room) => {
                    room.locate(time);
                    room.boxes.clone()
                }
                None => Vec::new(),
            },
            room_labels: self
                .room
                .as_ref()
                .map(|r| r.labels.clone())
                .unwrap_or_default(),
            room_id: self.room.as_ref().and_then(Room::room_id),
        };
        // The live depth map, acquired at the same predicted display time
        // and in the same space as the views. A failed creation is retried
        // from here (`EnvDepthSlot::get`). With the collide on, this
        // frame's atlas pass writes the world sim's obstacle texture in
        // the frame's submit, ahead of the sim, so the sim's rows carry
        // this frame's poses (board #3352).
        let mut env = self
            .env_depth
            .as_mut()
            .and_then(|slot| slot.get(&self.session, self.system, gfx));
        let env_frame = env.as_deref_mut().and_then(|e| {
            e.poll_check(&gfx.device);
            e.acquire(&self.space, time)
        });
        if let (Some(e), Some(f)) = (env.as_deref_mut(), env_frame.as_ref())
            && let Some(collide) = e.prepare_atlas(&gfx.queue, f)
            && let Some(s) = scene.as_deref_mut()
        {
            s.set_depth_collide(collide);
        }
        before_render(&input, scene.as_deref_mut());

        let cameras: Vec<EyeCamera> = views.iter().map(camera).collect();
        log_stereo(view_flags, &views, &cameras);
        if let (Some(e), Some(f)) = (env.as_deref_mut(), env_frame.as_ref()) {
            e.check(&gfx.device, &gfx.queue, f, &input.room_boxes);
        }
        let extents: Vec<[u32; 2]> = self.eyes.iter().map(Eye::size).collect();
        // The faces layer's occluder runs at its own size (board #3793).
        let faces_extents: Vec<[u32; 2]> = self.faces.iter().map(Eye::size).collect();
        let faces_on = !self.faces.is_empty();
        let env_passes = env
            .as_deref()
            .map_or_else(EnvDepthPasses::default, |e| EnvDepthPasses {
                occluder: env_frame.and_then(|f| {
                    e.prepare(
                        &gfx.queue,
                        &f,
                        &cameras,
                        &extents,
                        faces_on.then_some(faces_extents.as_slice()),
                    )
                }),
                atlas: e.atlas_pass(),
            });

        // The eyes' images, then the faces layer's beside them.
        let mut image_indices = [0u32; EYE_COUNT];
        let mut faces_indices = [0u32; EYE_COUNT];
        let acquires = self
            .eyes
            .iter_mut()
            .zip(image_indices.iter_mut())
            .chain(self.faces.iter_mut().zip(faces_indices.iter_mut()));
        for (eye, index) in acquires {
            *index = eye
                .swapchain
                .acquire_image()
                .context("xrAcquireSwapchainImage")?;
            eye.swapchain
                .wait_image(xr::Duration::INFINITE)
                .context("xrWaitSwapchainImage")?;
        }

        let targets = acquired_views(&self.eyes, &image_indices);
        let faces_targets = acquired_views(&self.faces, &faces_indices);
        // Only a world-mode scene draws into the eyes; a quad scene reaches
        // them through the quad texture.
        let atlas = gfx.render(
            &targets,
            faces_on.then_some(faces_targets.as_slice()),
            &cameras,
            clear,
            particles,
            scene.filter(|s| s.is_world()),
            &env_passes,
        );
        if let Some(e) = env {
            e.note_atlas(atlas);
        }

        for eye in self.eyes.iter_mut().chain(self.faces.iter_mut()) {
            eye.swapchain
                .release_image()
                .context("xrReleaseSwapchainImage")?;
        }

        let projection_views: Vec<xr::CompositionLayerProjectionView<'_, xr::Vulkan>> = self
            .eyes
            .iter()
            .zip(views.iter())
            .map(|(eye, view)| eye.projection_view(view))
            .collect();
        // Over passthrough the projection layer is blended by its alpha
        // (premultiplied, the wgpu default): where nothing was drawn the
        // camera image shows through. The blend mode stays OPAQUE; the
        // passthrough layer underneath is what shows the room.
        let layer_flags = if self.passthrough.is_some() {
            xr::CompositionLayerFlags::BLEND_TEXTURE_SOURCE_ALPHA
        } else {
            xr::CompositionLayerFlags::EMPTY
        };
        let layer = xr::CompositionLayerProjection::new()
            .layer_flags(layer_flags)
            .space(&self.space)
            .views(&projection_views);
        // Board #3793: the faces layer, the same poses and fovs over its
        // own smaller images, premultiplied like the main layer (it exists
        // only over passthrough); the runtime upsamples it at composition.
        let faces_views: Vec<xr::CompositionLayerProjectionView<'_, xr::Vulkan>> = self
            .faces
            .iter()
            .zip(views.iter())
            .map(|(eye, view)| eye.projection_view(view))
            .collect();
        let sharpen = xr::sys::CompositionLayerSettingsFB {
            ty: xr::sys::CompositionLayerSettingsFB::TYPE,
            next: std::ptr::null(),
            layer_flags: xr::sys::CompositionLayerSettingsFlagsFB::NORMAL_SHARPENING,
        };
        let faces_layer = faces_on.then(|| {
            let faces = xr::CompositionLayerProjection::new()
                .layer_flags(xr::CompositionLayerFlags::BLEND_TEXTURE_SOURCE_ALPHA)
                .space(&self.space)
                .views(&faces_views);
            if !self.faces_sharpen {
                return faces;
            }
            let mut raw = faces.into_raw();
            raw.next = std::ptr::from_ref(&sharpen).cast();
            // SAFETY: `raw` came from the builder just above, its space and
            // views borrowed from `self.space` and `faces_views`; `next`
            // points at `sharpen`, a fully initialized
            // `XrCompositionLayerSettingsFB` (its extension enabled, as
            // `faces_sharpen` requires) with a null `next`. All three
            // outlive `xrEndFrame` below, the last use of this layer.
            unsafe { xr::CompositionLayerProjection::from_raw(raw) }
        });
        let passthrough_layer = self.passthrough_layer();
        let order = crate::faces_layer::layer_order(passthrough_layer.is_some(), faces_on);
        let layers: Vec<&xr::CompositionLayerBase<'_, xr::Vulkan>> = order
            .iter()
            .filter_map(|l| match l {
                Layer::Passthrough => passthrough_layer.as_ref().map(as_layer_base),
                Layer::Faces => faces_layer.as_deref(),
                Layer::Main => Some(&*layer),
            })
            .collect();
        stats.record_cpu(cpu_start.elapsed());
        self.stream
            .end(frame_state.predicted_display_time, self.blend_mode, &layers)
            .context("xrEndFrame")?;
        Ok(())
    }
}

impl XrSession {
    /// The passthrough composition layer for this frame, if passthrough is
    /// on. The openxr crate does not re-export its builder, so the raw struct
    /// is filled in directly (spec: `space` is null and `flags` empty for a
    /// reconstruction layer).
    fn passthrough_layer(&self) -> Option<xr::sys::CompositionLayerPassthroughFB> {
        self.passthrough
            .as_ref()
            .map(|p| xr::sys::CompositionLayerPassthroughFB {
                ty: xr::sys::CompositionLayerPassthroughFB::TYPE,
                next: std::ptr::null(),
                flags: xr::CompositionLayerFlags::EMPTY,
                space: xr::sys::Space::NULL,
                layer_handle: p.layer.as_raw(),
            })
    }
}

/// The wgpu view of each swapchain's acquired image, in eye order.
fn acquired_views<'a>(eyes: &'a [Eye], indices: &[u32]) -> Vec<&'a wgpu::TextureView> {
    eyes.iter()
        .zip(indices)
        .map(|(eye, &index)| &eye.images[index as usize].1)
        .collect()
}

/// View a raw passthrough layer as the polymorphic layer base `xrEndFrame`
/// takes.
fn as_layer_base(
    p: &xr::sys::CompositionLayerPassthroughFB,
) -> &xr::CompositionLayerBase<'_, xr::Vulkan> {
    // SAFETY: `CompositionLayerBase` is `repr(transparent)` over
    // `XrCompositionLayerBaseHeader`, and every composition layer struct,
    // this one included, starts with that header (the spec's polymorphic
    // layer rule, which is also how the openxr crate's own builders deref to
    // the base). The returned borrow lives as long as `p`.
    unsafe { &*std::ptr::from_ref(p).cast::<xr::CompositionLayerBase<'_, xr::Vulkan>>() }
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
