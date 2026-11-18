//! The depth light (board #5439): what the live depth map still occludes is
//! drawn as a cloud of points of light fixed in the room, and with the room
//! dark the surfaces' light behind it shows between them.
//!
//! **What is lit.** The depth occluder's mesh (`env_depth.rs`,
//! `envdepthmesh`), where it still occludes. With geometry first on
//! (`geom_first.rs`) that is exactly what no room box explains: a chair, a
//! vacuum, a pile, a person, the wearer's own legs. The eye pass's mesh
//! writes the same depth as before, so the particles are hidden behind real
//! things as they were. The hands are not part of it, since the runtime
//! removes them from the map and the hand mesh occludes them.
//!
//! **Where the points draw** ([`mesh_draws`]). In the faces layer
//! (`faces_layer.rs`) when there is one, the default with passthrough: at
//! half the eye's resolution a quarter of the eye's fragments shade them,
//! and the runtime's upsample of the layer keeps them soft. That pass then
//! draws its own mesh (twice the eye's step) with the light, after its
//! occluders. The eye pass draws its mesh for the depth and the veil of its
//! own light (the edge bars) alone, or depth alone where the veil has
//! nothing to do. Without a faces layer the eye pass draws the points.
//!
//! **The veil** ([`veil`]). The map no longer hides the surfaces' light
//! (the sky, the lit faces, the edge bars) with its coarse cutout. Those
//! draw first, tested against the room's boxes and the hands alone; the
//! mesh then erases the veil's share of them where it lies in front, and
//! adds its points. The veil is `1 - level` of the immersion: with the room
//! visible (level 0) the map hides the surfaces' light behind a real thing
//! as before and the passthrough shows the thing; at level 0.5 half of it
//! shows through, over the passthrough at half; with the room black
//! (level 1) all of it shows, and the thing is only its points. The faces
//! layer gets the same erase from its own mesh. At level 0 the veil erases
//! all, which drawing the mesh first gives without shading the surfaces
//! behind real things, so both passes do that there ([`veil_passes`]); at
//! level 1 it erases nothing, so the eye pass's mesh writes depth alone.
//! The blend erases the
//! share in color and alpha alike, so at level 0 the eye layer stays
//! transparent over the passthrough where the veil erased it. The points
//! add their light: the layer's alpha carries the erase and not the light,
//! which an "over" of the light itself would need a second output or a
//! second draw for. `depthlightveil 0` keeps the hard cutout at every
//! level: the mesh draws before the surfaces, as in the first version.
//!
//! **The points** ([`fragment`], [`plane_points`], [`points_tile`]). A
//! tile of soft points made once and uploaded as a small texture
//! ([`TILE`] texels, [`TILE_CELLS`] cells a side, a point anywhere in each
//! cell): per texel, the distance to the nearest point's center as a cone
//! that falls to 0 at [`REACH`], and that point's size, twinkle phase and
//! rank. The fragment reads it once per plane and octave at the world
//! position and makes the point there: a soft disk of the fill's radius
//! ([`point_layout`]) times its size ([`SIZE`]), swelling with the music
//! ([`SWELL`]) and shrinking with what the vertex keeps, present where its
//! rank is under the fill's share of cells, twinkling at 1, 2 or 3 times
//! the clock by its size from its phase with a triangle wave ([`wave`]),
//! no sine. Each plane and octave reads the tile from its own place
//! ([`tile_offset`]), and it repeats every 16 cells (64 cm 1 m from the
//! eye). The planes are weighted by the fourth power of the normal's
//! components, so a surface uses the plane it faces; a second plane only
//! between about 35 and 55 degrees off an axis ([`PLANE_MIN`]). The share
//! covered is `depthlightfill`, a quarter by default. A
//! cell's side is [`CELL_M`] at 1 m from the eye and doubles with each
//! octave of the distance, the next octave fading in over the last part of
//! each ([`OCTAVE_BLEND`], the last quarter), so a thing keeps its look on
//! screen near or far and the map's noise, larger far away, stays small
//! against the cells. Through the points run the bands from the first
//! version as a slow wave of light, one every [`BAND_SPACING_M`] of height,
//! drifting up, its crests tinted toward the top's color, its profile the
//! same triangle wave squared. A surface that faces up takes the
//! top's color. The colors and the band are the theme's
//! ([`colors`], [`band`]): since board #5543 its own row for the depth
//! light (`themes::Depth`), not the row for other surfaces. Aurora draws
//! teal and the key's tint on the bass; a room with no theme other's own
//! green. The row also names a look (`themes::DepthLook`, below); one that
//! is not built draws as the points.
//!
//! **The looks** (board #5543; [`look_light`], [`look_id`]). A theme's
//! depth light can draw something other than the points. Each look is the
//! points' machinery read another way: the same tile, one read per plane
//! and octave, the same weights, fades and vertex stage, picked by an id
//! in the light's rows ([`LOOK_ROW`]), and every one stays on the thing's
//! surface. The points' own path is untouched and still draws what it
//! drew, bit for bit.
//!
//! - *Beams* ([`beam`]; Aurora). On an upright surface the tile is read
//!   [`BEAM_PACK`] times closer across and [`BEAM_STRETCH`] times drawn
//!   out along the world's up, so each point is a lane. Up each lane runs
//!   a streak, its head [`BEAM_PERIOD`] cells from the next, the light
//!   falling behind it: the sides' color along the streak, the tops' at
//!   the head. The heads travel up on a clock the band drives
//!   ([`BEAM_TRAVEL_HZ`]), slow in silence and fast on a kick, and the
//!   streak's length follows the band. A top shows the points, in the
//!   tops' color: the beams seen end on.
//! - *Coals* ([`coal`]; Flint). The points as lumps of heat: a cone from
//!   each center, its own slow smolder, and the band's level squared, so
//!   a kick flares. The color runs a ramp by the heat ([`look_color`]):
//!   ember, a dark red made from the sides' color ([`COAL_EMBER`]), then
//!   the sides' color, then the tops'. Flint's row gives amber and warm
//!   white, so the ramp is dark red, amber, white.
//! - *Grass* ([`blade`]; Contour). An upright surface is cut into rows
//!   [`GRASS_ROW`] cells high. Each row reads one line of the tile, so a
//!   point near the line is a blade rooted at the row's foot, tapering to
//!   its tip, the sides' color at the root and the tops' at the tip. The
//!   tips bend ([`grass_bend`]) with a wave that crosses the room: wider
//!   with the band, and leaning one way with the gust, the overall
//!   loudness smoothed ([`Drive::step_wind`]). A top shows the points,
//!   moved a little by the same wind.
//!
//! The music reaches a look through the same value the points' crests
//! take, the one the flash limit follows, so the limiter holds a kick's
//! flare and a streak's reach as it holds the points. The travel and the
//! wind are motion and not light, and run on the band as it is. Bubbles,
//! sparkle, glints and meters have their ids and are not built.
//!
//! **Stable in time.** A point is a function of the world position alone:
//! its cell and its place in the cell come from the position projected on
//! an axis plane, so it stays put while the head moves and while the mesh's
//! vertices slide over the surface. The map's noise moves the surface along
//! the view ray, and the projection on the plane a surface faces turns that
//! into a much smaller shift: [`JITTER_TEST_M`] of noise moves a facing
//! surface's point by a few hundredths of a cell. A head move changes only
//! the octaves' blend, slowly. The normal is taken toward the neighbor
//! nearer in depth, so a step does not swing it. Where cells shrink to a
//! few pixels (a surface seen edge-on, the fringe that leans back at a
//! step) the points fade out ([`AA_CELLS_PER_PX`]) instead of sparkling.
//! Fading them into their mean instead drew a faint line of glow along
//! every step of the mesh in renders of the recorded office.
//!
//! **Masses and edges** ([`vertex_light`]). Inside a mass nothing changes
//! at a step between things: no line, no thinning. A step to a neighbor
//! larger than [`JUMP_MIN_M`] or [`JUMP_REL`] of the distance, whichever
//! is larger, parts two masses: the far side keeps no points at its edge,
//! the near side [`NEAR_KEEP`] of them, and the triangles between them
//! (the fringe that leans back) carry the near side's points shrinking to
//! nothing. So a mass ends in thinning points and the one behind it starts
//! again a cell away, with the sky between. A point's area follows what its
//! vertex keeps. Something thinner than a mesh step is all near side, so a
//! vacuum's handle carries points at [`NEAR_KEEP`] of their area.
//!
//! **Far things keep their color.** The light is all there to 1.5 m and
//! falls to [`FAR_FLOOR`] by 3 m ([`FADE_M`]), and stays there as far as
//! the map has data.
//!
//! **How bright** ([`peak_alpha`]). A point's center rises with the
//! immersion level from [`ALPHA_LOW`] with the room plain to [`ALPHA_HIGH`]
//! with it black, times `depthlightgain`, the band's wave and the twinkle.
//! At a quarter covered the light averages well under a lit surface's. The
//! crests follow the band the theme gives the depth light ([`band`]; the
//! bass for Aurora), smoothed ([`MUSIC_ATTACK_S`], [`MUSIC_RELEASE_S`]).
//!
//! **Draw order.** With the veil letting some light through, the eye pass
//! draws the box and hand occluders, the quad, the surfaces when they are
//! in the eye pass, the highlight, the live outlines and the edge bars,
//! then the map's mesh, then the particles and everything after them; the
//! faces pass draws the occluders, its surfaces, then the mesh. Otherwise
//! a pass drawing the points draws them right after the box and hand
//! occluders and before its surfaces, and a pass drawing the mesh for its
//! depth alone draws it first, as before the light. Every occluder keeps
//! the nearest depth (compare `Less` or `LessEqual`), so the depth buffer
//! ends the same whatever the order.
//!
//! **Cost.** The first points (board #5439, measured on the Quest 3)
//! evaluated four hashed cells per plane and octave per fragment, a sine,
//! a square root and a length each, in the eye pass: about 4 ms for a pile
//! covering 15 % of the view, and the veil about 1.5 ms more. Now, per
//! vertex of the mesh that draws the points (the faces layer's, 6.6K per
//! eye at step 4; the eye's, 26K, without a faces layer), in the vertex
//! stage where the map is already read: four more texel loads, five rays,
//! two one-sided differences, a cross product, a rotation, two square roots
//! and three smoothsteps. Per fragment of that mesh: one `fwidth`, a
//! normalize, the planes' weights, a `log2` and a square root, then one
//! read of the tile (two in the last quarter of an octave or between two
//! axes, at most six) with about forty arithmetic operations each, no
//! transcendental; then the wave (a triangle, no cosine) and the blend.
//! No discard and no `frag_depth`, so early depth testing works as before.
//! A look other than the points reads the tile as often, once per plane
//! and octave, and counts about the same around each read: the coals about
//! thirty-five operations (no soft edge, one wave), the beams and the
//! grass about fifty (two more smoothsteps, a floor and a fract), the
//! grass one wave more per fragment for the wind, the bubbles about
//! fifty-five and one wave for the lane's snake, the sparkle about
//! forty-five and a dot product a fragment for the wash's front. No look adds a
//! transcendental, a hash or a read, and the vertex stage is the same.
//! These are counts of the source, not a device number.
//! The eye pass's mesh adds a constant fragment where the veil erases (0 <
//! level < 1) and nothing over the depth-only occluder otherwise. Between
//! the two levels the surfaces behind a real thing shade, where its depth
//! rejected them before.
//!
//! **Knobs**, read at launch and polled once a second
//! ([`DepthLightOptions::from_knobs`]): `debug.fosfora.depthlight 0|1`
//! (default 1; 0 is the frame as it was, with depth only, no color write
//! and the old order), `depthlightgain` (0 to [`GAIN_MAX`], default 1),
//! `depthlightfill` (the share covered, [`FILL_RANGE`], default [`FILL`]),
//! `depthlightveil 0|1` (0 keeps the hard cutout at every level, 1 the
//! veil; unset, the room's theme decides: the hard cutout where its depth
//! light occludes, board #5543, [`DepthLightOptions::under`], which is
//! every theme, and the veil in a room with none) and `depthlightpoints 0|1` (default 1; 0 draws no points, the
//! mesh still drawn in the light's passes and order, to cost the points
//! apart). It needs the occluder's mesh. With the full-screen pass
//! (`envdepthmesh 0`, or `envdepthfilter 0` or 1) or the gray diagnostic
//! (`envdepthshow`) it is off, and the log says so ([`blocked`]).
//! `geomfirstshow`'s tint replaces it while on.
//!
//! **What it does not do.** It does not change what occludes the
//! particles: the depth written, geometry first's rule and the hand
//! removal are as before. It does not sharpen the map's edge; at levels
//! between 0 and 1 the veil still follows the map's coarse silhouette, in
//! the faces layer the coarser one of its own mesh. Where no box explains a
//! wall (no scan, roam, geometry first off) the wall is points too. In the
//! faces layer the points' masses and edges come from its mesh at twice the
//! step: a thing narrower than about four texels can fall between its
//! vertices and carry no points.
//!
//! **The sweep** (board #5511, `sonar.rs`). On a frame a sweep or one of a
//! build's standing lines shows, the mesh that draws the points draws them
//! with `fs_mesh_light_sonar` instead ([`fragment_swept`], [`swept`]): the
//! same points, wave and veil, the points under a line swollen and
//! brightened, and the line solid under them. Its rows follow the light's
//! own ([`SONAR_ROW`], [`with_sonar`]) and are all 0 on every other frame
//! and with `debug.fosfora.sonar 0`, when the mesh draws with
//! `fs_mesh_light` as it did before the sweep.
//!
//! Plain numbers in, so it builds and tests on the desktop:
//! [`vertex_light`] mirrors the WGSL's `mesh_light`, [`fragment`] its
//! `fs_mesh_light`, [`fragment_swept`] its `fs_mesh_light_sonar` and
//! [`veil`] the blend, and `tests/depth_light_gpu.rs` runs the stages on a
//! desktop GPU against them.

use std::f32::consts::PI;

use glam::{Vec2, Vec3, Vec4};

use crate::env_depth::EyeReprojection;
use crate::surface_fx::{Audio, BAND_BASS, BAND_HIGH, BAND_MID};
use crate::surfaces::{COLOR_KEY, KIND_OTHER, SurfaceBehavior, color_name, surface_color};
use crate::themes::{DepthLook, Theme};

/// The knobs' names.
pub const KNOB: &str = "debug.fosfora.depthlight";
pub const KNOB_GAIN: &str = "debug.fosfora.depthlightgain";
pub const KNOB_FILL: &str = "debug.fosfora.depthlightfill";
pub const KNOB_VEIL: &str = "debug.fosfora.depthlightveil";
pub const KNOB_POINTS: &str = "debug.fosfora.depthlightpoints";

/// Rows of [`rows`], the end of `struct EnvEye` from `light` on
/// (`env_depth::LIGHT_ROW`): the light's own, the look's two from
/// [`LOOK_ROW`] (board #5543), then the sweep's (`sonar.rs`, board #5511)
/// from [`SONAR_ROW`].
pub const ROWS: usize = SONAR_ROW + crate::sonar::ROWS;
/// The look's row, `light_look` in `struct EnvEye`: the look's id
/// ([`look_id`]), the music 0..1 and the gust 0..1.
pub const LOOK_ROW: usize = 9;
/// The looks' clocks, `light_move` in `struct EnvEye`: the beams' travel,
/// the grass's sway and the bubbles' rise, each 0..1 of a cycle.
pub const MOVE_ROW: usize = LOOK_ROW + 1;
/// What the look drawn reads of the room, `light_more` in `struct EnvEye`
/// ([`Around`], [`with_around`]): for the sparkle the wash's front. All 0
/// for a look that reads nothing there.
pub const MORE_ROW: usize = MOVE_ROW + 1;
/// The first of the sweep's rows, `sonar` in `struct EnvEye`.
pub const SONAR_ROW: usize = MORE_ROW + 1;

/// Beams: how many times closer than the points' the streaks stand
/// across.
pub const BEAM_PACK: f32 = 2.0;
/// Beams: how many times a point is drawn out along the world's up, so it
/// is a streak's lane.
pub const BEAM_STRETCH: f32 = 6.0;
/// Beams: the height (cells) between one streak's head and the next in
/// its lane.
pub const BEAM_PERIOD: f32 = 4.0;
/// Beams: a streak's length as a share of the period with the music
/// silent; all of it at a full band.
pub const BEAM_TAIL_MIN: f32 = 0.3;
/// Beams: the least soft edge at a streak's head, a share of the period.
pub const BEAM_HEAD: f32 = 0.08;
/// Beams: a lane's width over a point's, as far as the tile's reach lets
/// the largest.
pub const BEAM_WIDE: f32 = 1.25;
/// Beams: the heads' travel (periods a second, up) in silence and at a
/// full band, so a kick launches them; the beat envelope adds up to as
/// much again. A streak runs at 1, 2 or 3 times it by its size.
pub const BEAM_TRAVEL_HZ: [f32; 2] = [0.1, 2.0];
/// Coals: a lump's width over a point's, capped as [`BEAM_WIDE`] is.
pub const COAL_WIDE: f32 = 1.2;
/// Coals: the heat with the music silent, as a share of a flare's.
pub const COAL_FLOOR: f32 = 0.45;
/// Coals: how deep a lump's own smolder goes, on the twinkle's clock.
pub const COAL_SMOLDER: f32 = 0.6;
/// Coals: the coldest color of the heat ramp as factors of the sides'
/// color, so amber gives a dark red.
pub const COAL_EMBER: [f32; 3] = [0.9, 0.12, 0.02];
/// Grass: how many times closer than the points' the blades stand across.
pub const GRASS_PACK: f32 = 3.0;
/// Grass: the height of one row of blades (cells), a blade's most.
pub const GRASS_ROW: f32 = 2.0;
/// Grass: how far up the tile the next row reads (cells), so no row
/// repeats the one under it.
pub const GRASS_ROW_STEP: f32 = 2.37;
/// Grass: the shortest blade as a share of the row.
pub const GRASS_SHORT: f32 = 0.75;
/// Grass: how much nearer a point counts than it is, so one further from
/// the row's line still grows a blade there and nearly every lane has one.
pub const GRASS_NEAR: f32 = 0.7;
/// Grass: a blade's greatest half-width (cells of the tile): under the
/// tile's reach times [`GRASS_NEAR`], so no blade is found where the tile
/// holds no point.
pub const GRASS_REACH: f32 = 0.33;
/// Grass: the share of the row a blade's root fades in over.
pub const GRASS_ROOT: f32 = 0.1;
/// Grass: how far a tip sways (packed cells) with the band silent, and
/// how much further at a full band.
pub const GRASS_SWAY: f32 = 0.3;
pub const GRASS_SWAY_BAND: f32 = 0.9;
/// Grass: how far a full gust leans every tip one way (packed cells).
pub const GRASS_LEAN: f32 = 1.0;
/// Grass: the wind's waves per meter along x and z.
pub const GRASS_WIND: [f32; 2] = [0.9, 0.5];
/// Grass: how far the blades seen from above (a top's points) move, as a
/// share of the tips' bend.
pub const GRASS_TOP_SHIFT: f32 = 0.25;
/// Grass: the sway's cycles a second in still air; a full gust doubles
/// it.
pub const SWAY_HZ: f32 = 0.35;
/// Grass: how fast the gust follows the loudness up and down (s).
pub const GUST_ATTACK_S: f32 = 0.4;
pub const GUST_RELEASE_S: f32 = 1.2;
/// Bubbles: how the tile is read across, under 1 so a lane is wider than
/// a point and a bubble up to 1.7 cells across.
pub const BUBBLE_PACK: f32 = 0.6;
/// Bubbles: how many times a point is drawn out along the world's up, so
/// it is the lane its bubbles rise in.
pub const BUBBLE_STRETCH: f32 = 8.0;
/// Bubbles: the height (cells) between one bubble and the next in its
/// lane.
pub const BUBBLE_PERIOD: f32 = 3.0;
/// Bubbles: how far a lane snakes to each side (cells), and the height of
/// one turn of it (cells), so a bubble wobbles as it rises.
pub const BUBBLE_WOBBLE: f32 = 0.35;
pub const BUBBLE_WOBBLE_CELLS: f32 = 5.0;
/// Bubbles: the light inside a bubble as a share of its rim's, and where
/// the rim rises, as shares of the radius squared.
pub const BUBBLE_FILL: f32 = 0.3;
pub const BUBBLE_RIM: [f32; 2] = [0.2, 0.6];
/// Bubbles: where a bubble's soft edge starts, a share of the radius
/// squared.
pub const BUBBLE_EDGE: f32 = 0.6;
/// Bubbles: how far over the center the highlight sits (a share of the
/// radius) and its size (a share of the radius squared).
pub const BUBBLE_SHINE: f32 = 0.45;
pub const BUBBLE_SHINE_SIZE: f32 = 0.12;
/// Bubbles: the rise (periods a second, up) in silence and at a full
/// band, so the band lets them go; the beat envelope adds up to as much
/// again. A bubble rises at 1, 2 or 3 times it by its size.
pub const BUBBLE_RISE_HZ: [f32; 2] = [0.04, 0.4];
/// Sparkle: a point's light between flares, as a share of a flare's.
pub const SPARKLE_DIM: f32 = 0.25;
/// Sparkle: a point's own flares with the band silent, as a share of
/// their light at a full band.
pub const SPARKLE_QUIET: f32 = 0.3;
/// Sparkle: how far to each side of the wash's front the points flare (m).
pub const SPARKLE_FRONT_M: f32 = 0.3;
/// Sparkle: how far the stand-in front travels to each side of the
/// wearer (m), until the wash gives its own ([`wash_front`]).
pub const SPARKLE_REACH_M: f32 = 2.5;

