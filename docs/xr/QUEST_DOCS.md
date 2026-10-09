# Quest performance: the public docs that settle things

Meta's developer documentation answers questions this project spent sweeps
on. All of it is public (nothing under the developer NDA); the links are to
the `developers.meta.com` pages as found Oct 9, 2026.

| Doc | What it settles for us |
|---|---|
| [Advanced GPU Pipelines and Loads, Stores, and Passes](https://developers.meta.com/vr/documentation/native/android/po-advanced-gpu-pipelines/) | Tiled rendering: every render pass is load, render, store per tile (bin); a store's cost scales with resolution; clear or discard attachments you do not keep (we discard depth); a full-screen pass without depth or MSAA can run in direct mode, which turns foveation off for that surface; subpasses read tile memory without a round trip. The Adreno 740 has about 2 MB of tile memory. |
| [Mobile GPUs and tiled rendering](https://developers.meta.com/vr/documentation/unity/gpu-tiled/) | The architecture, in one page: why bandwidth and per-pass overhead, not ALU, are usually the cost. |
| [Accurately Measure an App's Per-Frame GPU Cost](https://developers.meta.com/horizon/documentation/unity/po-per-frame-gpu/) | `App=` in the VrApi line can include the compositor's TimeWarp slices, since the compositor preempts the app's GPU frame. To read the app's own cost: `adb shell am broadcast -a com.oculus.vrruntimeservice.COMPOSITOR_SKIP_RENDERING --ei milliseconds 60000`, then `TW=0.0` and `App=` is the app alone. FPS is not a cost metric. |
| [Use ovrgpuprofiler for GPU Profiling](https://developers.meta.com/vr/documentation/unity/ts-ovrgpuprofiler/) | On the headset, no install: `ovrgpuprofiler -m` lists counters, `-r"4,5"` streams them, `-e <package>` enables detailed profiling (restart the app; about 10 percent overhead), `-t1.0 -v` traces one second of render stages: per surface (each render pass, per eye) the resolution, bin count, Binning, Render, StoreColor, Preempt in ms, and each compute dispatch. The first trace showed the particle sim's dispatch as the frame's largest item (`MEASURED.md`). |
| [RenderDoc Meta Fork](https://developers.meta.com/horizon/downloads/package/renderdoc-oculus/) and [its optimization guide](https://developers.meta.com/horizon/blog/graphics-optimization-renderdoc-meta-fork/) | Frame capture with a tile-level render stage trace (Tile Timeline) and a draw-call trace of up to 59 counters per draw. Windows and macOS only (the Mac for us). Per-draw timers are per render pass on a tiler, not per draw. |
| [Compositor layers](https://developers.meta.com/horizon/documentation/native/android/os-compositor-layers/) | Each layer costs compositor time; the Developer Hub's Performance Analyzer can toggle layers to find hidden ones; the VrApi logcat tag is still the live stats source for OpenXR apps. |
| [Performance Analyzer and Metrics](https://developers.meta.com/vr/documentation/native/android/ts-mqdh-logs-metrics/) | The Developer Hub's view of the same counters, with Perfetto render-stage tracing (the successor of GPU Systrace). Windows and macOS. |

What the docs do not give: numbers for a specific scene. Those stay in
`MEASURED.md`.

`scripts/xr/gpu-stages.sh` wraps `ovrgpuprofiler` the way `worn-ab.sh`
wraps the VrApi line: one trace per condition, parsed to the sim
dispatch, the eye surface's binning and render, and the faces surface's
render.
