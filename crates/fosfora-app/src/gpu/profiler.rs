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

/// How long timings accumulate before the panel's numbers change.
#[cfg(feature = "profiling")]
const WINDOW: std::time::Duration = std::time::Duration::from_millis(500);

/// Log the published table every this many windows (5 s), so a reading can be
/// taken off the log without anyone transcribing a moving panel.
#[cfg(feature = "profiling")]
const LOG_EVERY: u32 = 10;

#[cfg(feature = "profiling")]
pub struct Profiler {
    pub inner: GpuProfiler,
    /// The last published window: `(indented scope name, mean ms per frame)`.
    pub latest_timings: Vec<(String, f64)>,
    /// Frames per second over the last published window.
    pub latest_fps: f64,
    window: TimingWindow,
    windows_published: u32,
}

#[cfg(feature = "profiling")]
impl Profiler {
    pub fn new(device: &wgpu::Device) -> Self {
        let inner = GpuProfiler::new(device, GpuProfilerSettings::default())
            .expect("failed to create GPU profiler");
        Self {
            inner,
            latest_timings: Vec::new(),
            latest_fps: 0.0,
            window: TimingWindow::new(),
            windows_published: 0,
        }
    }

    /// Call after queue.submit() to finalize the frame and poll results.
    pub fn end_frame(&mut self, queue: &wgpu::Queue) {
        self.inner.end_frame().ok();
        if let Some(results) = self
            .inner
            .process_finished_frame(queue.get_timestamp_period())
        {
            let mut frame = Vec::new();
            flatten_results(&results, 0, "", &mut frame);
            self.window.add_frame(frame);
        }
        let elapsed = self.window.started.elapsed();
        if elapsed < WINDOW || self.window.frames == 0 {
            return;
        }
        self.latest_fps = f64::from(self.window.frames) / elapsed.as_secs_f64();
        self.latest_timings = self.window.publish();
        self.windows_published += 1;
        if self.windows_published.is_multiple_of(LOG_EVERY) {
            let table: Vec<String> = self
                .latest_timings
                .iter()
                .map(|(name, ms)| format!("{} {ms:.2}", name.trim_start()))
                .collect();
            log::info!(
                "gpu timings, mean ms/frame at {:.1} fps: {}",
                self.latest_fps,
                table.join(" | ")
            );
        }
    }

    /// Render the profiling panel into egui.
    ///
    /// Every number is a mean over the last half second, in a fixed-width
    /// column, and a scope keeps its row for the whole window even if it only
    /// ran on some frames. Raw per-frame values in a proportional font made the
    /// auto-sized window change width on every frame, and a scope that fires
    /// one frame in three (the trama preview blit) made it change HEIGHT twenty
    /// times a second — it could not be read.
    pub fn ui(&self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new("GPU Timings").strong().size(14.0));
        if self.latest_timings.is_empty() {
            ui.label("No GPU timing data (timestamps may not be supported)");
            return;
        }
        ui.label(
            egui::RichText::new(format!(
                "mean ms per frame, 0.5 s window   {:>6.1} fps",
                self.latest_fps
            ))
            .monospace(),
        );
        let width = self
            .latest_timings
            .iter()
            .map(|(name, _)| name.chars().count())
            .max()
            .unwrap_or(0);
        for (name, ms) in &self.latest_timings {
            ui.label(egui::RichText::new(format!("{name:<width$} {ms:>8.2} ms")).monospace());
        }
    }
}

/// One scope's reading on one frame: a key that is stable across frames, the
/// indented name to display, and the duration.
#[cfg(feature = "profiling")]
type ScopeReading = (String, String, f64);

/// Timings accumulated over one display window.
#[cfg(feature = "profiling")]
struct TimingWindow {
    started: std::time::Instant,
    frames: u32,
    /// `(key, display name, summed ms)`, kept in tree order.
    rows: Vec<ScopeReading>,
}

#[cfg(feature = "profiling")]
impl TimingWindow {
    fn new() -> Self {
        Self {
            started: std::time::Instant::now(),
            frames: 0,
            rows: Vec::new(),
        }
    }

