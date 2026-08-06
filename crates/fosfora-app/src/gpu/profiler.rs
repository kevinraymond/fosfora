//! GPU profiling via wgpu-profiler (feature-gated behind `profiling`).
//!
//! Wraps `wgpu_profiler::GpuProfiler` and provides an egui overlay panel
//! showing per-scope GPU timing.

use wgpu_profiler::{GpuProfiler, GpuProfilerSettings};

pub struct Profiler {
    pub inner: GpuProfiler,
    /// Latest completed frame timings (scope_name, duration_ms).
    pub latest_timings: Vec<(String, f64)>,
}

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
