//! The world sim's cell list for the room-box collide (board #3808):
//! `flux_xr_sim.wgsl`'s `xr_collide` tested every alive particle against
//! every room box every frame (about 1.1 ms of the 3.9 ms sim dispatch on
//! the Quest 3, `docs/xr/MEASURED.md`), while a particle only ever touches
//! the one or two boxes near it.
//!
//! The sim volume, the cube of half extent `half` around the anchor, is cut
//! into N x N x N cells, and each cell gets a 32-bit mask of the boxes that
//! can touch it (bit k = box k). The XR app builds the masks on the CPU each
//! frame from the anchor-relative boxes and uploads them after the surface
//! lanes (`scene.rs`); the sim looks a particle's cell up and visits only
//! the boxes in its mask, in the same ascending order, and runs the full
//! loop outside the grid (a free burst particle flies up to 13 m) or with
//! no grid (N = 0).
//!
//! A cell's mask is a superset of the boxes any point in it can be inside
//! (a box's conservative axis-aligned bounds, dilated by the collide's
//! margin and [`PAD_M`]), so a skipped box is one the full loop would have
//! tested and found the particle outside of: the same result, bit for bit.
//!
//! Aux layout (`XR_AUX_CELLS` in the sim): one header row, x = N as u32
//! bits (0: no grid), y = the grid's half extent (m), z and w 0; then
//! [`CELL_ROWS`] rows of masks, four cells per row (cell c in row c / 4,
//! lane c % 4, as u32 bits), cell c = (ix * N + iy) * N + iz. The block is
//! sized for [`CELLS_MAX`] whatever N is, so the layout never moves.

use glam::{Mat3, Vec3};

/// The most cells per axis (`XR_CELLS_MAX` in the sim).
pub const CELLS_MAX: u32 = 16;
/// Mask rows: [`CELLS_MAX`]^3 cells, four per row (`XR_AUX_CELL_ROWS`).
pub const CELL_ROWS: usize = (CELLS_MAX * CELLS_MAX * CELLS_MAX / 4) as usize;
/// The block's rows: the header, then the masks.
pub const ROWS: usize = 1 + CELL_ROWS;
/// Cells per axis when `debug.fosfora.boxcells` is unset.
pub const DEFAULT_CELLS: u32 = 8;
/// Extra clearance on every box's bounds (m): covers the rounding between
/// the CPU's bounds and the sim's inside test and cell index (both of
/// order 1e-6 m in a room), so the masks stay a superset on the GPU.
pub const PAD_M: f32 = 1e-3;

/// The cells per axis a `debug.fosfora.boxcells` value asks for: N in
/// 2..=[`CELLS_MAX`], or 0 for "0" and "1" (off: the full loop). `None`
/// for anything else (the caller warns and keeps [`DEFAULT_CELLS`]).
#[must_use]
pub fn parse(value: &str) -> Option<u32> {
    match value.trim().parse::<u32>().ok()? {
        0 | 1 => Some(0),
        n if n <= CELLS_MAX => Some(n),
        _ => None,
    }
}

/// The knob's value for the log: N, or "off".
#[must_use]
pub fn describe(cells: u32) -> String {
    if cells == 0 {
        "off".to_owned()
    } else {
        cells.to_string()
    }
}

/// The header row for a grid of `cells` per axis over the cube of half
/// extent `half`: x = N as u32 bits, y = `half`. A `cells` of 0, or beyond
/// [`CELLS_MAX`], or a `half` that is not positive writes N = 0: no grid.
#[must_use]
pub fn header(cells: u32, half: f32) -> [f32; 4] {
    let n = if (1..=CELLS_MAX).contains(&cells) && half > 0.0 {
        cells
    } else {
        0
    };
    [f32::from_bits(n), half, 0.0, 0.0]
}

/// The cell along one axis holding coordinate `v`, as the sim computes it:
/// floor((v + half) / (2 half) N), clamped to 0..N.
fn cell_of(v: f32, half: f32, n: u32) -> u32 {
    let t = ((v + half) / (2.0 * half) * n as f32).floor().max(0.0);
    // Saturating: a NaN or a huge value lands in a clamped cell.
    (t as u32).min(n - 1)
}