/// The height of one band of the wave (m).
pub const BAND_SPACING_M: f32 = 0.2;
/// The light between the wave's crests, as a share of a crest's.
pub const STEADY: f32 = 0.55;
/// How far a crest tints a side toward the top's color.
pub const BAND_TINT: f32 = 0.35;
/// The mean of the crest's profile, [`wave`] squared: what the wave fades
/// into where a band spans few pixels (13/35).
pub const CREST_MEAN: f32 = 13.0 / 35.0;
/// Bands per pixel where the wave starts to fade into its mean and where
/// it is gone: six pixels a band and two.
pub const AA_BANDS_PER_PX: [f32; 2] = [1.0 / 6.0, 0.5];
/// The distance from the eye (m) where the light starts to fall and where
/// it reaches its floor.
pub const FADE_M: [f32; 2] = [1.5, 3.0];
/// The light past the fade, as a share of a near thing's: a far shelf is a
/// dim constellation, never black.
pub const FAR_FLOOR: f32 = 0.35;
/// The smallest step in depth to a neighbor (m) that parts two masses, and
/// the share of the vertex's distance that does when that is larger: half
/// a meter to 2 m, a quarter of the distance past it.
pub const JUMP_MIN_M: f32 = 0.5;
pub const JUMP_REL: f32 = 0.25;
/// Where the thinning at a mass's edge starts, as a share of that step.
pub const JUMP_SOFT: f32 = 0.6;
/// What the near side of a mass's edge keeps of its points' area.
pub const NEAR_KEEP: f32 = 0.5;
/// A cell's side 1 m from the eye (m): about 30 pixels of the Quest 3's
/// eye buffer.
pub const CELL_M: f32 = 0.04;
/// The share of an octave of the distance where the next octave's cells
/// start to fade in: the last quarter of each reads the tile twice.
pub const OCTAVE_BLEND: f32 = 0.75;
/// The octaves' range around 1 m, as in the WGSL: cells from 1/16 to 16
/// times [`CELL_M`].
pub const OCTAVE_RANGE: [f32; 2] = [-4.0, 3.999];
/// Cells per pixel where the points start to fade out and where they are
/// gone: under about 7 and 2.5 pixels a cell. They fade to nothing rather
/// than to their mean, so the fringe at a step reads as sky, not a line.
pub const AA_CELLS_PER_PX: [f32; 2] = [0.15, 0.4];
/// Taken off each plane's share of the points, so a plane the surface
/// barely faces fades out rather than drops; one left with none is skipped
/// and the others shared out again, as in the WGSL. A surface reads a
/// second plane only between about 35 and 55 degrees off an axis.
pub const PLANE_MIN: f32 = 0.12;
/// A point's radius as factors of the base, uniformly between them.
pub const SIZE: [f32; 2] = [0.7, 1.3];
/// Where a point's soft edge starts, as a share of its radius.
pub const SOFT_CORE: f32 = 0.35;
/// The music's swell: the radius grows by up to this share.
pub const SWELL: f32 = 0.2;
/// The points' tile ([`points_tile`]): texels a side, and cells a side in
/// it, one point in each anywhere within it, the tile repeating every
/// [`TILE_CELLS`] cells (64 cm 1 m from the eye).
pub const TILE: u32 = 256;
pub const TILE_CELLS: u32 = 16;
/// The farthest a point's light reaches from its center (cells), the
/// tile's R at 0: a radius stays within it.
pub const REACH: f32 = 0.5;
/// The share covered by points (`depthlightfill`), and the knob's range:
/// the largest point must stay within half a cell, which caps it.
pub const FILL: f32 = 0.25;
pub const FILL_RANGE: [f32; 2] = [0.05, 0.33];
/// The smallest base radius (cells): a lower fill puts a point in fewer
/// cells rather than shrinking them.
pub const RADIUS_MIN: f32 = 0.16;
/// The twinkle's depth (the share of a point's light it swings) and its
/// rate in silence (cycles a second, times 1, 2 or 3 by point).
pub const TWINKLE_DEPTH: f32 = 0.5;
pub const TWINKLE_HZ: f32 = 0.25;
/// A point's peak alpha with the room plain (immersion level 0) and with
/// it black.
pub const ALPHA_LOW: f32 = 0.06;
pub const ALPHA_HIGH: f32 = 0.8;
/// `depthlightgain`'s range.
pub const GAIN_MAX: f32 = 4.0;
/// The crests' light in silence, as a share of a loud moment's.
pub const MUSIC_FLOOR: f32 = 0.4;
/// The music's smoothing: the time constants (s) while the band's level
/// rises and while it falls.
pub const MUSIC_ATTACK_S: f32 = 0.05;
pub const MUSIC_RELEASE_S: f32 = 0.3;
/// The wave's drift (bands a second, up) in silence and at a full band;
/// the beat envelope adds up to as much again, as it speeds the surfaces'
/// clock.
pub const DRIFT_BANDS_S: [f32; 2] = [0.03, 0.15];
/// The map noise the stability test applies (m), the recordings' near
/// noise.
pub const JITTER_TEST_M: f32 = 0.004;

fn unit(x: f32) -> f32 {
    if x.is_finite() {
        x.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// WGSL's `smoothstep`.
pub fn smoothstep(low: f32, high: f32, x: f32) -> f32 {
    let t = ((x - low) / (high - low)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// What the knobs ask.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DepthLightOptions {
    /// `debug.fosfora.depthlight`.
    pub on: bool,
    /// `debug.fosfora.depthlightgain`: multiplies the peak alpha.
    pub gain: f32,
    /// `debug.fosfora.depthlightfill`: the share covered by points.
    pub fill: f32,
    /// `debug.fosfora.depthlightveil`: the surfaces' light shows through
    /// with the room dark (on), or the hard cutout at every level (off).
    pub veil: bool,
    /// The veil's knob is set to 0 or 1, so it decides and the room's
    /// theme does not ([`Self::under`]).
    pub veil_forced: bool,
    /// `debug.fosfora.depthlightpoints`: the points drawn (on), or none,
    /// the mesh still drawn with the veil and in its order (off): the
    /// points' cost apart, for a cost run.
    pub points: bool,
}

impl Default for DepthLightOptions {
    fn default() -> Self {
        Self {
            on: true,
            gain: 1.0,
            fill: FILL,
            veil: true,
            veil_forced: false,
            points: true,
        }
    }
}

impl DepthLightOptions {
    /// The options the five knobs ask, each read through `prop` (a system
    /// property by name, `None` unset): `depthlight` and `depthlightveil`
    /// off only at `0`; an unreadable number is the default, one out of
    /// range the nearest end. `depthlightpoints` off only at `0` too. The
    /// veil's knob at `0` or `1` also takes the veil from the room's theme
    /// ([`Self::under`]); unset or empty leaves it the theme's.
    pub fn from_knobs(prop: impl Fn(&str) -> Option<String>) -> Self {
        let d = Self::default();
        let number = |name: &str| {
            prop(name)
                .and_then(|v| v.trim().parse::<f32>().ok())
                .filter(|v| v.is_finite())
        };
        let off = |name: &str| prop(name).as_deref().map(str::trim) == Some("0");
        Self {
            on: !off(KNOB),
            gain: number(KNOB_GAIN).map_or(d.gain, |v| v.clamp(0.0, GAIN_MAX)),
            fill: number(KNOB_FILL).map_or(d.fill, |v| v.clamp(FILL_RANGE[0], FILL_RANGE[1])),
            veil: !off(KNOB_VEIL),
            veil_forced: matches!(prop(KNOB_VEIL).as_deref().map(str::trim), Some("0" | "1")),
            points: !off(KNOB_POINTS),
        }
    }

    /// These options in a room whose saved theme is `theme` (board
    /// #5543): a theme whose depth light occludes (`themes::Depth::
    /// occlude`) keeps the hard cutout at every level, unless the veil's
    /// knob is set. With no theme the veil is the knob's.
    #[must_use]
    pub fn under(self, theme: Option<Theme>) -> Self {
        let occlude = theme.is_some_and(|t| t.depth().occlude);
        Self {
            veil: self.veil && (self.veil_forced || !occlude),
            ..self
        }
    }

    /// Whether the light draws: asked for and not `blocked`.
    pub fn lit(&self, blocked: Option<&str>) -> bool {
        self.on && blocked.is_none()
    }

    /// The state in a few words, for the log.
    pub fn describe(&self) -> String {
        if !self.on {
            return "off (debug.fosfora.depthlight 0): the depth map writes depth only".to_owned();
        }
        format!(
            "on, points · gain {} · fill {} · {} · a point's peak alpha {ALPHA_LOW} with the room plain to {ALPHA_HIGH} black · cells {CELL_M} m at 1 m, doubling each octave · the light falls to {FAR_FLOOR} from {} to {} m · two masses part at {JUMP_MIN_M} m or {JUMP_REL} of the distance",
            self.gain,
            self.fill,
            match (self.veil, self.points) {
                (true, true) if !self.veil_forced =>
                    "veil the theme's: the hard cutout where its depth light occludes, else the surfaces' light shows through by the immersion level",
                (true, true) => "veil on: the surfaces' light shows through by the immersion level",
                (false, true) =>
                    "veil off (debug.fosfora.depthlightveil 0): the hard cutout at every level",
                (true, false) => "no points (debug.fosfora.depthlightpoints 0), the veil alone",
                (false, false) =>
                    "no points (debug.fosfora.depthlightpoints 0) and the hard cutout (debug.fosfora.depthlightveil 0)",
            },
            FADE_M[0],
            FADE_M[1],
        )
    }

    /// The launch line; `blocked` says why the light cannot draw in this
    /// run ([`blocked`]).
    pub fn line(&self, blocked: Option<&str>) -> String {
        match blocked {
            Some(why) if self.on => format!("depth light: off, {why} (asked: {})", self.describe()),
            _ => format!("depth light: {}", self.describe()),
        }
    }

    /// The line for a change from `before`.
    pub fn change_line(&self, before: &Self, blocked: Option<&str>) -> String {
        format!("{} (was: {})", self.line(blocked), before.describe())
    }
}

/// Why the light cannot draw in a run whose environment depth occludes
/// (`occlude`), shows the gray diagnostic (`show`) and draws the eye pass's
/// mesh at `mesh_step` (`EnvDepthOptions::drawn_mesh_step`, 0 the
/// full-screen pass); `None` when it can.
pub fn blocked(occlude: bool, show: bool, mesh_step: u32) -> Option<&'static str> {
    if show {
        Some("the diagnostic gray draws the depth map (debug.fosfora.envdepthshow 1)")
    } else if !occlude {
        Some("the depth map does not occlude in this run (debug.fosfora.envdepth)")
    } else if mesh_step == 0 {
        Some(
            "it needs the occluder mesh, and the full-screen pass draws (envdepthmesh 0, or envdepthfilter 0 or 1)",
        )
    } else {
        None
    }
}

/// A point's peak alpha at immersion level `level` (0..1) and gain `gain`:
/// from [`ALPHA_LOW`] at level 0 to [`ALPHA_HIGH`] at 1, times the gain.
pub fn peak_alpha(level: f32, gain: f32) -> f32 {
    let gain = if gain.is_finite() {
        gain.clamp(0.0, GAIN_MAX)
    } else {
        0.0
    };
    (ALPHA_LOW + (ALPHA_HIGH - ALPHA_LOW) * unit(level)) * gain
}

/// The veil at immersion level `level`: the share of the surfaces' light
/// behind the map that the map erases, `1 - level`; 1 at every level with
/// the veil off (`on` false), which with the mesh drawn first changes
/// nothing but its own folds.
pub fn veil_share(level: f32, on: bool) -> f32 {
    if on { 1.0 - unit(level) } else { 1.0 }
}

/// The veil's share within which it counts as none or all (`veil_passes`).
pub const VEIL_EPS: f32 = 1e-3;

/// How the passes draw the mesh this frame with the veil asked for (`on`)
/// at share `v` ([`veil_share`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VeilPasses {
    /// The surfaces draw before the mesh, which erases `v` of them. Not at
    /// `v` 1 (the room plain): the mesh first, as without the veil, gives
    /// the same frame without shading the surfaces behind real things.
    pub surfaces_first: bool,
    /// The faces pass draws the mesh: first and depth only without the
    /// veil's order, after its surfaces with the veil alone in it, but not
    /// at `v` 0 (the room black), where there is nothing to erase.
    pub faces_mesh: bool,
}

/// [`VeilPasses`] for the veil asked for (`on`) at share `v`.
pub fn veil_passes(on: bool, v: f32) -> VeilPasses {
    let surfaces_first = on && v < 1.0 - VEIL_EPS;
    VeilPasses {
        surfaces_first,
        faces_mesh: !surfaces_first || v > VEIL_EPS,
    }
}

/// What a pass draws the mesh with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeshDraw {
    /// Depth alone, color masked off: the occluder as before the light.
    Depth,
    /// Depth and the veil alone (`fs_mesh_veil`): the veil's share of the
    /// light behind erased, nothing added.
    Veil,
    /// Depth, the veil and the points (`fs_mesh_light`).
    Light,
}

/// How the eye pass and the faces pass draw the mesh in a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeshDraws {
    pub eye: MeshDraw,
    /// `None`: the faces pass draws no mesh.
    pub faces: Option<MeshDraw>,
    /// Both passes draw their surfaces before the mesh.
    pub surfaces_first: bool,
}

/// How the passes draw the mesh with the light on or not (`lit`), the veil
/// asked for (`veil_on`) at share `v`, and with a faces layer this frame
/// (`faces_layer`). With a faces layer the points go there: at half the
/// eye's resolution a quarter of the eye's fragments shade them, and the
/// runtime's upsample of the layer keeps them soft. The eye pass then
/// draws the mesh for its depth (the particles) and the veil of its own
/// light (the edge bars): with the veil alone while it lets some light
/// through and erases some, depth alone otherwise. Without a faces layer
/// the eye pass draws the points. Without the light, as before: depth
/// alone, first.
pub fn mesh_draws(lit: bool, veil_on: bool, v: f32, faces_layer: bool) -> MeshDraws {
    if !lit {
        return MeshDraws {
            eye: MeshDraw::Depth,
            faces: Some(MeshDraw::Depth),
            surfaces_first: false,
        };
    }
    let passes = veil_passes(veil_on, v);
    if faces_layer {
        MeshDraws {
            eye: if passes.surfaces_first && v > VEIL_EPS {
                MeshDraw::Veil
            } else {
                MeshDraw::Depth
            },
            faces: Some(MeshDraw::Light),
            surfaces_first: passes.surfaces_first,
        }
    } else {
        MeshDraws {
            eye: MeshDraw::Light,
            faces: None,
            surfaces_first: passes.surfaces_first,
        }
    }
}

/// What the blend leaves in a pixel holding `dst` (premultiplied RGBA)
/// when the mesh's fragment writes `src`, whose alpha is the veil: the
/// veil's share of `dst` erased in color and alpha, `src`'s light added
/// (rgb One, OneMinusSrcAlpha; alpha Zero, OneMinusSrcAlpha). The faces
/// pass's veil is the same with no light.
pub fn veil(dst: [f32; 4], src: [f32; 4]) -> [f32; 4] {
    let keep = 1.0 - src[3];
    [
        src[0] + dst[0] * keep,
        src[1] + dst[1] * keep,
        src[2] + dst[2] * keep,
        dst[3] * keep,
    ]
}

/// Whether `theme` gives the light the key's tint, on its sides or its
/// tops.
pub fn uses_key(theme: Option<Theme>) -> bool {
    theme.is_some_and(|t| t.depth().colors.contains(&COLOR_KEY))
}

/// The color indices of the light's sides and tops under `theme`
/// (`themes::Depth::colors`); 0 (other's own) for both with no theme.
pub fn color_indices(theme: Option<Theme>) -> [u32; 2] {
    theme.map_or([0, 0], |t| t.depth().colors)
}

/// The sides' and the tops' colors (linear RGB) under `theme` while the
/// key's tint is `key`: the theme's two for the depth light (board #5543).
/// With no theme, other's own color on both.
pub fn colors(theme: Option<Theme>, key: [f32; 3]) -> [[f32; 3]; 2] {
    color_indices(theme).map(|i| surface_color(i, KIND_OTHER, key))
}

/// The colors in words, for the log: "depth light: beams as points, teal
/// sides, the key's tint on top, bass, occluding (theme aurora)".
pub fn colors_line(theme: Option<Theme>) -> String {
    let word = |i: u32| match color_name(i) {
        "kind" => "other's own",
        "key" => "the key's tint",
        name => name,
    };
    let [side, top] = color_indices(theme);
    let top = if top == side {
        "the same on top".to_owned()
    } else {
        format!("{} on top", word(top))
    };
    let Some(theme) = theme else {
        return format!(
            "depth light: {} sides, {top}, {} (no theme saved)",
            word(side),
            crate::surface_fx::band_name(band(None)),
        );
    };
    let depth = theme.depth();
    let look = if depth.look.built() {
        depth.look.name().to_owned()
    } else {
        format!("{} as points", depth.look.name())
    };
    format!(
        "depth light: {look}, {} sides, {top}, {}{} (theme {})",
        word(side),
        crate::surface_fx::band_name(depth.band),
        if depth.occlude { ", occluding" } else { "" },
        theme.name()
    )
}

/// The audio band (`surface_fx::BAND_*`) the crests and the swell follow
/// under `theme`: the one it gives the depth light; with no theme, other's
/// default behavior's.
pub fn band(theme: Option<Theme>) -> u32 {
    theme.map_or_else(
        || SurfaceBehavior::default_for(KIND_OTHER).default_band(),
        |t| t.depth().band,
    )
}

/// `band`'s level in `audio`, 0..1, as the surfaces pass reads it.
pub fn band_level(audio: &Audio, band: u32) -> f32 {
    unit(match band {
        BAND_BASS => audio.bass,
        BAND_MID => audio.mid,
        BAND_HIGH => audio.high,
        _ => audio.rms,
    })
}

/// The music the light follows, the wave's phase and the twinkle's clock,
/// stepped once a frame.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Drive {
    music: f32,
    phase: f32,
    twinkle: f32,
    /// The beams' travel, 0..1 of a period.
    travel: f32,
    /// The grass's sway, 0..1 of a cycle.
    sway: f32,
    /// The loudness the grass gusts with, smoothed, 0..1.
    gust: f32,
    /// The bubbles' rise, 0..1 of a period.
    rise: f32,
}

impl Drive {
    /// Advance by `dt` seconds with the band's level `level` and the beat
    /// envelope `beat` (both 0..1): the music one-pole toward the level
    /// ([`MUSIC_ATTACK_S`] rising, [`MUSIC_RELEASE_S`] falling); the wave's
    /// phase on by [`DRIFT_BANDS_S`] at that music and the twinkle's clock
    /// by [`TWINKLE_HZ`] times one plus the music, both times one plus the
    /// beat and wrapped to a whole cycle. The beams' travel goes on by
    /// [`BEAM_TRAVEL_HZ`] at that music squared, times one plus the beat:
    /// slow in silence and launched by a kick. The bubbles' rise goes on
    /// by [`BUBBLE_RISE_HZ`] at that music, times one plus the beat.
    pub fn step(&mut self, dt: f32, level: f32, beat: f32) {
        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
        let level = unit(level);
        let tau = if level > self.music {
            MUSIC_ATTACK_S
        } else {
            MUSIC_RELEASE_S
        };
        self.music += (level - self.music) * (1.0 - (-dt / tau).exp());
        let beat = 1.0 + unit(beat);
        let speed = DRIFT_BANDS_S[0] + (DRIFT_BANDS_S[1] - DRIFT_BANDS_S[0]) * self.music;
        self.phase = (self.phase + dt * speed * beat).fract();
        self.twinkle = (self.twinkle + dt * TWINKLE_HZ * (1.0 + self.music) * beat).fract();
        let launch =
            BEAM_TRAVEL_HZ[0] + (BEAM_TRAVEL_HZ[1] - BEAM_TRAVEL_HZ[0]) * self.music * self.music;
        self.travel = (self.travel + dt * launch * beat).fract();
        let rise = BUBBLE_RISE_HZ[0] + (BUBBLE_RISE_HZ[1] - BUBBLE_RISE_HZ[0]) * self.music;
        self.rise = (self.rise + dt * rise * beat).fract();
    }

