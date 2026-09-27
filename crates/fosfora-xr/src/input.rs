//! S7 hands: `XR_EXT_hand_tracking` joints as obstacle spheres, and a pinch
//! detector. Bare hands are the primary input (invariant I5); controllers
//! are not read at all in the spike.

use anyhow::{Context, Result};
use log::info;
use openxr as xr;

/// Per-frame hand data handed to the main loop, by value so it can outlive
/// the session borrow that produced it.
#[derive(Debug, Clone, Default)]
pub struct HandsFrame {
    /// One sphere per tracked joint: xyz center in the reference space, w
    /// radius (the runtime's per-joint radius).
    pub spheres: Vec<[f32; 4]>,
    /// Whether each hand (left, right) delivered valid joints this frame.
    pub tracked: [bool; 2],
    /// A pinch began on this hand this frame (rising edge only).
    pub pinch_began: [bool; 2],
    /// Pinch currently held on each hand.
    pub pinching: [bool; 2],
}

/// Thumb tip to index tip distance thresholds (meters), with hysteresis so
/// a held pinch does not flicker at the boundary.
const PINCH_ON_M: f32 = 0.015;
const PINCH_OFF_M: f32 = 0.030;

pub struct Hands {
    trackers: [xr::HandTracker; 2],
    pinching: [bool; 2],
    /// Frames since the last "tracked" log per hand, to log state changes only.
    was_tracked: [bool; 2],
}

impl Hands {
    pub fn new(session: &xr::Session<xr::Vulkan>) -> Result<Self> {
        let left = session
            .create_hand_tracker(xr::Hand::LEFT)
            .context("xrCreateHandTrackerEXT(left)")?;
        let right = session
            .create_hand_tracker(xr::Hand::RIGHT)
            .context("xrCreateHandTrackerEXT(right)")?;
        info!("hand trackers created (XR_EXT_hand_tracking, 26 joints per hand)");
        Ok(Self {
            trackers: [left, right],
            pinching: [false; 2],
            was_tracked: [false; 2],
        })
    }

    /// Locate both hands at `time` in `space` and run the pinch detector.
    pub fn locate(&mut self, space: &xr::Space, time: xr::Time) -> HandsFrame {
        let mut frame = HandsFrame {
            spheres: Vec::with_capacity(2 * xr::HAND_JOINT_COUNT),
            ..HandsFrame::default()
        };
        for (h, tracker) in self.trackers.iter().enumerate() {
            let joints = match space.locate_hand_joints(tracker, time) {
                Ok(Some(j)) => j,
                Ok(None) => {
                    note_tracked(&mut self.was_tracked, h, false);
                    self.pinching[h] = false;
                    frame.pinching[h] = false;
                    continue;
                }
                Err(e) => {
                    log::warn!("xrLocateHandJointsEXT({h}): {e}");
                    continue;
                }
            };
            let valid = |j: &xr::HandJointLocation| {
                j.location_flags
                    .contains(xr::SpaceLocationFlags::POSITION_VALID)
            };
            let mut any = false;
            for j in &joints {
                if valid(j) {
                    any = true;
                    let p = j.pose.position;
                    frame.spheres.push([p.x, p.y, p.z, j.radius]);
                }
            }
            frame.tracked[h] = any;
            note_tracked(&mut self.was_tracked, h, any);

            let thumb = &joints[xr::HandJoint::THUMB_TIP.into_raw() as usize];
            let index = &joints[xr::HandJoint::INDEX_TIP.into_raw() as usize];
            if any && valid(thumb) && valid(index) {
                let a = thumb.pose.position;
                let b = index.pose.position;
                let d = ((a.x - b.x).powi(2) + (a.y - b.y).powi(2) + (a.z - b.z).powi(2)).sqrt();
                if !self.pinching[h] && d < PINCH_ON_M {
                    self.pinching[h] = true;
                    frame.pinch_began[h] = true;
                    info!(
                        "pinch {} began (tip distance {:.1} mm)",
                        hand_name(h),
                        d * 1000.0
                    );
                } else if self.pinching[h] && d > PINCH_OFF_M {
                    self.pinching[h] = false;
                    info!(
                        "pinch {} released (tip distance {:.1} mm)",
                        hand_name(h),
                        d * 1000.0
                    );
                }
            } else {
                self.pinching[h] = false;
            }
            frame.pinching[h] = self.pinching[h];
        }
        frame
    }
}

/// Log a hand appearing or disappearing, once per change.
fn note_tracked(was_tracked: &mut [bool; 2], hand: usize, tracked: bool) {
    if was_tracked[hand] != tracked {
        was_tracked[hand] = tracked;
        info!(
            "hand {} {}",
            hand_name(hand),
            if tracked { "tracked" } else { "lost" }
        );
    }
}

fn hand_name(h: usize) -> &'static str {
    if h == 0 { "left" } else { "right" }
}
