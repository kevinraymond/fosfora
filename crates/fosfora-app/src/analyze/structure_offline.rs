//! Non-causal song segmentation (#2027, finding #2028).
//!
//! This is the offline counterpart of the parked Movements tracker (#1488). It borrows exactly
//! one thing from it — the 27-dim section fingerprint — and drops all the causal machinery,
//! because with the whole song in hand none of it is needed. Concretely:
//!
//! | Movements (causal)                                   | Here (offline)                          |
//! |------------------------------------------------------|-----------------------------------------|
//! | Kernel runs behind the playhead; section closes when the fall is *confirmed*, a material-dependent +3.5..4.6 s after the peak | Centred kernel; the section closes **at** the peak, so lag is zero by construction |
//! | Online clustering with LRU forgetting; identity depends on arrival order | All sections clustered together, agglomerative complete-linkage — order-independent |
//! | Energy rank against identities learned *so far*, degenerate at N=1 and N=2 (#1977) | Rank over the complete set; no cold start exists |
//! | Novelty self-normalized by a decaying running max, saturating on steady material (#1973) | Absolute, divided by the kernel's own weight — a flat song reads ~0 |

use crate::audio::AudioFeatures;

use super::HopStream;

/// 7 bands + MFCC 1..=8 + 12 chroma, matching the parked tracker so the two agree on what
/// "sounds the same" means. Chroma is what lets a chorus in the same key be recognised as *the
/// same chorus* rather than merely a similar texture.
pub const FP_DIM: usize = 27;

/// Analysis frames per second the segmenter runs at. The per-hop rate (~86 Hz) is far finer
/// than section structure needs, and the checkerboard kernel is O(K²) per frame, so decimate.
const SEG_HZ: f32 = 10.0;
/// Checkerboard kernel half-width, seconds. Sets the timescale of "a change worth calling a
/// boundary" — the same 8 s full width the live A18 kernel uses.
const KERNEL_SECONDS: f32 = 4.0;
/// A section shorter than this is a fill or a turnaround, not a section.
const MIN_SECTION_SECONDS: f32 = 8.0;
/// Peak must clear `mean + PEAK_SIGMA * stddev` of the novelty curve.
const PEAK_SIGMA: f32 = 1.0;
/// Cosine distance above which two sections are different identities. Complete-linkage stops
/// merging here.
const CLUSTER_MAX_DIST: f32 = 0.06;
/// Adjacent sections closer than this are the same passage, split by a transient novelty bump
/// (a fill, a filter sweep) rather than by a change of material — so they are joined.
///
/// Deliberately a quarter of `CLUSTER_MAX_DIST`, and deliberately *not* "same cluster label".
/// Measured on a dense psytrance track, neighbours sharing a label ranged from 0.008 to 0.039,
/// and the far end of that range carried real change (onset density 2.19 → 3.82 across one
/// such pair). Collapsing by label would have destroyed it. On material that genuinely
/// reinvents itself every ten seconds, a boundary is content — only near-duplicates are noise.
const MERGE_MAX_DIST: f32 = 0.015;
/// Frames quieter than this contribute neither to a fingerprint nor to a section's level, so a
/// quiet stretch cannot drag a section's identity or under-report its energy (the bug the
/// parked branch found late).
const SOUNDING_FLOOR: f32 = 0.05;
/// Below this total energy spread a song has no dynamic contrast to report, and any ranking
/// would be an ordering of noise.
const MIN_ENERGY_SPREAD: f32 = 0.02;

/// One detected section.
#[derive(Debug, Clone)]
pub struct Section {
    pub start_secs: f64,
    pub end_secs: f64,
    /// Identity index. Sections sharing one are the same musical section returning.
    pub cluster: usize,
    /// Human-facing label for the cluster: A, B, C, …
    pub label: String,
    /// Mean loudness over sounding frames, 0..1.
    pub energy: f32,
    /// Where this section's energy sits across *all* sections in the song, 0..1. The offline
    /// replacement for `chorus_likeness` — computed once, over the complete set, so it has no
    /// cold start and is a real gradient from the first section onward.
    pub energy_rank: f32,
    /// Unit-normalized 27-dim identity vector.
    pub fingerprint: [f32; FP_DIM],
}

