//! Vulkan interop and the XR-side renderer (S2 stereo pipeline, S4 quad,
//! S5 particle passes).
//!
//! The OpenXR runtime creates the Vulkan instance and device
//! (`XR_KHR_vulkan_enable2`) from create-infos that carry exactly the
//! extensions and features wgpu-hal asks for; both are then wrapped into
//! wgpu 27 through `wgpu::hal` (`Instance::from_raw` → `expose_adapter` →
//! `device_from_raw` → `wgpu::Instance::from_hal` and friends). Swapchain
//! images become `wgpu::Texture`s that wgpu never frees: OpenXR owns them.

use std::ffi::c_char;

use anyhow::{Context, Result, anyhow};
use ash::vk::{self, Handle as _};
use glam::Mat4;
use log::{error, info, warn};
use openxr as xr;
use wgpu::hal;
use wgpu::hal::api::Vulkan as HalVk;

use crate::env_depth::{AtlasOutcome, EnvDepthPasses};
use crate::particles3d::{DEPTH_FORMAT, Particles3d};
use crate::scene::XrScene;
use crate::xr::XrContext;

/// Top of the range the Quest runtime reports (1.0 .. 1.2). Everything the
/// core needs is available at 1.1; 1.2 promotes timeline semaphores.
const VK_API_VERSION: u32 = vk::API_VERSION_1_2;

/// Matches the desktop surface choice (`gpu/context.rs`); the runtime offers
/// the same `R8G8B8A8_SRGB` for swapchains.
pub const SWAPCHAIN_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const SWAPCHAIN_VK_FORMAT: vk::Format = vk::Format::R8G8B8A8_SRGB;

/// The limits the core's pipeline layouts rely on (`gpu/context.rs`). Below
/// these the S2 gate says stop and report.
const REQUIRED_STORAGE_BUFFERS_PER_STAGE: u32 = 16;
const REQUIRED_BIND_GROUPS: u32 = 5;

/// Raw handles the OpenXR session binding needs. They stay valid as long as
/// the owning `Gfx` (wgpu keeps the instance and device alive).
#[derive(Clone, Copy)]
pub struct RawHandles {
    pub instance: vk::Instance,
    pub physical_device: vk::PhysicalDevice,
    pub device: vk::Device,
    pub queue_family_index: u32,
}

/// One eye's camera for a frame, in the reference space.
#[derive(Debug, Clone, Copy)]
pub struct EyeCamera {
    pub view: Mat4,
    pub proj: Mat4,
}

impl EyeCamera {
    pub fn view_proj(&self) -> Mat4 {
        self.proj * self.view
    }
}

pub struct Gfx {
    // Field order is drop order: bindings and the pipeline go before the
    // device, the device before the adapter and instance.
    quad: Option<QuadBinding>,
    quad_layout: wgpu::BindGroupLayout,
    /// The debug panel: a posed, textured quad (`set_panel`, `set_panel_pose`).
    panel: PosedQuad,
    /// The room editor's label (`set_label`, `set_label_pose`, `label.rs`):
    /// another posed quad, drawn right after the panel.
    label: PosedQuad,
    panel_pipeline: wgpu::RenderPipeline,
    panel_layout: wgpu::BindGroupLayout,
    /// Beams (`set_beam`): the debug panel's pointer, one per hand for
    /// the reach extension and the room editor's pick.
    beam: QuadBinding,
    beam_visible: std::cell::Cell<[bool; BEAM_SLOTS]>,
    beam_pipeline: wgpu::RenderPipeline,
    /// The surfaces pass (`set_surfaces`, `surface_fx.rs`, board #3472):
    /// one uniform slot per lit face, the first `surface_count` drawn.
    surfaces: Vec<QuadBinding>,
    surface_count: std::cell::Cell<usize>,
    surface_pipeline: wgpu::RenderPipeline,
    /// The wall spectrum (`set_canvas`, `canvas.rs`): one lit quad on the
    /// wall the wearer faces.
    canvas: QuadBinding,
    canvas_visible: std::cell::Cell<bool>,
    canvas_pipeline: wgpu::RenderPipeline,
    /// The room editor's highlight (`set_highlight`, `highlight.rs`): one
    /// lit quad on the face the pick hit.
    highlight: QuadBinding,
    highlight_visible: std::cell::Cell<bool>,
    highlight_pipeline: wgpu::RenderPipeline,
    /// Whether the eye pass draws the world effect (`set_world_visible`;
    /// the hand menu's Cloud row, board #3326).
    world_visible: std::cell::Cell<bool>,
    /// Small sprites marking the virtual (reach-extended) hand joints.
    ghost: QuadBinding,
    ghost_count: std::cell::Cell<u32>,
    ghost_pipeline: wgpu::RenderPipeline,
    eyes: Vec<EyeUniform>,
    /// Per-eye depth attachment at the swapchain size (`set_eye_extent`).
    depth: Vec<wgpu::TextureView>,
    pipeline: wgpu::RenderPipeline,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    _adapter: wgpu::Adapter,
    _instance: wgpu::Instance,
    pub raw: RawHandles,
    pub swapchain_vk_format: vk::Format,
}