    /// Advance the wind by `dt` seconds with the overall loudness
    /// `loudness` (0..1), for the grass (board #5543): the gust one-pole
    /// toward it ([`GUST_ATTACK_S`] rising, [`GUST_RELEASE_S`] falling)
    /// and the sway's clock on by [`SWAY_HZ`] times one plus the gust.
    pub fn step_wind(&mut self, dt: f32, loudness: f32) {
        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
        let loudness = unit(loudness);
        let tau = if loudness > self.gust {
            GUST_ATTACK_S
        } else {
            GUST_RELEASE_S
        };
        self.gust += (loudness - self.gust) * (1.0 - (-dt / tau).exp());
        self.sway = (self.sway + dt * SWAY_HZ * (1.0 + self.gust)).fract();
    }

    /// The smoothed music, 0..1.
    pub fn music(&self) -> f32 {
        self.music
    }

    /// This drive with its music at `music` (0..1), for what the flash
    /// limit lets through of it (board #5451, `flash.rs`).
    #[must_use]
    pub fn with_music(mut self, music: f32) -> Self {
        self.music = unit(music);
        self
    }

    /// The wave's phase, 0..1 of a band.
    pub fn phase(&self) -> f32 {
        self.phase
    }

    /// The twinkle's clock, 0..1 of a cycle.
    pub fn twinkle(&self) -> f32 {
        self.twinkle
    }

    /// The beams' travel, 0..1 of a period.
    pub fn travel(&self) -> f32 {
        self.travel
    }

    /// The grass's sway, 0..1 of a cycle.
    pub fn sway(&self) -> f32 {
        self.sway
    }

    /// The gust, 0..1.
    pub fn gust(&self) -> f32 {
        self.gust
    }

    /// The bubbles' rise, 0..1 of a period.
    pub fn rise(&self) -> f32 {
        self.rise
    }
}

/// The look the light draws for a theme's `look`: itself once it is built
/// (`DepthLook::built`), the points until then.
pub fn drawn(look: DepthLook) -> DepthLook {
    if look.built() {
        look
    } else {
        DepthLook::Points
    }
}

/// A look's id in the light's rows ([`LOOK_ROW`]) and in the WGSL: the
/// points 0, then `DepthLook::ALL`'s order, so a look built later has its
/// number already.
pub fn look_id(look: DepthLook) -> u32 {
    match look {
        DepthLook::Points => 0,
        DepthLook::Beams => 1,
        DepthLook::Coals => 2,
        DepthLook::Grass => 3,
        DepthLook::Bubbles => 4,
        DepthLook::Sparkle => 5,
        DepthLook::Glints => 6,
        DepthLook::Meters => 7,
    }
}

/// The look a room whose saved theme is `theme` draws ([`drawn`] of the
/// theme's); the points with no theme.
pub fn look(theme: Option<Theme>) -> DepthLook {
    theme.map_or(DepthLook::Points, |t| drawn(t.depth().look))
}

/// The base radius (cells) and the share of cells with a point for a fill
/// of `fill` (the share covered): the radius that covers it with a point in
/// every cell, but at least [`RADIUS_MIN`] (then fewer cells hold one) and
/// at most what keeps the largest swollen point within half a cell.
pub fn point_layout(fill: f32) -> (f32, f32) {
    let fill = if fill.is_finite() {
        fill.clamp(FILL_RANGE[0], FILL_RANGE[1])
    } else {
        FILL
    };
    let area = PI * size_square_mean();
    let largest = 0.5 / (SIZE[1] * (1.0 + SWELL));
    let radius = (fill / area).sqrt().clamp(RADIUS_MIN, largest);
    (radius, (fill / (area * radius * radius)).min(1.0))
}

/// The mean of a point's size factor squared ([`SIZE`], uniform).
pub fn size_square_mean() -> f32 {
    let [a, b] = SIZE;
    (b * b * b - a * a * a) / (3.0 * (b - a))
}

/// A soft point's light over its disk, as a share of the disk's area: 1
/// inside `soft` of the radius, a smoothstep down to 0 at the radius.
pub fn profile_share(soft: f32) -> f32 {
    soft + 0.3 * (1.0 - soft) * (1.0 - soft)
}

/// This frame's rows and whether the veil is on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    pub rows: [[f32; 4]; ROWS],
    /// The surfaces draw before the mesh and show through it by the level.
    pub veil: bool,
}

/// The light's rows of `struct EnvEye` (from `light` on, written at
/// `env_depth::LIGHT_ROW` of both passes' uniforms) for `drive`, the peak
/// alpha `peak` ([`peak_alpha`]), the sides' and tops' colors
/// ([`colors`]), the fill and the veil's share ([`veil_share`]).
pub fn rows(
    drive: &Drive,
    peak: f32,
    [side, top]: [[f32; 3]; 2],
    fill: f32,
    veil: f32,
) -> [[f32; 4]; ROWS] {
    let music = unit(drive.music);
    let (radius, presence) = point_layout(fill);
    let own = [
        [
            1.0 / BAND_SPACING_M,
            drive.phase,
            peak,
            MUSIC_FLOOR + (1.0 - MUSIC_FLOOR) * music,
        ],
        [side[0], side[1], side[2], STEADY],
        [top[0], top[1], top[2], BAND_TINT],
        [
            AA_BANDS_PER_PX[0],
            AA_BANDS_PER_PX[1],
            CREST_MEAN,
            drive.twinkle,
        ],
        [FADE_M[0], FADE_M[1], FAR_FLOOR, unit(veil)],
        [JUMP_MIN_M, JUMP_REL, JUMP_SOFT, NEAR_KEEP],
        [CELL_M, OCTAVE_BLEND, AA_CELLS_PER_PX[0], AA_CELLS_PER_PX[1]],
        [radius, presence, REACH, 1.0 + SWELL * music],
        [SIZE[0], SIZE[1], SOFT_CORE, TWINKLE_DEPTH],
        // The look: the points until `with_look` names another.
        [0.0, music, unit(drive.gust), 0.0],
        [drive.travel, drive.sway, drive.rise, 0.0],
        // What the look reads of the room: nothing until `with_around`.
        [0.0; 4],
    ];
    // The sweep's rows stay 0, no sweep, until `with_sonar` sets them.
    let mut rows = [[0.0f32; 4]; ROWS];
    rows[..SONAR_ROW].copy_from_slice(&own);
    rows
}

/// `rows` drawing `look` ([`drawn`]: the points for one that is not
/// built).
pub fn with_look(mut rows: [[f32; 4]; ROWS], look: DepthLook) -> [[f32; 4]; ROWS] {
    rows[LOOK_ROW][0] = look_id(drawn(look)) as f32;
    rows
}

/// What a look reads of the room beside the drive (board #5543).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Around {
    /// The wash's front, for the sparkle ([`front`]): the way it travels
    /// over the floor (x, z, a unit), where it is along that way (m) and
    /// one over the half-width the points flare in (1/m). All 0 for none.
    pub front: [f32; 4],
}

/// `rows` with what their look reads of `around` in [`MORE_ROW`]: the
/// wash's front for the sparkle, nothing for a look that reads none.
pub fn with_around(mut rows: [[f32; 4]; ROWS], around: &Around) -> [[f32; 4]; ROWS] {
    rows[MORE_ROW] = match look_of(&rows) {
        DepthLook::Sparkle => around.front,
        _ => [0.0; 4],
    };
    rows
}

/// WASH FRONT HOOK. Where the floor's wash front is at `bar_phase` (0..1
/// of the bar) for a wearer at `center`, as [`Around::front`] has it. The
/// wash (Tide's floor) is not merged here yet, so this is a stand-in with
/// the wash's rhythm, one advance and one retreat a bar: a front square to
/// the world's -z that runs from [`SPARKLE_REACH_M`] behind the wearer to
/// as far ahead over the first half of the bar and back over the second.
/// When the wash lands, its own front goes here: the direction it runs
/// over the floor and its place along it, from the same bar phase.
pub fn wash_front(bar_phase: f32, center: Vec3) -> [f32; 4] {
    if !bar_phase.is_finite() {
        return [0.0; 4];
    }
    let out = 1.0 - (2.0 * bar_phase.rem_euclid(1.0) - 1.0).abs();
    let at = -center.z + SPARKLE_REACH_M * (2.0 * out - 1.0);
    [0.0, -1.0, at, 1.0 / SPARKLE_FRONT_M]
}

/// The wash's front in a room whose saved theme is `theme`
/// ([`wash_front`]): Tide's, whose floor the wash runs over. No other
/// theme has a wash, so its sparkle, if it had one, flares on its band
/// alone.
pub fn front(theme: Option<Theme>, bar_phase: f32, center: Vec3) -> [f32; 4] {
    if theme == Some(Theme::Tide) {
        wash_front(bar_phase, center)
    } else {
        [0.0; 4]
    }
}

/// The look `rows` draw.
pub fn look_of(rows: &[[f32; 4]; ROWS]) -> DepthLook {
    DepthLook::ALL
        .into_iter()
        .find(|l| (look_id(*l) as f32 - rows[LOOK_ROW][0]).abs() < 0.5)
        .unwrap_or(DepthLook::Points)
}

/// `rows` with the sweep's rows `sonar` (`sonar::Sonar::rows`, board
/// #5511; all 0 for none).
pub fn with_sonar(
    mut rows: [[f32; 4]; ROWS],
    sonar: &[[f32; 4]; crate::sonar::ROWS],
) -> [[f32; 4]; ROWS] {
    rows[SONAR_ROW..].copy_from_slice(sonar);
    rows
}

/// Whether a sweep or a standing line shows in `rows`: the sweep's peak
/// is not 0. The light's draw then takes `fs_mesh_light_sonar`; otherwise
/// `fs_mesh_light`, as before the sweep.
pub fn swept(rows: &[[f32; 4]; ROWS]) -> bool {
    rows[SONAR_ROW][0] > 0.0
}

/// The sweep's rows of `rows`.
pub fn sonar_rows(rows: &[[f32; 4]; ROWS]) -> [[f32; 4]; crate::sonar::ROWS] {
    let mut out = [[0.0f32; 4]; crate::sonar::ROWS];
    out.copy_from_slice(&rows[SONAR_ROW..]);
    out
}

/// `rows` for a target whose pixels are `scale` times the eye's in size
/// (the faces layer: 2): the points' and the wave's fades start and end at
/// `scale` times the cells and bands per pixel, so they fade at the same
/// size on the eye's screen.
pub fn scale_fades(mut rows: [[f32; 4]; ROWS], scale: f32) -> [[f32; 4]; ROWS] {
    for (row, col) in [(3, 0), (3, 1), (6, 2), (6, 3)] {
        rows[row][col] *= scale;
    }
    rows
}

/// This frame's [`Frame`] under the options `opts`, with `drive` stepped,
/// the room's `theme`, the key's tint `key`, the immersion `level`, the
/// sweep's rows `sonar` (`sonar::Sonar::rows`; all 0 for none) and what
/// the look reads of the room, `around`.
pub fn frame(
    opts: &DepthLightOptions,
    drive: &Drive,
    theme: Option<Theme>,
    key: [f32; 3],
    level: f32,
    sonar: &[[f32; 4]; crate::sonar::ROWS],
    around: &Around,
) -> Frame {
    Frame {
        rows: with_sonar(
            with_around(
                with_look(
                    rows(
                        drive,
                        if opts.points {
                            peak_alpha(level, opts.gain)
                        } else {
                            0.0
                        },
                        colors(theme, key),
                        opts.fill,
                        veil_share(level, opts.veil),
                    ),
                    look(theme),
                ),
                around,
            ),
            sonar,
        ),
        veil: opts.veil,
    }
}

/// What the vertex stage hands on for one vertex of the mesh.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VertexLight {
    /// The clip position, the occluder's own (`EyeReprojection::
    /// vertex_clip`).
    pub clip: Vec4,
    /// The world point.
    pub world: Vec3,
    /// How much of its points the vertex keeps, 0..1.
    pub keep: f32,
    /// The surface's normal turned toward the eye (unit).
    pub normal: Vec3,
    /// The light left at the vertex's distance, 0..1.
    pub bright: f32,
}

/// The vertex stage of the light (`mesh_light` in
/// `env_depth::ENV_DEPTH_WGSL`) for the vertex of `texel` in `r`'s mesh
/// (`r.mesh_step`), with the light's `rows` and the map's stored values
/// through `stored`. The normal: one-sided differences one mesh step along
/// each axis, toward the neighbor nearer in depth (a neighbor clamped to
/// the vertex at the map's border, or without data, does not count), the
/// way to the eye without one along an axis. What the vertex keeps: the
/// largest step to a nearer neighbor thins it to none and to a farther one
/// to [`NEAR_KEEP`], from [`JUMP_SOFT`] of a mass's step ([`JUMP_MIN_M`],
/// [`JUMP_REL`]) to all of it. The light by distance, all of it to
/// [`FADE_M`]'s start and [`FAR_FLOOR`] past its end. Nothing without data.
pub fn vertex_light(
    r: &EyeReprojection,
    rows: &[[f32; 4]; ROWS],
    texel: [u32; 2],
    stored: impl Fn([u32; 2]) -> f32,
) -> VertexLight {
    let dist = r.texel_distance(stored(texel));
    let p = r.mesh_point(texel, dist);
    let clip = r.vertex_clip(texel, stored(texel));
    let s = r.mesh_step;
    let last = [
        r.depth_size[0].saturating_sub(1),
        r.depth_size[1].saturating_sub(1),
    ];
    let [x, y] = texel;
    let neighbors = [
        [x.saturating_sub(s), y],
        [(x + s).min(last[0]), y],
        [x, y.saturating_sub(s)],
        [x, (y + s).min(last[1])],
    ];
    let inside = [x > 0, x < last[0], y > 0, y < last[1]];
    let d = neighbors.map(|t| r.texel_distance(stored(t)));
    let has: [bool; 4] = std::array::from_fn(|k| d[k].is_some() && inside[k]);
    let to_eye = r.eye_pos - p;
    let range = to_eye.length();
    let mut n = to_eye;
    let (mut keep, mut bright) = (0.0, 0.0);
    if let Some(d0) = dist.filter(|_| range > 0.0) {
        let ray = |t: [u32; 2]| r.depth_ray(r.mesh_uv(t));
        let dv = |k: usize| d[k].unwrap_or(-1.0);
        let gap = |k: usize| {
            if has[k] { (dv(k) - d0).abs() } else { 1e30 }
        };
        let q = ray(texel) * d0;
        let side = |a: usize, b: usize| {
            if gap(b) <= gap(a) {
                ray(neighbors[b]) * dv(b) - q
            } else {
                q - ray(neighbors[a]) * dv(a)
            }
        };
        let g = r
            .depth_view
            .inverse()
            .transform_vector3(side(0, 1).cross(side(2, 3)));
        if (has[0] || has[1]) && (has[2] || has[3]) && g.dot(g) > 0.0 {
            n = g;
        }
        let largest = |f: &dyn Fn(f32) -> f32| {
            (0..4)
                .filter(|&k| has[k])
                .map(|k| f(dv(k)))
                .fold(0.0f32, f32::max)
        };
        let nearer = largest(&|dk| d0 - dk);
        let farther = largest(&|dk| dk - d0);
        let [jump, fade] = [rows[5], rows[4]];
        let part = jump[0].max(jump[1] * d0);
        let from = jump[2] * part;
        keep = (1.0 - smoothstep(from, part, nearer))
            * (1.0 - (1.0 - jump[3]) * smoothstep(from, part, farther));
        bright = 1.0 + (fade[2] - 1.0) * smoothstep(fade[0], fade[1], range);
    }
    if n.dot(to_eye) < 0.0 {
        n = -n;
    }
    let len = n.length();
    VertexLight {
        clip,
        world: p,
        keep,
        normal: if len > 0.0 { n / len } else { Vec3::Z },
        bright,
    }
}

/// One PCG step, the points' tile's hash.
pub fn hash(v: u32) -> u32 {
    let s = v.wrapping_mul(747_796_405).wrapping_add(2_891_336_453);
    let w = ((s >> ((s >> 28) + 4)) ^ s).wrapping_mul(277_803_737);
    (w >> 22) ^ w
}

/// Coordinates in the cells' plane `k`: across x (0), y (1) or z (2).
pub fn light_plane(v: Vec3, k: u32) -> Vec2 {
    match k {
        0 => Vec2::new(v.y, v.z),
        1 => Vec2::new(v.x, v.z),
        _ => Vec2::new(v.x, v.y),
    }
}

/// Where the tile is read for plane `k` at octave `o`, as an offset of
/// whole tiles' share (the WGSL's `light_offset`): each plane and octave
/// reads it from its own place, so neighbors do not repeat each other.
pub fn tile_offset(k: u32, o: i32) -> Vec2 {
    let (k, o) = (k as f32, o as f32);
    Vec2::new(0.31 * k + 0.17 * o, 0.57 * k + 0.29 * o)
}

/// Cell `c` of the tile's point, from the cell's hash: its center (cells,
/// anywhere in the cell), its size (0..1 over [`SIZE`]), its twinkle's
/// phase and its rank (the fill keeps the points ranked under the share of
/// cells it asks for).
pub fn tile_point(c: [u32; 2]) -> (Vec2, f32, f32, f32) {
    let a = hash(c[0] + TILE_CELLS * c[1] + 0x5bd1_e995);
    let b = hash(a);
    let unit = |v: u32| (v & 65535) as f32 / 65536.0;
    let center = Vec2::new(c[0] as f32 + unit(a), c[1] as f32 + unit(a >> 16));
    (center, unit(b), unit(b >> 16), unit(hash(b)))
}

/// The tile of soft points the fragment samples ([`TILE`] texels a side,
/// [`TILE_CELLS`] cells, a point in each, wrapping), made once: per texel,
/// RGBA8, R `1 - d / REACH` for `d` the distance (cells) to the nearest
/// point's center, 0 past the reach; G that point's twinkle phase; B its
/// size; A its rank. Row-major, row 0 first.
pub fn points_tile() -> Vec<[u8; 4]> {
    let per = TILE / TILE_CELLS;
    let n = TILE_CELLS as i32;
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    let mut out = Vec::with_capacity((TILE * TILE) as usize);
    for y in 0..TILE {
        for x in 0..TILE {
            let g = Vec2::new((x as f32 + 0.5) / per as f32, (y as f32 + 0.5) / per as f32);
            let (gx, gy) = ((x / per) as i32, (y / per) as i32);
            let mut best = (f32::INFINITY, 0.0, 0.0, 0.0);
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let (cx, cy) = (gx + dx, gy + dy);
                    let wrap = [cx.rem_euclid(n) as u32, cy.rem_euclid(n) as u32];
                    let (center, phase, size, rank) = tile_point(wrap);
                    let shift =
                        Vec2::new((cx - wrap[0] as i32) as f32, (cy - wrap[1] as i32) as f32);
                    let d = (g - center - shift).length();
                    if d < best.0 {
                        best = (d, phase, size, rank);
                    }
                }
            }
            let (d, phase, size, rank) = best;
            out.push([byte(1.0 - d / REACH), byte(phase), byte(size), byte(rank)]);
        }
    }
    out
}

/// The tile read at `uv` (tiles; it repeats) as the GPU's linear filter
/// reads RGBA8 at level 0: the four texels around, by their centers.
pub fn sample_tile(tile: &[[u8; 4]], uv: Vec2) -> Vec4 {
    let t = uv * TILE as f32 - 0.5;
    let base = t.floor();
    let f = t - base;
    let at = |dx: i32, dy: i32| {
        let x = (base.x as i32 + dx).rem_euclid(TILE as i32) as usize;
        let y = (base.y as i32 + dy).rem_euclid(TILE as i32) as usize;
        let [r, g, b, a] = tile[y * TILE as usize + x];
        Vec4::new(r as f32, g as f32, b as f32, a as f32) / 255.0
    };
    let top = at(0, 0).lerp(at(1, 0), f.x);
    let bottom = at(0, 1).lerp(at(1, 1), f.x);
    top.lerp(bottom, f.y)
}

