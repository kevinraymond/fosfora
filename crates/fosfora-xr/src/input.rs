//! S7 hands: `XR_EXT_hand_tracking` joints as obstacle spheres, the pinch
//! detector's input (`pinch.rs`), and the runtime's skinned hand mesh (`XR_FB_hand_tracking_mesh`)
//! as the depth occluder. Bare hands are the primary input (invariant I5);
//! controllers are not read at all in the spike.
//!
//! The mesh is fetched once per hand at bind pose (two-call idiom through
//! the `openxr-sys` function pointer; the `openxr` crate has no safe
//! wrapper) and skinned on the GPU every frame from the located joints:
//! one matrix per joint = joint pose x inverse bind pose. Hand scale is
//! estimated from the located bone lengths against the bind pose, so a
//! larger or smaller hand than the mesh's default does not leave gaps at
//! the knuckles; `XrHandTrackingScaleFB` would need a raw locate call.

use std::ptr;

use anyhow::{Context, Result, anyhow};
use glam::{Mat4, Quat, Vec3};
use log::{info, warn};
use openxr as xr;
use xr::sys;

use crate::particles3d::{HAND_JOINTS, HandMeshData, HandMeshVertex, HandSkins};

/// Per-frame hand data handed to the main loop, by value so it can outlive
/// the session borrow that produced it.
#[derive(Debug, Clone, Default)]
pub struct HandsFrame {
    /// One sphere per tracked joint: xyz center in the reference space, w
    /// radius (the runtime's per-joint radius).
    pub spheres: Vec<[f32; 4]>,
    /// How many of `spheres` each hand contributed: the left hand's come
    /// first, then the right's.
    pub sphere_count: [usize; 2],
    /// Wrist joint position, where located (the reach beam starts there).
    pub wrist: [Option<[f32; 3]>; 2],
    /// Whether each hand (left, right) delivered valid joints this frame.
    pub tracked: [bool; 2],
    /// A pinch began on this hand this frame (rising edge only).
    pub pinch_began: [bool; 2],
    /// Pinch currently held on each hand.
    pub pinching: [bool; 2],
    /// Midpoint of the thumb and index tips, where both were located.
    pub pinch_point: [Option<[f32; 3]>; 2],
    /// Thumb tip to index tip distance (meters), where both were located.
    pub tip_distance: [Option<f32>; 2],
    /// Palm joint pose: position and (x, y, z, w) orientation. Its -Y axis
    /// is the palm normal (`XR_EXT_hand_tracking`: +Y points out of the
    /// back of the hand).
    pub palm: [Option<([f32; 3], [f32; 4])>; 2],
    /// Index fingertip: xyz and the joint radius (the debug panel's poke).
    pub index_tip: [Option<[f32; 4]>; 2],
    /// Index knuckle (proximal joint): the debug panel's ray passes
    /// through it.
    pub index_knuckle: [Option<[f32; 3]>; 2],
    /// Index, middle, ring and little fingertips, where all four were
    /// located (hand poses, `pose.rs`).
    pub finger_tips: [Option<[[f32; 3]; 4]>; 2],
    /// Skinning matrices for the hand meshes, valid where `mesh_ready`.
    pub skins: HandSkins,
    /// The hand has a mesh and every joint was located this frame.
    pub mesh_ready: [bool; 2],
}

pub use crate::pinch::PINCH_ON_M;
use crate::pinch::{CLOSE_DROP_M, CLOSE_WINDOW_S, PinchDetector, PinchEdge};
/// Log the estimated hand scale when it moves this much from the last log.
const SCALE_LOG_STEP: f32 = 0.02;

/// A hand mesh from `xrGetHandMeshFB` plus what the skinning needs.
struct HandMesh {
    data: HandMeshData,
    /// Inverse bind pose per joint.
    inv_bind: Vec<Mat4>,
    /// Parent joint index per joint (a root points at itself).
    parents: Vec<usize>,
    /// Bind-pose distance from each joint to its parent (0 for a root).
    bind_len: Vec<f32>,
}