    /// Fold one frame in. A scope not seen before in this window is inserted
    /// where this frame's order puts it — right after the scope that preceded
    /// it — so one that first shows up mid-window still lands inside its
    /// parent rather than at the bottom of the table.
    fn add_frame(&mut self, frame: Vec<ScopeReading>) {
        self.frames += 1;
        let mut cursor = 0;
        for (key, name, ms) in frame {
            match self.rows.iter().position(|(k, _, _)| *k == key) {
                Some(i) => {
                    self.rows[i].2 += ms;
                    cursor = i + 1;
                }
                None => {
                    self.rows.insert(cursor, (key, name, ms));
                    cursor += 1;
                }
            }
        }
    }

    /// Close the window: mean ms PER FRAME for each scope — summed over the
    /// frames it ran on, divided by every frame in the window, so a scope that
    /// runs one frame in three reads as what it costs the frame budget.
    fn publish(&mut self) -> Vec<(String, f64)> {
        let frames = f64::from(self.frames.max(1));
        let out = self
            .rows
            .drain(..)
            .map(|(_, name, sum)| (name, sum / frames))
            .collect();
        *self = Self::new();
        out
    }
}

/// Flatten nested profiling results into a flat list with indentation.
///
/// The key is the scope's path plus its position among same-named siblings
/// (`/layers#1/trama#1/hue_drift#3`): ten `hue_drift` nodes in one chain share a
/// label, and a plain index into the flattened list shifts whenever an
/// intermittent scope comes or goes.
#[cfg(feature = "profiling")]
fn flatten_results(
    results: &[wgpu_profiler::GpuTimerQueryResult],
    depth: usize,
    path: &str,
    out: &mut Vec<ScopeReading>,
) {
    let mut seen: Vec<(&str, u32)> = Vec::new();
    for r in results {
        let nth = match seen.iter_mut().find(|(label, _)| *label == r.label) {
            Some((_, n)) => {
                *n += 1;
                *n
            }
            None => {
                seen.push((&r.label, 1));
                1
            }
        };
        let key = format!("{path}/{}#{nth}", r.label);
        if let Some(ref time) = r.time {
            let duration_ms = (time.end - time.start) * 1000.0;
            let indent = "  ".repeat(depth);
            out.push((key.clone(), format!("{indent}{}", r.label), duration_ms));
        }
        // Still recurse into nested queries even if this scope has no timing
        flatten_results(&r.nested_queries, depth + 1, &key, out);
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

    /// Open a timing scope on `encoder`. Scopes opened while another is alive
    /// do NOT nest under it: wgpu-profiler only nests a scope created from its
    /// parent `Scope`, and every scope here is created from the profiler, so
    /// they all come back top-level, in the order they CLOSED — children before
    /// the scope that encloses them (measured: `… | trama | layers`). An
    /// enclosing scope's time still includes what ran inside it. Label
    /// allocation only happens when a profiler is
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

#[cfg(all(test, feature = "profiling"))]
mod tests {
    use super::TimingWindow;

    fn reading(key: &str, ms: f64) -> (String, String, f64) {
        (key.to_string(), key.to_string(), ms)
    }

    // Run: cargo test -p fosfora-app --features profiling timing_window
    //
    // The panel's row set must not depend on which frame you look at. A scope
    // that runs one frame in three keeps its row, in tree position, and reads
    // as its cost per frame of the window.
    #[test]
    fn timing_window_holds_intermittent_scopes_in_place() {
        let mut w = TimingWindow::new();
        w.add_frame(vec![
            reading("/a", 3.0),
            reading("/a/x", 1.0),
            reading("/b", 6.0),
        ]);
        w.add_frame(vec![
            reading("/a", 3.0),
            reading("/a/x", 1.0),
            reading("/a/blit", 0.9),
            reading("/b", 6.0),
        ]);
        w.add_frame(vec![
            reading("/a", 3.0),
            reading("/a/x", 1.0),
            reading("/b", 6.0),
        ]);

        let rows = w.publish();
        let names: Vec<&str> = rows.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["/a", "/a/x", "/a/blit", "/b"], "tree order kept");
        assert!((rows[0].1 - 3.0).abs() < 1e-9);
        assert!(
            (rows[2].1 - 0.3).abs() < 1e-9,
            "0.9 ms on one frame of three"
        );
        assert_eq!(w.frames, 0, "publishing starts a fresh window");
        assert!(w.rows.is_empty());
    }
}