/// A smooth wave with period 1, 1 at whole numbers and 0 halfway: a
/// triangle eased by a smoothstep, the light's cheap stand-in for a
/// cosine (the WGSL's `light_wave`).
pub fn wave(x: f32) -> f32 {
    let t = ((x - x.floor()) * 2.0 - 1.0).abs();
    t * t * (3.0 - 2.0 * t)
}

/// The points of plane `k` at octave `o` at `g` (cells), 0..1, from one
/// read of the tile: the nearest point's disk at its size, the fill's
/// radius ([`point_layout`]), the music's swell and the vertex's keep
/// (`radius_scale`, all but the size), soft from [`SOFT_CORE`] of the
/// radius; none where the point's rank is past the fill's share of cells;
/// twinkling at 1, 2 or 3 times the clock by its size, from its own phase.
/// The WGSL's `plane_points`.
pub fn plane_points(
    rows: &[[f32; 4]; ROWS],
    tile: &[[u8; 4]],
    g: Vec2,
    k: u32,
    o: i32,
    radius_scale: f32,
) -> f32 {
    let [wave_row, point, size_row] = [rows[3], rows[7], rows[8]];
    let s = sample_tile(tile, g / TILE_CELLS as f32 + tile_offset(k, o));
    let d = (1.0 - s.x) * point[2];
    let radius = radius_scale * (size_row[0] + (size_row[1] - size_row[0]) * s.z);
    if s.w >= point[1] || radius <= 1e-4 {
        return 0.0;
    }
    let core = 1.0 - smoothstep(radius * size_row[2], radius, d);
    let rate = 1.0 + (s.z * 2.999).floor();
    let twinkle = 1.0 - size_row[3] + size_row[3] * wave(rate * wave_row[3] + s.y);
    core * twinkle
}

/// The interpolated normal made unit, +Z where it has no length (two
/// opposite normals across a fold), as the WGSL does.
pub fn unit_normal(n: Vec3) -> Vec3 {
    if n.dot(n) > 1e-12 {
        n.normalize()
    } else {
        Vec3::Z
    }
}

/// The points' light at world point `p` seen from `eye` with the
/// interpolated normal `normal` and keep `keep`, where the world moves by
/// `fw` across a pixel (`fwidth`), before the peak, the distance and the
/// wave: the planes and octaves of the WGSL's fragment, one read of the
/// tile each.
pub fn points(
    rows: &[[f32; 4]; ROWS],
    tile: &[[u8; 4]],
    eye: Vec3,
    p: Vec3,
    normal: Vec3,
    keep: f32,
    fw: Vec3,
) -> f32 {
    points_under(rows, tile, eye, p, normal, keep, fw, None)
}

/// [`points`] under the sweep (`sonar.rs`): with `under` the sweep's light
/// on the fragment (the brighter of its line and its tail, 0..1), the
/// points' radius grows by the sweep's swell times it, as far as keeps the
/// largest point within its reach. `None` without a sweep this frame.
#[allow(
    clippy::too_many_arguments,
    reason = "the fragment's inputs, as the WGSL has them"
)]
pub fn points_under(
    rows: &[[f32; 4]; ROWS],
    tile: &[[u8; 4]],
    eye: Vec3,
    p: Vec3,
    normal: Vec3,
    keep: f32,
    fw: Vec3,
    under: Option<f32>,
) -> f32 {
    let [cell_row, point] = [rows[6], rows[7]];
    let n = unit_normal(normal);
    let n2 = n * n;
    let tri = n2 * n2 / n2.dot(n2);
    let l = (p - eye)
        .length()
        .max(1e-3)
        .log2()
        .clamp(OCTAVE_RANGE[0], OCTAVE_RANGE[1]);
    let li = l.floor();
    let next = smoothstep(cell_row[1], 1.0, l - li);
    let mut radius_scale = point[0] * point[3] * keep.max(0.0).sqrt();
    if let Some(under) = under {
        radius_scale =
            (radius_scale * (1.0 + rows[SONAR_ROW][2] * under)).min(point[2] / rows[8][1]);
    }
    let (mut sum, mut used) = (0.0, 0.0);
    for j in 0..2 {
        let wo = if j == 1 { next } else { 1.0 - next };
        if wo <= 0.0 {
            continue;
        }
        let o = li as i32 + j;
        let cell = cell_row[0] * 2f32.powi(o);
        for k in 0..3u32 {
            let wk = tri[k as usize] - PLANE_MIN;
            if wk <= 0.0 {
                continue;
            }
            let f = light_plane(fw, k);
            let aa = 1.0 - smoothstep(cell_row[2], cell_row[3], f.x.max(f.y) / cell);
            if aa > 0.0 {
                sum += wo
                    * wk
                    * aa
                    * plane_points(rows, tile, light_plane(p, k) / cell, k, o, radius_scale);
            }
            used += wo * wk;
        }
    }
    sum / used.max(1e-6)
}

/// Across and up (m) in the cells' plane `k` for the looks: on an upright
/// plane (0 or 2) the second is the world's up; the level plane (1) has
/// none. The WGSL's `look_plane`.
pub fn look_plane(v: Vec3, k: u32) -> Vec2 {
    match k {
        0 => Vec2::new(v.z, v.y),
        1 => Vec2::new(v.x, v.z),
        _ => Vec2::new(v.x, v.y),
    }
}

/// One read of the tile at `uv` (cells) for a look, for plane `k` at
/// octave `o`: the distance to the nearest point's center (cells), its
/// radius at `radius_scale` (0 where its rank is past the fill's share,
/// so there is none), its phase and its size. The WGSL's `look_point`.
fn look_point(
    rows: &[[f32; 4]; ROWS],
    tile: &[[u8; 4]],
    uv: Vec2,
    k: u32,
    o: i32,
    radius_scale: f32,
) -> [f32; 4] {
    let [point, size_row] = [rows[7], rows[8]];
    let s = sample_tile(tile, uv / TILE_CELLS as f32 + tile_offset(k, o));
    let radius = radius_scale * (size_row[0] + (size_row[1] - size_row[0]) * s.z);
    let there = s.w < point[1] && radius > 1e-4;
    [
        (1.0 - s.x) * point[2],
        if there { radius } else { 0.0 },
        s.y,
        s.z,
    ]
}

/// Beams on an upright plane at `g` (across and up, cells), where the
/// plane moves by `f` across a pixel: the light and the light times its
/// tint. The tile is read [`BEAM_PACK`] times closer across and
/// [`BEAM_STRETCH`] times drawn out along up, so a point is a lane; in the
/// lane a streak's head passes every [`BEAM_PERIOD`] cells of height,
/// moving up with the travel at 1, 2 or 3 times it by the point's size,
/// from the point's phase. Behind the head the light falls over the
/// streak's length, [`BEAM_TAIL_MIN`] of the period in silence and all of
/// it at a full band; the head's edge is soft over [`BEAM_HEAD`] of the
/// period or two pixels. The tint is the fourth power of that fall: the
/// sides' color along the streak, the tops' at its head.
/// The WGSL's `beam`.
pub fn beam(
    rows: &[[f32; 4]; ROWS],
    tile: &[[u8; 4]],
    g: Vec2,
    f: Vec2,
    k: u32,
    o: i32,
    radius_scale: f32,
) -> Vec2 {
    let uv = Vec2::new(g.x * BEAM_PACK, g.y / BEAM_STRETCH);
    let [d, radius, phase, size] = look_point(rows, tile, uv, k, o, radius_scale);
    if radius <= 0.0 {
        return Vec2::ZERO;
    }
    let core = 1.0 - smoothstep(radius * rows[8][2], radius, d);
    let rate = 1.0 + (size * 2.999).floor();
    let x = g.y / BEAM_PERIOD - rate * rows[MOVE_ROW][0] + phase;
    let t = x - x.floor();
    let tail = BEAM_TAIL_MIN + (1.0 - BEAM_TAIL_MIN) * rows[LOOK_ROW][1];
    let rise = smoothstep(1.0 - tail, 1.0, t);
    let edge = (2.0 * f.y / BEAM_PERIOD).clamp(BEAM_HEAD, 0.5);
    let head = 1.0 - smoothstep(1.0 - edge, 1.0, t);
    let light = core * rise * head;
    let r2 = rise * rise;
    Vec2::new(light, light * r2 * r2)
}

/// Coals at `g` (cells) on any plane: the light and the light times its
/// tint. A lump's heat is a cone from the point's center to its radius,
/// eased out so the middle is broad, times its own smolder ([`COAL_SMOLDER`] deep, on the twinkle's clock at
/// 1, 2 or 3 times it from the point's phase) and the music's: from
/// [`COAL_FLOOR`] in silence to 1 at a full band, as a square, so a kick
/// flares. The light eases out of the heat, so a cool rim still glows.
/// The tint runs with the square of the heat from -1 at none (the ember
/// color) through 0 (the sides') to 1 at full heat (the tops'), so most
/// of a lump is ember and only a flare's center white. The WGSL's `coal`.
pub fn coal(
    rows: &[[f32; 4]; ROWS],
    tile: &[[u8; 4]],
    g: Vec2,
    k: u32,
    o: i32,
    radius_scale: f32,
) -> Vec2 {
    let [d, radius, phase, size] = look_point(rows, tile, g, k, o, radius_scale);
    if radius <= 0.0 {
        return Vec2::ZERO;
    }
    let cone = (1.0 - d / radius.max(1e-4)).clamp(0.0, 1.0);
    let cone = cone * (2.0 - cone);
    let rate = 1.0 + (size * 2.999).floor();
    let smolder = 1.0 - COAL_SMOLDER + COAL_SMOLDER * wave(rate * rows[3][3] + phase);
    let m = rows[LOOK_ROW][1];
    let heat = cone * smolder * (COAL_FLOOR + (1.0 - COAL_FLOOR) * m * m);
    let light = heat * (2.0 - heat);
    Vec2::new(light, light * (2.0 * heat * heat - 1.0))
}

/// How far the grass's tips bend at world point `p` (packed cells): the
/// sway, a wave that travels across the room with the wind
/// ([`GRASS_WIND`]) on the sway's clock, [`GRASS_SWAY`] wide in silence
/// and [`GRASS_SWAY_BAND`] more at a full band, wider by the gust; and
/// the gust's lean one way ([`GRASS_LEAN`]). The WGSL's `grass_bend`.
pub fn grass_bend(rows: &[[f32; 4]; ROWS], p: Vec3) -> f32 {
    let [_, music, gust, _] = rows[LOOK_ROW];
    let osc = 2.0 * wave(rows[MOVE_ROW][1] + p.x * GRASS_WIND[0] + p.z * GRASS_WIND[1]) - 1.0;
    (GRASS_SWAY + GRASS_SWAY_BAND * music) * (1.0 + gust) * osc + GRASS_LEAN * gust
}

/// Grass on an upright plane at `g` (across and up, cells) with the tips
/// bent by `bend` ([`grass_bend`]): the light and the light times its
/// tint. The plane is cut into rows [`GRASS_ROW`] cells high, and each
/// row reads one line of the tile, [`GRASS_PACK`] times closer across, so
/// a point near the line ([`GRASS_NEAR`], [`GRASS_REACH`]) is a blade
/// rooted at the row's foot. A blade is
/// [`GRASS_SHORT`] to all of the row tall by the point's size, tapers to
/// nothing at its tip (by the square of the height, so it is a blade and
/// not a spike), bends with the square of the height, and its root
/// fades in over [`GRASS_ROOT`] of the row. The tint is the square of the
/// height up the blade: the sides' color at the root, the tops' at the
/// tip. The WGSL's `blade`.
pub fn blade(
    rows: &[[f32; 4]; ROWS],
    tile: &[[u8; 4]],
    g: Vec2,
    k: u32,
    o: i32,
    radius_scale: f32,
    bend: f32,
) -> Vec2 {
    let r = g.y / GRASS_ROW;
    let row = r.floor();
    let u = r - row;
    let uv = Vec2::new(g.x * GRASS_PACK + bend * u * u, row * GRASS_ROW_STEP + 0.5);
    let [d, full, _, size] = look_point(rows, tile, uv, k, o, radius_scale);
    let h = (u / (GRASS_SHORT + (1.0 - GRASS_SHORT) * size)).clamp(0.0, 1.0);
    let radius = full.min(GRASS_REACH) * (1.0 - h * h);
    if radius <= 1e-4 {
        return Vec2::ZERO;
    }
    let core = 1.0 - smoothstep(radius * rows[8][2], radius, d * GRASS_NEAR);
    let light = core * smoothstep(0.0, GRASS_ROOT, u);
    Vec2::new(light, light * h * h)
}

/// Bubbles on an upright plane at `g` (across and up, cells): the light
/// and the light times its tint. The tile is read [`BUBBLE_STRETCH`] times
/// drawn out along up and [`BUBBLE_PACK`] across, so a point is a wide
/// lane, and the lane snakes
/// ([`BUBBLE_WOBBLE`], on the sway's clock). In the lane a bubble passes
/// every [`BUBBLE_PERIOD`] cells of height, moving up with the rise at 1,
/// 2 or 3 times it by the point's size, from the point's phase. A bubble
/// is a round rim, [`BUBBLE_FILL`] of it inside, the point's radius
/// across, with a highlight over its center. The tint is the highlight:
/// the sides' color on the rim, the tops' on the shine. Toward a lane's
/// end the tile's distance grows, so a bubble swells in at the foot and
/// shrinks out at the head. The WGSL's `bubble`.
pub fn bubble(
    rows: &[[f32; 4]; ROWS],
    tile: &[[u8; 4]],
    g: Vec2,
    k: u32,
    o: i32,
    radius_scale: f32,
) -> Vec2 {
    let snake = 2.0 * wave(g.y / BUBBLE_WOBBLE_CELLS + rows[MOVE_ROW][1]) - 1.0;
    let uv = Vec2::new(
        (g.x + BUBBLE_WOBBLE * snake) * BUBBLE_PACK,
        g.y / BUBBLE_STRETCH,
    );
    let [d, radius, phase, size] = look_point(rows, tile, uv, k, o, radius_scale);
    if radius <= 0.0 {
        return Vec2::ZERO;
    }
    let rate = 1.0 + (size * 2.999).floor();
    let x = g.y / BUBBLE_PERIOD - rate * rows[MOVE_ROW][2] + phase;
    let up = (x - x.floor() - 0.5) * BUBBLE_PERIOD * BUBBLE_PACK;
    let inv = 1.0 / (radius * radius);
    let r2 = (d * d + up * up) * inv;
    let body = 1.0 - smoothstep(BUBBLE_EDGE, 1.0, r2);
    let rim = smoothstep(BUBBLE_RIM[0], BUBBLE_RIM[1], r2);
    let shell = body * (BUBBLE_FILL + (1.0 - BUBBLE_FILL) * rim);
    let over = up - BUBBLE_SHINE * radius;
    let shine = 1.0 - smoothstep(0.0, BUBBLE_SHINE_SIZE, (d * d + over * over) * inv);
    let light = shell.max(shine);
    Vec2::new(light, light * shine)
}

/// The wash's front over world point `p`, 0..1: 1 on the front, falling
/// to 0 at [`SPARKLE_FRONT_M`] to either side ([`MORE_ROW`]); 0 with no
/// wash. The WGSL's `sparkle_front`.
pub fn sparkle_front(rows: &[[f32; 4]; ROWS], p: Vec3) -> f32 {
    let [dx, dz, at, inv] = rows[MORE_ROW];
    if inv <= 0.0 {
        return 0.0;
    }
    1.0 - smoothstep(0.0, 1.0, (p.x * dx + p.z * dz - at).abs() * inv)
}

/// Sparkle at `g` (cells) on any plane, under the wash's front `front`
/// ([`sparkle_front`]): the light and the light times its tint. A point,
/// dim ([`SPARKLE_DIM`]) until it flares. It flares briefly on its own,
/// the eighth power of a wave on the twinkle's clock at 1, 2 or 3 times
/// it from the point's phase, as strong as the band ([`SPARKLE_QUIET`] of
/// it in silence), and as the front passes under it. The tint is the
/// flare: the sides' color dim, the tops' at a flare. The WGSL's
/// `sparkle`.
pub fn sparkle(
    rows: &[[f32; 4]; ROWS],
    tile: &[[u8; 4]],
    g: Vec2,
    k: u32,
    o: i32,
    radius_scale: f32,
    front: f32,
) -> Vec2 {
    let [d, radius, phase, size] = look_point(rows, tile, g, k, o, radius_scale);
    if radius <= 0.0 {
        return Vec2::ZERO;
    }
    let core = 1.0 - smoothstep(radius * rows[8][2], radius, d);
    let rate = 1.0 + (size * 2.999).floor();
    let w = wave(rate * rows[3][3] + phase);
    let w2 = w * w;
    let w4 = w2 * w2;
    let own = w4 * w4 * (SPARKLE_QUIET + (1.0 - SPARKLE_QUIET) * rows[LOOK_ROW][1]);
    let flare = own.max(front);
    let light = core * (SPARKLE_DIM + (1.0 - SPARKLE_DIM) * flare);
    Vec2::new(light, light * flare)
}

/// The radius factor a look's points are read at, for a vertex keeping
/// `keep` under the sweep's light `under` (0 without): the points' own
/// (the fill's radius, the music's swell, the keep), wider for the beams
/// ([`BEAM_WIDE`]) and the coals ([`COAL_WIDE`]), swollen under a sweep,
/// and at most what keeps the largest within the tile's reach. The WGSL's
/// `look_radius`.
pub fn look_radius(rows: &[[f32; 4]; ROWS], keep: f32, under: f32) -> f32 {
    let point = rows[7];
    let wide = match look_of(rows) {
        DepthLook::Beams => BEAM_WIDE,
        DepthLook::Coals => COAL_WIDE,
        _ => 1.0,
    };
    (point[0] * point[3] * keep.max(0.0).sqrt() * wide * (1.0 + rows[SONAR_ROW][2] * under))
        .min(point[2] / rows[8][1])
}

/// How the music lifts a look's light: by the crests' gain for the
/// beams, the grass and the bubbles; the coals carry it in their heat and
/// the sparkle in its flares. The WGSL's `look_band`.
pub fn look_band(rows: &[[f32; 4]; ROWS]) -> f32 {
    match look_of(rows) {
        DepthLook::Coals | DepthLook::Sparkle => 1.0,
        _ => rows[0][3],
    }
}

/// A look's color at `tint`: the sides' at 0 toward the tops' at 1, and
/// toward the ember color ([`COAL_EMBER`] of the sides') at -1. The WGSL's
/// `look_color`.
pub fn look_color(rows: &[[f32; 4]; ROWS], tint: f32) -> [f32; 3] {
    let [side, top] = [rows[1], rows[2]];
    let (up, down) = (tint.clamp(0.0, 1.0), (-tint).clamp(0.0, 1.0));
    std::array::from_fn(|k| {
        let hot = side[k] + (top[k] - side[k]) * up;
        hot + (side[k] * COAL_EMBER[k] - hot) * down
    })
}

/// A look's light at world point `p` seen from `eye` with the
/// interpolated normal `normal`, read at `radius_scale` ([`look_radius`]),
/// where the world moves by `fw` across a pixel: the light, 0..1, and its
/// tint ([`look_color`]). The points' planes, octaves and fades, each
/// read the look's own: on an upright plane the beams' streaks or the
/// grass's blades, on the level plane their points seen from the end
/// (the beams' in the tops' color, the grass's moved by the wind); the
/// coals and the sparkle on every plane; the bubbles on an upright plane,
/// the points on the level one. The fade for small cells counts the closer
/// packing across and the longer features along up. The WGSL's
/// `look_light`.
pub fn look_light(
    rows: &[[f32; 4]; ROWS],
    tile: &[[u8; 4]],
    eye: Vec3,
    p: Vec3,
    normal: Vec3,
    radius_scale: f32,
    fw: Vec3,
) -> Vec2 {
    let cell_row = rows[6];
    let look = look_of(rows);
    let n = unit_normal(normal);
    let n2 = n * n;
    let tri = n2 * n2 / n2.dot(n2);
    let l = (p - eye)
        .length()
        .max(1e-3)
        .log2()
        .clamp(OCTAVE_RANGE[0], OCTAVE_RANGE[1]);
    let li = l.floor();
    let next = smoothstep(cell_row[1], 1.0, l - li);
    let bend = if look == DepthLook::Grass {
        grass_bend(rows, p)
    } else {
        0.0
    };
    let front = if look == DepthLook::Sparkle {
        sparkle_front(rows, p)
    } else {
        0.0
    };
    let (mut sum, mut used) = (Vec2::ZERO, 0.0);
    for j in 0..2 {
        let wo = if j == 1 { next } else { 1.0 - next };
        if wo <= 0.0 {
            continue;
        }
        let o = li as i32 + j;
        let cell = cell_row[0] * 2f32.powi(o);
        for k in 0..3u32 {
            let wk = tri[k as usize] - PLANE_MIN;
            if wk <= 0.0 {
                continue;
            }
            let g = look_plane(p, k) / cell;
            let f = look_plane(fw, k) / cell;
            let upright = k != 1;
            let fine = match look {
                DepthLook::Beams if upright => (f.x * BEAM_PACK).max(f.y / BEAM_STRETCH),
                DepthLook::Grass if upright => (f.x * GRASS_PACK).max(f.y / GRASS_ROW),
                _ => f.x.max(f.y),
            };
            let aa = 1.0 - smoothstep(cell_row[2], cell_row[3], fine);
            if aa > 0.0 {
                let s = match look {
                    DepthLook::Beams if upright => beam(rows, tile, g, f, k, o, radius_scale),
                    DepthLook::Beams => {
                        let v = plane_points(rows, tile, g, k, o, radius_scale);
                        Vec2::new(v, v)
                    }
                    DepthLook::Grass if upright => blade(rows, tile, g, k, o, radius_scale, bend),
                    DepthLook::Grass => {
                        let at = g + Vec2::splat(GRASS_TOP_SHIFT * bend);
                        let v = plane_points(rows, tile, at, k, o, radius_scale);
                        Vec2::new(v, v * v)
                    }
                    DepthLook::Bubbles if upright => bubble(rows, tile, g, k, o, radius_scale),
                    DepthLook::Bubbles => {
                        let v = plane_points(rows, tile, g, k, o, radius_scale);
                        Vec2::new(v, 0.5 * v * v)
                    }
                    DepthLook::Sparkle => sparkle(rows, tile, g, k, o, radius_scale, front),
                    _ => coal(rows, tile, g, k, o, radius_scale),
                };
                sum += wo * wk * aa * s;
            }
            used += wo * wk;
        }
    }
    Vec2::new(sum.x / used.max(1e-6), sum.y / sum.x.max(1e-6))
}