/// The mask rows ([`CELL_ROWS`] of them) for `boxes` (center, rotation box
/// -> world as a quaternion (x, y, z, w), half extents; anchor-relative, in
/// upload order, at most 32) on a grid of `cells` per axis over the cube of
/// half extent `half`, with the collide's `margin`. Rows past N^3 / 4 are 0;
/// with no grid ([`header`] writes N = 0) every row is.
///
/// Box k sets bit k in every cell its bounds reach: center +- e, e_i =
/// sum_j |M_ij| (h_j + margin) + [`PAD_M`], where M takes the box's frame
/// to the world. M is the inverse of the sim's own world -> box operator
/// (`xr_quat_rotate` with the conjugate), which for a unit quaternion is
/// its rotation matrix, so the bounds hold whatever the sim computes. A box
/// whose bounds miss the grid sets nothing; one whose bounds are not
/// finite sets every cell (never fewer than the full loop's).
#[must_use]
pub fn masks(
    boxes: &[([f32; 3], [f32; 4], [f32; 3])],
    margin: f32,
    cells: u32,
    half: f32,
) -> Vec<[f32; 4]> {
    let mut bits = vec![0u32; CELL_ROWS * 4];
    let n = f32::to_bits(header(cells, half)[0]);
    if n == 0 {
        return vec![[0.0; 4]; CELL_ROWS];
    }
    for (k, &(center, rot, size)) in boxes.iter().enumerate().take(32) {
        let bit = 1u32 << k;
        let h = Vec3::from(size) + Vec3::splat(margin);
        // The sim's inside test needs |l_i| < h_i on every axis: a box
        // with an empty extent never holds a particle.
        if h.min_element() <= 0.0 {
            continue;
        }
        let to_world = world_from_box(rot);
        let e = Vec3::new(
            to_world.row(0).abs().dot(h),
            to_world.row(1).abs().dot(h),
            to_world.row(2).abs().dot(h),
        ) + Vec3::splat(PAD_M);
        let c = Vec3::from(center);
        let (lo, hi) = (c - e, c + e);
        let range = if lo.is_finite() && hi.is_finite() {
            if hi.cmplt(Vec3::splat(-half)).any() || lo.cmpgt(Vec3::splat(half)).any() {
                continue;
            }
            [lo, hi].map(|v| v.to_array().map(|x| cell_of(x, half, n)))
        } else {
            [[0; 3], [n - 1; 3]]
        };
        let [from, to] = range;
        for ix in from[0]..=to[0] {
            for iy in from[1]..=to[1] {
                for iz in from[2]..=to[2] {
                    bits[((ix * n + iy) * n + iz) as usize] |= bit;
                }
            }
        }
    }
    bits.chunks_exact(4)
        .map(|c| [c[0], c[1], c[2], c[3]].map(f32::from_bits))
        .collect()
}

/// The header and the mask rows, the block the sim reads ([`ROWS`]).
#[must_use]
pub fn rows(
    boxes: &[([f32; 3], [f32; 4], [f32; 3])],
    margin: f32,
    cells: u32,
    half: f32,
) -> Vec<[f32; 4]> {
    let mut out = Vec::with_capacity(ROWS);
    out.push(header(cells, half));
    out.extend(masks(boxes, margin, cells, half));
    out
}

/// The sim's `xr_quat_rotate(q, v)`: v + 2 q.xyz x (q.xyz x v + q.w v).
fn quat_rotate(q: [f32; 4], v: Vec3) -> Vec3 {
    let u = Vec3::new(q[0], q[1], q[2]);
    v + 2.0 * u.cross(u.cross(v) + q[3] * v)
}

