# Alpha in Fosfora

How transparency works through the render chain, what the output sinks carry, and
the conventions overlay effects must follow. Companion: [alpha-audit.md](alpha-audit.md)
(where alpha used to die and what changed).

## INV-A: premultiplied alpha, everywhere internal

All effect output, compositing, post-processing, capture textures and readback carry
**premultiplied alpha**: a pixel's RGB is already scaled by its coverage. A fully
transparent pixel is `(0, 0, 0, 0)`; RGB > A is legal and means *additive light*
(glow, bloom spill). Straight (unpremultiplied) alpha exists only at external
boundaries — if an encoder or host app needs it, conversion happens there, never
internally.

Consumers compositing Fosfora's output over their own content must use the
premultiplied over operator:

```
out.rgb = src.rgb + dst.rgb × (1 − src.a)
```

In Resolume, set the layer/clip blend to **Alpha** with premultiplied handling (or
"Add" for pure-glow overlays); in OBS, sources received via Spout/Syphon/NDI
composite correctly by default when the source is tagged premultiplied.

Overlay shaders return `vec4f(rgb * a, a)` — coverage-scaled color, real coverage
alpha, transparent background (`vec4f(0.0)`) everywhere they don't draw.

## Output alpha modes

**Settings → Output alpha** controls what the alpha channel of the final composite
carries — on screen, into every sink (the capture pass reuses the same composite),
and headless:

- **Auto** (default): Passthrough when every enabled layer is an effect tagged
  `alpha: true` (the scene *is* an overlay); otherwise the NDI "Alpha from
  brightness" checkbox selects Luma; otherwise Opaque. Existing setups render
  exactly as before.
- **Opaque**: alpha forced to 1.0 — the historical behavior.
- **Luma key**: alpha derived from output brightness (the legacy NDI keying trick).
- **Passthrough**: the scene's real premultiplied coverage survives to the output.

Post effects and passthrough: **bloom**, **chromatic aberration** and **vignette**
compose correctly (they alter light, not coverage — bloom over transparency reads
as glow spilling onto the content beneath). **Grain** adds noise into transparent
regions and is disabled in the shipped overlay effects; enabling it on an overlay
produces a faint full-frame noise-glow. **Tonemap** applies to RGB only.

## Sink alpha matrix

| Sink | Format on the wire | Alpha carried? | Notes |
|------|--------------------|----------------|-------|
| NDI | BGRA / RGBA FourCC (`ndi/ffi.rs`) | **Yes** | 4 bytes/px, real alpha channel; receivers key natively. |
| Spout (Windows) | DXGI BGRA8 / RGBA8 shared texture (`spout/sink.rs`) | **Yes** | Byte passthrough; whether a receiver *uses* alpha is receiver-side. |
| Syphon (macOS) | `BGRA8Unorm` → IOSurface (`syphon/sink.rs`) | **Yes** | RGBA input swizzled preserving `px[3]`; `flipped: true` is load-bearing. |
| v4l2 virtual camera (Linux) | YUYV or BGRX (`v4l2/types.rs`) | **No** (structural) | YUYV has no alpha; BGR4's 4th byte is "don't care" padding to consumers. |
| Recording (ffmpeg) | `yuv420p` encode | **No** | Input is BGRA but the encode discards alpha. Alpha-capable export (HAP-Alpha / ProRes 4444) is the loop-export phase's job. |

The capture/readback pipeline itself is 8-bit BGRA/RGBA and byte-exact — with
passthrough enabled, whatever alpha the composite writes is what every sink gets.

## Known limitations

- **Internal multi-layer compositing is straight-alpha.** The layer "Normal" blend is
  `mix(bg.rgb, fg.rgb, opacity · fg.a)` with `max`-union alpha — layering an overlay
  over another layer *inside* Fosfora slightly darkens antialiased fringes versus a
  true premultiplied over, and stacked coverage is a union, not accumulation. The
  solo-overlay fast path (one enabled layer) and external compositing are exact.
  A dedicated premultiplied-over blend mode is the planned follow-up.
- **Compositor displacement modes** keep the background's alpha only.
- The **on-screen window is always opaque** (the swapchain is pinned to
  `CompositeAlphaMode::Opaque`); passthrough shows as premultiplied-over-black,
  which is the correct preview of what a downstream compositor will add light to.