/// The world-locked quad: its placement uniform plus the texture it shows.
struct QuadBinding {
    uniform: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

/// A posed, textured quad the panel's pipeline draws (the hand menu, the
/// room editor's label): hidden until it has a texture and a pose.
#[derive(Default)]
struct PosedQuad {
    binding: Option<QuadBinding>,
    visible: std::cell::Cell<bool>,
}

/// Beam slots: the debug panel's pointer, each hand's reach beam and the
/// room editor's pick (board #3326). `BEAM_WGSL`'s array takes its length
/// from this, as the uniform does.
pub const BEAM_SLOTS: usize = 4;
pub const BEAM_PANEL: usize = 0;
pub const BEAM_REACH: [usize; 2] = [1, 2];
pub const BEAM_PICK: usize = 3;
/// Bytes per beam in the uniform: four corners and a color.
const BEAM_BYTES: u64 = 80;
/// Ghost sprites the uniform holds (both hands' 26 joints fit).
pub const GHOST_MAX: usize = 64;
const GHOST_BYTES: u64 = 32 + 16 * GHOST_MAX as u64;

/// A beam this frame: a thin strip from `start` to `end` turned to face
/// `eye`, `width_m` wide, drawn at `alpha`.
#[derive(Debug, Clone, Copy)]
pub struct Beam {
    pub start: [f32; 3],
    pub end: [f32; 3],
    pub eye: [f32; 3],
    pub width_m: f32,
    pub alpha: f32,
}

/// Where the debug panel (or the room editor's label) sits this frame: its
/// center and the vectors from the center to its right and top edges
/// (reference space, meters). The texture's top-left corner is at
/// `center - right + up`.
#[derive(Debug, Clone, Copy)]
pub struct PanelPose {
    pub center: [f32; 3],
    pub right: [f32; 3],
    pub up: [f32; 3],
    /// How much of the texture's height the panel shows, from the top
    /// (0..1): the hand menu uses only its top strip.
    pub v_max: f32,
}

struct EyeUniform {
    buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

/// Adapter limits recorded for `docs/xr/MEASURED.md`.
#[derive(Debug, Clone, Copy)]
pub struct RecordedLimits {
    pub max_storage_buffers_per_shader_stage: u32,
    pub max_bind_groups: u32,
    pub max_storage_buffer_binding_size: u32,
    pub max_buffer_size: u64,
}

impl Gfx {
    pub fn new(ctx: &XrContext, android_sdk_version: u32) -> Result<Self> {
        // SAFETY: loads the system `libvulkan.so`, Android's Vulkan loader.
        let entry = unsafe { ash::Entry::load() }.context("loading libvulkan.so")?;
        // openxr-sys and ash spell the `vkGetInstanceProcAddr` pointer type
        // differently; they are the same C signature.
        // SAFETY: both types are `unsafe extern "system" fn(VkInstance, *const c_char) -> PFN_vkVoidFunction`.
        let get_instance_proc_addr: xr::sys::platform::VkGetInstanceProcAddr =
            unsafe { std::mem::transmute(entry.static_fn().get_instance_proc_addr) };

        // ---- Instance: what wgpu-hal wants, created by the runtime. ----
        let instance_flags = wgpu::InstanceFlags::empty();
        let instance_extensions =
            hal::vulkan::Instance::desired_extensions(&entry, VK_API_VERSION, instance_flags)
                .map_err(|e| anyhow!("wgpu-hal desired_extensions: {e}"))?;
        info!(
            "vulkan instance extensions for wgpu: {}",
            instance_extensions
                .iter()
                .map(|e| e.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" ")
        );
        let instance_ext_ptrs: Vec<*const c_char> =
            instance_extensions.iter().map(|e| e.as_ptr()).collect();
        let app_info = vk::ApplicationInfo::default()
            .application_name(c"Fosfora VR")
            .application_version(1)
            .engine_name(c"fosfora")
            .engine_version(1)
            .api_version(VK_API_VERSION);
        let instance_info = vk::InstanceCreateInfo::default()
            .application_info(&app_info)
            .enabled_extension_names(&instance_ext_ptrs);
        // SAFETY: `instance_info` is a complete, valid VkInstanceCreateInfo
        // that outlives the call; the runtime appends what it needs.
        let raw_instance = unsafe {
            ctx.instance.create_vulkan_instance(
                ctx.system,
                get_instance_proc_addr,
                std::ptr::from_ref(&instance_info).cast(),
            )
        }
        .context("xrCreateVulkanInstanceKHR")?
        .map_err(|r| {
            anyhow!(
                "vkCreateInstance via runtime: {:?}",
                vk::Result::from_raw(r)
            )
        })?;
        // SAFETY: `raw_instance` is a live VkInstance created with this
        // entry's `vkGetInstanceProcAddr`.
        let ash_instance = unsafe {
            ash::Instance::load(
                entry.static_fn(),
                vk::Instance::from_raw(raw_instance as u64),
            )
        };
        // SAFETY: the instance was created from `entry` with exactly
        // `instance_extensions`, `VK_API_VERSION` and `instance_flags`;
        // `drop_callback` is None so wgpu-hal owns and destroys it.
        let hal_instance = unsafe {
            hal::vulkan::Instance::from_raw(
                entry.clone(),
                ash_instance,
                VK_API_VERSION,
                android_sdk_version,
                None,
                instance_extensions,
                instance_flags,
                wgpu::MemoryBudgetThresholds::default(),
                false,
                None,
            )
        }
        .map_err(|e| anyhow!("wgpu-hal Instance::from_raw: {e}"))?;

        // ---- Physical device: the one the runtime renders with. ----
        // SAFETY: the instance came from `create_vulkan_instance` above.
        let physical_device = vk::PhysicalDevice::from_raw(
            unsafe {
                ctx.instance.vulkan_graphics_device(
                    ctx.system,
                    hal_instance
                        .shared_instance()
                        .raw_instance()
                        .handle()
                        .as_raw() as _,
                )
            }
            .context("xrGetVulkanGraphicsDevice2KHR")? as u64,
        );
        let exposed = hal_instance
            .expose_adapter(physical_device)
            .ok_or_else(|| anyhow!("wgpu-hal rejected the runtime's physical device"))?;
        info!(
            "wgpu adapter: {} ({:?}, driver {} {}) · backend {:?}",
            exposed.info.name,
            exposed.info.device_type,
            exposed.info.driver,
            exposed.info.driver_info,
            exposed.info.backend
        );
        let limits = record_limits(&exposed.capabilities.limits);
        info!("wgpu features: {:?}", exposed.features);
        info!("wgpu downlevel: {:?}", exposed.capabilities.downlevel.flags);

        // ---- Device: extensions + features wgpu-hal needs, created by the runtime. ----
        let features = wgpu::Features::empty();
        let device_extensions = exposed.adapter.required_device_extensions(features);
        info!(
            "vulkan device extensions for wgpu: {}",
            device_extensions
                .iter()
                .map(|e| e.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" ")
        );
        let mut phd_features = exposed
            .adapter
            .physical_device_features(&device_extensions, features);

        // SAFETY: `physical_device` belongs to the hal instance's VkInstance.
        let families = unsafe {
            hal_instance
                .shared_instance()
                .raw_instance()
                .get_physical_device_queue_family_properties(physical_device)
        };
        let queue_family_index = families
            .iter()
            .position(|f| f.queue_flags.contains(vk::QueueFlags::GRAPHICS))
            .ok_or_else(|| anyhow!("no graphics queue family"))?
            as u32;
        let priorities = [1.0f32];
        let queue_infos = [vk::DeviceQueueCreateInfo::default()
            .queue_family_index(queue_family_index)
            .queue_priorities(&priorities)];
        let device_ext_ptrs: Vec<*const c_char> =
            device_extensions.iter().map(|e| e.as_ptr()).collect();
        let device_info = phd_features.add_to_device_create(
            vk::DeviceCreateInfo::default()
                .queue_create_infos(&queue_infos)
                .enabled_extension_names(&device_ext_ptrs),
        );
        // SAFETY: `device_info` (and the feature structs chained by wgpu-hal)
        // is complete and valid for the duration of the call, and
        // `physical_device` is the one the runtime selected.
        let raw_device = unsafe {
            ctx.instance.create_vulkan_device(
                ctx.system,
                get_instance_proc_addr,
                physical_device.as_raw() as _,
                std::ptr::from_ref(&device_info).cast(),
            )
        }
        .context("xrCreateVulkanDeviceKHR")?
        .map_err(|r| anyhow!("vkCreateDevice via runtime: {:?}", vk::Result::from_raw(r)))?;
        // SAFETY: `raw_device` is a live VkDevice created from this instance.
        let ash_device = unsafe {
            ash::Device::load(
                hal_instance.shared_instance().raw_instance().fp_v1_0(),
                vk::Device::from_raw(raw_device as u64),
            )
        };
        // SAFETY: the device was created from this adapter with
        // `queue_family_index`, `device_extensions` and the feature set
        // `physical_device_features()` produced; `drop_callback` is None so
        // wgpu-hal owns and destroys it.
        let open_device = unsafe {
            exposed.adapter.device_from_raw(
                ash_device,
                None,
                &device_extensions,
                features,
                &wgpu::MemoryHints::Performance,
                queue_family_index,
                0,
            )
        }
        .map_err(|e| anyhow!("wgpu-hal device_from_raw: {e}"))?;

        let raw = RawHandles {
            instance: hal_instance.shared_instance().raw_instance().handle(),
            physical_device,
            device: open_device.device.raw_device().handle(),
            queue_family_index,
        };

        // ---- wgpu on top. ----
        // SAFETY: `hal_instance` is a live, usable Vulkan hal instance.
        let instance = unsafe { wgpu::Instance::from_hal::<HalVk>(hal_instance) };
        // SAFETY: `exposed` was produced by that instance's `expose_adapter`.
        let adapter = unsafe { instance.create_adapter_from_hal::<HalVk>(exposed) };
        // SAFETY: `open_device` was created from this adapter; the requested
        // features (none) are a subset of what it supports.
        let (device, queue) = unsafe {
            adapter.create_device_from_hal::<HalVk>(
                open_device,
                &wgpu::DeviceDescriptor {
                    label: Some("fosfora-xr-device"),
                    required_features: features,
                    required_limits: wgpu::Limits {
                        max_storage_buffers_per_shader_stage: REQUIRED_STORAGE_BUFFERS_PER_STAGE,
                        max_bind_groups: REQUIRED_BIND_GROUPS,
                        max_storage_buffer_binding_size: limits.max_storage_buffer_binding_size,
                        max_buffer_size: limits.max_buffer_size,
                        ..wgpu::Limits::default()
                    },
                    experimental_features: wgpu::ExperimentalFeatures::default(),
                    memory_hints: wgpu::MemoryHints::Performance,
                    trace: wgpu::Trace::Off,
                },
            )
        }
        .context("wgpu create_device_from_hal")?;
        // The error's Display is only its kind ("Validation Error"); the
        // description carries what failed.
        device.on_uncaptured_error(std::sync::Arc::new(|e| match &e {
            wgpu::Error::Validation { description, .. }
            | wgpu::Error::Internal { description, .. } => error!("wgpu error: {description}"),
            _ => error!("wgpu error: {e}"),
        }));
        device
            .set_device_lost_callback(|reason, msg| error!("wgpu device lost ({reason:?}): {msg}"));

        let (pipeline, eyes, quad_layout, eye_layout) = build_quad_pipeline(&device);
        let (panel_pipeline, panel_layout) = build_panel_pipeline(&device, &eye_layout);
        let (beam_pipeline, beam) = build_beam_pipeline(&device, &eye_layout);
        let (ghost_pipeline, ghost) = build_ghost_pipeline(&device, &eye_layout);
        let (surface_pipeline, surfaces) = build_surface_slots(
            &device,
            &eye_layout,
            crate::surface_fx::SURFACE_FX_WGSL,
            "xr-surfaces",
            crate::surface_fx::UNIFORM_ROWS,
            crate::surface_fx::MAX_SLOTS,
        );
        let (canvas_pipeline, canvas) = build_surface_pipeline(
            &device,
            &eye_layout,
            crate::canvas::CANVAS_WGSL,
            "xr-canvas",
            crate::canvas::UNIFORM_ROWS,
        );
        let (highlight_pipeline, highlight) = build_surface_pipeline(
            &device,
            &eye_layout,
            crate::highlight::HIGHLIGHT_WGSL,
            "xr-highlight",
            crate::highlight::UNIFORM_ROWS,
        );
        info!("wgpu device ready");

        Ok(Self {
            quad: None,
            quad_layout,
            panel: PosedQuad::default(),
            label: PosedQuad::default(),
            panel_pipeline,
            panel_layout,
            beam,
            beam_visible: std::cell::Cell::new([false; BEAM_SLOTS]),
            beam_pipeline,
            surfaces,
            surface_count: std::cell::Cell::new(0),
            surface_pipeline,
            canvas,
            canvas_visible: std::cell::Cell::new(false),
            canvas_pipeline,
            highlight,
            highlight_visible: std::cell::Cell::new(false),
            world_visible: std::cell::Cell::new(true),
            highlight_pipeline,
            ghost,
            ghost_count: std::cell::Cell::new(0),
            ghost_pipeline,
            eyes,
            depth: Vec::new(),
            pipeline,
            device,
            queue,
            _adapter: adapter,
            _instance: instance,
            raw,
            swapchain_vk_format: SWAPCHAIN_VK_FORMAT,
        })
    }