/// Everything the segmenter derived, including the curve itself so a caller can plot or
/// re-threshold it without re-running the analysis.
pub struct Segmentation {
    pub sections: Vec<Section>,
    /// Novelty at `SEG_HZ`. An absolute mean dissimilarity in 0..1 — NOT ranged by its own
    /// max, so steady material genuinely reads ~0 instead of saturating.
    pub novelty: Vec<f32>,
    pub novelty_hz: f32,
    /// Cluster count = number of distinct musical identities found.
    pub cluster_count: usize,
}

/// 7 bands + MFCC 1..=8 + 12 chroma, unit-normalized. `None` when the frame carries no
/// direction (silence).
fn fingerprint(f: &AudioFeatures) -> Option<[f32; FP_DIM]> {
    let mut v = [0.0f32; FP_DIM];
    v[0] = f.sub_bass;
    v[1] = f.bass;
    v[2] = f.low_mid;
    v[3] = f.mid;
    v[4] = f.upper_mid;
    v[5] = f.presence;
    v[6] = f.brilliance;
    v[7..15].copy_from_slice(&f.mfcc[1..9]);
    v[15..27].copy_from_slice(&f.chroma);
    unit(&v)
}

fn unit(v: &[f32; FP_DIM]) -> Option<[f32; FP_DIM]> {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm <= 1e-4 {
        return None;
    }
    let mut out = *v;
    for x in &mut out {
        *x /= norm;
    }
    Some(out)
}

