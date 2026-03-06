use anyhow::Result;
use std::sync::Arc;
use wgpu::{
    Adapter, Device, DeviceDescriptor, ExperimentalFeatures, Instance, InstanceDescriptor,
    MemoryHints, PowerPreference, Queue, RequestAdapterOptions, Surface, SurfaceConfiguration,
    TextureFormat, TextureUsages, Trace,
};
use winit::window::Window;

/// Path for persisted pipeline cache data.
fn pipeline_cache_path() -> Option<std::path::PathBuf> {
    dirs::config_dir().map(|d| d.join("phosphor").join("pipeline_cache.bin"))
}

pub struct GpuContext {
    pub instance: Instance,
    pub adapter: Adapter,
    pub device: Device,
    pub queue: Queue,
    pub surface: Surface<'static>,
    pub surface_config: SurfaceConfiguration,
    pub format: TextureFormat,
    /// Pipeline cache for faster shader compilation on subsequent launches.
    pub pipeline_cache: Option<wgpu::PipelineCache>,
    /// Set to true when the GPU device is lost (driver crash/reset).
    pub device_lost: Arc<std::sync::atomic::AtomicBool>,
}

impl GpuContext {
    pub fn new(window: Arc<Window>) -> Result<Self> {
        let instance = Instance::new(&InstanceDescriptor::default());

        let surface = instance.create_surface(window.clone())?;

        let adapter = pollster::block_on(instance.request_adapter(&RequestAdapterOptions {
            power_preference: PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }))?;

        let mut required_features = wgpu::Features::empty();

        // Request pipeline cache if supported
        let supported = adapter.features();
        if supported.contains(wgpu::Features::PIPELINE_CACHE) {
            required_features |= wgpu::Features::PIPELINE_CACHE;
        }

        #[cfg(feature = "profiling")]
        {
            // Request timestamp queries if the adapter supports them
            if supported.contains(wgpu::Features::TIMESTAMP_QUERY) {
                required_features |= wgpu::Features::TIMESTAMP_QUERY;
            }
            if supported.contains(wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS) {
                required_features |= wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS;
            }
            if supported.contains(wgpu::Features::TIMESTAMP_QUERY_INSIDE_PASSES) {
                required_features |= wgpu::Features::TIMESTAMP_QUERY_INSIDE_PASSES;
            }
        }

        let adapter_limits = adapter.limits();
        let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
            label: Some("phosphor-device"),
            required_features,
            required_limits: wgpu::Limits {
                max_storage_buffers_per_shader_stage: 16,
                max_bind_groups: 5, // groups 0-3 standard + group 4 for R-D texture
                max_storage_buffer_binding_size: adapter_limits.max_storage_buffer_binding_size,
                max_buffer_size: adapter_limits.max_buffer_size,
                ..wgpu::Limits::default()
            },
            experimental_features: ExperimentalFeatures::default(),
            memory_hints: MemoryHints::Performance,
            trace: Trace::Off,
        }))?;

        // Set up error and device loss handlers
        let device_lost = Arc::new(std::sync::atomic::AtomicBool::new(false));
        {
            let lost = device_lost.clone();
            device.set_device_lost_callback(move |reason, msg| {
                log::error!("GPU device lost ({reason:?}): {msg}");
                lost.store(true, std::sync::atomic::Ordering::SeqCst);
            });
        }
        device.on_uncaptured_error(Arc::new(|error| {
            log::error!("Uncaptured GPU error: {error}");
        }));

        // Create pipeline cache (load from disk if available)
        let pipeline_cache =
            if device.features().contains(wgpu::Features::PIPELINE_CACHE) {
                let cached_data = pipeline_cache_path()
                    .and_then(|p| std::fs::read(p).ok());
                let cache = unsafe {
                    device.create_pipeline_cache(&wgpu::PipelineCacheDescriptor {
                        label: Some("phosphor-pipeline-cache"),
                        data: cached_data.as_deref(),
                        fallback: true,
                    })
                };
                log::info!("Pipeline cache created (loaded {} bytes from disk)",
                    cached_data.as_ref().map_or(0, |d| d.len()));
                Some(cache)
            } else {
                log::info!("Pipeline cache not supported by adapter");
                None
            };

        let size = window.inner_size();
        let capabilities = surface.get_capabilities(&adapter);
        let format = capabilities
            .formats
            .iter()
            .find(|f| f.is_srgb())
            .copied()
            .unwrap_or(capabilities.formats[0]);

        let present_mode = wgpu::PresentMode::AutoVsync;

        let surface_config = SurfaceConfiguration {
            usage: TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode,
            desired_maximum_frame_latency: 2,
            alpha_mode: capabilities.alpha_modes[0],
            view_formats: vec![],
        };
        surface.configure(&device, &surface_config);

        log::info!(
            "GPU initialized: {} ({:?}), present mode: {:?} (available: {:?})",
            adapter.get_info().name,
            adapter.get_info().backend,
            present_mode,
            capabilities.present_modes,
        );

        Ok(Self {
            instance,
            adapter,
            device,
            queue,
            surface,
            surface_config,
            format,
            pipeline_cache,
            device_lost,
        })
    }

    /// HDR intermediate format for render targets (16-bit float for bloom/feedback).
    pub fn hdr_format() -> TextureFormat {
        TextureFormat::Rgba16Float
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width > 0 && height > 0 {
            self.surface_config.width = width;
            self.surface_config.height = height;
            self.surface.configure(&self.device, &self.surface_config);
        }
    }

    /// Save pipeline cache data to disk for faster startup next time.
    pub fn save_pipeline_cache(&self) {
        if let Some(ref cache) = self.pipeline_cache {
            if let Some(data) = cache.get_data() {
                if let Some(path) = pipeline_cache_path() {
                    if let Some(parent) = path.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    match std::fs::write(&path, &data) {
                        Ok(()) => log::info!(
                            "Pipeline cache saved ({} bytes) to {}",
                            data.len(),
                            path.display()
                        ),
                        Err(e) => log::warn!("Failed to save pipeline cache: {e}"),
                    }
                }
            }
        }
    }
}