pub struct Hands {
    trackers: [xr::HandTracker; 2],
    meshes: [Option<HandMesh>; 2],
    /// The pinch detector per hand (`pinch.rs`).
    pinch: [PinchDetector; 2],
    /// The last locate's time, for the detector's frame length.
    last_time: Option<i64>,
    /// Frames since the last "tracked" log per hand, to log state changes only.
    was_tracked: [bool; 2],
    /// Last logged scale estimate per hand.
    scale_logged: [f32; 2],
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
        let instance = session.instance();
        let mesh_for = |h: usize, tracker: &xr::HandTracker| match fetch_mesh(instance, tracker) {
            Ok(Some(m)) => {
                let (lo, hi) = m.data.vertices.iter().fold(
                    (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)),
                    |(lo, hi), v| {
                        let p = Vec3::from(v.pos);
                        (lo.min(p), hi.max(p))
                    },
                );
                let wrist = m.inv_bind[xr::HandJoint::WRIST.into_raw() as usize]
                    .inverse()
                    .w_axis;
                info!(
                    "hand mesh {}: {} vertices, {} triangles, {} joints (XR_FB_hand_tracking_mesh) · bind pose bounds ({:.3}, {:.3}, {:.3})..({:.3}, {:.3}, {:.3}) m · wrist at ({:.3}, {:.3}, {:.3})",
                    hand_name(h),
                    m.data.vertices.len(),
                    m.data.indices.len() / 3,
                    m.inv_bind.len(),
                    lo.x,
                    lo.y,
                    lo.z,
                    hi.x,
                    hi.y,
                    hi.z,
                    wrist.x,
                    wrist.y,
                    wrist.z
                );
                Some(m)
            }
            Ok(None) => {
                info!(
                    "hand mesh {}: XR_FB_hand_tracking_mesh not enabled; joint spheres occlude",
                    hand_name(h)
                );
                None
            }
            Err(e) => {
                warn!("hand mesh {}: {e:#}; joint spheres occlude", hand_name(h));
                None
            }
        };
        let meshes = [mesh_for(0, &left), mesh_for(1, &right)];
        Ok(Self {
            trackers: [left, right],
            meshes,
            pinch: [PinchDetector::new(), PinchDetector::new()],
            last_time: None,
            was_tracked: [false; 2],
            scale_logged: [1.0; 2],
        })
    }

    /// The runtime's hand meshes (left, right), for upload once.
    pub fn meshes(&self) -> [Option<&HandMeshData>; 2] {
        [
            self.meshes[0].as_ref().map(|m| &m.data),
            self.meshes[1].as_ref().map(|m| &m.data),
        ]
    }

    /// Locate both hands at `time` in `space`, run the pinch detector and
    /// build the mesh skinning matrices.
    pub fn locate(&mut self, space: &xr::Space, time: xr::Time) -> HandsFrame {
        let mut frame = HandsFrame {
            spheres: Vec::with_capacity(2 * xr::HAND_JOINT_COUNT),
            ..HandsFrame::default()
        };
        // The frame's length from the runtime's clock, clamped so a pause
        // or a clock jump neither empties the detector's window at once nor
        // freezes it.
        let now = time.as_nanos();
        let dt = self
            .last_time
            .map_or(1.0 / 72.0, |last| (now - last) as f32 * 1e-9)
            .clamp(1.0 / 120.0, 1.0 / 30.0);
        self.last_time = Some(now);
        for (h, tracker) in self.trackers.iter().enumerate() {
            let joints = match space.locate_hand_joints(tracker, time) {
                Ok(Some(j)) => j,
                Ok(None) => {
                    note_tracked(&mut self.was_tracked, h, false);
                    self.pinch[h].reset();
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
                    frame.sphere_count[h] += 1;
                }
            }
            let wrist = &joints[xr::HandJoint::WRIST.into_raw() as usize];
            if valid(wrist) {
                let p = wrist.pose.position;
                frame.wrist[h] = Some([p.x, p.y, p.z]);
            }
            frame.tracked[h] = any;
            note_tracked(&mut self.was_tracked, h, any);

            if any && let Some(mesh) = &self.meshes[h] {
                if let Some(scale) = skin(mesh, &joints, &mut frame.skins[h]) {
                    frame.mesh_ready[h] = true;
                    if (scale - self.scale_logged[h]).abs() > SCALE_LOG_STEP {
                        self.scale_logged[h] = scale;
                        info!(
                            "hand mesh {}: scale estimate {scale:.3} from the bone lengths",
                            hand_name(h)
                        );
                    }
                }
            }

            let posed =
                xr::SpaceLocationFlags::POSITION_VALID | xr::SpaceLocationFlags::ORIENTATION_VALID;
            let palm = &joints[xr::HandJoint::PALM.into_raw() as usize];
            if palm.location_flags.contains(posed) {
                let (p, q) = (palm.pose.position, palm.pose.orientation);
                frame.palm[h] = Some(([p.x, p.y, p.z], [q.x, q.y, q.z, q.w]));
            }
            let knuckle = &joints[xr::HandJoint::INDEX_PROXIMAL.into_raw() as usize];
            if valid(knuckle) {
                let p = knuckle.pose.position;
                frame.index_knuckle[h] = Some([p.x, p.y, p.z]);
            }
            let tip = &joints[xr::HandJoint::INDEX_TIP.into_raw() as usize];
            if valid(tip) {
                let p = tip.pose.position;
                frame.index_tip[h] = Some([p.x, p.y, p.z, tip.radius]);
            }

            let tips = [
                xr::HandJoint::INDEX_TIP,
                xr::HandJoint::MIDDLE_TIP,
                xr::HandJoint::RING_TIP,
                xr::HandJoint::LITTLE_TIP,
            ]
            .map(|j| &joints[j.into_raw() as usize]);
            if tips.iter().all(|j| valid(j)) {
                frame.finger_tips[h] = Some(tips.map(|j| {
                    let p = j.pose.position;
                    [p.x, p.y, p.z]
                }));
            }

            let thumb = &joints[xr::HandJoint::THUMB_TIP.into_raw() as usize];
            let index = &joints[xr::HandJoint::INDEX_TIP.into_raw() as usize];
            let tips = (any && valid(thumb) && valid(index)).then(|| {
                let a = thumb.pose.position;
                let b = index.pose.position;
                let d = ((a.x - b.x).powi(2) + (a.y - b.y).powi(2) + (a.z - b.z).powi(2)).sqrt();
                frame.pinch_point[h] =
                    Some([0.5 * (a.x + b.x), 0.5 * (a.y + b.y), 0.5 * (a.z + b.z)]);
                frame.tip_distance[h] = Some(d);
                d
            });
            let mm = tips.unwrap_or(0.0) * 1000.0;
            match self.pinch[h].step(tips, dt) {
                PinchEdge::Began => {
                    frame.pinch_began[h] = true;
                    info!(
                        "pinch {} began (tip distance {mm:.1} mm, closed {:.0} mm within {CLOSE_WINDOW_S} s)",
                        hand_name(h),
                        self.pinch[h].recent_drop() * 1000.0
                    );
                }
                PinchEdge::Released => {
                    info!("pinch {} released (tip distance {mm:.1} mm)", hand_name(h));
                }
                PinchEdge::Rejected => {
                    // Board #3336: a hand that drifted shut, not a pinch.
                    info!(
                        "pinch {} not taken: tips at {mm:.1} mm closed only {:.0} mm within {CLOSE_WINDOW_S} s (a pinch closes {:.0} mm or more)",
                        hand_name(h),
                        self.pinch[h].recent_drop() * 1000.0,
                        CLOSE_DROP_M * 1000.0
                    );
                }
                PinchEdge::None => {}
            }
            frame.pinching[h] = self.pinch[h].pinching();
        }
        frame
    }
}