fn cosine(a: &[f32; FP_DIM], b: &[f32; FP_DIM]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

/// Segment `stream` into sections and give each an identity.
pub fn segment(stream: &HopStream) -> Segmentation {
    let empty = Segmentation {
        sections: Vec::new(),
        novelty: Vec::new(),
        novelty_hz: SEG_HZ,
        cluster_count: 0,
    };
    if stream.len() == 0 {
        return empty;
    }

    // --- decimate to SEG_HZ -------------------------------------------------------------
    let stride = ((stream.hop_hz() / SEG_HZ).round() as usize).max(1);
    let mut frames: Vec<[f32; FP_DIM]> = Vec::new();
    let mut levels: Vec<f32> = Vec::new();
    let mut times: Vec<f64> = Vec::new();
    for i in (0..stream.len()).step_by(stride) {
        // Fingerprint from the *pre-normalization* features: the causal percentile window
        // would otherwise have already flattened exactly the dynamics we are measuring.
        let Some(fp) = fingerprint(&stream.raw[i]) else {
            continue;
        };
        frames.push(fp);
        levels.push(stream.raw[i].loudness_s);
        times.push(stream.timestamps[i]);
    }
    if frames.len() < 4 {
        return empty;
    }
    let seg_hz = stream.hop_hz() / stride as f32;

    // --- centred Foote novelty ----------------------------------------------------------
    let novelty = foote_novelty(&frames, (KERNEL_SECONDS * seg_hz) as usize);

    // --- global peak-pick ---------------------------------------------------------------
    let min_sep = (MIN_SECTION_SECONDS * seg_hz) as usize;
    let peaks = pick_peaks(&novelty, min_sep);

    // --- sections between boundaries ----------------------------------------------------
    let mut bounds = vec![0usize];
    bounds.extend(peaks.iter().copied());
    bounds.push(frames.len());
    let mut sections = Vec::new();
    for w in bounds.windows(2) {
        let (lo, hi) = (w[0], w[1]);
        if hi <= lo {
            continue;
        }
        let Some((fp, energy)) = summarize(&frames[lo..hi], &levels[lo..hi]) else {
            continue;
        };
        sections.push(Section {
            start_secs: times[lo],
            end_secs: *times.get(hi).unwrap_or(&stream.duration_secs),
            cluster: 0,
            label: String::new(),
            energy,
            energy_rank: 0.0,
            fingerprint: fp,
        });
    }
    if sections.is_empty() {
        return empty;
    }

    // --- collapse NEAR-DUPLICATE neighbours ---------------------------------------------
    // Before clustering, not after: merging rewrites fingerprints, so clustering the unmerged
    // set would leave labels describing sections that no longer exist.
    merge_adjacent(&mut sections);

    // --- cluster all sections at once ---------------------------------------------------
    let assign = agglomerate(&sections);
    let cluster_count = assign.iter().copied().max().map_or(0, |m| m + 1);
    for (s, &c) in sections.iter_mut().zip(assign.iter()) {
        s.cluster = c;
        s.label = cluster_label(c);
    }

    // --- energy rank over the COMPLETE set ----------------------------------------------
    rank_by_energy(&mut sections);

    Segmentation {
        sections,
        novelty,
        novelty_hz: seg_hz,
        cluster_count,
    }
}

/// Foote checkerboard novelty with a **centred**, Gaussian-tapered kernel.
///
/// Only the diagonal band of the self-similarity matrix is needed, so this is O(N·K²) time and
/// O(K²) memory — never materializing the full N×N matrix, which for a 4-minute song would be
/// hundreds of millions of entries.
///
/// The first and last `k` frames have no full kernel and are left at 0; a boundary inside the
/// first few seconds is not actionable anyway.
fn foote_novelty(frames: &[[f32; FP_DIM]], k: usize) -> Vec<f32> {
    let n = frames.len();
    let k = k.clamp(2, n / 2).max(2);
    let mut out = vec![0.0f32; n];
    if n < 2 * k + 1 {
        return out;
    }

    // Gaussian taper over the kernel's half-width.
    let sigma = k as f32 / 2.0;
    let taper: Vec<f32> = (0..2 * k)
        .map(|i| {
            let d = (i as f32 - k as f32 + 0.5) / sigma;
            (-0.5 * d * d).exp()
        })
        .collect();
    // Total kernel weight. Dividing by it makes novelty an ABSOLUTE mean dissimilarity in
    // 0..1, comparable across songs.
    //
    // It is tempting to range the finished curve by its own max instead — offline the true
    // max is known, so that looks safe in a way the live running max (#1973) was not. It is
    // not: on steady material the whole curve is float residue around 1e-8, and dividing by
    // its own max lifts that noise to a saturated 1.0. Knowing the true maximum does not help
    // when the true maximum is noise.
    let weight: f32 = taper.iter().sum::<f32>().powi(2);

    for (i, slot) in out.iter_mut().enumerate().take(n - k).skip(k) {
        let mut acc = 0.0f32;
        for a in 0..2 * k {
            let ia = i + a - k;
            // +1 in the two same-side quadrants, -1 across the boundary.
            let sa = if a >= k { 1.0 } else { -1.0 };
            for b in 0..2 * k {
                let ib = i + b - k;
                let sb = if b >= k { 1.0 } else { -1.0 };
                acc += sa * sb * taper[a] * taper[b] * cosine(&frames[ia], &frames[ib]);
            }
        }
        // A boundary makes the cross quadrants dissimilar, which drives the sum positive.
        *slot = (acc / weight).max(0.0);
    }
    out
}

/// Take the globally strongest peaks first, rejecting any that crowd an already-accepted one.
/// Sorting by height before enforcing separation is a lookahead-only move: a causal picker has
/// to commit to the first peak it sees and can be pre-empted by a weaker one.
fn pick_peaks(novelty: &[f32], min_sep: usize) -> Vec<usize> {
    let valid: Vec<f32> = novelty.iter().copied().filter(|&v| v > 0.0).collect();
    if valid.is_empty() {
        return Vec::new();
    }
    let mean = valid.iter().sum::<f32>() / valid.len() as f32;
    let var = valid.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / valid.len() as f32;
    let threshold = mean + PEAK_SIGMA * var.sqrt();

    let mut cands: Vec<usize> = (1..novelty.len().saturating_sub(1))
        .filter(|&i| {
            novelty[i] > threshold && novelty[i] >= novelty[i - 1] && novelty[i] > novelty[i + 1]
        })
        .collect();
    cands.sort_by(|&a, &b| novelty[b].total_cmp(&novelty[a]));

    let mut accepted: Vec<usize> = Vec::new();
    for c in cands {
        if accepted.iter().all(|&a| a.abs_diff(c) >= min_sep) {
            accepted.push(c);
        }
    }
    accepted.sort_unstable();
    accepted
}

/// Loudness-weighted mean fingerprint + mean level, over *sounding* frames only.
fn summarize(frames: &[[f32; FP_DIM]], levels: &[f32]) -> Option<([f32; FP_DIM], f32)> {
    let mut acc = [0.0f32; FP_DIM];
    let mut wsum = 0.0f32;
    let mut lsum = 0.0f32;
    let mut lcount = 0usize;
    for (f, &l) in frames.iter().zip(levels.iter()) {
        if l < SOUNDING_FLOOR {
            continue;
        }
        for (a, x) in acc.iter_mut().zip(f.iter()) {
            *a += x * l;
        }
        wsum += l;
        lsum += l;
        lcount += 1;
    }
    if lcount == 0 || wsum <= 0.0 {
        return None;
    }
    for a in &mut acc {
        *a /= wsum;
    }
    Some((unit(&acc)?, lsum / lcount as f32))
}

/// Join neighbours whose fingerprints are all but identical, duration-weighting the merged
/// summary so a 17 s fill cannot pull a 167 s block's identity toward itself.
fn merge_adjacent(sections: &mut Vec<Section>) {
    let mut out: Vec<Section> = Vec::with_capacity(sections.len());
    for s in sections.drain(..) {
        let Some(prev) = out.last_mut() else {
            out.push(s);
            continue;
        };
        if 1.0 - cosine(&prev.fingerprint, &s.fingerprint) > MERGE_MAX_DIST {
            out.push(s);
            continue;
        }
        let (wa, wb) = (
            (prev.end_secs - prev.start_secs) as f32,
            (s.end_secs - s.start_secs) as f32,
        );
        let total = (wa + wb).max(1e-6);
        let mut fp = [0.0f32; FP_DIM];
        for (i, x) in fp.iter_mut().enumerate() {
            *x = (prev.fingerprint[i] * wa + s.fingerprint[i] * wb) / total;
        }
        prev.fingerprint = unit(&fp).unwrap_or(prev.fingerprint);
        prev.energy = (prev.energy * wa + s.energy * wb) / total;
        prev.end_secs = s.end_secs;
    }
    *sections = out;
}

/// Where each section's energy sits across the whole song, as an **ordinal percentile** rather
/// than a min-max position.
///
/// Min-max is anchored to two extremes, so one outlier consumes the range: measured on a real
/// track whose quiet outro sat at 0.587 against a body of 0.78–0.85, that single section ate
/// 74% of the span and crushed **23 of 24 sections above 0.7**. That is the same
/// endpoint-anchored failure that made the online `chorus_likeness` useless (#1977), surviving
/// the move offline because fixing the cold start did not fix the *form* of the statistic.
/// A percentile rank is outlier-robust by construction.
///
/// Honest limit: when the true energy spread is tiny (0.069 across that track's body), an
/// ordering is still only an ordering — it reports *which* section is louder, not that the
/// difference is audible. The `MIN_ENERGY_SPREAD` gate below reports 0 rather than inventing a
/// gradient when there is genuinely no contrast at all.
fn rank_by_energy(sections: &mut [Section]) {
    let n = sections.len();
    if n < 2 {
        if let Some(s) = sections.first_mut() {
            s.energy_rank = 0.0;
        }
        return;
    }
    let lo = sections.iter().map(|s| s.energy).fold(f32::MAX, f32::min);
    let hi = sections.iter().map(|s| s.energy).fold(f32::MIN, f32::max);
    if hi - lo <= MIN_ENERGY_SPREAD {
        for s in sections.iter_mut() {
            s.energy_rank = 0.0;
        }
        return;
    }
    let energies: Vec<f32> = sections.iter().map(|s| s.energy).collect();
    for s in sections.iter_mut() {
        // Ties share a rank (mid-rank), so two equally loud sections cannot be ordered by
        // float noise.
        let below = energies.iter().filter(|&&e| e < s.energy).count() as f32;
        let equal = energies.iter().filter(|&&e| e == s.energy).count() as f32;
        s.energy_rank = (below + (equal - 1.0) * 0.5) / (n - 1) as f32;
    }
}

/// Complete-linkage agglomerative clustering on cosine distance. Sections number in the tens,
/// so the O(n³) naive form is free — and unlike the online greedy assignment it does not depend
/// on the order sections arrive in.
fn agglomerate(sections: &[Section]) -> Vec<usize> {
    let n = sections.len();
    let mut groups: Vec<Vec<usize>> = (0..n).map(|i| vec![i]).collect();

    loop {
        let mut best: Option<(usize, usize, f32)> = None;
        for a in 0..groups.len() {
            for b in (a + 1)..groups.len() {
                // Complete linkage: the *worst* pair decides, so a chain of near-misses cannot
                // quietly merge two genuinely different sections.
                let mut worst = 0.0f32;
                for &i in &groups[a] {
                    for &j in &groups[b] {
                        let d = 1.0 - cosine(&sections[i].fingerprint, &sections[j].fingerprint);
                        worst = worst.max(d);
                    }
                }
                if best.is_none_or(|(_, _, d)| worst < d) {
                    best = Some((a, b, worst));
                }
            }
        }
        match best {
            Some((a, b, d)) if d <= CLUSTER_MAX_DIST => {
                let merged = groups.remove(b);
                groups[a].extend(merged);
            }
            _ => break,
        }
    }

    // Label clusters in order of first appearance, so A is the first section heard.
    groups.sort_by_key(|g| g.iter().copied().min().unwrap_or(usize::MAX));
    let mut assign = vec![0usize; n];
    for (c, g) in groups.iter().enumerate() {
        for &i in g {
            assign[i] = c;
        }
    }
    assign
}

/// 0 -> "A", 25 -> "Z", 26 -> "AA".
fn cluster_label(mut c: usize) -> String {
    let mut s = Vec::new();
    loop {
        s.push(b'A' + (c % 26) as u8);
        if c < 26 {
            break;
        }
        c = c / 26 - 1;
    }
    s.reverse();
    String::from_utf8(s).unwrap_or_else(|_| "?".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fp_from(bands: [f32; 7], chroma_root: usize) -> [f32; FP_DIM] {
        let mut v = [0.0f32; FP_DIM];
        v[..7].copy_from_slice(&bands);
        v[15 + chroma_root] = 1.0;
        unit(&v).unwrap()
    }

    #[test]
    fn cluster_labels_are_spreadsheet_columns() {
        assert_eq!(cluster_label(0), "A");
        assert_eq!(cluster_label(25), "Z");
        assert_eq!(cluster_label(26), "AA");
    }

    #[test]
    fn novelty_peaks_at_a_real_seam_and_is_flat_on_steady_material() {
        let a = fp_from([0.8, 0.7, 0.3, 0.2, 0.1, 0.1, 0.1], 0);
        let b = fp_from([0.1, 0.2, 0.3, 0.8, 0.7, 0.6, 0.5], 7);

        // 100 frames of A then 100 of B: one seam, at 100.
        let mut frames = vec![a; 100];
        frames.extend(vec![b; 100]);
        let nov = foote_novelty(&frames, 20);
        let peak = (0..nov.len())
            .max_by(|&i, &j| nov[i].total_cmp(&nov[j]))
            .unwrap();
        assert!(
            peak.abs_diff(100) <= 2,
            "seam should land on the boundary, got {peak}"
        );

        // Steady material must not manufacture one.
        let flat = foote_novelty(&vec![a; 200], 20);
        assert!(
            flat.iter().all(|&v| v < 1e-3),
            "steady material produced novelty"
        );
    }

    fn mk_energy(e: f32, root: usize) -> Section {
        Section {
            start_secs: 0.0,
            end_secs: 1.0,
            cluster: 0,
            label: String::new(),
            energy: e,
            energy_rank: 0.0,
            fingerprint: fp_from([0.5; 7], root),
        }
    }

    /// The regression that killed the online version: with one or two sections learned, the
    /// min-max rank collapsed to exactly 0 or 1 (#1977). Offline it is a real gradient because
    /// every section is present before any is ranked.
    #[test]
    fn energy_rank_is_a_gradient_not_an_endpoint_pair() {
        let mut secs = vec![mk_energy(0.30, 0), mk_energy(0.55, 1), mk_energy(0.80, 2)];
        rank_by_energy(&mut secs);
        // The middle section is the one the online formula could never place.
        assert!((secs[1].energy_rank - 0.5).abs() < 0.01);
        assert!(secs[0].energy_rank < 0.01 && secs[2].energy_rank > 0.99);
    }

    /// THE REAL-MUSIC REGIME (#1977 again). A single quiet outro against a uniformly loud body
    /// is the actual shape of a dance track, and under min-max it consumed 74% of the range and
    /// pushed 23 of 24 sections above 0.7 — measured on a live Psykovsky track. An evenly
    /// spread three-section fixture passed happily while this failed, which is exactly the trap:
    /// the fixture encoded a well-conditioned distribution real material does not produce.
    #[test]
    fn one_quiet_outlier_does_not_crush_the_rest_of_the_song() {
        // 12 loud body sections in a narrow band, plus one much quieter outro.
        let mut secs: Vec<Section> = (0..12)
            .map(|i| mk_energy(0.78 + i as f32 * 0.006, i % 7))
            .collect();
        secs.push(mk_energy(0.587, 3));
        rank_by_energy(&mut secs);

        let body: Vec<f32> = secs[..12].iter().map(|s| s.energy_rank).collect();
        assert!(
            (secs[12].energy_rank - 0.0).abs() < 1e-6,
            "the outlier is the floor"
        );
        // Under min-max every body rank landed above 0.7. A percentile rank must spread them.
        let above = body.iter().filter(|&&r| r > 0.7).count();
        assert!(
            above <= body.len() / 2 + 1,
            "body ranks still bunched at the top: {body:?}"
        );
        // And they must remain correctly *ordered* — robustness must not cost monotonicity.
        assert!(
            body.windows(2).all(|w| w[1] > w[0]),
            "ranks lost their order: {body:?}"
        );
    }

    #[test]
    fn equal_energies_share_a_rank_rather_than_being_ordered_by_noise() {
        let mut secs = vec![
            mk_energy(0.20, 0),
            mk_energy(0.60, 1),
            mk_energy(0.60, 2),
            mk_energy(0.90, 3),
        ];
        rank_by_energy(&mut secs);
        assert!((secs[1].energy_rank - secs[2].energy_rank).abs() < 1e-6);
    }

    /// Only near-duplicates collapse. A neighbour that genuinely changed must survive, even
    /// when clustering would call it the same identity — on dense material a boundary is
    /// content, not noise.
    #[test]
    fn merge_joins_near_duplicates_but_keeps_a_real_change() {
        let a = fp_from([0.8, 0.7, 0.3, 0.2, 0.1, 0.1, 0.1], 0);
        let mk = |fp: [f32; FP_DIM], t0: f64, t1: f64| Section {
            start_secs: t0,
            end_secs: t1,
            cluster: 0,
            label: String::new(),
            energy: 0.5,
            energy_rank: 0.0,
            fingerprint: fp,
        };
        // Second is all but identical to the first; third is a real move.
        let near = fp_from([0.801, 0.700, 0.301, 0.200, 0.101, 0.100, 0.100], 0);
        let far = fp_from([0.2, 0.3, 0.4, 0.8, 0.7, 0.6, 0.5], 7);
        assert!(1.0 - cosine(&a, &near) < MERGE_MAX_DIST);
        assert!(1.0 - cosine(&a, &far) > MERGE_MAX_DIST);

        let mut secs = vec![mk(a, 0.0, 10.0), mk(near, 10.0, 20.0), mk(far, 20.0, 30.0)];
        merge_adjacent(&mut secs);
        assert_eq!(secs.len(), 2, "near-duplicate should have joined");
        assert!((secs[0].end_secs - 20.0).abs() < 1e-6, "span must extend");
        assert!(
            (secs[1].start_secs - 20.0).abs() < 1e-6,
            "the real change survives"
        );
    }

    #[test]
    fn agglomerate_recalls_a_returning_section_regardless_of_order() {
        let mk = |root: usize, bands: [f32; 7]| Section {
            start_secs: 0.0,
            end_secs: 1.0,
            cluster: 0,
            label: String::new(),
            energy: 0.5,
            energy_rank: 0.0,
            fingerprint: fp_from(bands, root),
        };
        let verse = ([0.8, 0.7, 0.3, 0.2, 0.1, 0.1, 0.1], 0);
        let chorus = ([0.1, 0.2, 0.3, 0.8, 0.7, 0.6, 0.5], 7);

        // A B A' B' — A' must recall A and B' must recall B.
        let secs = vec![
            mk(verse.1, verse.0),
            mk(chorus.1, chorus.0),
            mk(verse.1, verse.0),
            mk(chorus.1, chorus.0),
        ];
        let a = agglomerate(&secs);
        assert_eq!(a[0], a[2], "A' must recall A");
        assert_eq!(a[1], a[3], "B' must recall B");
        assert_ne!(a[0], a[1], "verse and chorus must stay distinct");
    }

    /// N=1 and N=2 are the regimes the detector actually spends its first minutes in (#1977),
    /// so they are first-class cases, not edge cases.
    #[test]
    fn tiny_section_counts_do_not_panic_or_lie() {
        let mk = |root: usize| Section {
            start_secs: 0.0,
            end_secs: 1.0,
            cluster: 0,
            label: String::new(),
            energy: 0.5,
            energy_rank: 0.0,
            fingerprint: fp_from([0.5; 7], root),
        };
        assert_eq!(agglomerate(&[mk(0)]), vec![0]);
        // Two identical sections are one identity; two different ones are two.
        assert_eq!(agglomerate(&[mk(0), mk(0)]), vec![0, 0]);
        assert_eq!(agglomerate(&[mk(0), mk(7)]), vec![0, 1]);
    }

    #[test]
    fn peak_picker_enforces_minimum_separation_by_strength() {
        // Two peaks 3 apart; with min_sep 10 only the stronger survives.
        let mut nov = vec![0.0f32; 60];
        for (i, v) in nov.iter_mut().enumerate() {
            *v = 0.01 * (i % 3) as f32;
        }
        nov[20] = 0.6;
        nov[23] = 0.9;
        let peaks = pick_peaks(&nov, 10);
        assert!(peaks.contains(&23), "the stronger peak must win: {peaks:?}");
        assert!(!peaks.contains(&20), "the crowded weaker peak must lose");
    }
}