    /// Show `view` on a quad `width_m` wide (height from `aspect` = w/h)
    /// centered at `center` in the reference space, facing -Z.
    pub fn set_quad(
        &mut self,
        view: &wgpu::TextureView,
        width_m: f32,
        aspect: f32,
        center: [f32; 3],
    ) {
        let half = [width_m * 0.5, width_m * 0.5 / aspect];
        let data: [f32; 8] = [
            center[0], center[1], center[2], 0.0, half[0], half[1], 0.0, 0.0,
        ];
        let uniform = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("xr-quad-uniform"),
            size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue
            .write_buffer(&uniform, 0, bytemuck::bytes_of(&data));
        let sampler = self.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("xr-quad-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            ..wgpu::SamplerDescriptor::default()
        });
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("xr-quad"),
            layout: &self.quad_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        self.quad = Some(QuadBinding {
            uniform,
            bind_group,
        });
        info!(
            "quad: {:.2} m x {:.2} m at ({:.2}, {:.2}, {:.2})",
            half[0] * 2.0,
            half[1] * 2.0,
            center[0],
            center[1],
            center[2]
        );
    }

    /// The texture the debug panel shows (premultiplied alpha). Hidden until
    /// [`Self::set_panel_pose`] places it.
    pub fn set_panel(&mut self, view: &wgpu::TextureView) {
        self.panel.binding = Some(self.posed_binding("xr-panel", view));
    }

