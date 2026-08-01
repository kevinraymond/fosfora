# Loop export (`--render-loop`)

```
fosfora --render-loop my.loop.json [--out clip.mov]
```

Renders a [loop spec](loop-spec.md) to a mathematically seamless, beat-locked
loop file. Runs from the repo root or an installed build (assets resolve
beside the binary). Requires `ffmpeg` on PATH.

## Codecs

| codec | container | alpha | use |
|---|---|---|---|
| `hap_alpha` | .mov | **yes** | plays everywhere VJ software lives; ~2× HAP size |
| `hap` | .mov | no | lightweight GPU-decoded playback |
| `prores4444` | .mov | **yes** | master/archival (8-bit-sourced; see below) |
| `h264` / `hevc` | .mp4 | no | previews |

Missing encoder errors name the codec and the ffmpeg build to install (HAP
needs an ffmpeg with snappy).

## DXV (Resolume's native codec)

DXV encoding is closed — export `prores4444` (or `hap_alpha`) and transcode in
**Resolume Alley**. Community measurements have DXV-alpha files often *smaller*
than plain DXV; alpha is not a size penalty in the dominant target codec.

## Alpha

Frames leave the renderer **premultiplied** (see [alpha.md](alpha.md)). The
premult-vs-straight verdict per codec is established empirically in Resolume
(P2.6, pending the rig session); any required conversion becomes a fixed,
non-optional step of the encode path — there is deliberately no user toggle.

## Known limitations (v1)

- Readback is 8-bit RGBA: ProRes 4444 masters are 8-bit-sourced; dark
  gradients may band.
- `audio` modes `synthetic`/`file` are not wired yet; `none` is the golden path.
- 4/4 only; one effect per spec (no layer stacks — which also excludes the
  backdrop-reactive overlays).
