# Alpha-survival audit (overlay foundations, P0.2)

Where the alpha channel lives and dies between an effect writing `rgba != (r,g,b,1)`
and the bytes a sink receives. Audited at v1.31.0 (2026-07-31), pre-remediation;
the **Remediation** section records what changed. File:line references are to that
revision and drift.

## Pass-by-pass survival

| # | Stage | Alpha survives? | Why |
|---|-------|-----------------|-----|
| 1 | Effect fragment pass → `Rgba16Float` target | **Yes** (effect-dependent) | `blend: None`, clear `TRANSPARENT` (`gpu/pass_executor.rs`), `ColorWrites::ALL`. The value is whatever the `.wgsl` writes — about half the shipped effects hardcode `a = 1.0`, which is fine for non-overlays. |
| 2 | Particle render (alpha / 3DGS / WBOIT composite) | Yes | Alpha blend `One, OneMinusSrcAlpha` — correct src-over coverage. Additive modes accumulate `One, One` (saturates toward 1, acceptable for glow). |
| 3 | Volumetric / Lattice / Helix raymarch | Yes | Premultiplied over; `alpha = 1 − transmittance`. |
| 4 | Media blit | Yes | Passthrough sample; transparent letterbox bars. |
| 5 | Compositor, single-opaque-layer fast path | Yes | Bypasses compositing entirely (`gpu/frame_graph.rs`) — the effect's own target goes straight to post. **This is the overlay path.** |
| 6 | Compositor, multi-layer | Yes (approximate) | Clear `TRANSPARENT`; `composite.wgsl` emits `max(bg.a, fg.a·opacity)` — a coverage union, not a premultiplied "over" (see limitation in `alpha.md`). Displacement blend modes keep `bg.a` only. |
| 7 | Dissolve transition (snapshot + crossfade) | Yes | The `Color::BLACK` clears were **dead stores**: both passes draw a fullscreen triangle with `blend: None`, so the clear never reaches a surviving pixel, and `crossfade.wgsl` mixes the full vec4. Clears changed to `TRANSPARENT` anyway so no clear on the frame path manufactures opaque alpha. |
| 8 | Bloom extract / blur H / blur V | Yes (unused) | Full-vec4 Gaussian; the composite never reads bloom `.a`. |
| 9 | **Final post composite** (`post_composite.wgsl`) | **NO — this was the single point of death** | Sampled only `.rgb`; wrote `a = 1.0`, or `clamp(brightness·2, 0, 1)` under the NDI luma option. Scene alpha was never read. |
| 10 | Post-disabled blit fallback | Yes | `blit.wgsl` passthrough — before the fix, the only route real alpha had to the output. |
| 11 | Capture texture → staging → sink bytes | Yes | 8-bit BGRA/RGBA `RENDER_ATTACHMENT\|COPY_SRC`, byte-exact row-unpad (`gpu/frame_capture.rs`). |

## Remediation (chosen: fix the single point of death)

The feared "post chain is broadly alpha-hostile → build a `preserve_alpha` restricted
render path" scenario (handoff Q2) did not materialize. One shader wrote the alpha;
one shader was fixed:

- `PostParams.alpha_from_luma` → **`alpha_mode`** (same 48-byte layout):
  `0` opaque (historical), `1` luma-derived (legacy NDI key), `2` **passthrough** —
  the scene's real coverage alpha, clamped, survives to the surface and every capture.
- Resolution lives in `gpu/frame_graph.rs::resolve_output_alpha`, shared by both live
  render branches and the headless renderer. The user-facing setting is
  **Settings → Output alpha** (Auto / Opaque / Luma key / Passthrough); Auto keeps
  pre-overlay behavior byte-identical unless the scene is entirely `alpha: true`
  overlay effects.
- No restricted-post path: bloom/CA/vignette stay available on overlays. Bloom adding
  RGB without alpha is *additive premultiplied light* — legal and intended (glow
  spills over the underlying content in the external compositor). Grain is the one
  post effect overlays ship disabled: it adds time-hashed RGB into `a = 0` regions
  and breaks phase-locked determinism.

## Deferred: on-screen checkerboard preview

A checkerboard behind transparent regions (screen only, not capture) needs per-pass
`PostParams` — the on-screen composite and `render_composite_to` share one uniform
buffer by design. The v2 sketch: a second 48-byte buffer + bind group for the screen
pass only, a `preview_backdrop` flag in `PostParams`, checker generated in-shader.
Skipped for v1: in passthrough mode the opaque window already shows the correct
premultiplied-over-black preview.