    /// Place the debug panel for this frame, or hide it with `None`.
    pub fn set_panel_pose(&self, pose: Option<PanelPose>) {
        self.place_posed(&self.panel, pose);
    }

    /// The texture the room editor's label shows (premultiplied alpha,
    /// `label.rs`). Hidden until [`Self::set_label_pose`] places it.
    pub fn set_label(&mut self, view: &wgpu::TextureView) {
        self.label.binding = Some(self.posed_binding("xr-label", view));
    }

    /// Place the room editor's label for this frame, or hide it with
    /// `None`.
    pub fn set_label_pose(&self, pose: Option<PanelPose>) {
        self.place_posed(&self.label, pose);
    }

    /// A posed quad's uniform and bind group for `view`, named `name`.
    fn posed_binding(&self, name: &str, view: &wgpu::TextureView) -> QuadBinding {
        let uniform = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(&format!("{name}-uniform")),
            size: 48,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sampler = self.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some(&format!("{name}-sampler")),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..wgpu::SamplerDescriptor::default()
        });
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(name),
            layout: &self.panel_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        QuadBinding {
            uniform,
            bind_group,
        }
    }

    /// Place a posed quad for this frame, or hide it with `None`.
    fn place_posed(&self, quad: &PosedQuad, pose: Option<PanelPose>) {
        let (Some(binding), Some(pose)) = (&quad.binding, pose) else {
            quad.visible.set(false);
            return;
        };
        let (c, r, u) = (pose.center, pose.right, pose.up);
        let data: [f32; 12] = [
            c[0], c[1], c[2], pose.v_max, r[0], r[1], r[2], 0.0, u[0], u[1], u[2], 0.0,
        ];
        self.queue
            .write_buffer(&binding.uniform, 0, bytemuck::bytes_of(&data));
        quad.visible.set(true);
    }

    /// Place beam `slot` (`BEAM_PANEL`, `BEAM_REACH[h]`, `BEAM_PICK`) for
    /// this frame, or hide it with `None`.
    pub fn set_beam(&self, slot: usize, beam: Option<Beam>) {
        let mut visible = self.beam_visible.get();
        // A hidden slot is a zero-area strip: it draws no fragments.
        let mut data = [[0.0f32; 4]; 5];
        visible[slot] = false;
        if let Some(b) = beam {
            let (a, e) = (glam::Vec3::from(b.start), glam::Vec3::from(b.end));
            // Across the beam, facing the eye.
            let side = (e - a)
                .cross(glam::Vec3::from(b.eye) - a)
                .normalize_or_zero()
                * (b.width_m * 0.5);
            if side != glam::Vec3::ZERO {
                let corner = |p: glam::Vec3, across: f32| [p.x, p.y, p.z, across];
                data = [
                    corner(a - side, -1.0),
                    corner(a + side, 1.0),
                    corner(e + side, 1.0),
                    corner(e - side, -1.0),
                    [1.0, 1.0, 1.0, b.alpha],
                ];
                visible[slot] = true;
            }
        }
        self.queue.write_buffer(
            &self.beam.uniform,
            slot as u64 * BEAM_BYTES,
            bytemuck::cast_slice(&data),
        );
        self.beam_visible.set(visible);
    }

    /// The surfaces pass for this frame: one lit face per entry
    /// (`surface_fx::rows`), each into its own slot and drawn in order; at
    /// most `surface_fx::MAX_SLOTS`, the rest dropped. Empty hides the
    /// pass.
    pub fn set_surfaces(&self, faces: &[[[f32; 4]; crate::surface_fx::UNIFORM_ROWS]]) {
        let n = faces.len().min(self.surfaces.len());
        for (slot, rows) in self.surfaces.iter().zip(&faces[..n]) {
            self.queue
                .write_buffer(&slot.uniform, 0, bytemuck::cast_slice(rows));
        }
        self.surface_count.set(n);
    }

    /// The wall spectrum for this frame (`Canvas::uniform`'s rows), or
    /// hidden with `None`.
    pub fn set_canvas(&self, rows: Option<&[[f32; 4]; crate::canvas::UNIFORM_ROWS]>) {
        if let Some(rows) = rows {
            self.queue
                .write_buffer(&self.canvas.uniform, 0, bytemuck::cast_slice(rows));
        }
        self.canvas_visible.set(rows.is_some());
    }

    /// The room editor's highlight for this frame (`highlight::uniform`'s
    /// rows), or hidden with `None`.
    pub fn set_highlight(&self, rows: Option<&[[f32; 4]; crate::highlight::UNIFORM_ROWS]>) {
        if let Some(rows) = rows {
            self.queue
                .write_buffer(&self.highlight.uniform, 0, bytemuck::cast_slice(rows));
        }
        self.highlight_visible.set(rows.is_some());
    }

    /// Whether the eye pass draws the world effect from this frame on
    /// (`room_edit::Cloud::visible`; on at start). Hidden, `render` skips
    /// its `prepare_world` and `draw_world` for every eye, but still
    /// dispatches its sim, so showing it again is instant and the cloud
    /// is as it would have been. Everything else still draws: the depth
    /// occluders, the surfaces pass, the canvas, the highlight, the beams,
    /// the panel and the label. The pitcher's pour and the throw's bursts
    /// are particles of the world effect, so they hide with it.
    pub fn set_world_visible(&self, visible: bool) {
        self.world_visible.set(visible);
    }

    /// The ghost sprites for this frame: xyz and radius per point (at most
    /// `GHOST_MAX`), turned to face `eye`, at `alpha`. Empty hides them.
    pub fn set_ghost(&self, points: &[[f32; 4]], eye: [f32; 3], alpha: f32) {
        let n = points.len().min(GHOST_MAX);
        if n > 0 {
            let mut data = vec![[0.0f32; 4]; 2 + n];
            data[0] = [eye[0], eye[1], eye[2], 0.0];
            data[1] = [1.0, 1.0, 1.0, alpha];
            data[2..].copy_from_slice(&points[..n]);
            self.queue
                .write_buffer(&self.ghost.uniform, 0, bytemuck::cast_slice(&data));
        }
        self.ghost_count.set(n as u32);
    }

    /// Size the eye's depth attachment to its swapchain. Call once per eye,
    /// in eye order, after the swapchains exist.
    pub fn set_eye_extent(&mut self, eye: usize, width: u32, height: u32) {
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("xr-eye-depth"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        if eye < self.depth.len() {
            self.depth[eye] = view;
        } else {
            debug_assert_eq!(eye, self.depth.len());
            self.depth.push(view);
        }
    }

    /// Wrap an OpenXR swapchain image as a wgpu texture plus a full view. The
    /// no-op drop callback keeps wgpu from freeing the image: OpenXR owns it.
    pub fn wrap_swapchain_image(
        &self,
        image: vk::Image,
        width: u32,
        height: u32,
    ) -> Result<(wgpu::Texture, wgpu::TextureView)> {
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let hal_desc = hal::TextureDescriptor {
            label: Some("xr-swapchain-image"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SWAPCHAIN_FORMAT,
            usage: wgpu::TextureUses::COLOR_TARGET,
            memory_flags: hal::MemoryFlags::empty(),
            view_formats: vec![],
        };
        // SAFETY: the image was created by the runtime with COLOR_ATTACHMENT
        // usage, this format and size (`xrCreateSwapchain`); the Some(no-op)
        // callback means wgpu-hal never destroys it, and OpenXR keeps it alive
        // until the swapchain is destroyed, which happens before this device.
        let hal_texture = unsafe {
            self.device
                .as_hal::<HalVk>()
                .ok_or_else(|| anyhow!("wgpu device is not Vulkan"))?
                .texture_from_raw(image, &hal_desc, Some(Box::new(|| {})))
        };
        // SAFETY: `hal_texture` belongs to this device and matches `desc`.
        let texture = unsafe {
            self.device.create_texture_from_hal::<HalVk>(
                hal_texture,
                &wgpu::TextureDescriptor {
                    label: Some("xr-swapchain-image"),
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: SWAPCHAIN_FORMAT,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                },
            )
        };
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Ok((texture, view))
    }

    /// Wrap one image of the environment depth swapchain
    /// (`XR_META_environment_depth`): a `D16_UNORM` array of two layers
    /// (left, right) the shaders only read, as a wgpu texture plus a
    /// `D2Array` depth view. The no-op drop callback keeps wgpu from
    /// freeing the image: the runtime owns it.
    ///
    /// Layouts: `create_texture_from_hal` registers the texture as
    /// uninitialized, so the first time each image is bound wgpu's barrier
    /// goes from `UNDEFINED`, which Vulkan allows to discard the contents:
    /// the first frame that uses a given image may show no occlusion.
    /// After that wgpu tracks the image as sampled and issues no further
    /// barrier. wgpu-hal samples depth formats in
    /// `DEPTH_STENCIL_READ_ONLY_OPTIMAL`, not the
    /// `SHADER_READ_ONLY_OPTIMAL` the runtime hands the image over in and
    /// expects back at `xrEndFrame`; both are read-only layouts, and this
    /// is left as is until the device shows it matters.
    pub fn wrap_env_depth_image(
        &self,
        image: vk::Image,
        width: u32,
        height: u32,
    ) -> Result<(wgpu::Texture, wgpu::TextureView)> {
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 2,
        };
        let hal_desc = hal::TextureDescriptor {
            label: Some("xr-env-depth-image"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth16Unorm,
            usage: wgpu::TextureUses::RESOURCE,
            memory_flags: hal::MemoryFlags::empty(),
            view_formats: vec![],
        };
        // SAFETY: the image was created by the runtime as a two-layer
        // `D16_UNORM` 2D array of this size for sampling
        // (`xrCreateEnvironmentDepthSwapchainMETA`; the size is the
        // swapchain state's); the Some(no-op) callback means wgpu-hal never
        // destroys it, and the runtime keeps it alive until the depth
        // swapchain is destroyed, which `EnvDepth` does after dropping
        // these wrappers and before this device goes.
        let hal_texture = unsafe {
            self.device
                .as_hal::<HalVk>()
                .ok_or_else(|| anyhow!("wgpu device is not Vulkan"))?
                .texture_from_raw(image, &hal_desc, Some(Box::new(|| {})))
        };
        // SAFETY: `hal_texture` belongs to this device and matches `desc`.
        let texture = unsafe {
            self.device.create_texture_from_hal::<HalVk>(
                hal_texture,
                &wgpu::TextureDescriptor {
                    label: Some("xr-env-depth-image"),
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Depth16Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
            )
        };
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("xr-env-depth-view"),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            aspect: wgpu::TextureAspect::DepthOnly,
            base_array_layer: 0,
            array_layer_count: Some(2),
            ..wgpu::TextureViewDescriptor::default()
        });
        Ok((texture, view))
    }

    /// Render both eyes and submit once: the depth atlas pass with its copy
    /// into the world effect's obstacle texture (returning what it did)
    /// and the environment depth occluder (`env_depth`, each when there is
    /// one), the quad (depth-writing), the S7 occluders and then the S5 particles
    /// (depth-tested, additive) into each eye. With a world-mode `scene` (C3b), its sim is dispatched first and
    /// its sprites are drawn last in the same eye pass, tested against the
    /// depth the occluders and the primer wrote. The render pass is the last
    /// use of each swapchain image this frame, so it ends in
    /// `COLOR_ATTACHMENT_OPTIMAL` as `xrReleaseSwapchainImage` requires.
    pub fn render(
        &self,
        targets: &[&wgpu::TextureView],
        cameras: &[EyeCamera],
        clear: [f32; 4],
        particles: Option<&Particles3d>,
        mut scene: Option<&mut XrScene>,
        env_depth: &EnvDepthPasses<'_>,
    ) -> Option<AtlasOutcome> {
        for (i, (eye, cam)) in self.eyes.iter().zip(cameras).enumerate() {
            self.queue.write_buffer(
                &eye.buffer,
                0,
                bytemuck::bytes_of(&cam.view_proj().to_cols_array()),
            );
            if let Some(p) = particles {
                p.set_eye(&self.queue, i, cam.view, cam.proj);
            }
        }
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("xr-frame"),
            });
        // The depth atlas (board #3352): its pass and the copy into the
        // world effect's obstacle texture ride in this one submission,
        // before the sim reads them.
        let atlas = env_depth.atlas.as_ref().map(|a| {
            let target = if a.copies() {
                let (w, h) = a.size();
                scene
                    .as_deref_mut()
                    .filter(|s| s.is_world())
                    .and_then(|s| s.depth_atlas_target(w, h))
            } else {
                None
            };
            a.record(&mut encoder, target.as_ref().map(|(t, sized)| (t, *sized)))
        });
        if let Some(p) = particles {
            p.step(&mut encoder);
        }
        if let Some(s) = scene.as_deref_mut() {
            s.dispatch_world(&mut encoder);
        }
        for (i, ((eye, target), cam)) in self.eyes.iter().zip(targets).zip(cameras).enumerate() {
            let depth = self.depth.get(i);
            // Pipeline and camera slot before the pass; the draw goes inside it.
            // Neither while the world effect is hidden (`set_world_visible`).
            let world_draw = scene
                .as_deref_mut()
                .filter(|_| self.world_visible.get())
                .and_then(|s| s.prepare_world(&self.device, cam));
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("xr-eye"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: f64::from(clear[0]),
                            g: f64::from(clear[1]),
                            b: f64::from(clear[2]),
                            a: f64::from(clear[3]),
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: depth.map(|view| {
                    wgpu::RenderPassDepthStencilAttachment {
                        view,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(1.0),
                            store: wgpu::StoreOp::Discard,
                        }),
                        stencil_ops: None,
                    }
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            // The live depth map first, right after the clear: it writes
            // the real room's depth (compare Always) and everything after
            // tests against it.
            if let Some(d) = &env_depth.occluder {
                d.draw(&mut pass, i);
            }
            if let Some(quad) = &self.quad {
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &eye.bind_group, &[]);
                pass.set_bind_group(1, &quad.bind_group, &[]);
                pass.draw(0..6, 0..1);
            }
            if let Some(p) = particles {
                p.draw_occluders(&mut pass, i);
            }
            // The surfaces pass after the occluders, so a desk or a hand
            // in front of a surface hides its light, and before the
            // sprites, so embers on a surface draw over it: one draw per
            // lit face, its slot bound (board #3472; the floor ripple drew
            // here before it became the rings).
            let lit = self.surface_count.get();
            if lit > 0 {
                pass.set_pipeline(&self.surface_pipeline);
                pass.set_bind_group(0, &eye.bind_group, &[]);
                for slot in &self.surfaces[..lit] {
                    pass.set_bind_group(1, &slot.bind_group, &[]);
                    pass.draw(0..6, 0..1);
                }
            }
            // The wall spectrum right after, on the same terms: behind a
            // hand or a chair in front of the wall, under the embers.
            if self.canvas_visible.get() {
                pass.set_pipeline(&self.canvas_pipeline);
                pass.set_bind_group(0, &eye.bind_group, &[]);
                pass.set_bind_group(1, &self.canvas.bind_group, &[]);
                pass.draw(0..6, 0..1);
            }
            // The room editor's highlight on the same terms, after both.
            if self.highlight_visible.get() {
                pass.set_pipeline(&self.highlight_pipeline);
                pass.set_bind_group(0, &eye.bind_group, &[]);
                pass.set_bind_group(1, &self.highlight.bind_group, &[]);
                pass.draw(0..6, 0..1);
            }
            if let Some(p) = particles {
                p.draw(&mut pass, i);
            }
            if let (Some(s), Some(draw)) = (scene.as_deref_mut(), world_draw) {
                s.draw_world(&mut pass, draw);
            }
            let ghosts = self.ghost_count.get();
            if ghosts > 0 {
                pass.set_pipeline(&self.ghost_pipeline);
                pass.set_bind_group(0, &eye.bind_group, &[]);
                pass.set_bind_group(1, &self.ghost.bind_group, &[]);
                pass.draw(0..6 * ghosts, 0..1);
            }
            if self.beam_visible.get().iter().any(|&v| v) {
                pass.set_pipeline(&self.beam_pipeline);
                pass.set_bind_group(0, &eye.bind_group, &[]);
                pass.set_bind_group(1, &self.beam.bind_group, &[]);
                pass.draw(0..6 * BEAM_SLOTS as u32, 0..1);
            }
            // Last, over the sprites (which write no depth), so the panel
            // reads cleanly through the cloud; depth-tested against the
            // hand occluders, so a finger poking it shows in front. The
            // room editor's label right after it, on the same terms.
            for quad in [&self.panel, &self.label] {
                if let Some(b) = quad.binding.as_ref().filter(|_| quad.visible.get()) {
                    pass.set_pipeline(&self.panel_pipeline);
                    pass.set_bind_group(0, &eye.bind_group, &[]);
                    pass.set_bind_group(1, &b.bind_group, &[]);
                    pass.draw(0..6, 0..1);
                }
            }
        }
        self.queue.submit([encoder.finish()]);
        atlas
    }
}

