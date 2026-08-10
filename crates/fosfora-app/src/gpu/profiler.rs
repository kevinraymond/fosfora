//! GPU profiling via wgpu-profiler (feature-gated behind `profiling`).
//!
//! Wraps `wgpu_profiler::GpuProfiler` and provides an egui overlay panel
//! showing per-scope GPU timing. The [`ProfilerHandle`]/[`ProfilerScope`]
//! pair at the bottom compiles with or without the feature, so render paths
//! (frame graph, trama executor) take a handle parameter instead of
//! sprouting `#[cfg]`d signatures — without the feature both are zero-sized
//! no-ops and the optimizer erases them.

#[cfg(feature = "profiling")]
use wgpu_profiler::{GpuProfiler, GpuProfilerSettings};

#[cfg(feature = "profiling")]
pub struct Profiler {
    pub inner: GpuProfiler,
    /// Latest completed frame timings (scope_name, duration_ms).
    pub latest_timings: Vec<(String, f64)>,
}

#[cfg(feature = "profiling")]
impl Profiler {
    pub fn new(device: &wgpu::Device) -> Self {
        let inner = GpuProfiler::new(device, GpuProfilerSettings::default())
            .expect("failed to create GPU profiler");
        Self {
            inner,
            latest_timings: Vec::new(),
        }
    }

    /// Call after queue.submit() to finalize the frame and poll results.
    pub fn end_frame(&mut self, queue: &wgpu::Queue) {
        self.inner.end_frame().ok();
        if let Some(results) = self
            .inner
            .process_finished_frame(queue.get_timestamp_period())
        {
            self.latest_timings.clear();
            flatten_results(&results, 0, &mut self.latest_timings);
        }
    }

    /// Render the profiling panel into egui.
    pub fn ui(&self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new("GPU Timings").strong().size(14.0));
        if self.latest_timings.is_empty() {
            ui.label("No GPU timing data (timestamps may not be supported)");
            return;
        }
        egui::Grid::new("gpu_profiler_grid")
            .num_columns(2)
            .spacing([20.0, 2.0])
            .show(ui, |ui| {
                for (name, ms) in &self.latest_timings {
                    ui.label(name);
                    ui.label(format!("{ms:.2} ms"));
                    ui.end_row();
                }
            });
    }
}

/// Flatten nested profiling results into a flat list with indentation.
#[cfg(feature = "profiling")]
fn flatten_results(
    results: &[wgpu_profiler::GpuTimerQueryResult],
    depth: usize,
    out: &mut Vec<(String, f64)>,
) {
    for r in results {
        let indent = "  ".repeat(depth);
        if let Some(ref time) = r.time {
            let duration_ms = (time.end - time.start) * 1000.0;
            out.push((format!("{indent}{}", r.label), duration_ms));
        }
        // Still recurse into nested queries even if this scope has no timing
        flatten_results(&r.nested_queries, depth + 1, out);
    }
}

/// Borrowed profiler for render paths — always compiled, so `execute` chains
/// stay cfg-free. `scope()` opens a named encoder-level timing scope that
/// closes when the returned guard drops; encode through the guard's
/// [`ProfilerScope::encoder`]. Without the `profiling` feature (or with a
/// [`ProfilerHandle::none`] handle) this is a free passthrough.
#[derive(Clone, Copy)]
pub struct ProfilerHandle<'a> {
    #[cfg(feature = "profiling")]
    inner: Option<&'a GpuProfiler>,
    #[cfg(not(feature = "profiling"))]
    _marker: std::marker::PhantomData<&'a ()>,
}

impl<'a> ProfilerHandle<'a> {
    /// A handle that records nothing (headless renderers, tests).
    pub fn none() -> Self {
        Self {
            #[cfg(feature = "profiling")]
            inner: None,
            #[cfg(not(feature = "profiling"))]
            _marker: std::marker::PhantomData,
        }
    }

    #[cfg(feature = "profiling")]
    pub fn some(profiler: &'a GpuProfiler) -> Self {
        Self {
            inner: Some(profiler),
        }
    }

    /// Open a timing scope on `encoder`. Scopes opened while another is
    /// alive nest under it in the profiler panel (wgpu-profiler tracks the
    /// open-query stack). Label allocation only happens when a profiler is
    /// actually attached, so I8's steady-state clause is untouched in
    /// ordinary builds.
    pub fn scope<'e>(&self, label: &str, encoder: &'e mut wgpu::CommandEncoder) -> ProfilerScope<'e>
    where
        'a: 'e,
    {
        #[cfg(feature = "profiling")]
        {
            match self.inner {
                Some(p) => ProfilerScope {
                    scope: Some(p.scope(label, encoder)),
                    raw: None,
                },
                None => ProfilerScope {
                    scope: None,
                    raw: Some(encoder),
                },
            }
        }
        #[cfg(not(feature = "profiling"))]
        {
            let _ = label;
            ProfilerScope { raw: encoder }
        }
    }
}

/// Guard for one open timing scope; the query closes on drop.
pub struct ProfilerScope<'e> {
    #[cfg(feature = "profiling")]
    scope: Option<wgpu_profiler::Scope<'e, wgpu::CommandEncoder>>,
    #[cfg(feature = "profiling")]
    raw: Option<&'e mut wgpu::CommandEncoder>,
    #[cfg(not(feature = "profiling"))]
    raw: &'e mut wgpu::CommandEncoder,
}

impl ProfilerScope<'_> {
    /// The encoder to record this scope's work on.
    pub fn encoder(&mut self) -> &mut wgpu::CommandEncoder {
        #[cfg(feature = "profiling")]
        {
            match self.scope.as_mut() {
                Some(s) => s.recorder,
                None => self.raw.as_mut().expect("scope or raw, always one"),
            }
        }
        #[cfg(not(feature = "profiling"))]
        {
            self.raw
        }
    }
}
