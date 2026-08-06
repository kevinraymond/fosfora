use crossbeam_channel::{Receiver, Sender, unbounded};
use std::thread;
use wgpu::{
    BindGroupLayout, ComputePipeline, Device, PipelineCompilationOptions, PipelineLayoutDescriptor,
    TextureFormat,
};

use super::ShaderPipeline;

/// A request to compile a shader on a background thread.
pub enum CompileRequest {
    /// Compile a fragment shader into a full render pipeline.
    RenderPass {
        layer_idx: usize,
        pass_idx: usize,
        source: String,
        device: Device,
        format: TextureFormat,
        /// Multi-pass graph input count for this pass's bind-group layout (#1481).
        input_count: usize,
    },
    /// Compile a compute shader into a compute pipeline.
    ComputeShader {
        layer_idx: usize,
        source: String,
        device: Device,
        /// Cloned bind group layouts needed for the pipeline layout.
        bind_group_layouts: Vec<BindGroupLayout>,
    },
}

/// Result of a background shader compilation.
pub enum CompileResult {
    RenderPass {
        layer_idx: usize,
        pass_idx: usize,
        result: Result<ShaderPipeline, String>,
        source: String,
    },
    ComputeShader {
        layer_idx: usize,
        result: Result<ComputePipeline, String>,
        source: String,
    },
}

/// Background shader compiler. Shader compilation (WGSL parsing + naga validation +
/// backend pipeline creation) can take 50-500ms — this keeps it off the main thread
/// so the render loop doesn't hitch during hot-reload.
pub struct ShaderCompiler {
    request_tx: Sender<CompileRequest>,
    result_rx: Receiver<CompileResult>,
    _thread: thread::JoinHandle<()>,
}

impl ShaderCompiler {
    pub fn new() -> Self {
        let (request_tx, request_rx) = unbounded::<CompileRequest>();
        let (result_tx, result_rx) = unbounded::<CompileResult>();

        let thread = thread::Builder::new()
            .name("shader-compiler".into())
            .spawn(move || Self::worker(request_rx, result_tx))
            .expect("failed to spawn shader compiler thread");

        Self {
            request_tx,
            result_rx,
            _thread: thread,
        }
    }

    /// Submit a render pass compilation request.
    pub fn compile_render_pass(
        &self,
        layer_idx: usize,
        pass_idx: usize,
        source: String,
        device: &Device,
        format: TextureFormat,
        input_count: usize,
    ) {
        let _ = self.request_tx.send(CompileRequest::RenderPass {
            layer_idx,
            pass_idx,
            source,
            device: device.clone(),
            format,
            input_count,
        });
    }

    /// Submit a compute shader compilation request.
    pub fn compile_compute_shader(
        &self,
        layer_idx: usize,
        source: String,
        device: &Device,
        bind_group_layouts: Vec<BindGroupLayout>,
    ) {
        let _ = self.request_tx.send(CompileRequest::ComputeShader {
            layer_idx,
            source,
            device: device.clone(),
            bind_group_layouts,
        });
    }

    /// Drain all completed compilation results (non-blocking).
    pub fn drain_results(&self) -> Vec<CompileResult> {
        self.result_rx.try_iter().collect()
    }

    fn worker(rx: Receiver<CompileRequest>, tx: Sender<CompileResult>) {
        for request in rx {
            match request {
                CompileRequest::RenderPass {
                    layer_idx,
                    pass_idx,
                    source,
                    device,
                    format,
                    input_count,
                } => {
                    let result = ShaderPipeline::new(&device, format, &source, None, input_count)
                        .map_err(|e| e.to_string());
                    let _ = tx.send(CompileResult::RenderPass {
                        layer_idx,
                        pass_idx,
                        result,
                        source,
                    });
                }
                CompileRequest::ComputeShader {
                    layer_idx,
                    source,
                    device,
                    bind_group_layouts,
                } => {
                    let result = compile_compute_pipeline(&device, &source, &bind_group_layouts);
                    let _ = tx.send(CompileResult::ComputeShader {
                        layer_idx,
                        result,
                        source,
                    });
                }
            }
        }
    }
}

/// Compile a compute pipeline from WGSL source (runs on background thread).
fn compile_compute_pipeline(
    device: &Device,
    source: &str,
    bind_group_layouts: &[BindGroupLayout],
) -> Result<ComputePipeline, String> {
    device.push_error_scope(wgpu::ErrorFilter::Validation);

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("particle-compute-hotreload"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });

    let layout_refs: Vec<&BindGroupLayout> = bind_group_layouts.iter().collect();
    let layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
        label: Some("particle-compute-layout"),
        bind_group_layouts: &layout_refs,
        push_constant_ranges: &[],
    });

    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("particle-compute-pipeline"),
        layout: Some(&layout),
        module: &shader,
        entry_point: Some("cs_main"),
        compilation_options: PipelineCompilationOptions::default(),
        cache: None,
    });

    if let Some(error) = pollster::block_on(device.pop_error_scope()) {
        return Err(format!("{error}"));
    }

    Ok(pipeline)
}