/// Fill `out` with this frame's skinning matrices for `mesh` from the
/// located joints, and return the hand-scale estimate used. `None` when a
/// joint has no valid pose (the mesh is not drawn that frame).
fn skin(
    mesh: &HandMesh,
    joints: &[xr::HandJointLocation],
    out: &mut [[f32; 16]; HAND_JOINTS],
) -> Option<f32> {
    let posed = xr::SpaceLocationFlags::POSITION_VALID | xr::SpaceLocationFlags::ORIENTATION_VALID;
    if joints.len() < HAND_JOINTS || joints.iter().any(|j| !j.location_flags.contains(posed)) {
        return None;
    }
    let pos = |j: usize| {
        let p = joints[j].pose.position;
        Vec3::new(p.x, p.y, p.z)
    };
    // Hand scale: located bone lengths over bind-pose bone lengths, summed
    // over the skeleton so one noisy joint does not swing it.
    let (mut now, mut bind) = (0.0f32, 0.0f32);
    for (j, &parent) in mesh.parents.iter().enumerate() {
        if parent != j {
            now += pos(j).distance(pos(parent));
            bind += mesh.bind_len[j];
        }
    }
    let scale = if bind > 1e-4 && now > 1e-4 {
        (now / bind).clamp(0.5, 2.0)
    } else {
        1.0
    };
    let scale_m = Mat4::from_scale(Vec3::splat(scale));
    for (j, slot) in out.iter_mut().enumerate() {
        let q = joints[j].pose.orientation;
        let world = Mat4::from_rotation_translation(Quat::from_xyzw(q.x, q.y, q.z, q.w), pos(j));
        *slot = (world * scale_m * mesh.inv_bind[j]).to_cols_array();
    }
    Some(scale)
}

