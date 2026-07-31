# Splat

> A captured 3D scene as a breathing cloud of Gaussians — shatters on the drop, re-forms with the beat; bright timbre pulls focus. Load a .ply/.splat scene or the demo capture.

- type: particle
- pace param: orbit_speed (lo=-0.7 default=0.15 hi=0.8)
- motion (loud half, mean Δ): lo=0.00294 default=0.00294 hi=0.00294
- luma (quiet/loud at default): 0.109 / 0.108
- headless caveats: "layer 0: splat scene loads are App-side — 'Splat' renders without its point cloud"

## Imagery
NOT REPRESENTATIVE. The headless render contains no content: every frame is an empty
dark navy vignette — a slightly brighter blue-gray haze at center falling off to black
corners. No point cloud, no shards, no scene. Splat scenes load App-side, so the
catalog rig renders the effect with nothing to draw (the summary.json warning above).
In the real app this is a photographic 3D capture — a room, object, or landscape as a
cloud of soft gaussian blobs the camera orbits through.

## Motion character
Nothing measurable moves: motion is ~0.001–0.003 (essentially static), and all three
orbit_speed variants report byte-identical numbers — with no cloud there is nothing
for the orbit to move. No pace judgment is possible from these renders.

## Energy response
Only a faint breathing of the background haze separates quiet from loud (motion
0.0012 → 0.0029; luma flat at ~0.108). The documented behavior — drop shatters the
scene, beat re-coalesces it, onsets scatter splats, centroid pulls focus — is not
visible here and cannot be verified from this render.

## Palette
Empty render: desaturated navy/ink blue gradient, low value, near-monochrome. Real
palette depends entirely on the loaded capture.

## Casting notes
Do not cast from these renders — they are blank. When a scene is loaded in the app,
this is the engine's "real place made of light dust" card: a photographed room or
object that breathes with RMS, disintegrates on drops, and re-forms through the
phrase. Cast it for reveals, drops, and nostalgic/documentary moods where recognizable
photographic content matters; it needs a .ply/.splat asset to exist at all. Any
screenplay slot using Splat must confirm a scene asset; the headless catalog cannot
show what it will look like.