fn record_limits(l: &wgpu::Limits) -> RecordedLimits {
    let rec = RecordedLimits {
        max_storage_buffers_per_shader_stage: l.max_storage_buffers_per_shader_stage,
        max_bind_groups: l.max_bind_groups,
        max_storage_buffer_binding_size: l.max_storage_buffer_binding_size,
        max_buffer_size: l.max_buffer_size,
    };
    info!(
        "adapter limits: max_storage_buffers_per_shader_stage {} · max_bind_groups {} · max_storage_buffer_binding_size {} · max_buffer_size {} · max_texture_dimension_2d {} · max_compute_workgroup_storage_size {} · max_compute_invocations_per_workgroup {} · max_compute_workgroups_per_dimension {} · max_uniform_buffer_binding_size {} · max_push_constant_size {}",
        l.max_storage_buffers_per_shader_stage,
        l.max_bind_groups,
        l.max_storage_buffer_binding_size,
        l.max_buffer_size,
        l.max_texture_dimension_2d,
        l.max_compute_workgroup_storage_size,
        l.max_compute_invocations_per_workgroup,
        l.max_compute_workgroups_per_dimension,
        l.max_uniform_buffer_binding_size,
        l.max_push_constant_size,
    );
    if rec.max_storage_buffers_per_shader_stage < REQUIRED_STORAGE_BUFFERS_PER_STAGE
        || rec.max_bind_groups < REQUIRED_BIND_GROUPS
    {
        // The S2 gate: below this the core's pipeline layouts have to change.
        warn!(
            "ADAPTER LIMITS BELOW CORE REQUIREMENTS: storage buffers/stage {} (need {}), bind groups {} (need {})",
            rec.max_storage_buffers_per_shader_stage,
            REQUIRED_STORAGE_BUFFERS_PER_STAGE,
            rec.max_bind_groups,
            REQUIRED_BIND_GROUPS
        );
    }
    rec
}

