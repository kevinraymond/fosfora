# Native vs pivot — decision (Oct 4, 2026)

*Written at the S7 gate (Sep 27, 2026); decided by Kevin the same day.
Numbers from `MEASURED.md`; task ids are blackboard entries.*

Decision: **continue native** (Rust + OpenXR + wgpu, one repo). Recommended
at the S7 gate and accepted Sep 27, 2026, a week ahead of the Oct 4 date.

Gates: S1 pass · S2 pass · S3 pass (seam merged to main; the S6 core audio additions followed as PR #40, merged Sep 27) · S4 partial (a
core compute-raster effect as shipped runs at 6 fps on the quad; the
billboard path holds 72 Hz, and C3a landed a world-space render entry in
core) · S5 pass, revised (750K sprites at 72 and 90 Hz, but only with a
depth-writing draw in the pass; see the S7 quad finding) · S6 pass (tempo
within 0.3 BPM, 46 ms beat jitter, external USB mic is the room source,
audio-to-photon filmed: visual leads by ~115 ms) · S7 pass (passthrough, hand
trackers and scene anchors all up natively once the manifest declared
`USE_ANCHOR_API`; in the headset particles land on and slide off the real
desk, part around the hands, and a pinch toggles a parameter; hand depth
occlusion still needs the hand mesh).

Budgets: particle ceiling @72 Hz 750K, @90 Hz 750K (world-space sprites with a
depth-writing draw in the pass; without one, ~250K) · frame time with MR on
(passthrough, hands, 16 room anchors, floor, depth occluders): 5.5 ms at
250K, 8.6 ms at 500K, both holding 72 Hz with zero long frames.

What native cost us this week: about 17 hours wall clock for S0–S7 (S0
Sep 26 22:16 → S6 done Sep 27 08:02, S7 14:00 → 18:30), of which roughly:
packaging and loader 1 h (S1), Vulkan/wgpu interop 1 h (S2), the seam
0.2 h (S3, plus a cloud task), audio on device 5.5 h (S6, most of it
finding out what the Quest microphones do, not code), MR glue 2 h to
measurable plus 2.5 h of worn iteration (S7: manifest gating, anchor
components, depth occlusion, physics tuning). No step hit its pivot trigger; S2 and
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
- Hands: `XR_FB_hand_tracking_mesh` as the depth occluder (joint spheres
  do not read as a hand); consider the global mesh as an obstacle and
  occluder for furniture the anchors miss.
- Room: in-app runtime permission request for `USE_SCENE`; keep the Space
  Setup flow (it works) but show a hint before launching it.