/// The wave's term at height `y` (m) with the light's `rows`, before the
/// music and its fade: the crest's profile, 1 on a crest and 0 midway
/// between two, [`wave`] squared. A function of the height alone.
pub fn crest(rows: &[[f32; 4]; ROWS], y: f32) -> f32 {
    let c = wave(y * rows[0][0] - rows[0][1]);
    c * c
}

/// The fragment stage of the light (`fs_mesh_light`) at world point `p`
/// seen from `eye`, with the interpolated normal `normal`, keep `keep` and
/// light by distance `bright`, where the world moves by `fw` across a
/// pixel: premultiplied light in rgb and the veil in alpha, which the blend
/// ([`veil`]) erases by.
pub fn fragment(
    rows: &[[f32; 4]; ROWS],
    tile: &[[u8; 4]],
    eye: Vec3,
    p: Vec3,
    normal: Vec3,
    keep: f32,
    bright: f32,
    fw: Vec3,
) -> [f32; 4] {
    let [light, side, top, wave, fade, ..] = *rows;
    let weight = light[2] * bright;
    if rows[LOOK_ROW][0] > 0.5 {
        // Board #5543: a look other than the points.
        let s = if keep > 0.0 && weight > 0.0 {
            let radius = look_radius(rows, keep, 0.0);
            look_light(rows, tile, eye, p, normal, radius, fw)
        } else {
            Vec2::ZERO
        };
        let a = (weight * look_band(rows) * s.x).clamp(0.0, 1.0);
        let c = look_color(rows, s.y);
        return [c[0] * a, c[1] * a, c[2] * a, fade[3]];
    }
    let (mut lit, mut up) = (0.0, 0.0);
    if keep > 0.0 && weight > 0.0 {
        up = unit_normal(normal).y.clamp(0.0, 1.0);
        lit = points(rows, tile, eye, p, normal, keep, fw);
    }
    let fade_wave = 1.0 - smoothstep(wave[0], wave[1], fw.y * light[0]);
    let crest = wave[2] + (crest(rows, p.y) - wave[2]) * fade_wave;
    let band = side[3] + (1.0 - side[3]) * crest * light[3];
    let tint = (up + top[3] * crest).clamp(0.0, 1.0);
    let a = (weight * band * lit).clamp(0.0, 1.0);
    let rgb = |k: usize| (side[k] + (top[k] - side[k]) * tint) * a;
    [rgb(0), rgb(1), rgb(2), fade[3]]
}