const QUAD_WGSL: &str = r"
struct Eye { view_proj: mat4x4<f32> }
struct Quad { center: vec4<f32>, half: vec4<f32> }
@group(0) @binding(0) var<uniform> eye: Eye;
@group(1) @binding(0) var<uniform> quad: Quad;
@group(1) @binding(1) var quad_tex: texture_2d<f32>;
@group(1) @binding(2) var quad_samp: sampler;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

// Two triangles, corners in the quad's own -1..1 space; it faces -Z.
@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VsOut {
    var c = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0),
    );
    let corner = c[i];
    let world = quad.center.xyz + vec3<f32>(corner.x * quad.half.x, corner.y * quad.half.y, 0.0);
    var out: VsOut;
    out.pos = eye.view_proj * vec4<f32>(world, 1.0);
    out.uv = vec2<f32>(corner.x * 0.5 + 0.5, 0.5 - corner.y * 0.5);
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(textureSample(quad_tex, quad_samp, in.uv).rgb, 1.0);
}
";

fn build_quad_pipeline(
    device: &wgpu::Device,
) -> (
    wgpu::RenderPipeline,
    Vec<EyeUniform>,
    wgpu::BindGroupLayout,
    wgpu::BindGroupLayout,
) {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("xr-quad"),
        source: wgpu::ShaderSource::Wgsl(QUAD_WGSL.into()),
    });
    let eye_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("xr-eye-uniform"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: wgpu::BufferSize::new(64),
            },
            count: None,
        }],
    });
    let quad_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("xr-quad"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(32),
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("xr-quad"),
        bind_group_layouts: &[&eye_layout, &quad_layout],
        push_constant_ranges: &[],
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("xr-quad"),
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
        // The quad writes depth so S5 particles behind it are hidden.
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: true,
            depth_compare: wgpu::CompareFunction::Less,
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
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview: None,
        cache: None,
    });
    let eyes = (0..crate::xr::EYE_COUNT)
        .map(|i| {
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("xr-eye-view-proj"),
                size: 64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(if i == 0 { "xr-eye-0" } else { "xr-eye-1" }),
                layout: &eye_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                }],
            });
            EyeUniform { buffer, bind_group }
        })
        .collect();
    (pipeline, eyes, quad_layout, eye_layout)
}