/// `xrGetHandMeshFB` for one tracker: the bind-pose mesh with skinning
/// data. `Ok(None)` when the extension is not enabled on the instance.
fn fetch_mesh(instance: &xr::Instance, tracker: &xr::HandTracker) -> Result<Option<HandMesh>> {
    let Some(fp) = instance.exts().fb_hand_tracking_mesh.as_ref() else {
        return Ok(None);
    };
    let mut mesh = sys::HandTrackingMeshFB {
        ty: sys::HandTrackingMeshFB::TYPE,
        next: ptr::null_mut(),
        joint_capacity_input: 0,
        joint_count_output: 0,
        joint_bind_poses: ptr::null_mut(),
        joint_radii: ptr::null_mut(),
        joint_parents: ptr::null_mut(),
        vertex_capacity_input: 0,
        vertex_count_output: 0,
        vertex_positions: ptr::null_mut(),
        vertex_normals: ptr::null_mut(),
        vertex_u_vs: ptr::null_mut(),
        vertex_blend_indices: ptr::null_mut(),
        vertex_blend_weights: ptr::null_mut(),
        index_capacity_input: 0,
        index_count_output: 0,
        indices: ptr::null_mut(),
    };
    // SAFETY: the two-call idiom's sizing call: every capacity is 0 and every
    // array pointer null, so the runtime only writes the three counts. The
    // tracker handle is live (owned by `Hands`, dropped before the session).
    let result = unsafe { (fp.get_hand_mesh)(tracker.as_raw(), &mut mesh) };
    crate::room::check(result).context("xrGetHandMeshFB (sizes)")?;
    let (nj, nv, ni) = (
        mesh.joint_count_output as usize,
        mesh.vertex_count_output as usize,
        mesh.index_count_output as usize,
    );
    if nj != HAND_JOINTS {
        return Err(anyhow!(
            "hand mesh has {nj} joints, the skinning shader expects {HAND_JOINTS}"
        ));
    }
    if nv == 0 || ni == 0 || !ni.is_multiple_of(3) {
        return Err(anyhow!(
            "hand mesh has {nv} vertices and {ni} indices (not a triangle list)"
        ));
    }
    if nv > usize::from(u16::MAX) {
        return Err(anyhow!(
            "hand mesh has {nv} vertices, over the u16 index range"
        ));
    }

    let mut bind_poses = vec![sys::Posef::IDENTITY; nj];
    let mut radii = vec![0.0f32; nj];
    let mut parents = vec![xr::HandJoint::from_raw(0); nj];
    let mut positions = vec![sys::Vector3f::default(); nv];
    let mut normals = vec![sys::Vector3f::default(); nv];
    let mut uvs = vec![sys::Vector2f::default(); nv];
    let mut blend_indices = vec![sys::Vector4sFB::default(); nv];
    let mut blend_weights = vec![sys::Vector4f::default(); nv];
    let mut indices = vec![0i16; ni];
    mesh.joint_capacity_input = mesh.joint_count_output;
    mesh.joint_bind_poses = bind_poses.as_mut_ptr();
    mesh.joint_radii = radii.as_mut_ptr();
    mesh.joint_parents = parents.as_mut_ptr();
    mesh.vertex_capacity_input = mesh.vertex_count_output;
    mesh.vertex_positions = positions.as_mut_ptr();
    mesh.vertex_normals = normals.as_mut_ptr();
    mesh.vertex_u_vs = uvs.as_mut_ptr();
    mesh.vertex_blend_indices = blend_indices.as_mut_ptr();
    mesh.vertex_blend_weights = blend_weights.as_mut_ptr();
    mesh.index_capacity_input = mesh.index_count_output;
    mesh.indices = indices.as_mut_ptr();
    // SAFETY: every array pointer now addresses a Vec of exactly the
    // capacity passed beside it, all alive until this function returns; the
    // runtime writes at most that many elements of the matching `repr(C)`
    // type. Same live tracker handle as above.
    let result = unsafe { (fp.get_hand_mesh)(tracker.as_raw(), &mut mesh) };
    crate::room::check(result).context("xrGetHandMeshFB (data)")?;
    if mesh.joint_count_output as usize != nj
        || mesh.vertex_count_output as usize != nv
        || mesh.index_count_output as usize != ni
    {
        return Err(anyhow!(
            "hand mesh sizes changed between calls ({} joints, {} vertices, {} indices)",
            mesh.joint_count_output,
            mesh.vertex_count_output,
            mesh.index_count_output
        ));
    }

    let parents: Vec<usize> = parents
        .iter()
        .enumerate()
        .map(|(j, p)| {
            usize::try_from(p.into_raw())
                .ok()
                .filter(|&p| p < nj)
                .unwrap_or(j)
        })
        .collect();
    let bind_pos = |j: usize| {
        let p = bind_poses[j].position;
        Vec3::new(p.x, p.y, p.z)
    };
    let inv_bind: Vec<Mat4> = bind_poses
        .iter()
        .map(|pose| {
            let q = pose.orientation;
            Mat4::from_rotation_translation(
                Quat::from_xyzw(q.x, q.y, q.z, q.w),
                Vec3::new(pose.position.x, pose.position.y, pose.position.z),
            )
            .inverse()
        })
        .collect();
    let bind_len: Vec<f32> = parents
        .iter()
        .enumerate()
        .map(|(j, &p)| {
            if p == j {
                0.0
            } else {
                bind_pos(j).distance(bind_pos(p))
            }
        })
        .collect();
    // A joint index outside the skeleton (the spec leaves unused slots
    // unspecified) is clamped; its weight is 0 in practice.
    let joint_index = |i: i16| u8::try_from(i.clamp(0, HAND_JOINTS as i16 - 1)).unwrap_or(0);
    let vertices: Vec<HandMeshVertex> = (0..nv)
        .map(|i| {
            let p = positions[i];
            let w = blend_weights[i];
            let b = blend_indices[i];
            HandMeshVertex {
                pos: [p.x, p.y, p.z],
                weights: [w.x, w.y, w.z, w.w],
                joints: [
                    joint_index(b.x),
                    joint_index(b.y),
                    joint_index(b.z),
                    joint_index(b.w),
                ],
            }
        })
        .collect();
    let indices: Vec<u16> = indices
        .iter()
        .map(|&i| u16::try_from(i).unwrap_or(0))
        .collect();
    Ok(Some(HandMesh {
        data: HandMeshData { vertices, indices },
        inv_bind,
        parents,
        bind_len,
    }))
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
