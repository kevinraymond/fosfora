# Native vs pivot — decision (Oct 4, 2026)

*Draft written at the S7 gate (Sep 27, 2026). Numbers from `MEASURED.md`;
task ids are blackboard entries.*

Recommendation: **continue native** (Rust + OpenXR + wgpu, one repo).

Gates: S1 pass · S2 pass · S3 pass (seam merged to main) · S4 partial (a
core compute-raster effect as shipped runs at 6 fps on the quad; the
billboard path holds 72 Hz, and C3a landed a world-space render entry in
core) · S5 pass, revised (750K sprites at 72 and 90 Hz, but only with a
depth-writing draw in the pass; see the S7 quad finding) · S6 pass (tempo
within 0.3 BPM, 46 ms beat jitter, external USB mic is the room source,
audio-to-photon filmed: visual leads by ~115 ms) · S7 reachability pass
(passthrough layer, hand trackers, scene query, all up natively on the first
run once the manifest declared `USE_ANCHOR_API`); wearer checks (bounce off a
real table and hands, pinch toggle) pending at the time of writing.

Budgets: particle ceiling @72 Hz 750K, @90 Hz 750K (world-space sprites with a
depth-writing draw in the pass; without one, ~250K) · frame time with MR on:
8.2 ms at 250K (holds 72 Hz with passthrough, hands, room and floor);
~350K is the edge, 500K at 13.1 ms drops to 65 fps.

What native cost us this week: about 13 hours wall clock for S0–S7 (S0
Sep 26 22:16 → S6 done Sep 27 08:02, S7 to measurable by 15:30), of which
roughly: packaging and loader 1 h (S1), Vulkan/wgpu interop 1 h (S2), the
seam 0.2 h (S3, plus a cloud task), audio on device 5.5 h (S6, most of it
finding out what the Quest microphones do, not code), MR glue 2 h (S7,
including the manifest gating detour). No step hit its pivot trigger; S2 and
S7 each came in under their box.

Biggest remaining native risk: the shipped effects are compute-raster,
screen-space designs and the one tried runs at 6 fps on device; the product
needs 2–3 effects rebuilt on the world-space billboard path inside a ~250K
MR budget — mitigation: C3 (core `render_world`, ports of the lead effects)
is already scoped and the budget is measured, so this is design work on a
known path, not an unknown.

Next 2 weeks if native:
- C3: port 2–3 lead effects onto `render_world` (world xyz, alpha over
  passthrough), tuned to 250K sprites at 72 Hz with MR on; decide 72 vs 90 Hz
  (#3203).
- Understand the depth-writing-draw effect (#3259) and either make the primer
  a deliberate part of the pass or find the real cause; try
  `XR_FB_foveation` and a 0.75× raster for MR headroom.
- Hands-first interaction: pinch + gaze ray for a small parameter panel; a
  pinch-drag to place the cloud; controllers stay optional.
- Audio: delay the playback tap by the output latency (#3253); keep headset
  playback with a USB mic attached (#3254); in-app runtime permission
  requests (`USE_SCENE`, `RECORD_AUDIO`) through JNI.
- Room: run the scene query from a real room and, when empty, offer Space
  Setup from inside the app; consider the global mesh as an obstacle.