const PANEL_WGSL: &str = r"
struct Eye { view_proj: mat4x4<f32> }
// center.w: the fraction of the texture's height shown, from the top.
struct Panel { center: vec4<f32>, right: vec4<f32>, up: vec4<f32> }
@group(0) @binding(0) var<uniform> eye: Eye;
@group(1) @binding(0) var<uniform> panel: Panel;
@group(1) @binding(1) var panel_tex: texture_2d<f32>;
@group(1) @binding(2) var panel_samp: sampler;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VsOut {
    var c = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0),
    );
    let corner = c[i];
    let world = panel.center.xyz + corner.x * panel.right.xyz + corner.y * panel.up.xyz;
    var out: VsOut;
    out.pos = eye.view_proj * vec4<f32>(world, 1.0);
    out.uv = vec2<f32>(corner.x * 0.5 + 0.5, (0.5 - corner.y * 0.5) * panel.center.w);
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // egui renders premultiplied alpha; the eye pass blends it over.
    return textureSample(panel_tex, panel_samp, in.uv);
}
";

/// The beams' shader. `BEAM_SLOTS` in it is replaced by the constant's
/// value when the pipeline is built, so the array and the uniform buffer
/// are the same size.
const BEAM_WGSL: &str = r"
struct Eye { view_proj: mat4x4<f32> }
struct Beam { corners: array<vec4<f32>, 4>, color: vec4<f32> }
@group(0) @binding(0) var<uniform> eye: Eye;
@group(1) @binding(0) var<uniform> beams: array<Beam, BEAM_SLOTS>;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) across: f32,
    @location(1) along: f32,
    @location(2) @interpolate(flat) color: vec4<f32>,
}

// Six vertices per beam slot; a hidden slot is all zeros (no area).
@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VsOut {
    var order = array<u32, 6>(0u, 1u, 2u, 0u, 2u, 3u);
    let beam = beams[i / 6u];
    let k = order[i % 6u];
    let c = beam.corners[k];
    var out: VsOut;
    out.pos = eye.view_proj * vec4<f32>(c.xyz, 1.0);
    out.across = c.w;
    out.along = select(0.0, 1.0, k >= 2u);
    out.color = beam.color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // Soft across the width, fading in from the start toward the end.
    let a = in.color.a * (1.0 - in.across * in.across) * (0.35 + 0.65 * in.along);
    return vec4<f32>(in.color.rgb * a, a);
}
";

/// The aiming beam's pipeline and its uniform: a camera-facing strip,
/// premultiplied alpha, depth-tested (a hand or the panel hides it), no
/// depth write.
fn build_beam_pipeline(
    device: &wgpu::Device,
    eye_layout: &wgpu::BindGroupLayout,
) -> (wgpu::RenderPipeline, QuadBinding) {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("xr-beam"),
        source: wgpu::ShaderSource::Wgsl(
            BEAM_WGSL
                .replace("BEAM_SLOTS", &BEAM_SLOTS.to_string())
                .into(),
        ),
    });
    let beam_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("xr-beam"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: wgpu::BufferSize::new(BEAM_BYTES * BEAM_SLOTS as u64),
            },
            count: None,
        }],
    });
    let uniform = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("xr-beam-uniform"),
        size: BEAM_BYTES * BEAM_SLOTS as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("xr-beam"),
        layout: &beam_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: uniform.as_entire_binding(),
        }],
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("xr-beam"),
        bind_group_layouts: &[eye_layout, &beam_layout],
        push_constant_ranges: &[],
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("xr-beam"),
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
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: false,
            depth_compare: wgpu::CompareFunction::Less,
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
                blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview: None,
        cache: None,
    });
    (
        pipeline,
        QuadBinding {
            uniform,
            bind_group,
        },
    )
}

