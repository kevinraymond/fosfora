//! The runtime's own frame timing (`XR_META_performance_metrics`, a public
//! Khronos-registry extension): app GPU and CPU frame time, compositor
//! drops and device utilization, the numbers `VrApi`'s logcat line shows,
//! readable in-app for the debug panel. The `openxr` crate has no safe
//! wrapper, so the calls go through its raw function table.

use anyhow::{Context, Result};
use log::info;
use openxr as xr;
use xr::sys;

use crate::room::check;

/// Counters the panel shows, by path. A path the runtime does not list is
/// skipped (its value stays `None`).
/// (The Quest 3 on v207 lists 17 counters and no app CPU frame time; the
/// panel shows the frame loop's own CPU time instead.)
const COUNTERS: [(&str, Counter); 5] = [
    ("/perfmetrics_meta/app/gpu_frametime", Counter::AppGpuMs),
    (
        "/perfmetrics_meta/compositor/dropped_frame_count",
        Counter::Dropped,
    ),
    ("/perfmetrics_meta/device/gpu_utilization", Counter::GpuUtil),
    (
        "/perfmetrics_meta/device/cpu_utilization_average",
        Counter::CpuUtil,
    ),
    (
        "/perfmetrics_meta/app/motion_to_photon_latency",
        Counter::Latency,
    ),
];

#[derive(Debug, Clone, Copy)]
enum Counter {
    AppGpuMs,
    Dropped,
    GpuUtil,
    CpuUtil,
    Latency,
}

/// The latest value of each counter the runtime reported.
#[derive(Debug, Clone, Copy, Default)]
pub struct PerfSample {
    pub app_gpu_ms: Option<f32>,
    pub dropped_frames: Option<u32>,
    pub gpu_util_pct: Option<f32>,
    pub cpu_util_pct: Option<f32>,
    pub latency_ms: Option<f32>,
}

pub struct PerfMetrics {
    fp: xr::raw::PerformanceMetricsMETA,
    session: sys::Session,
    counters: Vec<(sys::Path, Counter)>,
    pub latest: PerfSample,
}

impl PerfMetrics {
    /// Enable the counters on `session`. `Ok(None)` when the extension is
    /// not enabled on the instance.
    pub fn new(session: &xr::Session<xr::Vulkan>) -> Result<Option<Self>> {
        let instance = session.instance();
        let Some(fp) = instance.exts().meta_performance_metrics else {
            return Ok(None);
        };
        let mut count = 0u32;
        // SAFETY: the two-call idiom's sizing call: capacity 0 and a null
        // array, so the runtime writes only the count. The instance is live.
        check(unsafe {
            (fp.enumerate_performance_metrics_counter_paths)(
                instance.as_raw(),
                0,
                &mut count,
                std::ptr::null_mut(),
            )
        })
        .context("xrEnumeratePerformanceMetricsCounterPathsMETA (count)")?;
        let mut paths = vec![sys::Path::NULL; count as usize];
        // SAFETY: `paths` holds exactly `count` elements, the capacity
        // passed; the runtime writes at most that many.
        check(unsafe {
            (fp.enumerate_performance_metrics_counter_paths)(
                instance.as_raw(),
                count,
                &mut count,
                paths.as_mut_ptr(),
            )
        })
        .context("xrEnumeratePerformanceMetricsCounterPathsMETA")?;
        paths.truncate(count as usize);
        let names: Vec<String> = paths
            .iter()
            .filter_map(|&p| instance.path_to_string(p).ok())
            .collect();
        info!(
            "performance metrics ({} counters): {}",
            names.len(),
            names.join(" ")
        );
        let counters = COUNTERS
            .iter()
            .filter(|(name, _)| names.iter().any(|n| n == name))
            .filter_map(|&(name, c)| Some((instance.string_to_path(name).ok()?, c)))
            .collect();
        let state = sys::PerformanceMetricsStateMETA {
            ty: sys::PerformanceMetricsStateMETA::TYPE,
            next: std::ptr::null(),
            enabled: sys::TRUE,
        };
        // SAFETY: `state` is a fully initialized input struct that outlives
        // the call; the session handle is live.
        check(unsafe { (fp.set_performance_metrics_state)(session.as_raw(), &state) })
            .context("xrSetPerformanceMetricsStateMETA")?;
        Ok(Some(Self {
            fp,
            session: session.as_raw(),
            counters,
            latest: PerfSample::default(),
        }))
    }

    /// Read every counter into `latest`. A failed or not-yet-valid read
    /// keeps the previous value.
    pub fn poll(&mut self) {
        for &(path, which) in &self.counters {
            let mut c = sys::PerformanceMetricsCounterMETA {
                ty: sys::PerformanceMetricsCounterMETA::TYPE,
                next: std::ptr::null(),
                counter_flags: sys::PerformanceMetricsCounterFlagsMETA::EMPTY,
                counter_unit: sys::PerformanceMetricsCounterUnitMETA::GENERIC,
                uint_value: 0,
                float_value: 0.0,
            };
            // SAFETY: `c` is an initialized output struct of the type the
            // call fills; the session outlives `self` (dropped with it in
            // `XrSession`) and `path` came from xrStringToPath.
            let res =
                unsafe { (self.fp.query_performance_metrics_counter)(self.session, path, &mut c) };
            if check(res).is_err() {
                continue;
            }
            let float = c
                .counter_flags
                .contains(sys::PerformanceMetricsCounterFlagsMETA::FLOAT_VALUE_VALID)
                .then_some(c.float_value);
            let uint = c
                .counter_flags
                .contains(sys::PerformanceMetricsCounterFlagsMETA::UINT_VALUE_VALID)
                .then_some(c.uint_value);
            #[allow(clippy::cast_precision_loss, reason = "counter values are small")]
            let value = float.or(uint.map(|u| u as f32));
            let l = &mut self.latest;
            match which {
                Counter::AppGpuMs => l.app_gpu_ms = value.or(l.app_gpu_ms),
                Counter::Dropped => {
                    l.dropped_frames = uint.or(float.map(|f| f as u32)).or(l.dropped_frames);
                }
                Counter::GpuUtil => l.gpu_util_pct = value.or(l.gpu_util_pct),
                Counter::CpuUtil => l.cpu_util_pct = value.or(l.cpu_util_pct),
                Counter::Latency => l.latency_ms = value.or(l.latency_ms),
            }
        }
    }
}