/// The box -> world matrix: the inverse of the sim's world -> box operator,
/// `xr_quat_rotate(xr_quat_conj(q), _)` (linear in its argument, so its
/// matrix is its images of the axes). Not finite when that operator is
/// singular (never for a quaternion near unit length).
fn world_from_box(q: [f32; 4]) -> Mat3 {
    let conj = [-q[0], -q[1], -q[2], q[3]];
    Mat3::from_cols(
        quat_rotate(conj, Vec3::X),
        quat_rotate(conj, Vec3::Y),
        quat_rotate(conj, Vec3::Z),
    )
    .inverse()
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestBox = ([f32; 3], [f32; 4], [f32; 3]);

    const IDENTITY: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

    /// The mask of cell (ix, iy, iz) in `rows`.
    fn mask_at(rows: &[[f32; 4]], n: u32, i: [u32; 3]) -> u32 {
        let c = ((i[0] * n + i[1]) * n + i[2]) as usize;
        rows[c / 4][c % 4].to_bits()
    }

    /// The cells (ix, iy, iz) whose mask has bit `k`.
    fn cells_with(rows: &[[f32; 4]], n: u32, k: u32) -> Vec<[u32; 3]> {
        let mut out = Vec::new();
        for ix in 0..n {
            for iy in 0..n {
                for iz in 0..n {
                    if mask_at(rows, n, [ix, iy, iz]) & (1 << k) != 0 {
                        out.push([ix, iy, iz]);
                    }
                }
            }
        }
        out
    }

    /// The sim's inside test (`xr_collide`'s box loop): the point in the
    /// box's frame is inside the half extents plus the margin.
    fn inside(p: Vec3, b: &TestBox, margin: f32) -> bool {
        let (c, q, h) = b;
        let l = quat_rotate([-q[0], -q[1], -q[2], q[3]], p - Vec3::from(*c));
        let pen = Vec3::from(*h) + Vec3::splat(margin) - l.abs();
        pen.x > 0.0 && pen.y > 0.0 && pen.z > 0.0
    }

    fn yaw(deg: f32) -> [f32; 4] {
        glam::Quat::from_rotation_y(deg.to_radians()).to_array()
    }

    #[test]
    fn the_knob_takes_two_to_sixteen_and_zero_or_one_for_off() {
        assert_eq!(parse("8"), Some(8));
        assert_eq!(parse(" 16 "), Some(16));
        assert_eq!(parse("2"), Some(2));
        assert_eq!(parse("1"), Some(0));
        assert_eq!(parse("0"), Some(0));
        assert_eq!(parse("17"), None);
        assert_eq!(parse("eight"), None);
        assert_eq!(parse("-1"), None);
        assert_eq!(describe(0), "off");
        assert_eq!(describe(8), "8");
    }

    #[test]
    fn the_header_carries_n_and_the_half_extent() {
        let h = header(8, 1.5);
        assert_eq!(h[0].to_bits(), 8);
        assert_close!(h[1], 1.5);
        assert_eq!(header(0, 1.5)[0].to_bits(), 0);
        assert_eq!(header(17, 1.5)[0].to_bits(), 0);
        assert_eq!(header(8, 0.0)[0].to_bits(), 0);
        assert_eq!(rows(&[], 0.0, 8, 1.5).len(), ROWS);
        assert_eq!(CELL_ROWS, 1024);
    }

    #[test]
    fn no_grid_writes_no_masks() {
        let b = ([0.0; 3], IDENTITY, [0.5; 3]);
        let rows = masks(&[b], 0.0, 0, 1.0);
        assert_eq!(rows.len(), CELL_ROWS);
        assert!(rows.iter().flatten().all(|v| v.to_bits() == 0));
    }

    #[test]
    fn a_centered_box_marks_only_the_cells_it_overlaps() {
        // Four cells of 0.5 m over [-1, 1]: a 0.2 m box at the center
        // reaches the middle two along each axis.
        let b = ([0.0; 3], IDENTITY, [0.2; 3]);
        let rows = masks(&[b], 0.0, 4, 1.0);
        let mut want = Vec::new();
        for ix in 1..3 {
            for iy in 1..3 {
                for iz in 1..3 {
                    want.push([ix, iy, iz]);
                }
            }
        }
        assert_eq!(cells_with(&rows, 4, 0), want);
        // Off center, inside one cell, it marks that cell alone.
        let b = ([0.75, -0.75, 0.25], IDENTITY, [0.1; 3]);
        assert_eq!(cells_with(&masks(&[b], 0.0, 4, 1.0), 4, 0), [[3, 0, 2]]);
        // Rows past N^3 / 4 stay 0.
        assert!(
            masks(&[b], 0.0, 4, 1.0)[16..]
                .iter()
                .flatten()
                .all(|v| v.to_bits() == 0)
        );
    }

    /// The lowest and highest cell along `axis` that has bit 0.
    fn span(b: TestBox, n: u32, axis: usize) -> (u32, u32) {
        let cells = cells_with(&masks(&[b], 0.0, n, 1.0), n, 0);
        let along = cells.iter().map(|c| c[axis]);
        (along.clone().min().unwrap(), along.max().unwrap())
    }

    #[test]
    fn a_rotated_box_is_wider_than_its_half_extent() {
        // Eight cells of 0.25 m over [-1, 1]. A 0.3 m box at x = 0.6
        // reaches 0.3..0.9 straight (cells 5..7); turned 45 degrees about
        // y its corners reach 0.6 +- 0.42 (cells 4..7).
        let straight = ([0.6, 0.0, 0.0], IDENTITY, [0.3, 0.1, 0.3]);
        let turned = ([0.6, 0.0, 0.0], yaw(45.0), [0.3, 0.1, 0.3]);
        assert_eq!(span(straight, 8, 0), (5, 7));
        assert_eq!(span(turned, 8, 0), (4, 7));
        // A thin deep box at the center: 0.05 m in x straight (cells
        // 3..4); turned 30 degrees, x reaches 0.45 sin 30 + 0.05 cos 30 =
        // 0.27 (cells 2..5).
        let deep = ([0.0; 3], IDENTITY, [0.05, 0.1, 0.45]);
        let deep_turned = ([0.0; 3], yaw(30.0), [0.05, 0.1, 0.45]);
        assert_eq!(span(deep, 8, 0), (3, 4));
        assert_eq!(span(deep_turned, 8, 0), (2, 5));
        // y is the turn's axis: unchanged.
        assert_eq!(span(deep_turned, 8, 1), span(deep, 8, 1));
    }

    #[test]
    fn the_margin_dilates_the_bounds() {
        // A box ending 0.02 m short of a cell border reaches past it with
        // a 0.05 m margin.
        let b = ([0.0, 0.0, 0.0], IDENTITY, [0.48, 0.1, 0.1]);
        let xs = |margin| {
            let cells = cells_with(&masks(&[b], margin, 4, 1.0), 4, 0);
            let mut xs: Vec<u32> = cells.iter().map(|c| c[0]).collect();
            xs.sort_unstable();
            xs.dedup();
            xs
        };
        assert_eq!(xs(0.0), [1, 2]);
        assert_eq!(xs(0.05), [0, 1, 2, 3]);
    }

    #[test]
    fn a_box_outside_the_grid_marks_nothing() {
        let far = ([3.0, 0.0, 0.0], IDENTITY, [0.5; 3]);
        let below = ([0.0, -1.6, 0.0], IDENTITY, [5.0, 0.5, 5.0]);
        let rows = masks(&[far, below], 0.01, 8, 1.0);
        assert!(rows.iter().flatten().all(|v| v.to_bits() == 0));
        // A box reaching in from outside marks the edge cells it reaches.
        let wall = ([1.2, 0.0, 0.0], IDENTITY, [0.3, 2.0, 2.0]);
        let cells = cells_with(&masks(&[wall], 0.0, 8, 1.0), 8, 0);
        assert!(cells.iter().all(|c| c[0] >= 7));
        assert_eq!(cells.len(), 64);
    }

    #[test]
    fn each_box_sets_its_own_bit() {
        let a = ([-0.75, 0.0, 0.0], IDENTITY, [0.1; 3]);
        let b = ([0.75, 0.0, 0.0], IDENTITY, [0.1; 3]);
        let rows = masks(&[a, b], 0.0, 4, 1.0);
        assert_eq!(cells_with(&rows, 4, 0).len(), 4);
        assert!(cells_with(&rows, 4, 0).iter().all(|c| c[0] == 0));
        assert!(cells_with(&rows, 4, 1).iter().all(|c| c[0] == 3));
        assert_eq!(mask_at(&rows, 4, [0, 1, 1]), 1);
        assert_eq!(mask_at(&rows, 4, [3, 1, 1]), 2);
    }

    /// A small deterministic generator (xorshift), so the random sets are
    /// the same on every run.
    struct Rng(u32);

    impl Rng {
        fn next(&mut self) -> f32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 17;
            self.0 ^= self.0 << 5;
            (self.0 >> 8) as f32 / (1u32 << 24) as f32
        }

        fn range(&mut self, lo: f32, hi: f32) -> f32 {
            lo + (hi - lo) * self.next()
        }
    }

    #[test]
    fn every_cell_a_point_inside_a_box_lies_in_is_marked() {
        let (half, margin) = (1.5, 0.005);
        for seed in 1..=6u32 {
            let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9));
            let count = 4 + seed as usize * 2;
            let boxes: Vec<TestBox> = (0..count)
                .map(|_| {
                    let axis =
                        Vec3::new(rng.range(-1.0, 1.0), rng.range(-1.0, 1.0), 0.5).normalize();
                    let q = glam::Quat::from_axis_angle(axis, rng.range(0.0, 6.3));
                    (
                        [
                            rng.range(-2.0, 2.0),
                            rng.range(-2.0, 2.0),
                            rng.range(-2.0, 2.0),
                        ],
                        q.to_array(),
                        [
                            rng.range(0.01, 0.8),
                            rng.range(0.005, 0.5),
                            rng.range(0.01, 0.8),
                        ],
                    )
                })
                .collect();
            for n in [2, 5, 8, 16] {
                let rows = masks(&boxes, margin, n, half);
                let step = 2.0 * half / n as f32;
                // Each cell's corners (a hair inside) and center, and
                // random points over the grid, each checked in the cell
                // the sim computes for it.
                let mut points = Vec::new();
                for ix in 0..n {
                    for iy in 0..n {
                        for iz in 0..n {
                            let lo = Vec3::new(ix as f32, iy as f32, iz as f32) * step
                                - Vec3::splat(half);
                            points.push(lo + Vec3::splat(step / 2.0));
                            for corner in 0..8u32 {
                                let o = Vec3::new(
                                    (corner & 1) as f32,
                                    ((corner >> 1) & 1) as f32,
                                    ((corner >> 2) & 1) as f32,
                                );
                                points.push(lo + (o * 0.999 + Vec3::splat(0.0005)) * step);
                            }
                        }
                    }
                }
                for _ in 0..20_000 {
                    points.push(Vec3::new(
                        rng.range(-half, half),
                        rng.range(-half, half),
                        rng.range(-half, half),
                    ));
                }
                // Points on each box's dilated faces and its corners, a
                // hair inside: the ones the bounds must just reach (a
                // turned box's bounds are its corners').
                for (c, q, h) in &boxes {
                    let h = Vec3::from(*h) + Vec3::splat(margin);
                    for corner in 0..8u32 {
                        let l = Vec3::new(
                            if corner & 1 == 0 { 0.9999 } else { -0.9999 },
                            if corner & 2 == 0 { 0.9999 } else { -0.9999 },
                            if corner & 4 == 0 { 0.9999 } else { -0.9999 },
                        );
                        points.push(Vec3::from(*c) + glam::Quat::from_array(*q) * (l * h));
                    }
                    for face in 0..600 {
                        let mut l = Vec3::new(
                            rng.range(-1.0, 1.0),
                            rng.range(-1.0, 1.0),
                            rng.range(-1.0, 1.0),
                        );
                        l[face % 3] = if face % 2 == 0 { 0.9999 } else { -0.9999 };
                        let q = glam::Quat::from_array(*q);
                        points.push(Vec3::from(*c) + q * (l * h));
                    }
                }
                let mut hits = 0;
                for p in points {
                    // Outside the grid the sim runs the full loop.
                    if p.abs().max_element() > half {
                        continue;
                    }
                    let cell = p.to_array().map(|v| cell_of(v, half, n));
                    let mask = mask_at(&rows, n, cell);
                    for (k, b) in boxes.iter().enumerate() {
                        if inside(p, b, margin) {
                            hits += 1;
                            assert!(
                                mask & (1 << k) != 0,
                                "seed {seed} n {n} cell {cell:?}: box {k} holds {p} but is not in the mask {mask:#x}"
                            );
                        }
                    }
                }
                assert!(hits > 100, "seed {seed} n {n}: only {hits} points in boxes");
            }
        }
    }
}