/// A lit surface quad's pipeline and its uniform of `rows` vec4s: the
/// wall spectrum (`canvas.rs`) and the room editor's highlight
/// (`highlight.rs`); [`build_surface_slots`] with one slot. The beam's setup
/// (premultiplied alpha, depth-tested, no depth write, no culling) plus a
/// depth bias toward the camera. The surface's occluder writes depth at
/// its face; the quad sits `LIFT_M` off it, and the bias keeps it in
/// front at grazing angles, where the lift is worth little depth, the
/// usual decal setup. `wgsl` has `vs_main` / `fs_main` with the
/// eye camera at group 0 and the uniform at group 1.
fn build_surface_pipeline(
    device: &wgpu::Device,
    eye_layout: &wgpu::BindGroupLayout,
    wgsl: &str,
    label: &str,
    rows: usize,
) -> (wgpu::RenderPipeline, QuadBinding) {
    let (pipeline, mut slots) = build_surface_slots(device, eye_layout, wgsl, label, rows, 1);
    (pipeline, slots.swap_remove(0))
}

/// [`build_surface_pipeline`]'s pipeline with `slots` uniforms of `rows`
/// vec4s, each with its own bind group on the one layout: the surfaces
/// pass (`surface_fx.rs`) binds one per lit face and draws it.
fn build_surface_slots(
    device: &wgpu::Device,
    eye_layout: &wgpu::BindGroupLayout,
    wgsl: &str,
    label: &str,
    rows: usize,
    slots: usize,
) -> (wgpu::RenderPipeline, Vec<QuadBinding>) {
    let bytes = 16 * rows as u64;
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(wgsl.into()),
    });
    let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(label),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: wgpu::BufferSize::new(bytes),
            },
            count: None,
        }],
    });
    let bindings = (0..slots)
        .map(|_| {
            let uniform = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: bytes,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &uniform_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                }],
            });
            QuadBinding {
                uniform,
                bind_group,
            }
        })
        .collect();
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &[eye_layout, &uniform_layout],
        push_constant_ranges: &[],
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
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
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: false,
            depth_compare: wgpu::CompareFunction::Less,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState {
                constant: SURFACE_DEPTH_BIAS,
                slope_scale: SURFACE_DEPTH_SLOPE_BIAS,
                clamp: 0.0,
            },
        }),
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: SWAPCHAIN_FORMAT,
                blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview: None,
        cache: None,
    });
    (pipeline, bindings)
}

/// The surface quads' depth bias toward the camera (negative: nearer), in
/// the depth format's minimum resolvable units and per unit of depth slope.
const SURFACE_DEPTH_BIAS: i32 = -16;
const SURFACE_DEPTH_SLOPE_BIAS: f32 = -2.0;

const GHOST_WGSL: &str = r"
struct Eye { view_proj: mat4x4<f32> }
struct Ghost { eye: vec4<f32>, color: vec4<f32>, points: array<vec4<f32>, 64> }
@group(0) @binding(0) var<uniform> eye: Eye;
@group(1) @binding(0) var<uniform> ghost: Ghost;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) corner: vec2<f32>,
}

// Six vertices per sprite: a quad facing the eye, `points[i].w` in radius.
@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VsOut {
    var c = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0),
    );
    let p = ghost.points[i / 6u];
    let corner = c[i % 6u];
    let to_eye = normalize(ghost.eye.xyz - p.xyz);
    let right = normalize(cross(vec3<f32>(0.0, 1.0, 0.0), to_eye));
    let up = cross(to_eye, right);
    let world = p.xyz + (corner.x * right + corner.y * up) * p.w;
    var out: VsOut;
    out.pos = eye.view_proj * vec4<f32>(world, 1.0);
    out.corner = corner;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let r = length(in.corner);
    if (r > 1.0) {
        discard;
    }
    let a = ghost.color.a * (1.0 - smoothstep(0.5, 1.0, r));
    return vec4<f32>(ghost.color.rgb * a, a);
}
";

/// The ghost sprites' pipeline and uniform: soft camera-facing discs,
/// premultiplied alpha, depth-tested (real objects hide them), no depth
/// write.
fn build_ghost_pipeline(
    device: &wgpu::Device,
    eye_layout: &wgpu::BindGroupLayout,
) -> (wgpu::RenderPipeline, QuadBinding) {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("xr-ghost"),
        source: wgpu::ShaderSource::Wgsl(GHOST_WGSL.into()),
    });
    let ghost_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("xr-ghost"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: wgpu::BufferSize::new(GHOST_BYTES),
            },
            count: None,
        }],
    });
    let uniform = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("xr-ghost-uniform"),
        size: GHOST_BYTES,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("xr-ghost"),
        layout: &ghost_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: uniform.as_entire_binding(),
        }],
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("xr-ghost"),
        bind_group_layouts: &[eye_layout, &ghost_layout],
        push_constant_ranges: &[],
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("xr-ghost"),
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
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: false,
            depth_compare: wgpu::CompareFunction::Less,
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
                blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview: None,
        cache: None,
    });
    (
        pipeline,
        QuadBinding {
            uniform,
            bind_group,
        },
    )
}

/// The debug panel's pipeline (the room editor's label's too): a posed
/// quad, premultiplied alpha over the eye target, depth-tested and
/// depth-writing.
fn build_panel_pipeline(
    device: &wgpu::Device,
    eye_layout: &wgpu::BindGroupLayout,
) -> (wgpu::RenderPipeline, wgpu::BindGroupLayout) {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("xr-panel"),
        source: wgpu::ShaderSource::Wgsl(PANEL_WGSL.into()),
    });
    let panel_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("xr-panel"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(48),
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("xr-panel"),
        bind_group_layouts: &[eye_layout, &panel_layout],
        push_constant_ranges: &[],
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("xr-panel"),
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
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: true,
            depth_compare: wgpu::CompareFunction::Less,
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
                blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview: None,
        cache: None,
    });
    (pipeline, panel_layout)
}