/// The fragment stage of the light under the sweep (`fs_mesh_light_sonar`,
/// board #5511, `sonar.rs`), drawn in [`fragment`]'s place on a frame the
/// sweep shows ([`swept`]): [`fragment`]'s inputs, with the world's change
/// across a pixel each way, `dx` and `dy`, in `fw`'s place. The points
/// under a line or a tail swell and brighten to the line's peak; the line
/// and its tail's glow are solid under them, the crest tinted toward the
/// top's color, by `sonar::solid_share` of what the vertex keeps and of how
/// squarely the triangle (`dx` x `dy`) faces the eye.
#[allow(
    clippy::too_many_arguments,
    reason = "the fragment's inputs, as the WGSL has them"
)]
pub fn fragment_swept(
    rows: &[[f32; 4]; ROWS],
    tile: &[[u8; 4]],
    eye: Vec3,
    p: Vec3,
    normal: Vec3,
    keep: f32,
    bright: f32,
    [dx, dy]: [Vec3; 2],
) -> [f32; 4] {
    let [light, side, top, wave, fade, ..] = *rows;
    let sonar = sonar_rows(rows);
    let fw = dx.abs() + dy.abs();
    let weight = light[2] * bright;
    let sweep = if keep > 0.0 {
        crate::sonar::shade(&sonar, p)
    } else {
        [0.0; 2]
    };
    let under = sweep[0].max(sweep[1]);
    let flat = crate::sonar::solid_share(&sonar, dx.cross(dy), eye - p, keep);
    if rows[LOOK_ROW][0] > 0.5 {
        // Board #5543: a look other than the points, swollen and lifted
        // under the sweep as they are, the line solid under it.
        let s = if keep > 0.0 && weight > 0.0 {
            let radius = look_radius(rows, keep, under);
            look_light(rows, tile, eye, p, normal, radius, fw)
        } else {
            Vec2::ZERO
        };
        let peak = sonar[0];
        let lifted = ((weight * look_band(rows)).max(peak[0] * under) * s.x).clamp(0.0, 1.0);
        let crest_tint = peak[3] * sweep[0];
        let solid = (peak[0] * flat * sweep[0].max(peak[1] * sweep[1])).clamp(0.0, 1.0);
        let over = look_color(rows, (s.y + crest_tint).clamp(-1.0, 1.0));
        let line = look_color(rows, crest_tint.clamp(0.0, 1.0));
        let rgb = |k: usize| over[k] * lifted + line[k] * solid * (1.0 - lifted);
        return [rgb(0), rgb(1), rgb(2), fade[3]];
    }
    let (mut lit, mut up) = (0.0, 0.0);
    if keep > 0.0 && weight > 0.0 {
        up = unit_normal(normal).y.clamp(0.0, 1.0);
        lit = points_under(rows, tile, eye, p, normal, keep, fw, Some(under));
    }
    let fade_wave = 1.0 - smoothstep(wave[0], wave[1], fw.y * light[0]);
    let crest = wave[2] + (crest(rows, p.y) - wave[2]) * fade_wave;
    let band = side[3] + (1.0 - side[3]) * crest * light[3];
    let tint = (up + top[3] * crest).clamp(0.0, 1.0);
    let color = |tint: f32, k: usize| side[k] + (top[k] - side[k]) * tint;
    let peak = sonar[0];
    let lifted = ((weight * band).max(peak[0] * under) * lit).clamp(0.0, 1.0);
    let crest_tint = peak[3] * sweep[0];
    let solid = (peak[0] * flat * sweep[0].max(peak[1] * sweep[1])).clamp(0.0, 1.0);
    let rgb = |k: usize| {
        color((tint + crest_tint).clamp(0.0, 1.0), k) * lifted
            + color(crest_tint.clamp(0.0, 1.0), k) * solid * (1.0 - lifted)
    };
    [rgb(0), rgb(1), rgb(2), fade[3]]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env_depth::{
        DepthView, ENV_DEPTH_WGSL, FILTER_SOFT_M, LIGHT_ROW, NEAR_CUT_M, UNIFORM_ROWS, mesh_side,
        mesh_texel,
    };
    use crate::math::{self, Fov};
    use crate::surfaces::{GREEN, TEAL, WARM_WHITE};
    use glam::Quat;
    use std::f32::consts::TAU;

    /// The Quest 3's depth fov (v207).
    const FOV: Fov = Fov {
        left: -0.942,
        right: 0.698,
        up: 0.768,
        down: -0.960,
    };
    const NEAR: f32 = 0.1;
    const MAP: u32 = 320;
    const STEP: u32 = 2;
    /// No sweep: the sweep's rows all 0.
    const NO_SONAR: [[f32; 4]; crate::sonar::ROWS] = [[0.0; 4]; crate::sonar::ROWS];

    /// An eye and its depth camera at `pos`, turned by `rot`, the eye's
    /// view the depth camera's (the parallax between them is millimeters).
    fn eye_at(pos: Vec3, rot: Quat) -> EyeReprojection {
        let depth = DepthView {
            orientation: rot.to_array(),
            position: pos.to_array(),
            fov: FOV,
        };
        EyeReprojection {
            view_proj: math::view_projection(rot.to_array(), pos.to_array(), FOV, 0.05, 100.0),
            eye_pos: pos,
            depth_view: depth.view_matrix(),
            depth_fov: FOV,
            near: NEAR,
            far: f32::INFINITY,
            depth_size: [MAP, MAP],
            target_size: [1680, 1760],
            layer: 0,
            near_cut_m: NEAR_CUT_M,
            flip_v: false,
            filter_edge_m: FILTER_SOFT_M,
            mesh_step: STEP,
        }
    }

    /// The stored value for a depth along the camera's -Z (infinite far).
    fn encode(depth: f32) -> f32 {
        1.0 - NEAR / depth
    }

    /// The map of a scene: `hit` gives the depth along the depth camera's
    /// -Z of the first surface along a world ray (origin, direction scaled
    /// so the camera-space z is -1), `None` for nothing. Read through the
    /// texel's center, as the runtime stores it.
    fn map_of(
        r: &EyeReprojection,
        hit: impl Fn(Vec3, Vec3) -> Option<f32>,
    ) -> impl Fn([u32; 2]) -> f32 {
        let to_world = r.depth_view.inverse();
        let origin = to_world.transform_point3(Vec3::ZERO);
        let (r, size) = (*r, r.depth_size);
        move |t: [u32; 2]| {
            let uv = Vec2::new(
                (t[0] as f32 + 0.5) / size[0] as f32,
                (t[1] as f32 + 0.5) / size[1] as f32,
            );
            let dir = to_world.transform_vector3(r.depth_ray(uv));
            hit(origin, dir).map_or(1.0, encode)
        }
    }

    /// Where the ray `o + t d` meets the plane through `c` with normal
    /// `n`, t > 0.
    fn plane(o: Vec3, d: Vec3, c: Vec3, n: Vec3) -> Option<f32> {
        let t = (c - o).dot(n) / d.dot(n);
        (t > 0.0).then_some(t)
    }

    /// Where it meets the vertical cylinder through `c` of radius `radius`
    /// (no caps), t > 0.
    fn pole(o: Vec3, d: Vec3, c: Vec3, radius: f32) -> Option<f32> {
        let (o2, d2) = (Vec2::new(o.x - c.x, o.z - c.z), Vec2::new(d.x, d.z));
        let (a, b, k) = (d2.dot(d2), 2.0 * o2.dot(d2), o2.dot(o2) - radius * radius);
        let disc = b * b - 4.0 * a * k;
        (disc >= 0.0)
            .then(|| (-b - disc.sqrt()) / (2.0 * a))
            .filter(|&t| t > 0.0)
    }

    fn nearest(hits: impl IntoIterator<Item = Option<f32>>) -> Option<f32> {
        hits.into_iter().flatten().min_by(f32::total_cmp)
    }

    /// The texel of the mesh vertex nearest where the depth camera sees
    /// world point `p` (the mesh's vertices sit on every `STEP`th texel).
    fn vertex_near(r: &EyeReprojection, p: Vec3) -> [u32; 2] {
        let uv = r
            .depth_uv(r.depth_view.transform_point3(p))
            .expect("in view");
        let t = r.texel(uv);
        let side = mesh_side(MAP, STEP);
        let k = |v: u32| (((v as f32) / STEP as f32).round() as u32).min(side - 1);
        [
            mesh_texel(k(t[0]), MAP, STEP),
            mesh_texel(k(t[1]), MAP, STEP),
        ]
    }

    /// The rows at full immersion with the music full, the wave's phase
    /// at 0 and the twinkle at its brightest for a phase-0 point.
    fn lit_rows() -> [[f32; 4]; ROWS] {
        let mut d = Drive::default();
        d.step(1.0, 1.0, 0.0);
        let mut rows = rows(&d, peak_alpha(1.0, 1.0), [TEAL, [1.0, 0.4, 0.9]], FILL, 0.0);
        rows[0][1] = 0.0;
        rows
    }

    /// The points' tile, made once for the tests.
    fn tile() -> &'static [[u8; 4]] {
        static TILE_ONCE: std::sync::OnceLock<Vec<[u8; 4]>> = std::sync::OnceLock::new();
        TILE_ONCE.get_or_init(points_tile)
    }

    /// A seated eye 1.2 m up looking ahead and a little down.
    fn seated() -> EyeReprojection {
        eye_at(Vec3::new(0.0, 1.2, 0.0), Quat::from_rotation_x(-0.25))
    }

    #[test]
    fn the_veil_erases_the_surfaces_by_the_level_and_the_points_add() {
        // A pixel of sky behind a real thing: premultiplied light at
        // alpha 0.6; a point's light on the thing.
        let sky = [0.1, 0.5, 0.4, 0.6];
        let point = [0.05, 0.3, 0.25];
        for (level, kept) in [(0.0, 0.0), (0.5, 0.5), (1.0, 1.0)] {
            let v = veil_share(level, true);
            let out = veil(sky, [point[0], point[1], point[2], v]);
            for k in 0..3 {
                assert_close!(out[k], point[k] + sky[k] * kept);
            }
            // The alpha is the sky's kept share: the point adds no alpha,
            // so at level 0 the layer is clear over the passthrough.
            assert_close!(out[3], sky[3] * kept);
            // The faces pass: the veil alone.
            let faces = veil(sky, [0.0, 0.0, 0.0, v]);
            assert_close!(faces, sky.map(|c| c * kept));
        }
        // Off: the mesh draws first over the clear, at a veil of 1, and
        // its own fold erases what it drew behind.
        assert_close!(veil_share(0.3, false), 1.0);
        assert_close!(
            veil([0.2, 0.2, 0.2, 0.0], [point[0], point[1], point[2], 1.0]),
            [point[0], point[1], point[2], 0.0]
        );
        assert_close!(veil_share(f32::NAN, true), 1.0);
        // The passes: the veil's order only while it lets some light
        // through, the faces pass's mesh only while it erases some.
        let passes = |level: f32, on: bool| veil_passes(on, veil_share(level, on));
        assert_eq!(
            passes(0.0, true),
            VeilPasses {
                surfaces_first: false,
                faces_mesh: true
            }
        );
        assert_eq!(
            passes(0.5, true),
            VeilPasses {
                surfaces_first: true,
                faces_mesh: true
            }
        );
        assert_eq!(
            passes(1.0, true),
            VeilPasses {
                surfaces_first: true,
                faces_mesh: false
            }
        );
        for level in [0.0, 0.5, 1.0] {
            assert_eq!(
                passes(level, false),
                VeilPasses {
                    surfaces_first: false,
                    faces_mesh: true
                }
            );
        }
        // The faces layer's fades, at twice its cells per pixel.
        let base = rows(&Drive::default(), 1.0, [TEAL; 2], FILL, 0.0);
        let half = scale_fades(base, 2.0);
        assert_close!(
            [half[3][0], half[3][1]],
            [2.0 * base[3][0], 2.0 * base[3][1]]
        );
        assert_close!(
            [half[6][2], half[6][3]],
            [2.0 * base[6][2], 2.0 * base[6][3]]
        );
        assert_close!(half[0], base[0]);
        // Where the points draw: the faces layer when there is one, the
        // eye pass then holding the depth and its own veil.
        let draws =
            |level: f32, on: bool, faces: bool| mesh_draws(true, on, veil_share(level, on), faces);
        use MeshDraw::{Depth, Light, Veil};
        let want = |eye, faces, surfaces_first| MeshDraws {
            eye,
            faces,
            surfaces_first,
        };
        assert_eq!(draws(0.0, true, true), want(Depth, Some(Light), false));
        assert_eq!(draws(0.5, true, true), want(Veil, Some(Light), true));
        assert_eq!(draws(1.0, true, true), want(Depth, Some(Light), true));
        assert_eq!(draws(1.0, false, true), want(Depth, Some(Light), false));
        assert_eq!(draws(0.5, true, false), want(Light, None, true));
        assert_eq!(draws(0.0, true, false), want(Light, None, false));
        assert_eq!(
            mesh_draws(false, true, 0.5, true),
            want(Depth, Some(Depth), false)
        );
        // The rows carry it, and the fragment hands it on as its alpha.
        let mut rows = lit_rows();
        rows[4][3] = veil_share(0.25, true);
        let f = fragment(
            &rows,
            tile(),
            Vec3::ZERO,
            Vec3::new(0.0, 0.0, -1.0),
            Vec3::Z,
            1.0,
            1.0,
            Vec3::splat(1e-3),
        );
        assert_close!(f[3], 0.75);
    }

    #[test]
    fn a_point_stays_put_under_depth_jitter_and_a_head_move() {
        let rows = lit_rows();
        let fw = Vec3::splat(5e-4);
        // A surface 1 m out turned 20 degrees from the eye: walk a row of
        // world points across it and keep each one's light.
        let eye = Vec3::new(0.0, 1.2, 0.0);
        let n = Vec3::new(20f32.to_radians().sin(), 0.0, 20f32.to_radians().cos());
        let across = Vec3::Y.cross(n).normalize();
        let base = eye - n;
        let row: Vec<Vec3> = (0..200)
            .map(|i| base + across * (i as f32 * 0.002 - 0.2))
            .collect();
        let light = |e: Vec3, p: Vec3| points(&rows, tile(), e, p, n, 1.0, fw);
        let lit: Vec<f32> = row.iter().map(|&p| light(eye, p)).collect();
        assert!(lit.iter().any(|&v| v > 0.4) && lit.iter().any(|&v| v < 0.01));
        // The map's noise moves each point along its view ray by up to
        // JITTER_TEST_M. On the plane the surface faces that is a few
        // hundredths of a cell where the ray meets the surface squarely,
        // under a tenth anywhere on the row.
        let shift = |p: Vec3| {
            let moved = p + (p - eye).normalize() * JITTER_TEST_M;
            (light_plane(moved, 2) - light_plane(p, 2)).length() / CELL_M
        };
        assert!(shift(base) < 0.04, "{}", shift(base));
        let (mut worst_shift, mut worst) = (0.0f32, 0.0f32);
        for (&p, &v) in row.iter().zip(&lit) {
            worst_shift = worst_shift.max(shift(p));
            let moved = p + (p - eye).normalize() * JITTER_TEST_M;
            worst = worst.max((light(eye, moved) - v).abs());
        }
        eprintln!(
            "{} mm of noise: a point moves {:.3} of a cell at most, the light at a pixel by {worst:.2} at most (at a point's edge)",
            JITTER_TEST_M * 1000.0,
            worst_shift
        );
        assert!(worst_shift < 0.1, "{worst_shift}");
        assert!(worst < 0.45, "{worst}");
        // A head move of 10 cm at 1 m, inside an octave: nothing changes.
        let moved_eye = eye + Vec3::new(0.1, 0.0, 0.0);
        for (&p, &v) in row.iter().zip(&lit) {
            assert_close!(light(moved_eye, p), v);
        }
        // Where the octaves blend (1.8 m), a centimeter's move changes
        // the light only a little: a cross-fade, not a crawl.
        let far: Vec<Vec3> = row.iter().map(|&p| eye + (p - eye) * 1.8).collect();
        let step = Vec3::new(0.01, 0.0, 0.0);
        let changed = far
            .iter()
            .map(|&p| (light(eye + step, p) - light(eye, p)).abs())
            .fold(0.0f32, f32::max);
        assert!(changed < 0.05, "{changed}");
    }

    /// The light's rows drawing `look` in `theme`'s colors at immersion
    /// `level`, the band at `music` and the loudness at `gust`, 3.7 s in.
    fn look_rows(theme: Theme, level: f32, music: f32, gust: f32) -> [[f32; 4]; ROWS] {
        let mut d = Drive::default();
        for _ in 0..266 {
            d.step(1.0 / 72.0, music, 0.0);
            d.step_wind(1.0 / 72.0, gust);
        }
        // The wash's front, where a theme has one, a third of a bar in:
        // over the wall the tests look at.
        let eye = Vec3::new(0.0, 1.2, 0.0);
        frame(
            &DepthLightOptions::default().under(Some(theme)),
            &d,
            Some(theme),
            [0.7, 0.3, 1.0],
            level,
            &NO_SONAR,
            &Around {
                front: front(Some(theme), 0.35, eye),
            },
        )
        .rows
    }

    /// A wall 1 m ahead of a seated eye, 0.6 m square, as a grid of world
    /// points 3 mm apart.
    fn wall_grid() -> (Vec3, Vec<Vec3>) {
        let eye = Vec3::new(0.0, 1.2, 0.0);
        let grid = (0..200)
            .flat_map(|y| (0..200).map(move |x| (x, y)))
            .map(|(x, y)| Vec3::new(x as f32 * 0.003 - 0.3, 0.9 + y as f32 * 0.003, -1.0))
            .collect();
        (eye, grid)
    }

    const BUILT: [(Theme, DepthLook); 5] = [
        (Theme::Aurora, DepthLook::Beams),
        (Theme::Flint, DepthLook::Coals),
        (Theme::Contour, DepthLook::Grass),
        (Theme::Drift, DepthLook::Bubbles),
        (Theme::Tide, DepthLook::Sparkle),
    ];

    #[test]
    fn a_theme_names_its_look_in_the_rows() {
        // Board #5543: the ids are `DepthLook::ALL`'s order, the points 0,
        // so a look built later has its number already.
        for (k, look) in DepthLook::ALL.into_iter().enumerate() {
            assert_eq!(look_id(look), k as u32, "{}", look.name());
            let built = drawn(look) == look;
            assert_eq!(built, look.built());
            let rows = with_look(lit_rows(), look);
            assert_eq!(look_of(&rows), if built { look } else { DepthLook::Points });
        }
        assert_eq!(look(None), DepthLook::Points);
        for (theme, want) in BUILT {
            assert_eq!(look(Some(theme)), want);
            assert_eq!(look_of(&look_rows(theme, 1.0, 0.5, 0.5)), want);
        }
        // A theme whose look is not built draws the points.
        assert_eq!(look(Some(Theme::Mirror)), DepthLook::Points);
        assert_close!(look_rows(Theme::Mirror, 1.0, 0.5, 0.5)[LOOK_ROW][0], 0.0);
        // The look's rows: the id, the music and the gust; the clocks.
        let mut d = Drive::default();
        d.step(0.5, 0.8, 0.0);
        d.step_wind(0.5, 0.6);
        let rows = with_look(rows(&d, 0.2, [TEAL, GREEN], FILL, 0.0), DepthLook::Grass);
        assert_close!(rows[LOOK_ROW], [3.0, d.music(), d.gust(), 0.0]);
        assert_close!(rows[MOVE_ROW], [d.travel(), d.sway(), d.rise(), 0.0]);
        assert_close!(rows[MORE_ROW], [0.0; 4]);
        const {
            assert!(MOVE_ROW == LOOK_ROW + 1 && MORE_ROW == MOVE_ROW + 1);
            assert!(SONAR_ROW == MORE_ROW + 1);
        };
        // The WGSL's constants are these.
        let vec = |v: &[f32]| {
            let inner: Vec<String> = v.iter().map(|x| format!("{x:?}")).collect();
            format!("vec{}<f32>({})", v.len(), inner.join(", "))
        };
        for (name, value) in [
            ("BEAM_PACK", BEAM_PACK),
            ("BEAM_STRETCH", BEAM_STRETCH),
            ("BEAM_PERIOD", BEAM_PERIOD),
            ("BEAM_TAIL_MIN", BEAM_TAIL_MIN),
            ("BEAM_HEAD", BEAM_HEAD),
            ("BEAM_WIDE", BEAM_WIDE),
            ("COAL_WIDE", COAL_WIDE),
            ("COAL_FLOOR", COAL_FLOOR),
            ("COAL_SMOLDER", COAL_SMOLDER),
            ("GRASS_PACK", GRASS_PACK),
            ("GRASS_ROW", GRASS_ROW),
            ("GRASS_ROW_STEP", GRASS_ROW_STEP),
            ("GRASS_SHORT", GRASS_SHORT),
            ("GRASS_NEAR", GRASS_NEAR),
            ("GRASS_REACH", GRASS_REACH),
            ("GRASS_ROOT", GRASS_ROOT),
            ("GRASS_SWAY", GRASS_SWAY),
            ("GRASS_SWAY_BAND", GRASS_SWAY_BAND),
            ("GRASS_LEAN", GRASS_LEAN),
            ("GRASS_TOP_SHIFT", GRASS_TOP_SHIFT),
            ("BUBBLE_PACK", BUBBLE_PACK),
            ("BUBBLE_STRETCH", BUBBLE_STRETCH),
            ("BUBBLE_PERIOD", BUBBLE_PERIOD),
            ("BUBBLE_WOBBLE", BUBBLE_WOBBLE),
            ("BUBBLE_WOBBLE_CELLS", BUBBLE_WOBBLE_CELLS),
            ("BUBBLE_FILL", BUBBLE_FILL),
            ("BUBBLE_EDGE", BUBBLE_EDGE),
            ("BUBBLE_SHINE", BUBBLE_SHINE),
            ("BUBBLE_SHINE_SIZE", BUBBLE_SHINE_SIZE),
            ("SPARKLE_DIM", SPARKLE_DIM),
            ("SPARKLE_QUIET", SPARKLE_QUIET),
        ] {
            let line = format!("const {name}: f32 = {value:?};");
            assert!(ENV_DEPTH_WGSL.contains(&line), "{line}");
        }
        for line in [
            format!("const COAL_EMBER: vec3<f32> = {};", vec(&COAL_EMBER)),
            format!("const GRASS_WIND: vec2<f32> = {};", vec(&GRASS_WIND)),
            format!("const BUBBLE_RIM: vec2<f32> = {};", vec(&BUBBLE_RIM)),
        ] {
            assert!(ENV_DEPTH_WGSL.contains(&line), "{line}");
        }
        // A blade's half-width stays under what the tile can tell from no
        // point at all, and the widened looks within the tile's reach.
        const { assert!(GRASS_REACH < REACH * GRASS_NEAR) };
        for theme in [Theme::Aurora, Theme::Flint] {
            let rows = look_rows(theme, 1.0, 1.0, 0.0);
            assert!(look_radius(&rows, 1.0, 1.0) * SIZE[1] <= REACH + 1e-6);
        }
    }

    #[test]
    fn a_kick_launches_the_beams_and_the_loudness_blows_the_wind() {
        let (mut quiet, mut loud) = (Drive::default(), Drive::default());
        for _ in 0..36 {
            quiet.step(1.0 / 72.0, 0.0, 0.0);
            loud.step(1.0 / 72.0, 1.0, 0.0);
        }
        // Half a second: slow in silence, most of a period at a full band.
        assert!((quiet.travel() - 0.5 * BEAM_TRAVEL_HZ[0]).abs() < 1e-4);
        assert!(loud.travel() > 0.7 && loud.travel() < 0.5 * BEAM_TRAVEL_HZ[1]);
        // It wraps to a whole period, and the beat envelope speeds it.
        let mut on_beat = Drive::default();
        for _ in 0..720 {
            loud.step(1.0 / 72.0, 1.0, 0.0);
            assert!((0.0..1.0).contains(&loud.travel()));
        }
        for _ in 0..36 {
            on_beat.step(1.0 / 72.0, 0.0, 1.0);
        }
        assert!((on_beat.travel() - 2.0 * quiet.travel()).abs() < 1e-4);
        // The wind: the gust follows the loudness up in under a second and
        // down more slowly, and the sway runs up to twice as fast in it.
        let (mut still, mut blown) = (Drive::default(), Drive::default());
        for _ in 0..72 {
            still.step_wind(1.0 / 72.0, 0.0);
            blown.step_wind(1.0 / 72.0, 1.0);
        }
        assert_close!(still.gust(), 0.0);
        assert!((still.sway() - SWAY_HZ).abs() < 1e-4);
        assert!(blown.gust() > 0.9 && blown.sway() > 1.6 * SWAY_HZ);
        for _ in 0..72 {
            blown.step_wind(1.0 / 72.0, 0.0);
            assert!((0.0..1.0).contains(&blown.sway()));
        }
        assert!(blown.gust() > 0.3 && blown.gust() < 0.5);
        // Bad input moves nothing; the music's step leaves the wind alone.
        let mut d = Drive::default();
        d.step_wind(f32::NAN, f32::NAN);
        assert_eq!(d, Drive::default());
        assert_close!(loud.gust(), 0.0);
        assert_close!(loud.sway(), 0.0);
    }

    #[test]
    fn a_look_stays_put_under_depth_jitter_and_a_head_move() {
        let fw = Vec3::splat(5e-4);
        let (eye, grid) = wall_grid();
        for (theme, look) in BUILT {
            let rows = look_rows(theme, 1.0, 0.8, 0.4);
            let radius = look_radius(&rows, 1.0, 0.0);
            let light = |e: Vec3, p: Vec3| look_light(&rows, tile(), e, p, Vec3::Z, radius, fw);
            let lit: Vec<Vec2> = grid.iter().map(|&p| light(eye, p)).collect();
            let share = lit.iter().filter(|v| v.x > 0.2).count() as f32 / lit.len() as f32;
            eprintln!(
                "{}: {:.0} % of the wall over 0.2",
                look.name(),
                share * 100.0
            );
            assert!(share > 0.05 && share < 0.6, "{}: {share}", look.name());
            assert!(
                lit.iter()
                    .all(|v| (0.0..=1.0).contains(&v.x) && v.y.abs() <= 1.0 + 1e-4)
            );
            // The map's noise moves a point along its view ray. On the
            // wall's plane that is under a tenth of the look's closest
            // packing, and the light changes at a feature's edge alone.
            let pack = match look {
                DepthLook::Beams => BEAM_PACK,
                DepthLook::Grass => GRASS_PACK,
                _ => 1.0,
            };
            let (mut worst_shift, mut changed) = (0.0f32, 0usize);
            for (&p, v) in grid.iter().zip(&lit) {
                let moved = p + (p - eye).normalize() * JITTER_TEST_M;
                let d = look_plane(moved, 2) - look_plane(p, 2);
                worst_shift = worst_shift.max(d.x.abs() * pack / CELL_M);
                changed += usize::from((light(eye, moved).x - v.x).abs() > 0.25);
            }
            let changed = changed as f32 / grid.len() as f32;
            eprintln!(
                "{}: {} mm of noise moves it {worst_shift:.3} of a packed cell at most; {:.1} % of the wall changes by over 0.25",
                look.name(),
                JITTER_TEST_M * 1000.0,
                changed * 100.0
            );
            assert!(worst_shift < 0.1, "{}: {worst_shift}", look.name());
            assert!(changed < 0.03, "{}: {changed}", look.name());
            // A head move of 10 cm at 1 m, inside an octave: nothing
            // changes. The look is the world point's and the clocks'.
            let moved_eye = eye + Vec3::new(0.1, 0.0, 0.0);
            for (&p, v) in grid.iter().zip(&lit).step_by(7) {
                assert_close!(light(moved_eye, p).to_array(), v.to_array());
            }
        }
    }

    #[test]
    fn the_beams_run_up_and_reach_with_the_music() {
        let fw = Vec3::splat(5e-4);
        let (eye, grid) = wall_grid();
        // The light's center of height over the wall, and the light.
        let measure = |rows: &[[f32; 4]; ROWS]| {
            let radius = look_radius(rows, 1.0, 0.0);
            let (mut sum, mut tip) = (0.0f64, 0.0f64);
            for &p in &grid {
                let v = look_light(rows, tile(), eye, p, Vec3::Z, radius, fw);
                sum += f64::from(v.x);
                tip += f64::from(v.x * v.y);
            }
            (sum, tip / sum.max(1e-9))
        };
        let quiet = look_rows(Theme::Aurora, 1.0, 0.0, 0.0);
        let loud = look_rows(Theme::Aurora, 1.0, 1.0, 0.0);
        // A full band draws a streak the whole period long, silence
        // BEAM_TAIL_MIN of it: over twice the light.
        let (q, _) = measure(&quiet);
        let (l, tint) = measure(&loud);
        eprintln!("beams: light {q:.0} quiet, {l:.0} loud, mean tint {tint:.2}");
        assert!(l > 2.0 * q && q > 0.0, "{q} {l}");
        // The sides' color along the streak, the tops' at its head.
        assert!(tint > 0.2 && tint < 0.6, "{tint}");
        // One lane: its head is higher a moment later. The brightest
        // height up one column of the wall, with the travel a twentieth
        // of a period on (the slowest streaks move by that, 0.8 cm here).
        let radius = look_radius(&loud, 1.0, 0.0);
        let column = |rows: &[[f32; 4]; ROWS], x: f32| -> Vec<f32> {
            (0..400)
                .map(|y| Vec3::new(x, 0.9 + y as f32 * 0.0005, -1.0))
                .map(|p| look_light(rows, tile(), eye, p, Vec3::Z, radius, fw).x)
                .collect()
        };
        let mut later = loud;
        later[MOVE_ROW][0] = (loud[MOVE_ROW][0] + 0.05).fract();
        let mut rose = 0;
        let mut lanes = 0;
        for n in 0..60 {
            let x = n as f32 * 0.01 - 0.3;
            let (a, b) = (column(&loud, x), column(&later, x));
            // A head: the light falls from over 0.5 to nothing within
            // BEAM_HEAD of a period going up.
            let head = |c: &[f32]| (1..c.len()).find(|&i| c[i - 1] > 0.5 && c[i] < c[i - 1] - 0.02);
            if let (Some(h0), Some(h1)) = (head(&a), head(&b)) {
                lanes += 1;
                rose += usize::from(h1 > h0);
            }
        }
        eprintln!("beams: of {lanes} heads found, {rose} are higher 0.05 of a period on");
        assert!(lanes >= 10 && rose * 10 >= lanes * 9, "{rose} of {lanes}");
        // A top (the level plane): the points, in the tops' color.
        let top = look_light(
            &loud,
            tile(),
            eye,
            Vec3::new(0.1, 0.5, -0.9),
            Vec3::Y,
            radius,
            fw,
        );
        assert!(top.x == 0.0 || (top.y - 1.0).abs() < 1e-4);
    }

    #[test]
    fn the_coals_run_the_heat_ramp_and_flare_with_the_music() {
        let rows = look_rows(Theme::Flint, 1.0, 1.0, 0.0);
        let [side, top] = colors(Some(Theme::Flint), [1.0; 3]);
        // The ramp: ember, a dark red made of the sides' amber, then the
        // sides' color, then the tops' warm white.
        let ember: [f32; 3] = std::array::from_fn(|k| side[k] * COAL_EMBER[k]);
        assert_close!(look_color(&rows, -1.0), ember);
        assert_close!(look_color(&rows, 0.0), side);
        assert_close!(look_color(&rows, 1.0), top);
        assert!(ember[0] > 0.8 && ember[1] < 0.1 && ember[2] < 0.01);
        // The points' ramp has no ember: a tint of 0 or more is theirs.
        assert_close!(
            look_color(&rows, 0.5),
            [
                f32::midpoint(side[0], top[0]),
                f32::midpoint(side[1], top[1]),
                f32::midpoint(side[2], top[2])
            ]
        );
        let fw = Vec3::splat(5e-4);
        let (eye, grid) = wall_grid();
        let measure = |music: f32| {
            let rows = look_rows(Theme::Flint, 1.0, music, 0.0);
            let radius = look_radius(&rows, 1.0, 0.0);
            let (mut sum, mut hot, mut ember) = (0.0f64, 0usize, 0usize);
            for &p in &grid {
                let v = look_light(&rows, tile(), eye, p, Vec3::Z, radius, fw);
                sum += f64::from(v.x);
                hot += usize::from(v.x > 0.02 && v.y > 0.3);
                ember += usize::from(v.x > 0.02 && v.y < -0.3);
            }
            (sum, hot, ember)
        };
        let (quiet, quiet_hot, quiet_ember) = measure(0.0);
        let (loud, loud_hot, loud_ember) = measure(1.0);
        eprintln!(
            "coals: light {quiet:.0} quiet ({quiet_hot} px toward white, {quiet_ember} ember), {loud:.0} loud ({loud_hot}, {loud_ember})"
        );
        // In silence the lumps smolder, all of them ember; a full band
        // flares them, their centers toward white, their rims still ember.
        assert!(quiet > 0.0 && loud > 1.5 * quiet);
        assert_eq!(quiet_hot, 0);
        assert!(loud_hot > 500 && loud_ember > loud_hot);
        // The music is in the heat, so the band adds nothing over it.
        assert_close!(look_band(&rows), 1.0);
        assert_close!(look_band(&look_rows(Theme::Aurora, 1.0, 1.0, 0.0)), 1.0);
        let half = look_rows(Theme::Aurora, 1.0, 0.5, 0.0);
        assert_close!(look_band(&half), half[0][3]);
    }

    #[test]
    fn the_grass_tapers_and_bends_with_the_wind() {
        let fw = Vec3::splat(5e-4);
        let still = look_rows(Theme::Contour, 1.0, 0.0, 0.0);
        let blown = look_rows(Theme::Contour, 1.0, 1.0, 1.0);
        // The bend: a sway either way in still air, wider with the band,
        // and a lean one way in a gust.
        let p = Vec3::new(0.2, 1.0, -1.0);
        assert!(grass_bend(&still, p).abs() <= GRASS_SWAY + 1e-5);
        let most = (GRASS_SWAY + GRASS_SWAY_BAND) * 2.0 + GRASS_LEAN;
        let mean: f32 = (0..100)
            .map(|i| grass_bend(&blown, Vec3::new(i as f32 * 0.05, 1.0, -1.0)))
            .inspect(|b| assert!(b.abs() <= most + 1e-4))
            .sum::<f32>()
            / 100.0;
        assert!(mean > 0.5 * GRASS_LEAN, "{mean}");
        // A blade: rooted at the row's foot, nothing at the row's top, the
        // light's mean height low in the row and the tint rising to the
        // tip. Over one row 1 m out, by the share of the row's height.
        let (eye, _) = wall_grid();
        let radius = look_radius(&still, 1.0, 0.0);
        let row_m = GRASS_ROW * CELL_M;
        let mut by_height = [(0.0f64, 0.0f64); 10];
        for x in 0..400 {
            for y in 0..100 {
                let u = (y as f32 + 0.5) / 100.0;
                let p = Vec3::new(x as f32 * 0.0015 - 0.3, row_m * (14.0 + u), -1.0);
                let v = look_light(&still, tile(), eye, p, Vec3::Z, radius, fw);
                let bin = &mut by_height[y / 10];
                bin.0 += f64::from(v.x);
                bin.1 += f64::from(v.x * v.y);
            }
        }
        let light: Vec<f64> = by_height.iter().map(|b| b.0).collect();
        eprintln!("grass: light by tenth of the row's height {light:.0?}");
        assert!(light[1] > 0.0 && light[9] < 0.05 * light[1], "{light:?}");
        assert!(light[1] > light[4] && light[4] > light[7], "{light:?}");
        let tint = |k: usize| by_height[k].1 / by_height[k].0.max(1e-9);
        assert!(tint(1) < 0.1 && tint(6) > tint(3) && tint(3) > tint(1));
        // Wind moves the tips and leaves the roots: the light in the
        // lowest tenth is the same, higher up it is not.
        let low = |rows: &[[f32; 4]; ROWS], u: f32| -> Vec<f32> {
            (0..400)
                .map(|x| Vec3::new(x as f32 * 0.0015 - 0.3, row_m * (14.0 + u), -1.0))
                .map(|p| look_light(rows, tile(), eye, p, Vec3::Z, radius, fw).x)
                .collect()
        };
        // The same radius and band for both, so only the bend differs.
        let mut bent = still;
        bent[LOOK_ROW][2] = 1.0;
        let moved = |u: f32| {
            let (a, b) = (low(&still, u), low(&bent, u));
            a.iter().zip(&b).map(|(x, y)| (x - y).abs()).sum::<f32>()
        };
        assert!(
            moved(0.5) > 20.0 * moved(0.03).max(0.05),
            "{} {}",
            moved(0.5),
            moved(0.03)
        );
        // A top: the points, moved a little by the wind.
        let top = Vec3::new(0.1, 0.5, -0.9);
        let v = look_light(&still, tile(), eye, top, Vec3::Y, radius, fw);
        assert!((0.0..=1.0).contains(&v.x) && (0.0..=1.0).contains(&v.y));
    }

    #[test]
    fn the_bubbles_rise_and_carry_a_highlight() {
        let fw = Vec3::splat(5e-4);
        let (eye, grid) = wall_grid();
        let rows = look_rows(Theme::Drift, 1.0, 0.6, 0.3);
        assert_eq!(look_of(&rows), DepthLook::Bubbles);
        let radius = look_radius(&rows, 1.0, 0.0);
        let at = |rise: f32| -> Vec<Vec2> {
            let mut r = rows;
            r[MOVE_ROW][2] = rise;
            grid.iter()
                .map(|&p| look_light(&r, tile(), eye, p, Vec3::Z, radius, fw))
                .collect()
        };
        // A whole period on, every bubble is where the last one was.
        let (start, wrapped) = (at(0.2), at(1.2));
        for (a, b) in start.iter().zip(&wrapped).step_by(11) {
            assert!((*a - *b).abs().max_element() < 1e-3, "{a} {b}");
        }
        // A little on, the light has moved up: it matches the light a
        // little lower before better than the light in place. Slowest
        // bubbles move a twentieth of a period, 5 points of the grid.
        let later = at(0.25);
        let diff = |shift: usize| -> f32 {
            (shift * 200..grid.len())
                .map(|i| (later[i].x - start[i - shift * 200].x).abs())
                .sum::<f32>()
                / (grid.len() - shift * 200) as f32
        };
        let step = (0.05 * BUBBLE_PERIOD * CELL_M / 0.003).round() as usize;
        assert!(step >= 2);
        eprintln!(
            "bubbles 0.05 of a period on: the light differs by {:.4} in place, {:.4} moved up {step} points",
            diff(0),
            diff(step)
        );
        assert!(diff(step) < diff(0));
        // The shine is the tops' color and smaller than the bubble; the
        // rim is the sides'. With the room black both show.
        let lit = start.iter().filter(|v| v.x > 0.5).count();
        let shine = start.iter().filter(|v| v.x > 0.5 && v.y > 0.6).count();
        let rim = start.iter().filter(|v| v.x > 0.5 && v.y < 0.2).count();
        eprintln!("bubbles: {lit} points lit, {shine} of them shine, {rim} rim");
        assert!(shine > 0 && rim > shine, "{shine} {rim}");
        // A top shows the points: a bubble seen from above is a dot.
        let top = Vec3::new(0.01, 0.8, -1.0);
        let v = look_light(&rows, tile(), eye, top, Vec3::Y, radius, fw);
        assert!((0.0..=1.0).contains(&v.x) && (0.0..=1.0).contains(&v.y));
        // The rise's clock: slow in silence, faster with the band.
        let (mut quiet, mut loud) = (Drive::default(), Drive::default());
        for _ in 0..72 {
            quiet.step(1.0 / 72.0, 0.0, 0.0);
            loud.step(1.0 / 72.0, 1.0, 0.0);
        }
        assert_close!(quiet.rise(), BUBBLE_RISE_HZ[0]);
        assert!(loud.rise() > 0.8 * BUBBLE_RISE_HZ[1] && loud.rise() <= BUBBLE_RISE_HZ[1]);
    }

    #[test]
    fn the_sparkle_is_dim_and_flares_on_its_band_and_under_the_washs_front() {
        let fw = Vec3::splat(5e-4);
        let (eye, grid) = wall_grid();
        // The wall is 1 m ahead. The stand-in front runs from 2.5 m
        // behind the wearer to 2.5 m ahead over half a bar and back.
        let center = Vec3::new(0.3, 1.2, 0.0);
        assert_close!(wash_front(0.0, center), [0.0, -1.0, -2.5, 1.0 / 0.3]);
        assert_close!(wash_front(0.5, center)[2], 2.5);
        assert_close!(wash_front(0.25, center)[2], 0.0);
        assert_close!(wash_front(0.75, center)[2], 0.0);
        assert_close!(wash_front(1.25, center), wash_front(0.25, center));
        let none = |row: [f32; 4]| row.iter().all(|v| *v == 0.0);
        assert!(none(wash_front(f32::NAN, center)));
        // Tide alone has a wash.
        for t in Theme::ALL {
            let has = !none(front(Some(t), 0.3, center));
            assert_eq!(has, t == Theme::Tide, "{t:?}");
        }
        assert!(none(front(None, 0.3, center)));
        // Only the sparkle reads the front.
        let around = Around {
            front: wash_front(0.35, eye),
        };
        for look in DepthLook::ALL {
            let rows = with_around(with_look(lit_rows(), look), &around);
            let reads = look == DepthLook::Sparkle;
            assert_eq!(!none(rows[MORE_ROW]), reads, "{}", look.name());
        }
        // The light over the wall with the front far away, quiet and
        // loud, and with it on the wall.
        let base = look_rows(Theme::Tide, 1.0, 0.0, 0.0);
        assert_eq!(look_of(&base), DepthLook::Sparkle);
        let radius = look_radius(&base, 1.0, 0.0);
        let total = |rows: &[[f32; 4]; ROWS]| -> (f32, f32) {
            let (mut light, mut most) = (0.0f32, 0.0f32);
            for &p in &grid {
                let v = look_light(rows, tile(), eye, p, Vec3::Z, radius, fw);
                assert!((0.0..=1.0).contains(&v.x) && (0.0..=1.0 + 1e-4).contains(&v.y));
                light += v.x;
                most = most.max(v.x);
            }
            (light, most)
        };
        let mut away = base;
        away[MORE_ROW] = [0.0; 4];
        let mut loud = away;
        loud[LOOK_ROW][1] = 1.0;
        let mut under = away;
        under[MORE_ROW] = wash_front(0.35, eye);
        assert_close!(sparkle_front(&under, Vec3::new(0.2, 1.0, -1.0)), 1.0);
        assert_close!(sparkle_front(&under, Vec3::new(0.2, 1.0, -1.4)), 0.0);
        assert_close!(sparkle_front(&away, Vec3::new(0.2, 1.0, -1.0)), 0.0);
        let (quiet, quiet_most) = total(&away);
        let (loud, loud_most) = total(&loud);
        let (front, front_most) = total(&under);
        eprintln!(
            "sparkle over the wall: {quiet:.0} quiet (most {quiet_most:.2}), {loud:.0} loud ({loud_most:.2}), {front:.0} under the front ({front_most:.2})"
        );
        // Quiet, most points sit at their dim light and a few flare a
        // little; loud, the flares reach full; under the front all do.
        assert!(
            quiet > 0.0 && quiet_most < SPARKLE_DIM + (1.0 - SPARKLE_DIM) * SPARKLE_QUIET + 1e-3
        );
        assert!(loud > quiet && loud_most > 0.95);
        assert!(front > 2.0 * quiet && front_most > 0.99);
        // Brief: loud, with no front, the flares add under a third to the
        // dim light of all the points.
        assert!(loud < 1.6 * quiet, "{loud} {quiet}");
    }

    #[test]
    fn every_look_draws_with_the_room_visible_and_with_it_black() {
        // Every theme occludes (`DepthLightOptions::under`), so a look is
        // drawn over a black base at every level: at the points' lowest
        // peak with the room plain, at their highest with it black.
        let fw = Vec3::splat(5e-4);
        let (eye, grid) = wall_grid();
        for (theme, look) in BUILT {
            let sum = |level: f32| -> f64 {
                let rows = look_rows(theme, level, 0.6, 0.3);
                grid.iter()
                    .step_by(3)
                    .map(|&p| fragment(&rows, tile(), eye, p, Vec3::Z, 1.0, 1.0, fw))
                    // The hard cutout: all of what is behind erased.
                    .inspect(|c| assert_close!(c[3], 1.0))
                    .map(|c| f64::from(c[0] + c[1] + c[2]))
                    .sum()
            };
            let (plain, black) = (sum(0.0), sum(1.0));
            let ratio = plain / black;
            eprintln!(
                "{}: light {plain:.1} with the room visible, {black:.1} black",
                look.name()
            );
            assert!(plain > 0.0, "{}", look.name());
            assert!(
                (ratio - f64::from(ALPHA_LOW / ALPHA_HIGH)).abs() < 0.01,
                "{}: {ratio}",
                look.name()
            );
            // Under a sweep's line the look is lifted to the line's peak.
            let rows = look_rows(theme, 1.0, 0.6, 0.3);
            let mut swept = rows;
            swept[SONAR_ROW][0] = 1.0;
            swept[SONAR_ROW + 2] = [eye.x, eye.y, eye.z, 1.0];
            swept[SONAR_ROW + 3] = [1.0, 1.0 / 0.05, 1.0 / 0.5, 0.0];
            let d = [Vec3::X * 5e-4, Vec3::Y * 5e-4];
            let under: f32 = grid
                .iter()
                .filter(|p| ((**p - eye).length() - 1.0).abs() < 0.01)
                .map(|&p| {
                    let a = fragment_swept(&swept, tile(), eye, p, Vec3::Z, 1.0, 1.0, d);
                    let b = fragment(&rows, tile(), eye, p, Vec3::Z, 1.0, 1.0, fw);
                    a[0] + a[1] + a[2] - (b[0] + b[1] + b[2])
                })
                .sum();
            assert!(under > 0.0, "{}: {under}", look.name());
        }
    }

    #[test]
    fn a_small_step_draws_nothing_and_a_large_one_parts_the_masses() {
        let rows = lit_rows();
        let r = eye_at(Vec3::new(0.0, 1.2, 0.0), Quat::IDENTITY);
        // Two things side by side facing the eye, the right one `step`
        // behind the left one at 1.2 m.
        let walk = |step: f32| {
            let map = map_of(&r, move |o, d| {
                let t = plane(o, d, Vec3::new(0.0, 0.0, -1.2), Vec3::Z)?;
                if (o + d * t).x < 0.0 {
                    Some(t)
                } else {
                    plane(o, d, Vec3::new(0.0, 0.0, -1.2 - step), Vec3::Z)
                }
            });
            let mid = vertex_near(&r, Vec3::new(0.0, 1.2, -1.2));
            (0..12)
                .map(|k| [mid[0] + k * STEP - 6 * STEP, mid[1]])
                .map(|t| vertex_light(&r, &rows, t, &map))
                .collect::<Vec<_>>()
        };
        // 10 and 25 cm: one mass. Every vertex keeps all of its points and
        // its normal faces the eye: no line, no thinning, no tilt.
        for step in [0.1, 0.25] {
            for v in walk(step) {
                assert_close!(v.keep, 1.0);
                assert!(v.normal.z > 0.99, "{step}: {v:?}");
                assert_close!(v.bright, 1.0);
            }
        }
        // 1.5 m: two masses. The near side keeps NEAR_KEEP at its edge,
        // the far side none at its own; inside each, all of it.
        let parted = walk(1.5);
        let keeps: Vec<f32> = parted.iter().map(|v| v.keep).collect();
        let edge = parted
            .iter()
            .position(|v| v.world.z < -1.5)
            .expect("the far thing");
        assert_close!(keeps[edge - 1], NEAR_KEEP);
        assert_close!(keeps[edge], 0.0);
        assert_close!(keeps[edge - 3], 1.0);
        assert_close!(keeps[edge + 2], 1.0);
        // The normals on both sides of the edge still face the eye.
        assert!(parted[edge - 1].normal.z > 0.99 && parted[edge].normal.z > 0.99);
        // The step that parts them: half a meter here, a quarter of the
        // distance farther out.
        let part = |d: f32| JUMP_MIN_M.max(JUMP_REL * d);
        assert_close!(part(1.2), 0.5);
        assert_close!(part(4.0), 1.0);
    }

    #[test]
    fn the_fringe_between_two_masses_carries_shrinking_points() {
        // Across the fringe the keep falls from the near side's to none,
        // and a point's radius with its square root: the points shrink and
        // thin out, no line. Take one point of the tile and see how far
        // from its center it still lights at each keep.
        let rows = lit_rows();
        // A point whose neighbors stand at least a cell off, so its own
        // disk is what lights around it.
        let cells = (0..TILE_CELLS).flat_map(|y| (0..TILE_CELLS).map(move |x| [x, y]));
        let spacing = |c: [u32; 2]| {
            let (center, ..) = tile_point(c);
            let n = TILE_CELLS as i32;
            (-1..=1)
                .flat_map(|dy| (-1..=1).map(move |dx| (dx, dy)))
                .filter(|&d| d != (0, 0))
                .map(|(dx, dy)| {
                    let (x, y) = (c[0] as i32 + dx, c[1] as i32 + dy);
                    let w = [x.rem_euclid(n) as u32, y.rem_euclid(n) as u32];
                    let shift = Vec2::new((x - w[0] as i32) as f32, (y - w[1] as i32) as f32);
                    (tile_point(w).0 + shift - center).length()
                })
                .fold(f32::INFINITY, f32::min)
        };
        let lonely = cells
            .max_by(|a, b| spacing(*a).total_cmp(&spacing(*b)))
            .unwrap();
        assert!(spacing(lonely) > 1.0, "{}", spacing(lonely));
        let (center, _, size, _) = tile_point(lonely);
        // The plane's coordinates that read the tile at the center.
        let g = center - tile_offset(2, 0) * TILE_CELLS as f32;
        let scale = |keep: f32| rows[7][0] * rows[7][3] * keep.sqrt();
        let reach = |keep: f32| {
            (0..500)
                .map(|i| i as f32 * 0.001)
                .take_while(|&dx| {
                    plane_points(&rows, tile(), g + Vec2::new(dx, 0.0), 2, 0, scale(keep)) > 0.0
                })
                .count() as f32
                * 0.001
        };
        let radius = |keep: f32| scale(keep) * (SIZE[0] + (SIZE[1] - SIZE[0]) * size);
        for keep in [1.0, NEAR_KEEP, 0.1] {
            // The tile holds the distance in 8 bits and is read through a
            // linear filter: a few thousandths of a cell.
            assert!(
                (reach(keep) - radius(keep)).abs() < 0.01,
                "{keep}: {} vs {}",
                reach(keep),
                radius(keep)
            );
        }
        assert_close!(reach(0.0), 0.0);
        let area = |keep: f32| radius(keep).powi(2);
        assert!((area(NEAR_KEEP) / area(1.0) - NEAR_KEEP).abs() < 1e-5);
    }

    #[test]
    fn the_tile_holds_a_soft_point_in_every_cell_and_wraps() {
        let t = tile();
        assert_eq!(t.len(), (TILE * TILE) as usize);
        let per = (TILE / TILE_CELLS) as f32;
        let mut shared = 0;
        for cy in 0..TILE_CELLS {
            for cx in 0..TILE_CELLS {
                let (center, phase, size, rank) = tile_point([cx, cy]);
                // At the center, all of its reach (the linear filter rounds
                // the cone's tip by under a texel) and its own attributes,
                // but where another point's center lies within a texel or
                // two and the filter blends the two.
                let s = sample_tile(t, center / TILE_CELLS as f32);
                assert!(s.x > 0.9, "{cx} {cy}: {s}");
                let own = (s.y - phase).abs() < 0.01
                    && (s.z - size).abs() < 0.01
                    && (s.w - rank).abs() < 0.01;
                shared += usize::from(!own);
                assert!(center.x - (cx as f32) < 1.0 && center.y - (cy as f32) < 1.0);
            }
        }
        assert!(shared <= 3, "{shared}");
        // Wrapping: the texels across the tile's edge read like neighbors.
        let row = |y: usize| (0..TILE as usize).map(move |x| t[y * TILE as usize + x][0]);
        let jumps = |v: Vec<u8>| v.windows(2).map(|w| w[0].abs_diff(w[1])).max().unwrap();
        let seam: Vec<u8> = (0..TILE as usize)
            .map(|y| t[y * TILE as usize + TILE as usize - 1][0].abs_diff(t[y * TILE as usize][0]))
            .collect();
        assert!(
            seam.iter().max() <= Some(&(jumps(row(7).collect()) + 4)),
            "{seam:?}"
        );
        // The cone falls 255 / (REACH * per) a texel.
        let slope = 255.0 / (REACH * per);
        assert!(jumps(row(100).collect()) as f32 <= slope + 2.0);
        // A point's area: at most a quarter of a cell's per point, as the
        // fill's largest radius allows.
        let (radius, _) = point_layout(FILL_RANGE[1]);
        assert!(radius * SIZE[1] * (1.0 + SWELL) <= REACH);
    }

    #[test]
    fn the_wave_is_a_cheap_cosine() {
        // 1 at whole numbers, 0 halfway, period 1, within about a hundredth
        // of the raised cosine it stands in for, and its square's mean is
        // CREST_MEAN.
        for x in [-3.0f32, -1.0, 0.0, 1.0, 7.0] {
            assert!((wave(x) - 1.0).abs() < 1e-6);
            assert!(wave(x + 0.5) < 1e-6);
        }
        let n = 4000;
        let (mut worst, mut mean) = (0.0f32, 0.0f32);
        for i in 0..n {
            let x = i as f32 / n as f32 * 3.0 - 1.5;
            let cosine = 0.5 + 0.5 * (TAU * x).cos();
            worst = worst.max((wave(x) - cosine).abs());
            assert!((wave(x) - wave(x + 1.0)).abs() < 1e-4);
            mean += wave(x).powi(2) / n as f32;
        }
        assert!(worst < 0.012, "{worst}");
        assert!((mean - CREST_MEAN).abs() < 2e-3, "{mean}");
    }

    #[test]
    fn far_things_keep_a_floor_of_light() {
        let rows = lit_rows();
        let r = eye_at(Vec3::new(0.0, 1.2, 0.0), Quat::IDENTITY);
        let wall_at = |z: f32| {
            let map = map_of(&r, move |o, d| {
                plane(o, d, Vec3::new(0.0, 0.0, -z), Vec3::Z)
            });
            vertex_light(&r, &rows, vertex_near(&r, Vec3::new(0.0, 1.2, -z)), &map)
        };
        assert_close!(wall_at(1.0).bright, 1.0);
        assert_close!(wall_at(FADE_M[0] - 0.01).bright, 1.0);
        let mid = wall_at(f32::midpoint(FADE_M[0], FADE_M[1])).bright;
        assert!((mid - f32::midpoint(1.0, FAR_FLOOR)).abs() < 0.02, "{mid}");
        for z in [3.01, 4.0, 6.0, 9.0] {
            let v = wall_at(z);
            assert_close!(v.bright, FAR_FLOOR);
            assert_close!(v.keep, 1.0);
        }
        // So a far shelf's points have color: the fragment's light there is
        // the floor's share of a near one's.
        let at = |bright: f32| {
            fragment(
                &rows,
                tile(),
                Vec3::ZERO,
                Vec3::new(0.0, 0.0, -3.0),
                Vec3::Z,
                1.0,
                bright,
                Vec3::ZERO,
            )
        };
        let (near, far) = (at(1.0), at(FAR_FLOOR));
        assert!((far[1] - near[1] * FAR_FLOOR).abs() < 1e-5);
    }

    #[test]
    fn a_thin_pole_carries_points_wherever_it_lies() {
        // Poles 3 texels across, 1 m out in front of a wall 3 m out, slid
        // across the texel grid by quarter texels: a vertex always lands
        // on the pole, keeps the near side's share and the full light;
        // the wall's vertices beside it keep none.
        let rows = lit_rows();
        let r = eye_at(Vec3::new(0.0, 1.2, 0.0), Quat::IDENTITY);
        let texel_m = |dist: f32| dist * (FOV.right.tan() - FOV.left.tan()) / MAP as f32;
        let far_wall = |o, d| plane(o, d, Vec3::new(0.0, 0.0, -3.0), Vec3::Z);
        for shift in [0.0, 0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 1.75] {
            let x = shift * texel_m(1.0);
            let c = Vec3::new(x, 0.0, -1.0);
            let radius = 1.5 * texel_m(1.0);
            let map = map_of(&r, move |o, d| {
                nearest([pole(o, d, c, radius), far_wall(o, d)])
            });
            let mid = vertex_near(&r, Vec3::new(x, 1.2, -1.0));
            let row: Vec<VertexLight> = (0..9)
                .map(|k| [mid[0] + k * STEP - 4 * STEP, mid[1]])
                .map(|t| vertex_light(&r, &rows, t, &map))
                .collect();
            let on: Vec<&VertexLight> = row.iter().filter(|v| v.world.z > -1.5).collect();
            assert!(!on.is_empty(), "{shift}");
            for v in &on {
                assert_close!(v.keep, NEAR_KEEP);
                assert_close!(v.bright, 1.0);
            }
            let beside = row
                .iter()
                .filter(|v| v.world.z < -2.5)
                .map(|v| v.keep)
                .fold(1.0f32, f32::min);
            assert_close!(beside, 0.0);
        }
    }

    #[test]
    fn a_texel_with_no_data_keeps_nothing_and_its_neighbors_keep_theirs() {
        let r = seated();
        let rows = lit_rows();
        let c = Vec3::new(0.0, 0.9, -1.0);
        let t = vertex_near(&r, c);
        let good = map_of(&r, |o, d| plane(o, d, c, Vec3::Z));
        assert_close!(vertex_light(&r, &rows, t, &good).keep, 1.0);
        for bad in [1.0, encode(NEAR_CUT_M * 0.5)] {
            let hole = |x: [u32; 2]| if x == t { bad } else { good(x) };
            let v = vertex_light(&r, &rows, t, hole);
            assert_close!([v.keep, v.bright], [0.0, 0.0]);
            assert_close!(v.clip.z, v.clip.w);
            // A neighbor without data: the other side gives the normal and
            // nothing thins.
            let next = [t[0] + STEP, t[1]];
            let hole = |x: [u32; 2]| if x == next { bad } else { good(x) };
            let v = vertex_light(&r, &rows, t, hole);
            assert_close!(v.keep, 1.0);
            assert!(v.normal.z > 0.99, "{v:?}");
        }
    }

    #[test]
    fn the_points_cover_about_the_fill_and_fade_out_when_small() {
        let mut d = Drive::default();
        d.step(0.0, 0.0, 0.0);
        for fill in [0.1, FILL, FILL_RANGE[1]] {
            let rows = rows(&d, 1.0, [TEAL; 2], fill, 0.0);
            let scale = rows[7][0] * rows[7][3];
            // Over 40 x 40 cells of the plane facing z, sampled finely.
            let (mut covered, mut n) = (0, 0);
            for i in 0..400 {
                for j in 0..400 {
                    let g = Vec2::new(i as f32 * 0.1 + 0.05, j as f32 * 0.1 + 0.05);
                    covered += usize::from(plane_points(&rows, tile(), g, 2, 0, scale) > 0.0);
                    n += 1;
                }
            }
            let share = covered as f32 / n as f32;
            // Overlapping points cover a little less than their areas.
            assert!(share > 0.7 * fill && share < 1.05 * fill, "{fill}: {share}");
        }
        // Cells a few pixels small: the points fade out.
        let rows = lit_rows();
        let p = Vec3::new(0.013, 1.007, -1.0);
        let small = points(
            &rows,
            tile(),
            Vec3::new(0.0, 1.2, 0.0),
            p,
            Vec3::Z,
            1.0,
            Vec3::splat(0.03),
        );
        assert_close!(small, 0.0);
        // The fill's layout: a point in every cell down to the smallest
        // radius, fewer below it; the largest swollen point within half a
        // cell.
        let (radius, presence) = point_layout(FILL);
        assert_close!(presence, 1.0);
        assert!(radius * SIZE[1] * (1.0 + SWELL) <= 0.5);
        let (low, fewer) = point_layout(FILL_RANGE[0]);
        assert_close!(low, RADIUS_MIN);
        assert!(fewer < 1.0);
        let (high, _) = point_layout(1.0);
        assert!(high * SIZE[1] * (1.0 + SWELL) <= 0.5 + 1e-6);
        // The soft profile's share of the disk, by numbers.
        let k = 2000;
        let numeric: f32 = (0..k)
            .map(|i| {
                let x = (i as f32 + 0.5) / k as f32;
                (1.0 - smoothstep(SOFT_CORE, 1.0, x)) * 2.0 * x / k as f32
            })
            .sum();
        assert!((numeric - profile_share(SOFT_CORE)).abs() < 1e-3);
    }

    #[test]
    fn far_or_near_a_cell_keeps_its_size_on_screen() {
        let rows = lit_rows();
        let eye = Vec3::ZERO;
        // At 1, 2 and 4 m the cells are 1, 2 and 4 times CELL_M: the same
        // angle. Across an octave's boundary the light is continuous.
        for d in [1.0f32, 2.0, 4.0] {
            let l = d.log2();
            assert_close!(CELL_M * 2f32.powi(l.floor() as i32), CELL_M * d);
        }
        let at = |z: f32| {
            points(
                &rows,
                tile(),
                eye,
                Vec3::new(0.0123, 0.0456, -z),
                Vec3::Z,
                1.0,
                Vec3::splat(1e-5),
            )
        };
        let (below, above) = (at(2.0 - 1e-4), at(2.0 + 1e-4));
        assert!((below - above).abs() < 2e-3, "{below} {above}");
    }

    #[test]
    fn the_light_rises_with_immersion_and_the_gain() {
        assert_close!(peak_alpha(0.0, 1.0), ALPHA_LOW);
        assert_close!(peak_alpha(1.0, 1.0), ALPHA_HIGH);
        assert_close!(peak_alpha(0.5, 1.0), f32::midpoint(ALPHA_LOW, ALPHA_HIGH));
        assert_close!(peak_alpha(2.0, 1.0), ALPHA_HIGH);
        assert_close!(peak_alpha(f32::NAN, 1.0), ALPHA_LOW);
        assert_close!(peak_alpha(1.0, 10.0), GAIN_MAX * ALPHA_HIGH);
        assert_close!(peak_alpha(1.0, 0.0), 0.0);
        // The mean light at a quarter covered stays well under a lit
        // surface's full alpha.
        let mean = FILL * profile_share(SOFT_CORE) * (1.0 - TWINKLE_DEPTH / 2.0);
        assert!(ALPHA_HIGH * mean < crate::immerse::ALPHA_FULL / 4.0);
    }

    #[test]
    fn the_wave_runs_through_the_points_and_fades_when_thin() {
        // A top (its points on the horizontal plane, which the height's
        // change across a pixel does not touch), one color for both, so
        // the light out is the points times the wave.
        let mut rows = lit_rows();
        rows[2] = [rows[1][0], rows[1][1], rows[1][2], BAND_TINT];
        let eye = Vec3::new(0.0, 1.0, 0.0);
        let at = |rows: &[[f32; 4]; ROWS], p: Vec3, fwy: f32| {
            fragment(
                rows,
                tile(),
                eye,
                p,
                Vec3::Y,
                1.0,
                1.0,
                Vec3::new(1e-5, fwy, 1e-5),
            )[1]
        };
        let lit = (0..400)
            .map(|i| Vec3::new(i as f32 * 0.001, 0.0, -0.5))
            .max_by(|a, b| at(&rows, *a, 1e-5).total_cmp(&at(&rows, *b, 1e-5)))
            .expect("a point");
        let crest_light = at(&rows, lit, 1e-5);
        assert!(crest_light > 0.05, "{crest_light}");
        // The wave moved half a band puts the point in a trough.
        let mut trough = rows;
        trough[0][1] = 0.5;
        let crest_band = STEADY + (1.0 - STEADY) * rows[0][3];
        let ratio = at(&trough, lit, 1e-5) / crest_light;
        assert!((ratio - STEADY / crest_band).abs() < 1e-3, "{ratio}");
        // A band a few pixels thin: the wave's mean, the points unchanged.
        let mean_band = STEADY + (1.0 - STEADY) * CREST_MEAN * rows[0][3];
        let ratio = at(&rows, lit, 1.0) / crest_light;
        assert!((ratio - mean_band / crest_band).abs() < 1e-3, "{ratio}");
        assert!((crest(&rows, 0.0) - 1.0).abs() < 1e-6);
        assert!(crest(&rows, BAND_SPACING_M / 2.0) < 1e-6);
        let n = 1000;
        let m: f32 = (0..n)
            .map(|k| crest(&rows, k as f32 / n as f32 * BAND_SPACING_M))
            .sum::<f32>()
            / n as f32;
        assert!((m - CREST_MEAN).abs() < 1e-4, "{m}");
    }

    #[test]
    fn the_drive_drifts_twinkles_with_the_music_and_wraps() {
        let mut quiet = Drive::default();
        let mut loud = Drive::default();
        for _ in 0..72 {
            quiet.step(1.0 / 72.0, 0.0, 0.0);
            loud.step(1.0 / 72.0, 1.0, 0.0);
        }
        assert!(
            (quiet.phase() - DRIFT_BANDS_S[0]).abs() < 1e-4,
            "{}",
            quiet.phase()
        );
        assert!((quiet.twinkle() - TWINKLE_HZ).abs() < 1e-4);
        assert!(loud.phase() > 0.9 * DRIFT_BANDS_S[1], "{}", loud.phase());
        assert!(loud.twinkle() > 1.9 * TWINKLE_HZ);
        assert!(loud.music() > 0.99);
        // The beat speeds both.
        let (mut a, mut b) = (loud, loud);
        a.step(0.5, 1.0, 0.0);
        b.step(0.5, 1.0, 1.0);
        let moved = |x: f32, from: f32| (x - from).rem_euclid(1.0);
        assert!(
            (moved(b.phase(), loud.phase()) - 2.0 * moved(a.phase(), loud.phase())).abs() < 1e-4
        );
        // Both wrap in 0..1, the twinkle's integer rates keep a point's
        // twinkle continuous across the wrap.
        for _ in 0..1000 {
            a.step(0.1, 1.0, 1.0);
            assert!((0.0..1.0).contains(&a.phase()) && (0.0..1.0).contains(&a.twinkle()));
        }
        for rate in [1.0f32, 2.0, 3.0] {
            let tw = |t: f32| (TAU * (rate * t + 0.3)).sin();
            assert!((tw(0.0) - tw(1.0)).abs() < 1e-4);
        }
        // Bad input moves nothing.
        let mut f = Drive::default();
        f.step(f32::NAN, f32::NAN, f32::INFINITY);
        assert_eq!(f, Drive::default());
    }

    #[test]
    fn the_colors_follow_the_theme() {
        let key = [0.9, 0.3, 1.0];
        // Board #5543: the theme's own two for the depth light.
        assert!(uses_key(Some(Theme::Aurora)));
        assert_close!(colors(Some(Theme::Aurora), key), [TEAL, key]);
        assert!(!uses_key(Some(Theme::Drift)));
        assert_close!(colors(Some(Theme::Drift), key), [TEAL, WARM_WHITE]);
        assert_close!(colors(Some(Theme::Classic), key), [GREEN, GREEN]);
        assert_close!(colors(None, key), [GREEN, GREEN]);
        for t in Theme::ALL {
            let [side, top] = t.depth().colors;
            assert_eq!(color_indices(Some(t)), [side, top]);
            assert_close!(
                colors(Some(t), key),
                [
                    surface_color(side, KIND_OTHER, key),
                    surface_color(top, KIND_OTHER, key)
                ]
            );
            assert_eq!(band(Some(t)), t.depth().band);
        }
        assert_eq!(
            colors_line(Some(Theme::Aurora)),
            "depth light: beams, teal sides, the key's tint on top, bass, occluding (theme aurora)"
        );
        assert_eq!(
            colors_line(Some(Theme::Mirror)),
            "depth light: glints as points, warm white sides, the same on top, high, occluding (theme mirror)"
        );
        assert_eq!(
            colors_line(None),
            "depth light: other's own sides, the same on top, rms (no theme saved)"
        );
        assert_eq!(band(Some(Theme::Aurora)), BAND_BASS);
        assert_eq!(band(Some(Theme::Contour)), BAND_MID);
        assert_eq!(band(None), crate::surface_fx::BAND_RMS);
        // A theme whose depth light occludes keeps the hard cutout; the
        // veil's knob at 0 or 1 decides over it, and with no theme the
        // veil is the knob's.
        let d = DepthLightOptions::default();
        assert!(d.veil && !d.veil_forced);
        for t in Theme::ALL {
            assert!(!d.under(Some(t)).veil, "{t:?}");
        }
        assert!(d.under(None).veil);
        let knob = |v: &'static str| {
            DepthLightOptions::from_knobs(|name| (name == KNOB_VEIL).then(|| v.to_owned()))
        };
        assert!(knob("1").veil_forced && knob("1").under(Some(Theme::Aurora)).veil);
        assert!(knob("0").veil_forced && !knob("0").under(None).veil);
        assert!(!knob("").veil_forced && !knob("").under(Some(Theme::Aurora)).veil);
        assert!(!knob(" ").veil_forced && knob(" ").under(None).veil);
        assert_eq!(
            DepthLightOptions { gain: 2.0, ..d }.under(Some(Theme::Flint)),
            DepthLightOptions {
                gain: 2.0,
                veil: false,
                ..d
            }
        );
        let audio = Audio {
            rms: 0.2,
            bass: 0.7,
            mid: 0.4,
            high: 1.5,
            ..Audio::default()
        };
        assert_close!(band_level(&audio, BAND_BASS), 0.7);
        assert_close!(band_level(&audio, BAND_MID), 0.4);
        assert_close!(band_level(&audio, BAND_HIGH), 1.0);
        assert_close!(band_level(&audio, crate::surface_fx::BAND_RMS), 0.2);
    }

    #[test]
    fn the_rows_pack_what_the_shader_reads() {
        let mut d = Drive::default();
        d.step(0.5, 0.5, 0.0);
        let rows = rows(&d, 0.2, [[0.1, 0.2, 0.3], [0.4, 0.5, 0.6]], FILL, 0.7);
        let (radius, presence) = point_layout(FILL);
        assert_close!(
            rows[0],
            [
                1.0 / BAND_SPACING_M,
                d.phase(),
                0.2,
                MUSIC_FLOOR + (1.0 - MUSIC_FLOOR) * d.music()
            ]
        );
        assert_close!(rows[1], [0.1, 0.2, 0.3, STEADY]);
        assert_close!(rows[2], [0.4, 0.5, 0.6, BAND_TINT]);
        assert_close!(
            rows[3],
            [
                AA_BANDS_PER_PX[0],
                AA_BANDS_PER_PX[1],
                CREST_MEAN,
                d.twinkle()
            ]
        );
        assert_close!(rows[4], [FADE_M[0], FADE_M[1], FAR_FLOOR, 0.7]);
        assert_close!(rows[5], [JUMP_MIN_M, JUMP_REL, JUMP_SOFT, NEAR_KEEP]);
        assert_close!(
            rows[6],
            [CELL_M, OCTAVE_BLEND, AA_CELLS_PER_PX[0], AA_CELLS_PER_PX[1]]
        );
        assert_close!(rows[7], [radius, presence, REACH, 1.0 + SWELL * d.music()]);
        assert_close!(rows[8], [SIZE[0], SIZE[1], SOFT_CORE, TWINKLE_DEPTH]);
        const {
            assert!(FADE_M[0] < FADE_M[1] && AA_BANDS_PER_PX[0] < AA_BANDS_PER_PX[1]);
            assert!(AA_CELLS_PER_PX[0] < AA_CELLS_PER_PX[1] && SIZE[0] < SIZE[1]);
            assert!(REACH <= 0.5 && JUMP_SOFT < 1.0);
        };
        // The rows end the occluder's uniform, from `light` on.
        assert_eq!(LIGHT_ROW + ROWS, UNIFORM_ROWS);
        let module = naga::front::wgsl::parse_str(ENV_DEPTH_WGSL).expect("WGSL parses");
        let (_, ty) = module
            .types
            .iter()
            .find(|(_, t)| t.name.as_deref() == Some("EnvEye"))
            .expect("struct EnvEye");
        let naga::TypeInner::Struct { members, span } = &ty.inner else {
            panic!("EnvEye is not a struct");
        };
        assert_eq!(*span as usize, UNIFORM_ROWS * 16);
        let offset = |name: &str| {
            members
                .iter()
                .find(|m| m.name.as_deref() == Some(name))
                .map(|m| m.offset as usize)
                .unwrap_or_else(|| panic!("member {name}"))
        };
        for (k, name) in [
            "light",
            "light_side",
            "light_top",
            "light_wave",
            "light_fade",
            "light_jump",
            "light_cell",
            "light_point",
            "light_size",
            "light_look",
            "light_move",
            "light_more",
            "sonar",
        ]
        .into_iter()
        .enumerate()
        {
            assert_eq!(offset(name), (LIGHT_ROW + k) * 16, "{name}");
        }
        assert_eq!(offset("mesh"), crate::env_depth::MESH_ROW * 16);
        // The WGSL's constants are these.
        for line in [
            format!("const PLANE_MIN: f32 = {PLANE_MIN:?};"),
            format!("const OCTAVE_MIN: f32 = {:?};", OCTAVE_RANGE[0]),
            format!("const OCTAVE_MAX: f32 = {:?};", OCTAVE_RANGE[1]),
            format!("const TILE_CELLS: f32 = {:?};", TILE_CELLS as f32),
        ] {
            assert!(ENV_DEPTH_WGSL.contains(&line), "{line}");
        }
        // The hash is PCG's: a known value.
        assert_eq!(hash(0), 129_708_002);
    }

    #[test]
    fn the_knobs_parse_clamp_and_log() {
        let knobs = |pairs: &[(&str, &str)]| {
            let map: std::collections::HashMap<String, String> = pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect();
            DepthLightOptions::from_knobs(move |name| map.get(name).cloned())
        };
        assert_eq!(knobs(&[]), DepthLightOptions::default());
        let d = DepthLightOptions::default();
        assert!(d.on && d.veil);
        assert_close!([d.gain, d.fill], [1.0, FILL]);
        assert!(!knobs(&[(KNOB, " 0 ")]).on);
        assert!(knobs(&[(KNOB, "yes")]).on);
        assert!(!knobs(&[(KNOB_VEIL, "0")]).veil);
        assert!(knobs(&[]).points && !knobs(&[(KNOB_POINTS, "0")]).points);
        let no_points = frame(
            &knobs(&[(KNOB_POINTS, "0")]),
            &Drive::default(),
            None,
            [1.0; 3],
            1.0,
            &NO_SONAR,
            &Around::default(),
        );
        assert_close!(no_points.rows[0][2], 0.0);
        assert!(
            knobs(&[(KNOB_POINTS, "0")])
                .line(None)
                .contains("no points")
        );
        assert!(knobs(&[(KNOB_VEIL, "1")]).veil);
        assert_close!(knobs(&[(KNOB_GAIN, "2.5")]).gain, 2.5);
        assert_close!(knobs(&[(KNOB_GAIN, "50")]).gain, GAIN_MAX);
        assert_close!(knobs(&[(KNOB_GAIN, "nan")]).gain, 1.0);
        assert_close!(knobs(&[(KNOB_FILL, "0.1")]).fill, 0.1);
        assert_close!(knobs(&[(KNOB_FILL, "0.9")]).fill, FILL_RANGE[1]);
        assert_close!(knobs(&[(KNOB_FILL, "0")]).fill, FILL_RANGE[0]);
        assert_close!(knobs(&[(KNOB_FILL, "x")]).fill, FILL);
        // It needs the mesh, and the occluder, and not the gray.
        assert_eq!(blocked(true, false, 2), None);
        assert!(blocked(true, false, 0).is_some_and(|w| w.contains("envdepthmesh 0")));
        assert!(blocked(false, false, 2).is_some_and(|w| w.contains("envdepth")));
        assert!(blocked(true, true, 2).is_some_and(|w| w.contains("envdepthshow")));
        let on = DepthLightOptions::default();
        let off = knobs(&[(KNOB, "0")]);
        assert!(on.lit(None) && !on.lit(Some("x")) && !off.lit(None));
        let line = on.line(None);
        assert!(
            line.starts_with("depth light: on, points · gain 1 · fill 0.25 · veil the theme's")
        );
        assert!(knobs(&[(KNOB_VEIL, "1")]).line(None).contains("veil on:"));
        assert!(knobs(&[(KNOB_VEIL, "0")]).line(None).contains("veil off"));
        assert!(
            off.line(None)
                .starts_with("depth light: off (debug.fosfora.depthlight 0)")
        );
        let mesh0 = blocked(true, false, 0);
        assert!(
            on.line(mesh0)
                .starts_with("depth light: off, it needs the occluder mesh")
        );
        assert_eq!(off.line(mesh0), off.line(None));
        let change = on.change_line(&off, None);
        assert!(change.starts_with("depth light: on") && change.contains("(was: off"));
        // The frame: the veil by the level, or 1 with it off.
        let drive = Drive::default();
        let f = frame(
            &on,
            &drive,
            Some(Theme::Aurora),
            [1.0; 3],
            0.75,
            &NO_SONAR,
            &Around::default(),
        );
        assert!(f.veil);
        assert_close!(f.rows[4][3], 0.25);
        let f = frame(
            &knobs(&[(KNOB_VEIL, "0")]),
            &drive,
            None,
            [1.0; 3],
            0.75,
            &NO_SONAR,
            &Around::default(),
        );
        assert!(!f.veil);
        assert_close!(f.rows[4][3], 1.0);
    }
}
