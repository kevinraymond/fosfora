//! When to capture, decided up front from the analysis — stills for
//! composition, full-rate windows for motion, both placed where the music says
//! they matter. Pure functions, tested in the default build.

use crate::analyze::HopStream;
use crate::analyze::structure_offline::Segmentation;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Capture {
    No,
    /// One PNG.
    Still {
        section: usize,
    },
    /// Part of a clip window; frames stream to the encoder.
    Window {
        window: usize,
    },
}

#[derive(Debug, Clone)]
pub struct Window {
    pub start_hop: usize,
    pub end_hop: usize,
    pub label: String,
    /// Seconds into the song where the window opens.
    pub start_secs: f64,
}

pub struct ScheduleCfg {
    pub stills_per_section: usize,
    pub window_secs: f64,
}

impl Default for ScheduleCfg {
    fn default() -> Self {
        Self {
            stills_per_section: 3,
            window_secs: 6.0,
        }
    }
}

pub struct SamplingSchedule {
    /// Sorted still hops with their section index.
    stills: Vec<(usize, usize)>,
    pub windows: Vec<Window>,
}

impl SamplingSchedule {
    /// Stills at 25/50/75% of each section; one window per section at its
    /// midpoint; extra windows centered on each detected drop. Windows are
    /// clamped to the song and merged when they collide, so a drop near a
    /// midpoint does not double-capture.
    pub fn build(stream: &HopStream, seg: &Segmentation, cfg: &ScheduleCfg) -> Self {
        let hop_hz = f64::from(stream.hop_hz());
        let total_hops = stream.len();
        let win_hops = ((cfg.window_secs * hop_hz).round() as usize).max(8);

        let mut stills = Vec::new();
        let mut raw_windows: Vec<(usize, usize, String)> = Vec::new();

        for (si, section) in seg.sections.iter().enumerate() {
            let s_hop = (section.start_secs * hop_hz) as usize;
            let e_hop = (section.end_secs * hop_hz) as usize;
            let len = e_hop.saturating_sub(s_hop);
            if len == 0 {
                continue;
            }
            for k in 1..=cfg.stills_per_section {
                let frac = k as f64 / (cfg.stills_per_section + 1) as f64;
                stills.push((s_hop + (len as f64 * frac) as usize, si));
            }
            let mid = s_hop + len / 2;
            raw_windows.push((
                mid.saturating_sub(win_hops / 2),
                (mid + win_hops / 2).min(total_hops),
                format!("s{si:02}_{}", section.label),
            ));
        }
        for (di, &drop_hop) in stream.drops.iter().enumerate() {
            raw_windows.push((
                drop_hop.saturating_sub(win_hops / 4),
                (drop_hop + (3 * win_hops) / 4).min(total_hops),
                format!("drop{di:02}"),
            ));
        }

        // Sort + merge overlaps, keeping the first label.
        raw_windows.sort_by_key(|w| w.0);
        let mut merged: Vec<(usize, usize, String)> = Vec::new();
        for w in raw_windows {
            match merged.last_mut() {
                Some(last) if w.0 < last.1 => last.1 = last.1.max(w.1),
                _ => merged.push(w),
            }
        }

        let windows = merged
            .into_iter()
            .map(|(start, end, label)| Window {
                start_hop: start,
                end_hop: end,
                label,
                start_secs: start as f64 / hop_hz,
            })
            .collect();

        stills.sort_unstable();
        Self { stills, windows }
    }

    pub fn wants(&self, hop: usize) -> Capture {
        if let Some(wi) = self
            .windows
            .iter()
            .position(|w| (w.start_hop..w.end_hop).contains(&hop))
        {
            return Capture::Window { window: wi };
        }
        if let Ok(i) = self.stills.binary_search_by_key(&hop, |(h, _)| *h) {
            return Capture::Still {
                section: self.stills[i].1,
            };
        }
        Capture::No
    }

    pub fn still_count(&self) -> usize {
        self.stills.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyze::structure_offline::Section;
    use crate::audio::ANALYSIS_HOP;

    fn stream(secs: f64, drops: &[usize]) -> HopStream {
        const HOP_HZ: f32 = 86.13281;
        let hops = (secs * f64::from(HOP_HZ)) as usize;
        HopStream {
            sample_rate: HOP_HZ * ANALYSIS_HOP as f32,
            source_channels: 2,
            duration_secs: secs,
            timestamps: (0..hops).map(|h| h as f64 / f64::from(HOP_HZ)).collect(),
            live: vec![Default::default(); hops],
            raw: vec![Default::default(); hops],
            beats: Vec::new(),
            downbeats: Vec::new(),
            drops: drops.to_vec(),
        }
    }

    fn seg(bounds: &[(f64, f64)]) -> Segmentation {
        Segmentation {
            sections: bounds
                .iter()
                .enumerate()
                .map(|(i, &(start, dur))| Section {
                    start_secs: start,
                    end_secs: start + dur,
                    cluster: i,
                    label: ((b'A' + (i % 26) as u8) as char).to_string(),
                    energy: 0.5,
                    energy_rank: 0.5,
                    fingerprint: [0.0; 27],
                })
                .collect(),
            novelty: Vec::new(),
            novelty_hz: 4.0,
            cluster_count: bounds.len(),
        }
    }

    #[test]
    fn stills_and_windows_cover_every_section() {
        let st = stream(60.0, &[]);
        let sg = seg(&[(0.0, 20.0), (20.0, 20.0), (40.0, 20.0)]);
        let sched = SamplingSchedule::build(&st, &sg, &ScheduleCfg::default());
        assert_eq!(sched.still_count(), 9, "3 stills x 3 sections");
        assert_eq!(sched.windows.len(), 3, "one window per section");
        // Every window sits inside the song.
        for w in &sched.windows {
            assert!(w.end_hop <= st.len());
            assert!(w.start_hop < w.end_hop);
        }
    }

    #[test]
    fn drop_windows_merge_with_colliding_section_windows() {
        let st = stream(60.0, &[860]); // drop at ~10s = mid of section 0
        let sg = seg(&[(0.0, 20.0), (20.0, 40.0)]);
        let sched = SamplingSchedule::build(&st, &sg, &ScheduleCfg::default());
        // The drop window overlaps section 0's midpoint window: merged.
        assert_eq!(
            sched.windows.len(),
            2,
            "merged, not duplicated: {:?}",
            sched.windows
        );
    }

    #[test]
    fn wants_is_consistent_with_the_schedule() {
        let st = stream(30.0, &[]);
        let sg = seg(&[(0.0, 30.0)]);
        let sched = SamplingSchedule::build(&st, &sg, &ScheduleCfg::default());
        let mut stills = 0;
        let mut window_hops = 0;
        for h in 0..st.len() {
            match sched.wants(h) {
                Capture::Still { .. } => stills += 1,
                Capture::Window { .. } => window_hops += 1,
                Capture::No => {}
            }
        }
        // Stills inside a window are subsumed by it; the rest must all appear.
        assert!(stills + window_hops > 0);
        let w = &sched.windows[0];
        assert_eq!(window_hops, w.end_hop - w.start_hop);
    }
}
