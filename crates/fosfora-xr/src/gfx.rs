//! Vulkan through `ash`, with the instance and device created by the OpenXR
//! runtime (`XR_KHR_vulkan_enable2`). S1 only clears swapchain images; S2
//! replaces this with wgpu on the same instance/device/queue.

use std::ffi::CStr;

use anyhow::{Context, Result, anyhow};
use ash::vk::{self, Handle as _};
use log::info;
use openxr as xr;

use crate::xr::XrContext;

/// Command buffers / fences in rotation. Two is enough: xrWaitFrame already
/// throttles the CPU to one frame ahead of the compositor.
pub const FRAMES_IN_FLIGHT: usize = 2;

pub struct Gfx {
    _entry: ash::Entry,
    pub instance: ash::Instance,
    pub physical_device: vk::PhysicalDevice,
    pub device: ash::Device,
    pub queue_family_index: u32,
    pub queue: vk::Queue,
    command_pool: vk::CommandPool,
    command_buffers: Vec<vk::CommandBuffer>,
    fences: Vec<vk::Fence>,
}

impl Gfx {
    pub fn new(ctx: &XrContext) -> Result<Self> {
        // SAFETY: loads the system `libvulkan.so`, Android's Vulkan loader.
        let entry = unsafe { ash::Entry::load() }.context("loading libvulkan.so")?;
        // openxr-sys and ash spell the `vkGetInstanceProcAddr` pointer type
        // differently; they are the same C signature.
        // SAFETY: both types are `unsafe extern "system" fn(VkInstance, *const c_char) -> PFN_vkVoidFunction`.
        let get_instance_proc_addr: xr::sys::platform::VkGetInstanceProcAddr =
            unsafe { std::mem::transmute(entry.static_fn().get_instance_proc_addr) };

        let app_info = vk::ApplicationInfo::default()
            .application_name(c"Fosfora VR")
            .application_version(1)
            .engine_name(c"fosfora")
            .engine_version(1)
            .api_version(vk::make_api_version(0, 1, 1, 0));
        let instance_info = vk::InstanceCreateInfo::default().application_info(&app_info);
        // SAFETY: `instance_info` is a complete, valid VkInstanceCreateInfo that
        // outlives the call; the runtime adds the extensions it needs.
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
        // SAFETY: `raw_instance` is a live VkInstance created with this entry's
        // `vkGetInstanceProcAddr`.
        let instance = unsafe {
            ash::Instance::load(
                entry.static_fn(),
                vk::Instance::from_raw(raw_instance as u64),
            )
        };

        // SAFETY: `instance` was created through `create_vulkan_instance` above.
        let physical_device = vk::PhysicalDevice::from_raw(
            unsafe {
                ctx.instance
                    .vulkan_graphics_device(ctx.system, instance.handle().as_raw() as _)
            }
            .context("xrGetVulkanGraphicsDevice2KHR")? as u64,
        );
        // SAFETY: `physical_device` belongs to `instance`.
        let props = unsafe { instance.get_physical_device_properties(physical_device) };
        // SAFETY: `device_name` is a NUL-terminated fixed array filled by the driver.
        let name = unsafe { CStr::from_ptr(props.device_name.as_ptr()) }.to_string_lossy();
        info!(
            "vulkan device: {name} · api {}.{}.{} · driver {:#x} · vendor {:#x} device {:#x}",
            vk::api_version_major(props.api_version),
            vk::api_version_minor(props.api_version),
            vk::api_version_patch(props.api_version),
            props.driver_version,
            props.vendor_id,
            props.device_id,
        );

        // SAFETY: `physical_device` belongs to `instance`.
        let families =
            unsafe { instance.get_physical_device_queue_family_properties(physical_device) };
        let queue_family_index = families
            .iter()
            .position(|f| f.queue_flags.contains(vk::QueueFlags::GRAPHICS))
            .ok_or_else(|| anyhow!("no graphics queue family"))?
            as u32;

        let priorities = [1.0f32];
        let queue_infos = [vk::DeviceQueueCreateInfo::default()
            .queue_family_index(queue_family_index)
            .queue_priorities(&priorities)];
        let device_info = vk::DeviceCreateInfo::default().queue_create_infos(&queue_infos);
        // SAFETY: `device_info` is complete and valid for the duration of the
        // call, and `physical_device` is the one the runtime selected.
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
        // SAFETY: `raw_device` is a live VkDevice created from `instance`.
        let device = unsafe {
            ash::Device::load(instance.fp_v1_0(), vk::Device::from_raw(raw_device as u64))
        };
        // SAFETY: queue family `queue_family_index` with one queue was requested above.
        let queue = unsafe { device.get_device_queue(queue_family_index, 0) };

        // SAFETY: plain object creation on a live device; the pool is destroyed in Drop.
        let command_pool = unsafe {
            device.create_command_pool(
                &vk::CommandPoolCreateInfo::default()
                    .queue_family_index(queue_family_index)
                    .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
                None,
            )
        }
        .context("vkCreateCommandPool")?;
        // SAFETY: `command_pool` is live; the buffers are freed with it.
        let command_buffers = unsafe {
            device.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(command_pool)
                    .command_buffer_count(FRAMES_IN_FLIGHT as u32),
            )
        }
        .context("vkAllocateCommandBuffers")?;
        let fences = (0..FRAMES_IN_FLIGHT)
            .map(|_| {
                // SAFETY: plain object creation on a live device; destroyed in Drop.
                unsafe {
                    device.create_fence(
                        &vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED),
                        None,
                    )
                }
                .context("vkCreateFence")
            })
            .collect::<Result<Vec<_>>>()?;

        Ok(Self {
            _entry: entry,
            instance,
            physical_device,
            device,
            queue_family_index,
            queue,
            command_pool,
            command_buffers,
            fences,
        })
    }

    /// Wait for slot `slot`'s previous submission, then start recording into
    /// its command buffer.
    pub fn begin_commands(&self, slot: usize) -> Result<vk::CommandBuffer> {
        let fence = self.fences[slot];
        let cmd = self.command_buffers[slot];
        // SAFETY: `fence` and `cmd` belong to this device; the fence was either
        // created signaled or signaled by the submission of this slot, so
        // resetting and re-recording cannot race the GPU.
        unsafe {
            self.device
                .wait_for_fences(&[fence], true, u64::MAX)
                .context("vkWaitForFences")?;
            self.device
                .reset_fences(&[fence])
                .context("vkResetFences")?;
            self.device
                .begin_command_buffer(
                    cmd,
                    &vk::CommandBufferBeginInfo::default()
                        .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                )
                .context("vkBeginCommandBuffer")?;
        }
        Ok(cmd)
    }

    /// Clear a swapchain image and leave it in `COLOR_ATTACHMENT_OPTIMAL`, the
    /// layout OpenXR requires at `xrReleaseSwapchainImage`.
    pub fn record_clear(&self, cmd: vk::CommandBuffer, image: vk::Image, color: [f32; 4]) {
        let range = vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 1,
        };
        let to_transfer = vk::ImageMemoryBarrier::default()
            .image(image)
            .subresource_range(range)
            // Contents are discarded: UNDEFINED is valid whatever layout the
            // compositor left the image in.
            .old_layout(vk::ImageLayout::UNDEFINED)
            .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .src_access_mask(vk::AccessFlags::empty())
            .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED);
        let to_attachment = vk::ImageMemoryBarrier::default()
            .image(image)
            .subresource_range(range)
            .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .new_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
            .dst_access_mask(
                vk::AccessFlags::COLOR_ATTACHMENT_READ | vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
            )
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED);
        // SAFETY: `cmd` is in the recording state (from `begin_commands`) and
        // `image` is a swapchain image the runtime handed us for this frame
        // (acquired and waited on) with TRANSFER_DST usage.
        unsafe {
            self.device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[to_transfer],
            );
            self.device.cmd_clear_color_image(
                cmd,
                image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &vk::ClearColorValue { float32: color },
                &[range],
            );
            self.device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[to_attachment],
            );
        }
    }

    /// End recording and submit on the queue the OpenXR session was bound to.
    pub fn submit(&self, cmd: vk::CommandBuffer, slot: usize) -> Result<()> {
        let cmds = [cmd];
        let submit = vk::SubmitInfo::default().command_buffers(&cmds);
        // SAFETY: `cmd` was recorded by `begin_commands` / `record_clear` in
        // this slot and the slot's fence was reset in `begin_commands`.
        unsafe {
            self.device
                .end_command_buffer(cmd)
                .context("vkEndCommandBuffer")?;
            self.device
                .queue_submit(self.queue, &[submit], self.fences[slot])
                .context("vkQueueSubmit")?;
        }
        Ok(())
    }
}

impl Drop for Gfx {
    fn drop(&mut self) {
        // SAFETY: the session (and with it every swapchain image) is already
        // gone (drop order in `app::run_inner`); after the idle wait nothing
        // references these objects.
        unsafe {
            if let Err(e) = self.device.device_wait_idle() {
                log::warn!("vkDeviceWaitIdle: {e:?}");
            }
            for fence in self.fences.drain(..) {
                self.device.destroy_fence(fence, None);
            }
            self.device.destroy_command_pool(self.command_pool, None);
            self.device.destroy_device(None);
            self.instance.destroy_instance(None);
        }
        info!("vulkan device destroyed");
    }
}
